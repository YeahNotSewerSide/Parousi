//! Saving the presentation to a file and opening it again. The file is a ZIP
//! archive of `presentation.json`, which describes the presentation, beside
//! the files of its images.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::error::Error;
use std::fs;
use std::io::{self, BufReader, Cursor, Read, Seek, Write};
use std::path::Path;
use std::sync::Arc;

use ::image::ImageFormat;
use iced::time::Duration;
use iced::widget::image::{self, Allocation};
use iced::{Point, Rectangle, Size, Task, Vector};
use resvg::usvg;
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::export::{self, ImageFile};
use crate::{
    AspectRatio, Background, BoardSize, Camera, DEFAULT_PARALLAX, Frame, Item, MAX_PARALLAX,
    MAX_ZOOM, Picture, Presentation, Shape, Transition, TransitionKind, background_scale,
    parse_color, rasterize_svg,
};

/// Extension of a saved presentation's file.
pub const EXTENSION: &str = "parousi";

/// Tells a saved presentation from other JSON.
const FORMAT: &str = "parousi";

/// Raised by changes to the format that older versions can't read.
const VERSION: u32 = 1;

/// Name of the presentation's description in the archive.
const MANIFEST: &str = "presentation.json";

/// What tells whether a file is a presentation this version can read,
/// before reading the rest, which a newer version may have changed.
#[derive(Deserialize)]
struct Header {
    format: String,
    version: u32,
}

/// The presentation as `presentation.json` describes it, in the same units
/// as in the editor. Images are named by their files in the archive.
#[derive(Serialize, Deserialize)]
struct Manifest {
    format: String,
    version: u32,
    /// See [`Presentation::view`].
    view: [f32; 2],
    board: SavedBoard,
    background: Option<SavedBackground>,
    frames: Vec<SavedFrame>,
    /// In the order they are drawn.
    items: Vec<SavedItem>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedBoard {
    size: [f32; 2],
    /// As typed, since it can't always be worked out from the size, or as
    /// its value over 1 while the inputs don't form a valid one.
    aspect_ratio: [f32; 2],
}

#[derive(Serialize, Deserialize)]
struct SavedBackground {
    image: String,
    /// See [`Background::parallax`]. Files from before it could be set have
    /// the one a background is picked with.
    #[serde(default = "default_parallax")]
    parallax: f32,
}

fn default_parallax() -> f32 {
    DEFAULT_PARALLAX
}

#[derive(Serialize, Deserialize)]
struct SavedFrame {
    camera: SavedCamera,
    transition: SavedTransition,
}

#[derive(Serialize, Deserialize)]
struct SavedCamera {
    zoom: f32,
    rotation: f32,
    offset: [f32; 2],
}

#[derive(Serialize, Deserialize)]
struct SavedTransition {
    kind: SavedTransitionKind,
    /// In seconds.
    duration: f32,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum SavedTransitionKind {
    Instant,
    Linear,
    Sine,
}

#[derive(Serialize, Deserialize)]
struct SavedItem {
    #[serde(flatten)]
    shape: SavedShape,
    rotation: f32,
}

/// A [`Shape`], tagged with its type. Colors are `#rrggbb`.
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum SavedShape {
    Polyline {
        points: Vec<[f32; 2]>,
        closed: bool,
        /// Files from before shapes could be filled have none filled.
        #[serde(default)]
        filled: bool,
        color: String,
    },
    Arrow {
        from: [f32; 2],
        to: [f32; 2],
        color: String,
    },
    Ellipse {
        center: [f32; 2],
        radii: [f32; 2],
        #[serde(default)]
        filled: bool,
        color: String,
    },
    Text {
        position: [f32; 2],
        content: String,
        size: f32,
        color: String,
    },
    Image {
        /// `[x, y, width, height]`.
        bounds: [f32; 4],
        image: String,
    },
}

impl Presentation {
    /// Writes the presentation to `path`, replacing the file there only once
    /// it is written in full.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let mut images = Images::default();
        let board_size = &self.board_size;

