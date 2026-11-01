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

use crate::typeface::{FontFile, Typeface};
use crate::{
    BACKGROUND_ZOOM_POWER, BOARD_COLOR, Item, LINE_WIDTH, MAX_BACKGROUND_SCALE, Presentation,
    Shape, TEXT_LINE_HEIGHT, THEME, TransitionKind, arrow_head, workspace_color,
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

        let mut fonts = Fonts::default();
        let items = self
            .items
            .iter()
            .map(|item| item_json(item, &mut fonts))
            .collect::<io::Result<Vec<_>>>()?;

        let background = match &self.background {
            Some(background) => {
                // The page can't always tell the size of a vector image.
                let size = background.picture.size();
                json!({
                    "url": background.picture.file.to_url()?,
                    "size": [size.width, size.height],
                    "parallax": background.parallax,
                    "zoomPower": BACKGROUND_ZOOM_POWER,
                    "maxScale": MAX_BACKGROUND_SCALE,
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
            "fonts": fonts.to_json(),
            "background": background,
            "workspace": workspace_color(&THEME).to_string(),
        });

        // The data sits in a script, which a `</script>` in it would end.
        let data = presentation.to_string().replace("</", r"<\/");
        Ok(PAGE.replacen(DATA, &data, 1))
    }
}

/// The font files the page embeds, each once however much text is written in
/// it.
#[derive(Default)]
struct Fonts(Vec<Arc<FontFile>>);

impl Fonts {
    /// The CSS font families text in `typeface` is written in, falling back
    /// to the default where the font is missing. A font file is embedded
    /// unless it already is.
    fn families(&mut self, typeface: &Typeface) -> String {
        let family = match typeface {
            Typeface::Default => return String::from(DEFAULT_FAMILY),
            Typeface::System(family) => css_string(family),
            Typeface::File(file) => {
                let index = match self.0.iter().position(|added| added == file) {
                    Some(index) => index,
                    None => {
                        self.0.push(file.clone());
                        self.0.len() - 1
                    }
                };
                css_string(&embedded_family(index))
            }
        };

        format!("{family}, {DEFAULT_FAMILY}")
    }

    /// The embedded fonts, as the page loads them.
    fn to_json(&self) -> Vec<Value> {
        self.0
            .iter()
            .enumerate()
            .map(|(index, file)| {
                json!({
                    "family": embedded_family(index),
                    "url": format!(
                        "data:font/{};base64,{}",
                        file.extension(),
                        BASE64.encode(&file.data),
                    ),
                })
            })
            .collect()
    }
}

/// The CSS font family text falls back to, like the editor's default font.
const DEFAULT_FAMILY: &str = "sans-serif";

/// Family the page names the embedded font file `index` by, apart from any
/// installed font of the family it goes by.
fn embedded_family(index: usize) -> String {
    format!("parousi-font-{index}")
}

/// `value` as a quoted CSS string.
fn css_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `item` as the page draws it: its shape, in its own coordinates, turned by
/// its rotation around the board center. Arrows come as the shapes they are
/// drawn with, so the page doesn't work them out again. Text names its font
/// as CSS does, embedding a font file in `fonts`.
fn item_json(item: &Item, fonts: &mut Fonts) -> io::Result<Value> {
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
            typeface,
            color,
        } => json!({
            "type": "text",
            "position": point(*position),
            "content": content,
            "size": size,
            "font": fonts.families(typeface),
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
