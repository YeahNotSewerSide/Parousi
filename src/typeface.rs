//! The fonts text is written in: the installed ones, and font files picked to
//! write in, which the presentation keeps.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use iced::advanced::graphics::text::cosmic_text::fontdb;
use iced::advanced::graphics::text::font_system;
use iced::{Font, Task};

/// File extensions of the font files text can be written in. Collections
/// (e.g. `.ttc`) are left out, as browsers can't show them.
pub const EXTENSIONS: &[&str] = &["ttf", "otf"];

/// The font text is written in.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Typeface {
    /// The renderer's sans-serif font.
    #[default]
    Default,
    /// An installed font family, by name. Where it isn't installed, text
    /// falls back to another font.
    System(&'static str),
    /// A font file picked to write in, which the presentation keeps, so it
    /// shows the same wherever it is opened.
    File(Arc<FontFile>),
}

impl Typeface {
    /// The installed font family named `family`.
    pub fn system(family: &str) -> Self {
        Self::System(intern(family))
    }

    /// The font the renderer writes in, by family.
    pub fn font(&self) -> Font {
        match self {
            Typeface::Default => Font::DEFAULT,
            Typeface::System(family) => Font::with_name(family),
            Typeface::File(file) => Font::with_name(file.family),
        }
    }
}

impl fmt::Display for Typeface {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Typeface::Default => f.write_str("Default"),
            Typeface::System(family) => f.write_str(family),
            Typeface::File(file) => write!(f, "{} (custom)", file.family),
        }
    }
}

/// The installed font families, sorted by name. The renderer finds them
/// where each platform keeps its fonts.
pub fn installed() -> Vec<Typeface> {
    let mut font_system = font_system().write().expect("Write font system");

    let families: BTreeSet<&str> = font_system
        .raw()
        .db()
        .faces()
        // Fonts loaded from memory are the renderer's own (e.g. its icons) or
        // picked from files.
        .filter(|face| !matches!(face.source, fontdb::Source::Binary(_)))
        .filter_map(|face| face.families.first())
        .map(|(family, _)| family.as_str())
        // Hidden ones, like macOS's interface fonts, can't be named in a
        // browser.
        .filter(|family| !family.starts_with('.'))
        .collect();

    let mut typefaces: Vec<_> = families.into_iter().map(Typeface::system).collect();
    typefaces.sort_by_cached_key(|typeface| typeface.to_string().to_lowercase());
    typefaces
}

/// A font file picked to write text in.
#[derive(PartialEq, Eq)]
pub struct FontFile {
    /// Family of the font, which text names it by.
    pub family: &'static str,
    pub data: Arc<[u8]>,
}

impl FontFile {
    /// The font in `data`, a TrueType or OpenType file.
    pub fn new(data: Arc<[u8]>) -> io::Result<Self> {
        let mut database = fontdb::Database::new();
        database.load_font_source(fontdb::Source::Binary(Arc::new(data.clone())));

        let (family, _) = database
            .faces()
            .next()
            .and_then(|face| face.families.first())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "not a font file"))?;

        Ok(Self {
            family: intern(family),
            data,
        })
    }

    pub fn read(path: &Path) -> io::Result<Self> {
        Self::new(fs::read(path)?.into())
    }

    /// Extension of the file's format: OpenType with PostScript outlines, or
    /// else TrueType.
    pub fn extension(&self) -> &'static str {
        if self.data.starts_with(b"OTTO") {
            "otf"
        } else {
            "ttf"
        }
    }

    /// Loads the font into the renderer, so text can be written in it.
    pub fn load<T>(&self) -> Task<T>
    where
        T: Send + 'static,
    {
        iced::font::load(self.data.to_vec()).discard()
    }
}

impl fmt::Debug for FontFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Not the data, which is far too long to read.
        f.debug_struct("FontFile")
            .field("family", &self.family)
            .finish_non_exhaustive()
    }
}

/// `name` as a `&'static str`, which [`Font`] names families by. Each name is
/// leaked once, however often it is asked for.
fn intern(name: &str) -> &'static str {
    static NAMES: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());

    let mut names = NAMES.lock().unwrap_or_else(PoisonError::into_inner);

    if let Some(&interned) = names.get(name) {
        return interned;
    }

    let interned: &'static str = Box::leak(name.into());
    names.insert(interned);
    interned
}