        let manifest = Manifest {
            format: FORMAT.to_owned(),
            version: VERSION,
            view: [self.view.width, self.view.height],
            board: SavedBoard {
                size: [board_size.size.width, board_size.size.height],
                aspect_ratio: saved_aspect_ratio(&board_size.aspect_ratio),
            },
            background: self.background.as_ref().map(|background| SavedBackground {
                image: images.add(&background.picture.file),
                parallax: background.parallax,
            }),
            frames: self.frames.iter().map(SavedFrame::new).collect(),
            items: self
                .items
                .iter()
                .map(|item| SavedItem::new(item, &mut images))
                .collect(),
        };

        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let mut archive = ZipWriter::new(Cursor::new(Vec::new()));

        archive.start_file(MANIFEST, options)?;
        serde_json::to_writer(&mut archive, &manifest)?;

        for (name, file) in &images.0 {
            archive.start_file(name, options)?;
            archive.write_all(match file {
                ImageFile::Raster { data, .. } => data,
                ImageFile::Vector(svg) => svg.as_bytes(),
            })?;
        }

        let archive = archive.finish()?.into_inner();

        // Into a file beside it first, so failing midway doesn't ruin the
        // one there.
        let mut temporary = path.as_os_str().to_owned();
        temporary.push(".tmp");

        fs::write(&temporary, archive)
            .and_then(|()| fs::rename(&temporary, path))
            .inspect_err(|_| {
                let _ = fs::remove_file(&temporary);
            })
    }
}

/// `aspect_ratio` as typed, or as its value over 1 while its inputs don't
/// form a valid one.
fn saved_aspect_ratio(aspect_ratio: &AspectRatio) -> [f32; 2] {
    match (
        aspect_ratio.width.parse::<f32>(),
        aspect_ratio.height.parse::<f32>(),
    ) {
        (Ok(width), Ok(height)) if width / height == aspect_ratio.value => [width, height],
        _ => [aspect_ratio.value, 1.0],
    }
}

/// The image files to write into the archive, each once however many times
/// it is used, by name.
#[derive(Default)]
struct Images(Vec<(String, ImageFile)>);

impl Images {
    /// Name of `file` in the archive, adding it unless it already is.
    fn add(&mut self, file: &ImageFile) -> String {
        if let Some((name, _)) = self.0.iter().find(|(_, added)| added == file) {
            return name.clone();
        }

        let extension = match file {
            // Which tells its format, see `ImageFile::Raster`.
            ImageFile::Raster { format, .. } => format.extensions_str()[0],
            ImageFile::Vector(_) => "svg",
        };

        let name = format!("images/{}.{extension}", self.0.len());
        self.0.push((name.clone(), file.clone()));
        name
    }
}

impl SavedFrame {
    fn new(frame: &Frame) -> Self {
        let Camera {
            zoom,
            rotation,
            offset,
        } = frame.camera;

        Self {
            camera: SavedCamera {
                zoom,
                rotation,
                offset: [offset.x, offset.y],
            },
            transition: SavedTransition {
                kind: match frame.transition.kind {
                    TransitionKind::Instant => SavedTransitionKind::Instant,
                    TransitionKind::Linear => SavedTransitionKind::Linear,
                    TransitionKind::Sine => SavedTransitionKind::Sine,
                },
                duration: frame.transition.duration.as_secs_f32(),
            },
        }
    }

