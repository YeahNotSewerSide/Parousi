//! Exporting the presentation as a web page, which plays it full screen in a
//! browser.

use std::fs;
use std::io::{self, Cursor};
use std::path::Path;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use iced::Point;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, ImageResult, RgbaImage};
use resvg::usvg;
use serde_json::{Value, json};

use crate::{
    BOARD_COLOR, Item, LINE_WIDTH, Presentation, Shape, TEXT_LINE_HEIGHT, THEME, TransitionKind,
    arrow_head, workspace_color,
};

/// The page, which draws the presentation like the editor does. Its script
/// takes the presentation from in place of [`DATA`].
const PAGE: &str = include_str!("export.html");

/// Stands for the presentation in [`PAGE`].
const DATA: &str = "/* DATA */";

/// The file of a picture, which the page embeds.
#[derive(Debug, Clone, PartialEq)]
pub enum ImageFile {
    Raster {
        data: Arc<[u8]>,
        /// As told by the file extension, like the renderer does, since some
        /// formats (e.g. TGA) can't be told from the data.
        format: ImageFormat,
    },
    /// SVG with its text turned into paths and the images it links to
    /// embedded, so it shows the same in a browser as in the editor.
    Vector(Arc<str>),
}

impl ImageFile {
    pub fn raster(path: &Path) -> io::Result<Self> {
        Ok(Self::Raster {
            format: ImageFormat::from_path(path).map_err(io::Error::other)?,
            data: fs::read(path)?.into(),
        })
    }

    pub fn vector(tree: &usvg::Tree) -> Self {
        let options = usvg::WriteOptions {
            indent: usvg::Indent::None,
            ..usvg::WriteOptions::default()
        };

        Self::Vector(tree.to_string(&options).into())
    }

    /// The image as a data URL. A raster image in a format not every browser
    /// shows is converted to PNG.
    fn to_url(&self) -> io::Result<String> {
        let png;
        let (media_type, data) = match self {
            Self::Raster {
                data,
                format:
                    format @ (ImageFormat::Png
                    | ImageFormat::Jpeg
                    | ImageFormat::Gif
                    | ImageFormat::WebP
                    | ImageFormat::Bmp
                    | ImageFormat::Ico),
            } => (format.to_mime_type(), &**data),
            Self::Raster { data, format } => {
                png = to_png(data, *format).map_err(io::Error::other)?;
                ("image/png", png.as_slice())
            }
            Self::Vector(svg) => ("image/svg+xml", svg.as_bytes()),
        };

        Ok(format!("data:{media_type};base64,{}", BASE64.encode(data)))
    }
}

/// Decodes the raster image `data`, upright as the renderer shows it. The
/// renderer shows 8 bits per channel, whatever the image holds.
pub fn decode(data: &[u8], format: ImageFormat) -> ImageResult<RgbaImage> {
    let mut decoder = ImageReader::with_format(Cursor::new(data), format).into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok(image.into_rgba8())
}

/// Converts the raster image `data` to PNG, upright as the renderer shows it.
fn to_png(data: &[u8], format: ImageFormat) -> ImageResult<Vec<u8>> {
    // PNG can't hold every color type (e.g. HDR's floats), which decoding
    // leaves out anyway.
    let mut png = Vec::new();
    DynamicImage::from(decode(data, format)?)
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)?;
    Ok(png)
}

impl Presentation {
    /// Writes the presentation to `path` as a page that needs nothing else.
    pub fn export(&self, path: &Path) -> io::Result<()> {
        fs::write(path, self.to_page()?)
    }

    fn to_page(&self) -> io::Result<String> {
        let frames: Vec<Value> = self
            .frames
            .iter()
            .map(|frame| {
                let camera = frame.camera;
                let transition = &frame.transition;

                json!({
                    "camera": {
                        "zoom": camera.zoom,
                        "rotation": camera.rotation,
                        "offset": [camera.offset.x, camera.offset.y],
                    },
                    "transition": {
                        "kind": match transition.kind {
                            TransitionKind::Instant => "instant",
                            TransitionKind::Linear => "linear",
                            TransitionKind::Sine => "sine",
                        },
                        "duration": transition.duration.as_secs_f32(),
                    },
                })
            })
            .collect();

        let items = self
            .items
            .iter()
            .map(item_json)
            .collect::<io::Result<Vec<_>>>()?;

        let background = match &self.background {
            Some(background) => {
                // The page can't always tell the size of a vector image.
                let size = background.picture.size();
                json!({
                    "url": background.picture.file.to_url()?,
                    "size": [size.width, size.height],
                    "parallax": background.parallax,
                })
            }
            None => Value::Null,
        };

        let presentation = json!({
            "view": [self.view.width, self.view.height],
            "board": {
                "size": [self.board_size.size.width, self.board_size.size.height],
                "color": BOARD_COLOR.to_string(),
            },
            "frames": frames,
            "items": items,
            "line": {
                "width": LINE_WIDTH,
            },
            "text": {
                "lineHeight": TEXT_LINE_HEIGHT,
            },
            "background": background,
            "workspace": workspace_color(&THEME).to_string(),
        });

        // The data sits in a script, which a `</script>` in it would end.
        let data = presentation.to_string().replace("</", r"<\/");
        Ok(PAGE.replacen(DATA, &data, 1))
    }
}

/// `item` as the page draws it: its shape, in its own coordinates, turned by
/// its rotation around the board center. Arrows come as the shapes they are
/// drawn with, so the page doesn't work them out again.
fn item_json(item: &Item) -> io::Result<Value> {
    let point = |point: Point| json!([point.x, point.y]);

    let mut json = match &item.shape {
        Shape::Polyline {
            points,
            closed,
            filled,
            color,
        } => json!({
            "type": "polyline",
            "points": points.iter().copied().map(point).collect::<Vec<_>>(),
            "closed": closed,
            "filled": filled,
            "color": color.to_string(),
        }),
        Shape::Arrow { from, to, color } => {
            let (base, head) = arrow_head(*from, *to);
            json!({
                "type": "arrow",
                "shaft": [point(*from), point(base)],
                "head": head.map(point),
                "color": color.to_string(),
            })
        }
        Shape::Ellipse {
            center,
            radii,
            filled,
            color,
        } => json!({
            "type": "ellipse",
            "center": point(*center),
            "radii": [radii.x, radii.y],
            "filled": filled,
            "color": color.to_string(),
        }),
        Shape::Text {
            position,
            content,
            size,
            color,
        } => json!({
            "type": "text",
            "position": point(*position),
            "content": content,
            "size": size,
            "color": color.to_string(),
        }),
        Shape::Image { bounds, picture } => json!({
            "type": "image",
            "bounds": [bounds.x, bounds.y, bounds.width, bounds.height],
            "url": picture.file.to_url()?,
        }),
    };

    json["rotation"] = json!(item.rotation);
    Ok(json)
}