    fn to_frame(&self) -> io::Result<Frame> {
        let SavedCamera {
            zoom,
            rotation,
            offset: [x, y],
        } = self.camera;

        if !(zoom > 0.0 && zoom.is_finite()) {
            return Err(invalid(format!("invalid zoom {zoom}")));
        }

        let seconds = self.transition.duration;
        let duration = Duration::try_from_secs_f32(seconds)
            .map_err(|error| invalid(format!("invalid duration {seconds}: {error}")))?;

        Ok(Frame {
            camera: Camera {
                zoom,
                rotation,
                offset: Vector::new(x, y),
            },
            transition: Transition {
                kind: match self.transition.kind {
                    SavedTransitionKind::Instant => TransitionKind::Instant,
                    SavedTransitionKind::Linear => TransitionKind::Linear,
                    SavedTransitionKind::Sine => TransitionKind::Sine,
                },
                duration,
                duration_input: seconds.to_string(),
            },
        })
    }
}

impl SavedItem {
    fn new(item: &Item, images: &mut Images) -> Self {
        let point = |point: Point| [point.x, point.y];

        let shape = match &item.shape {
            Shape::Polyline {
                points,
                closed,
                filled,
                color,
            } => SavedShape::Polyline {
                points: points.iter().copied().map(point).collect(),
                closed: *closed,
                filled: *filled,
                color: color.to_string(),
            },
            Shape::Arrow { from, to, color } => SavedShape::Arrow {
                from: point(*from),
                to: point(*to),
                color: color.to_string(),
            },
            Shape::Ellipse {
                center,
                radii,
                filled,
                color,
            } => SavedShape::Ellipse {
                center: point(*center),
                radii: [radii.x, radii.y],
                filled: *filled,
                color: color.to_string(),
            },
            Shape::Text {
                position,
                content,
                size,
                color,
            } => SavedShape::Text {
                position: point(*position),
                content: content.clone(),
                size: *size,
                color: color.to_string(),
            },
            Shape::Image { bounds, picture } => SavedShape::Image {
                bounds: [bounds.x, bounds.y, bounds.width, bounds.height],
                image: images.add(&picture.file),
            },
        };

        Self {
            shape,
            rotation: item.rotation,
        }
    }

    fn to_item(&self, pictures: &HashMap<&str, Picture>) -> io::Result<Item> {
        let point = |[x, y]: [f32; 2]| Point::new(x, y);
        let color = |color: &str| {
            parse_color(color).ok_or_else(|| invalid(format!("invalid color {color:?}")))
        };

        let shape = match &self.shape {
            SavedShape::Polyline {
                points,
                closed,
                filled,
                color: hex,
            } => {
                if *filled && !*closed {
                    return Err(invalid("an open polyline is filled"));
                }

                Shape::Polyline {
                    points: points.iter().copied().map(point).collect(),
                    closed: *closed,
                    filled: *filled,
                    color: color(hex)?,
                }
            }
            SavedShape::Arrow {
                from,
                to,
                color: hex,
            } => Shape::Arrow {
                from: point(*from),
                to: point(*to),
                color: color(hex)?,
            },
            SavedShape::Ellipse {
                center,
                radii: [x, y],
                filled,
                color: hex,
            } => Shape::Ellipse {
                center: point(*center),
                radii: Vector::new(*x, *y),
                filled: *filled,
                color: color(hex)?,
            },
            SavedShape::Text {
                position,
                content,
                size,
                color: hex,
            } => Shape::Text {
                position: point(*position),
                content: content.clone(),
                size: *size,
                color: color(hex)?,
            },
            SavedShape::Image {
                bounds: [x, y, width, height],
                image,
            } => Shape::Image {
                bounds: Rectangle::new(Point::new(*x, *y), Size::new(*width, *height)),
                picture: pictures[image.as_str()].clone(),
            },
        };

        Ok(Item {
            shape,
            rotation: self.rotation,
        })
    }
}

/// A presentation read from a file, whose images have yet to be allocated.
pub struct Opened {
    manifest: Manifest,
    /// Each image file the presentation uses, by name, ready for the
    /// renderer.
    images: Vec<(String, image::Handle, ImageFile)>,
}

/// Reads the presentation saved at `path`, preparing its images for the
/// renderer.
pub fn open(path: &Path) -> io::Result<Opened> {
    let mut archive = ZipArchive::new(BufReader::new(fs::File::open(path)?))?;
    let manifest = read(&mut archive, MANIFEST)?;

    let header: Header = serde_json::from_slice(&manifest)?;

    if header.format != FORMAT {
        return Err(invalid("not a Parousi presentation"));
    }

    if header.version > VERSION {
        return Err(invalid(format!(
            "saved in format version {}, which is newer than this Parousi reads",
            header.version
        )));
    }

    let manifest: Manifest = serde_json::from_slice(&manifest)?;

    if manifest.frames.is_empty() {
        return Err(invalid("the presentation has no frames"));
    }

    // Each image is read once, however many times it is used.
    let mut sources = HashMap::new();

    for (name, usage) in manifest.image_uses() {
        let source = match sources.entry(name) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(Source::read(&mut archive, name)?),
        };

        if let Source::Vector { tree, scale, .. } = source {
            let size = Size::new(tree.size().width(), tree.size().height());
            *scale = scale.max(usage.scale(size));
        }
    }

    let images = sources
        .into_iter()
        .map(|(name, source)| {
            let (handle, file) = source
                .into_image()
                .map_err(|error| invalid(format!("{name}: {error}")))?;
            Ok((name.to_owned(), handle, file))
        })
        .collect::<io::Result<_>>()?;

    Ok(Opened { manifest, images })
}

impl Opened {
    /// Allocates the images, so they show as soon as the presentation does,
    /// then puts the presentation together.
    pub fn allocate(self) -> Task<Result<Presentation, Arc<io::Error>>> {
        self.images
            .iter()
            .map(|(_, handle, _)| image::allocate(handle.clone()))
            .fold(Task::none(), Task::chain)
            .collect()
            .map(move |allocations| self.assemble(allocations).map_err(Arc::new))
    }

    /// The presentation, with the image files' `allocations` in the order of
    /// [`Opened::images`].
    fn assemble(
        &self,
        allocations: Vec<Result<Allocation, image::Error>>,
    ) -> io::Result<Presentation> {
        let mut pictures = HashMap::new();

        for ((name, _, file), allocation) in self.images.iter().zip(allocations) {
            let image = allocation.map_err(|error| invalid(format!("{name}: {error}")))?;

            pictures.insert(
                name.as_str(),
                Picture {
                    image,
                    file: file.clone(),
                },
            );
        }

        let manifest = &self.manifest;
        let [aspect_width, aspect_height] = manifest.board.aspect_ratio;
        let aspect_ratio = aspect_width / aspect_height;

        if !(aspect_width > 0.0 && aspect_height > 0.0 && aspect_ratio.is_normal()) {
            return Err(invalid(format!(
                "invalid aspect ratio {aspect_width}:{aspect_height}"
            )));
        }

        let size = sides(manifest.board.size)?;

        let background = match &manifest.background {
            Some(background) => {
                let parallax = background.parallax;

                if !(0.0..=MAX_PARALLAX).contains(&parallax) {
                    return Err(invalid(format!("invalid background parallax {parallax}")));
                }

                Some(Background::new(
                    pictures[background.image.as_str()].clone(),
                    parallax,
                ))
            }
            None => None,
        };

        Ok(Presentation {
            view: Size::new(manifest.view[0], manifest.view[1]),
            board_size: BoardSize {
                width: size.width.to_string(),
                height: size.height.to_string(),
                size,
                aspect_ratio: AspectRatio {
                    width: aspect_width.to_string(),
                    height: aspect_height.to_string(),
                    value: aspect_ratio,
                },
            },
            frames: manifest
                .frames
                .iter()
                .map(SavedFrame::to_frame)
                .collect::<io::Result<_>>()?,
            items: manifest
                .items
                .iter()
                .map(|item| item.to_item(&pictures))
                .collect::<io::Result<_>>()?,
            background,
        })
    }
}

impl Manifest {
    /// Each use of an image, by the image's name.
    fn image_uses(&self) -> impl Iterator<Item = (&str, Usage)> {
        let background = self
            .background
            .iter()
            .map(|background| (background.image.as_str(), Usage::Background));

        let items = self.items.iter().filter_map(|item| match &item.shape {
            SavedShape::Image { bounds, image } => {
                Some((image.as_str(), Usage::Item { width: bounds[2] }))
            }
            _ => None,
        });

        background.chain(items)
    }
}

/// Where an image is shown, which tells how sharp it has to be.
enum Usage {
    Background,
    Item { width: f32 },
}

impl Usage {
    /// Pixels per unit to rasterize a vector image of `size` at, as when it
    /// was picked.
    fn scale(&self, size: Size) -> f32 {
        match *self {
            Usage::Background => background_scale(size),
            Usage::Item { width } => width / size.width * MAX_ZOOM,
        }
    }
}

/// An image file read from the archive.
enum Source {
    Raster(image::Handle, ImageFile),
    /// Rasterized once every use has told the scale it needs, so it is
    /// sharp in all of them.
    Vector {
        svg: Arc<str>,
        tree: Box<usvg::Tree>,
        scale: f32,
    },
}

impl Source {
    fn read(archive: &mut ZipArchive<impl Read + Seek>, name: &str) -> io::Result<Self> {
        let data = read(archive, name)?;
        let in_name = |error: &dyn Error| invalid(format!("{name}: {error}"));

        if Path::new(name)
            .extension()
            .is_some_and(|extension| extension == "svg")
        {
            let svg: Arc<str> = String::from_utf8(data)
                .map_err(|error| in_name(&error))?
                .into();
            let tree = usvg::Tree::from_str(&svg, &usvg::Options::default())
                .map_err(|error| in_name(&error))?;

            return Ok(Source::Vector {
                svg,
                tree: Box::new(tree),
                scale: 0.0,
            });
        }

        let format = ImageFormat::from_path(name).map_err(|error| in_name(&error))?;

        // The renderer decodes an image in the format its data looks like,
        // which some (e.g. TGA) don't tell, so those are decoded here.
        let handle = if ::image::guess_format(&data).ok() == Some(format) {
            image::Handle::from_bytes(data.clone())
        } else {
            let image = export::decode(&data, format).map_err(|error| in_name(&error))?;
            image::Handle::from_rgba(image.width(), image.height(), image.into_raw())
        };

        Ok(Source::Raster(
            handle,
            ImageFile::Raster {
                data: data.into(),
                format,
            },
        ))
    }

    /// The image ready for the renderer, and its file.
    fn into_image(self) -> Result<(image::Handle, ImageFile), image::Error> {
        match self {
            Source::Raster(handle, file) => Ok((handle, file)),
            Source::Vector { svg, tree, scale } => {
                Ok((rasterize_svg(&tree, scale)?, ImageFile::Vector(svg)))
            }
        }
    }
}

/// Reads the file `name` out of `archive`.
fn read(archive: &mut ZipArchive<impl Read + Seek>, name: &str) -> io::Result<Vec<u8>> {
    let mut file = archive
        .by_name(name)
        .map_err(|error| invalid(format!("{name}: {error}")))?;

    let mut data = Vec::new();
    file.read_to_end(&mut data)?;
    Ok(data)
}

/// `[width, height]` as a size, of at least one unit each way.
fn sides([width, height]: [f32; 2]) -> io::Result<Size> {
    if width >= 1.0 && height >= 1.0 && width.is_finite() && height.is_finite() {
        Ok(Size::new(width, height))
    } else {
        Err(invalid(format!("invalid size {width} × {height}")))
    }
}

/// An error for a file that isn't a presentation this version can read.
fn invalid(error: impl Into<Box<dyn Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}
