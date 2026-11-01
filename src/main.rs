mod export;
mod save;
mod typeface;

use std::collections::VecDeque;
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io;
use std::iter;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use iced::advanced::mouse::{Click, click};
use iced::advanced::text::{self as advanced_text, Paragraph as _};
use iced::alignment::{Horizontal, Vertical};
use iced::keyboard::key::Named;
use iced::time::{Duration, Instant};
use iced::widget::canvas::{self, Canvas, Event, Geometry};
use iced::widget::text::LineHeight;
use iced::widget::{
    Column, Row, Stack, button, checkbox, column, combo_box, container, image, pick_list, row,
    rule, scrollable, sensor, space, text, text_input,
};
use iced::{
    Border, Center, Color, Element, Fill, Font, Point, Radians, Rectangle, Renderer, Size,
    Subscription, Task, Theme, Vector, event, keyboard, mouse, window,
};
use resvg::{tiny_skia, usvg};

use export::ImageFile;
use typeface::{FontFile, Typeface};

const THEME: Theme = Theme::Dark;

fn main() -> iced::Result {
    // A daemon rather than an application, so the about window can open
    // beside the main one.
    iced::daemon(Parousi::new, Parousi::update, Parousi::view)
        .subscription(Parousi::subscription)
        .title(Parousi::title)
        .theme(THEME)
        .run()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Mode {
    /// Dragging moves the camera.
    #[default]
    Move,
    /// Dragging turns the camera around the canvas center.
    Rotate,
}

impl Mode {
    const ALL: [Mode; 2] = [Mode::Move, Mode::Rotate];
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Mode::Move => "Move",
            Mode::Rotate => "Rotate",
        })
    }
}

/// What dragging and clicking on the board does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tool {
    /// Picks an item to move or delete. Dragging elsewhere moves the camera.
    #[default]
    Select,
    /// Moves the camera, even over items.
    Pan,
    Line,
    Arrow,
    /// Spans the rectangle dragged over.
    Rectangle,
    /// Fits the ellipse in the rectangle dragged over.
    Ellipse,
    /// Draws a regular polygon of [`Corners`] from its center.
    Polygon,
    /// Draws a star of [`Corners`] points from its center.
    Star,
    /// Draws freehand.
    Pencil,
    /// Draws straight segments between clicks.
    Path,
    /// Writes text where clicked.
    Text,
    /// Places an image in the area dragged over, or a default area around
    /// the click.
    Image,
}

impl Tool {
    /// Whether the tool draws closed shapes, which can be filled in, see
    /// [`Parousi::filled`]. Paths are only once closed.
    fn can_fill(self) -> bool {
        matches!(
            self,
            Tool::Rectangle | Tool::Ellipse | Tool::Polygon | Tool::Star | Tool::Path
        )
    }
}

impl fmt::Display for Tool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Tool::Select => "Select",
            Tool::Pan => "Pan",
            Tool::Line => "Line",
            Tool::Arrow => "Arrow",
            Tool::Rectangle => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Polygon => "Polygon",
            Tool::Star => "Star",
            Tool::Pencil => "Pencil",
            Tool::Path => "Path",
            Tool::Text => "Text",
            Tool::Image => "Image",
        })
    }
}

/// How to use the app, by topic, as the about window tells it.
const USAGE: [(&str, &[&str]); 8] = [
    (
        "Drawing",
        &[
            "Pick a tool under Objects and drag on the board to draw with it.",
            "Polygons and stars get their corners from the input under the shapes.",
            "Path: click to add points. Double-click or press Enter to finish, click the first \
             point to close it, or press Escape to drop it.",
            "Text: click where it goes and type. Press Escape or click elsewhere to finish.",
            "New text is written at the size and in the font set under Text. Pick an installed \
             font, or load one from a .ttf or .otf file with Font file.",
            "Image: drag out an area, or just click, then pick a file.",
            "New items are drawn in the color picked under Color, or typed in as #rrggbb.",
            "Tick Filled under Color to fill in new rectangles, ellipses, polygons, stars \
             and closed paths, rather than outline them.",
        ],
    ),
    (
        "Editing",
        &[
            "With Select, click an item to select it, and drag it to move it.",
            "Drag a square handle to resize it. Shift keeps its proportions.",
            "Drag the round handle to turn it. Shift turns it in 15° steps.",
            "Delete or Backspace deletes the selected item, and Escape deselects it.",
            "While an item is selected, Color and Text set its color, whether it is filled in, \
             and the size and font of text, rather than those of new items.",
        ],
    ),
    (
        "Camera",
        &[
            "The camera belongs to the selected frame: moving it changes that frame's view.",
            "Drag the board away from items, or anywhere with Pan, to move it.",
            "In Rotate mode, dragging turns it around the center instead.",
            "Ctrl + scroll zooms in and out around the cursor.",
        ],
    ),
    (
        "Turning slowly",
        &[
            "Turning the camera or an item slowly stops it at right angles, so it's easy to \
             set straight. Drag a little further to go on. Turning fast goes right through.",
        ],
    ),
    (
        "Frames",
        &[
            "Each frame in the timeline is a view of the board. The presentation shows them \
             one after another.",
            "+ adds a frame after the selected one, starting from its view. Delete removes \
             the selected frame.",
            "Between two frames, pick how the camera moves from one to the next, and how many \
             seconds it takes. Click a frame to go to it the same way.",
        ],
    ),
    (
        "Board",
        &[
            "Set the size of the board in the toolbar. With Aspect ratio ticked, changing one \
             side changes the other to match.",
            "Background fills the screen with an image in place of the board, so only the \
             drawings show over it, and Remove takes it away.",
            "Parallax sets how far the background drifts as the camera moves from one side \
             of the board to the other, in percent of the screen. At 0, it stays put.",
            "As the camera zooms in past the whole board, the background zooms in too, though \
             less, as if far behind it.",
        ],
    ),
    (
        "Saving",
        &[
            "Save keeps the presentation in a .parousi file, asking where the first time, and \
             Open opens one to go on editing it.",
            "Ctrl+S saves, Ctrl+Shift+S saves to another file, and Ctrl+O opens.",
            "Fonts loaded from files are kept in the presentation, so text shows in them \
             anywhere. Installed fonts are kept by name, so where one isn't installed, its text \
             shows in another font.",
        ],
    ),
    (
        "Presenting",
        &[
            "Export turns the presentation into a single web page.",
            "Open it in a browser, and step through the frames with the arrow keys, Page Up \
             and Page Down, or the buttons on the page.",
            "Ctrl+Z undoes, and Ctrl+Shift+Z or Ctrl+Y redoes.",
        ],
    ),
];

const PALETTE: [(&str, &[Tool]); 4] = [
    ("Selection", &[Tool::Select, Tool::Pan]),
    (
        "Shapes",
        &[
            Tool::Line,
            Tool::Arrow,
            Tool::Rectangle,
            Tool::Ellipse,
            Tool::Polygon,
            Tool::Star,
        ],
    ),
    ("Freehand", &[Tool::Pencil, Tool::Path]),
    ("Content", &[Tool::Text, Tool::Image]),
];

#[derive(Debug, Clone)]
enum Message {
    AspectWidthChanged(String),
    AspectHeightChanged(String),
    /// Enables or disables keeping the aspect ratio while resizing the
    /// board.
    AspectRatioToggled(bool),
    BoardWidthChanged(String),
    BoardHeightChanged(String),
    ModeSelected(Mode),
    ToolSelected(Tool),
    /// A color from [`COLORS`] was picked, for the selected item or new
    /// ones, like the rest of the [`Style`].
    ColorPicked(Color),
    /// The color input was edited.
    ColorInputChanged(String),
    /// Enables or disables filling in closed shapes.
    FilledToggled(bool),
    /// The text size input was edited.
    TextSizeChanged(String),
    TypefacePicked(Typeface),
    /// Opens a file dialog to pick a font file to write text in.
    PickFontFile,
    /// The picked font file has been read and loaded, or failed to be.
    FontFileLoaded(Result<Arc<FontFile>, Arc<io::Error>>),
    /// The corners input was edited.
    CornersChanged(String),
    FrameSelected(usize),
    /// Moves the selected frame's camera by a screen offset.
    CameraMoved(Vector),
    /// Turns the selected frame's camera to this rotation, see
    /// [`Camera::rotate`].
    CameraRotated(f32),
    /// Zooms the selected frame's camera, see [`Camera::zoom_at`].
    CameraZoomed {
        anchor: Vector,
        factor: f32,
    },
    /// The camera started being dragged.
    CameraGrabbed,
    /// The camera or the selected item stopped being dragged.
    Released,
    ItemDrawn(Item),
    /// An item, or a handle of its outline, was clicked to select it, which
    /// starts dragging it, or the selection was cleared.
    ItemSelected(Option<usize>),
    /// The selected item was moved, turned or resized into this, while
    /// being dragged.
    ItemEdited(Item),
    /// Deletes the selected item.
    DeleteItem,
    /// Opens a file dialog to pick an image to place in an area of the
    /// board.
    PlaceImage(Rectangle),
    /// The picked image has loaded, or failed to.
    ImageLoaded {
        area: Rectangle,
        result: Result<Picture, image::Error>,
    },
    /// Opens a file dialog to pick the background image.
    PickBackground,
    /// The picked background image has loaded, or failed to.
    BackgroundLoaded(Result<Picture, image::Error>),
    RemoveBackground,
    BackgroundParallaxChanged(String),
    TransitionKindSelected {
        frame: usize,
        kind: TransitionKind,
    },
    TransitionDurationChanged {
        frame: usize,
        value: String,
    },
    /// A window frame is about to be drawn during a transition.
    Tick(Instant),
    /// The workspace has been laid out with a new size.
    WorkspaceResized(Size),
    Undo,
    Redo,
    /// Opens a file dialog to pick a presentation to open.
    Open,
    /// The picked presentation has been read, or failed to.
    Opened(Result<(PathBuf, Box<Presentation>), Arc<io::Error>>),
    /// Saves the presentation to its file, or asks for one without one.
    Save,
    /// Opens a file dialog to save the presentation to another file.
    SaveAs,
    /// The presentation has been saved to this file, or failed to.
    Saved(Result<PathBuf, Arc<io::Error>>),
    /// Opens a file dialog to export the presentation as a web page.
    Export,
    /// The exported page has been written, or failed to.
    Exported(Result<(), Arc<io::Error>>),
    /// Opens the about window, or brings it to the front.
    About,
    WindowClosed(window::Id),
    DeleteFrame,
    AddFrame,
}

struct Parousi {
    /// The editor's window, closing which quits.
    main_window: window::Id,
    about_window: Option<window::Id>,
    board_size: BoardSize,
    /// Whether resizing one side of the board resizes the other to keep the
    /// aspect ratio, rather than changing it. Like the tool, it is how the
    /// presentation is edited, so it isn't undone.
    keep_aspect_ratio: bool,
    mode: Mode,
    tool: Tool,
    /// How new items are drawn.
    style: Style,
    /// The selected item's style as the panel shows it, with its inputs as
    /// typed. Changing it changes the item.
    selection_style: Style,
    corners: Corners,
    /// Fonts to pick for text: the default, the font files loaded to write
    /// in, then the installed fonts.
    typefaces: combo_box::State<Typeface>,
    /// Never empty, so there is always a camera to edit.
    frames: Vec<Frame>,
    /// Index of the selected frame.
    frame: usize,
    /// Shared by all frames, in the order they are drawn.
    items: Vec<Item>,
    /// Index of the item picked with [`Tool::Select`].
    selected: Option<usize>,
    /// An image filling the screen in place of the board, drifting and
    /// zooming a little as the camera moves for a sense of depth. Shared by
    /// all frames.
    background: Option<Background>,
    /// Camera move to the selected frame, while a transition plays.
    flight: Option<Flight>,
    history: History,
    /// Size of the workspace, whose view of each frame the exported
    /// presentation shows.
    workspace: Size,
    /// File the presentation was opened from or last saved to, which saving
    /// writes to.
    path: Option<PathBuf>,
}

/// The presentation apart from how it is being edited, as saved and
/// exported. It is taken as it is, so it can be written out while editing
/// goes on.
#[derive(Debug, Clone)]
struct Presentation {
    /// Size of the editor's workspace. The exported page shows what it shows
    /// of each frame, scaled to fit the browser window.
    view: Size,
    board_size: BoardSize,
    frames: Vec<Frame>,
    items: Vec<Item>,
    background: Option<Background>,
}

impl Parousi {
    /// An empty presentation, and the task opening its window.
    fn new() -> (Self, Task<Message>) {
        let (main_window, open) = window::open(window::Settings {
            size: Size::new(1280.0, 800.0),
            ..window::Settings::default()
        });

        let parousi = Self {
            main_window,
            about_window: None,
            board_size: BoardSize::default(),
            keep_aspect_ratio: true,
            mode: Mode::default(),
            tool: Tool::default(),
            style: Style::default(),
            selection_style: Style::default(),
            corners: Corners::default(),
            typefaces: combo_box::State::new(
                iter::once(Typeface::Default)
                    .chain(typeface::installed())
                    .collect(),
            ),
            frames: vec![Frame::default()],
            frame: 0,
            items: Vec::new(),
            selected: None,
            background: None,
            flight: None,
            history: History::default(),
            workspace: Size::ZERO,
            path: None,
        };

        (parousi, open.discard())
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::AspectWidthChanged(width) => {
                let from = self.board_size.clone();
                let to = from.with_aspect_ratio(width, from.aspect_ratio.height.clone());
                self.perform(Action::BoardSize { from, to });
            }
            Message::AspectHeightChanged(height) => {
                let from = self.board_size.clone();
                let to = from.with_aspect_ratio(from.aspect_ratio.width.clone(), height);
                self.perform(Action::BoardSize { from, to });
            }
            Message::AspectRatioToggled(keep) => {
                // Resizing after toggling is an edit of its own.
                self.history.seal();
                self.keep_aspect_ratio = keep;
            }
            Message::BoardWidthChanged(width) => {
                let from = self.board_size.clone();
                let to = from.with_width(width, self.keep_aspect_ratio);
                self.perform(Action::BoardSize { from, to });
            }
            Message::BoardHeightChanged(height) => {
                let from = self.board_size.clone();
                let to = from.with_height(height, self.keep_aspect_ratio);
                self.perform(Action::BoardSize { from, to });
            }
            Message::ModeSelected(mode) => self.mode = mode,
            Message::ToolSelected(tool) => {
                self.tool = tool;
                // Only the select tool shows the selection.
                if tool != Tool::Select {
                    self.selected = None;
                }
            }
            // Typing goes on with an edit of the selected item, but picking is
            // an edit of its own.
            Message::ColorPicked(color) => self.restyle_once(|style| style.ink = Ink::new(color)),
            Message::ColorInputChanged(input) => self.restyle(|style| style.ink.set_input(input)),
            Message::FilledToggled(filled) => self.restyle_once(|style| style.filled = filled),
            Message::TextSizeChanged(input) => {
                self.restyle(|style| style.text_size.set_input(input));
            }
            Message::TypefacePicked(typeface) => {
                self.restyle_once(|style| style.typeface = typeface);
            }
            Message::PickFontFile => {
                let dialog = rfd::AsyncFileDialog::new()
                    .set_title("Font")
                    .add_filter("Fonts", typeface::EXTENSIONS)
                    .pick_file();

                return Task::future(async move {
                    let path = dialog.await?.path().to_owned();
                    Some(FontFile::read(&path).map(Arc::new).map_err(Arc::new))
                })
                .and_then(Task::done)
                // Before it is picked, so text shows in it right away.
                .and_then(|file| file.load().chain(Task::done(Ok(file))))
                .map(Message::FontFileLoaded);
            }
            Message::FontFileLoaded(Ok(file)) => {
                let file = self.add_font_file(file);
                self.restyle_once(|style| style.typeface = Typeface::File(file));
            }
            Message::FontFileLoaded(Err(error)) => {
                eprintln!("Couldn't load the font: {error}");
            }
            // Like the tool, the corners are how new items are drawn rather
            // than part of the presentation, so they aren't undone.
            Message::CornersChanged(input) => self.corners.set_input(input),
            Message::FrameSelected(frame) => {
                // Editing the newly selected frame shouldn't merge into an
                // earlier edit of it.
                self.history.seal();
                self.select_frame(frame);
            }
            Message::CameraMoved(delta) => {
                let from = self.frames[self.frame].camera;
                let to = Camera {
                    offset: from.offset + delta,
                    ..from
                };
                self.perform(Action::Camera {
                    frame: self.frame,
                    from,
                    to,
                });
            }
            Message::CameraRotated(rotation) => {
                let from = self.frames[self.frame].camera;
                let mut to = from;
                to.rotate(wrapped(rotation - from.rotation));
                self.perform(Action::Camera {
                    frame: self.frame,
                    from,
                    to,
                });
            }
            Message::CameraZoomed { anchor, factor } => {
                let from = self.frames[self.frame].camera;
                let mut to = from;
                to.zoom_at(anchor, factor);
                self.perform(Action::Camera {
                    frame: self.frame,
                    from,
                    to,
                });
            }
            // A drag undoes on its own, apart from any scrolling before or
            // after it.
            Message::CameraGrabbed | Message::Released => self.history.seal(),
            Message::ItemDrawn(item) => self.perform(Action::ItemAdded {
                index: self.items.len(),
                item,
            }),
            Message::ItemSelected(selected) => {
                self.history.seal();
                self.selected = selected;
                // The panel shows the item's style, and new items' where it has
                // none (e.g. a text size, on a shape).
                self.selection_style = self.style.clone();
                self.follow_selection();
            }
            Message::ItemEdited(to) => {
                if let Some(index) = self.selected {
                    let from = self.items[index].clone();
                    self.perform(Action::ItemChanged { index, from, to });
                }
            }
            Message::DeleteItem => {
                if let Some(index) = self.selected {
                    self.perform(Action::ItemRemoved {
                        index,
                        item: self.items[index].clone(),
                    });
                }
            }
            Message::PlaceImage(area) => {
                // As sharp as the image can show, once fitted in the area.
                let scale = move |size| fit(size, area).width / size.width * MAX_ZOOM;

                return pick_picture("Image", scale)
                    .map(move |result| Message::ImageLoaded { area, result });
            }
            Message::ImageLoaded {
                area,
                result: Ok(picture),
            } => self.perform(Action::ItemAdded {
                index: self.items.len(),
                item: Item::new(Shape::Image {
                    bounds: fit(picture.size(), area),
                    picture,
                }),
            }),
            Message::ImageLoaded {
                result: Err(error), ..
            } => {
                eprintln!("Couldn't load the image: {error}");
            }
            Message::PickBackground => {
                return pick_picture("Background", background_scale).map(Message::BackgroundLoaded);
            }
            Message::BackgroundLoaded(Ok(picture)) => {
                // A new image drifts as far as the one it replaces.
                let parallax = self
                    .background
                    .as_ref()
                    .map_or(DEFAULT_PARALLAX, |background| background.parallax);

                self.perform(Action::Background {
                    from: self.background.clone(),
                    to: Some(Background::new(picture, parallax)),
                });
            }
            Message::BackgroundLoaded(Err(error)) => {
                eprintln!("Couldn't load the background: {error}");
            }
            Message::RemoveBackground => self.perform(Action::Background {
                from: self.background.clone(),
                to: None,
            }),
            Message::BackgroundParallaxChanged(parallax) => {
                if let Some(from) = &self.background {
                    let to = from.with_parallax(parallax);
                    self.perform(Action::Background {
                        from: Some(from.clone()),
                        to: Some(to),
                    });
                }
            }
            Message::TransitionKindSelected { frame, kind } => {
                let from = self.frames[frame].transition.clone();
                let to = Transition {
                    kind,
                    ..from.clone()
                };
                self.perform(Action::Transition { frame, from, to });
            }
            Message::TransitionDurationChanged { frame, value } => {
                let from = self.frames[frame].transition.clone();
                let mut to = from.clone();
                to.set_duration(value);
                self.perform(Action::Transition { frame, from, to });
            }
            Message::Tick(now) => {
                if self
                    .flight
                    .as_mut()
                    .is_some_and(|flight| flight.advance(now))
                {
                    self.flight = None;
                }
            }
            Message::AddFrame => {
                // Start from the selected camera, so the new frame can be
                // moved on from where the presentation currently is.
                self.perform(Action::FrameAdded {
                    index: self.frame + 1,
                    frame: self.frames[self.frame].clone(),
                });
            }
            Message::DeleteFrame => {
                if self.frames.len() > 1 {
                    self.perform(Action::FrameDeleted {
                        index: self.frame,
                        frame: self.frames[self.frame].clone(),
                    });
                }
            }
            // Undoing and redoing jump straight to the result.
            Message::Undo => {
                if let Some(action) = self.history.undo() {
                    self.flight = None;
                    self.apply(action);
                }
            }
            Message::Redo => {
                if let Some(action) = self.history.redo() {
                    self.flight = None;
                    self.apply(action);
                }
            }
            Message::WorkspaceResized(size) => self.workspace = size,
            Message::Open => {
                let dialog = rfd::AsyncFileDialog::new()
                    .set_title("Open")
                    .add_filter("Presentation", &[save::EXTENSION])
                    .pick_file();

                // Reading the images can take a while, so the file isn't read
                // in `update`.
                return Task::future(async move {
                    let path = dialog.await?.path().to_owned();
                    Some(
                        save::open(&path)
                            .map(|opened| (path, opened))
                            .map_err(Arc::new),
                    )
                })
                .and_then(Task::done)
                .and_then(|(path, opened)| {
                    opened
                        .allocate()
                        .map(move |presentation| Ok((path.clone(), Box::new(presentation?))))
                })
                .map(Message::Opened);
            }
            Message::Opened(Ok((path, presentation))) => {
                // The workspace stays as big as the window, whatever it was
                // when saved.
                let Presentation {
                    view: _,
                    board_size,
                    frames,
                    items,
                    background,
                } = *presentation;

                // Their font files can be picked for other text too.
                for item in &items {
                    if let Shape::Text {
                        typeface: Typeface::File(file),
                        ..
                    } = &item.shape
                    {
                        self.add_font_file(file.clone());
                    }
                }

                self.board_size = board_size;
                self.frames = frames;
                self.frame = 0;
                self.items = items;
                self.selected = None;
                self.background = background;
                self.flight = None;
                // Undoing goes no further back than the opened presentation.
                self.history = History::default();
                self.path = Some(path);
            }
            Message::Opened(Err(error)) => {
                eprintln!("Couldn't open the presentation: {error}");
            }
            Message::Save => return self.save(self.path.clone()),
            Message::SaveAs => return self.save(None),
            Message::Saved(Ok(path)) => self.path = Some(path),
            Message::Saved(Err(error)) => {
                eprintln!("Couldn't save the presentation: {error}");
            }
            Message::Export => {
                let presentation = self.presentation();
                let dialog = rfd::AsyncFileDialog::new()
                    .set_title("Export")
                    .add_filter("Web page", &["html", "htm"])
                    .set_file_name("presentation.html")
                    .save_file();

                // Embedding the background can take a while, so the page
                // isn't written in `update`.
                return Task::future(async move {
                    let file = dialog.await?;
                    Some(presentation.export(file.path()).map_err(Arc::new))
                })
                .and_then(Task::done)
                .map(Message::Exported);
            }
            Message::Exported(Ok(())) => {}
            Message::Exported(Err(error)) => {
                eprintln!("Couldn't export the presentation: {error}");
            }
            Message::About => {
                if let Some(about) = self.about_window {
                    return window::gain_focus(about);
                }

                let (about, open) = window::open(window::Settings {
                    size: Size::new(560.0, 680.0),
                    min_size: Some(Size::new(360.0, 280.0)),
                    ..window::Settings::default()
                });
                self.about_window = Some(about);
                return open.discard();
            }
            Message::WindowClosed(window) => {
                if window == self.main_window {
                    return iced::exit();
                }

                if self.about_window == Some(window) {
                    self.about_window = None;
                }
            }
        }

        Task::none()
    }

    fn subscription(&self) -> Subscription<Message> {
        // Only keys pressed in the main window, so they don't edit the
        // presentation from the about window.
        let shortcuts = event::listen_with(|event, status, window| match (event, status) {
            (Event::Keyboard(event), event::Status::Ignored) => Some((window, event)),
            _ => None,
        })
        .with(self.main_window)
        .filter_map(|(main, (window, event))| (window == main).then(|| shortcut(event))?);

        let closed = window::close_events().map(Message::WindowClosed);

        if self.flight.is_some() {
            Subscription::batch([shortcuts, closed, window::frames().map(Message::Tick)])
        } else {
            Subscription::batch([shortcuts, closed])
        }
    }

    fn title(&self, window: window::Id) -> String {
        if self.about_window == Some(window) {
            String::from("About Parousi")
        } else if let Some(name) = self.path.as_deref().and_then(Path::file_stem) {
            format!("{} — Parousi", name.to_string_lossy())
        } else {
            String::from("Parousi")
        }
    }

    /// The presentation as it is now, to write out.
    fn presentation(&self) -> Presentation {
        Presentation {
            view: self.workspace,
            board_size: self.board_size.clone(),
            frames: self.frames.clone(),
            items: self.items.clone(),
            background: self.background.clone(),
        }
    }

    /// Saves the presentation to `path`, or to a file asked for without one.
    fn save(&self, path: Option<PathBuf>) -> Task<Message> {
        let mut dialog = rfd::AsyncFileDialog::new()
            .set_title("Save")
            .add_filter("Presentation", &[save::EXTENSION])
            .set_file_name(format!("presentation.{}", save::EXTENSION));

        // Starting from the presentation's file, if it has one.
        if let Some(current) = &self.path
            && let (Some(directory), Some(name)) = (current.parent(), current.file_name())
        {
            dialog = dialog
                .set_directory(directory)
                .set_file_name(name.to_string_lossy());
        }

        let presentation = self.presentation();

        // Writing the images can take a while, so the file isn't written in
        // `update`.
        Task::future(async move {
            let path = match path {
                Some(path) => path,
                None => dialog.save_file().await?.path().to_owned(),
            };

            Some(presentation.save(&path).map(|()| path).map_err(Arc::new))
        })
        .and_then(Task::done)
        .map(Message::Saved)
    }

    /// Applies `action` and records it, so it can be undone.
    fn perform(&mut self, action: Action) {
        if action.is_noop() {
            return;
        }

        self.history.record(action.clone());
        self.apply(action);
    }

    /// Applies `action` to the presentation, selecting the frame it changes.
    fn apply(&mut self, action: Action) {
        match action {
            Action::BoardSize { to, .. } => self.board_size = to,
            Action::Background { to, .. } => self.background = to,
            Action::Camera { frame, to, .. } => {
                // Lands any transition, so the camera shows the edit.
                self.flight = None;
                self.frame = frame;
                self.frames[frame].camera = to;
            }
            Action::Transition { frame, to, .. } => self.frames[frame].transition = to,
            // The selection stays on the same item as others come and go.
            Action::ItemAdded { index, item } => {
                self.items.insert(index, item);
                if let Some(selected) = &mut self.selected
                    && *selected >= index
                {
                    *selected += 1;
                }
            }
            Action::ItemRemoved { index, .. } => {
                self.items.remove(index);
                self.selected = match self.selected {
                    Some(selected) if selected == index => None,
                    Some(selected) if selected > index => Some(selected - 1),
                    selected => selected,
                };
            }
            Action::ItemChanged { index, to, .. } => self.items[index] = to,
            Action::FrameAdded { index, frame } => {
                self.frames.insert(index, frame);
                self.frame = index;
            }
            Action::FrameDeleted { index, .. } => {
                self.frames.remove(index);
                self.frame = index.saturating_sub(1);
                self.flight = None;
            }
        }

        // E.g. undoing a change of its color, or resizing text.
        self.follow_selection();
    }

    /// Changes the style the panel shows with `change`: the selected item's,
    /// which changes the item, or else new items'. Like the tool, new items'
    /// style is how the presentation is edited, so changing it isn't undone.
    fn restyle(&mut self, change: impl FnOnce(&mut Style)) {
        let Some(index) = self.selected else {
            change(&mut self.style);
            return;
        };

        change(&mut self.selection_style);

        let from = self.items[index].clone();
        let to = Item {
            shape: from.shape.styled(&self.selection_style),
            rotation: from.rotation,
        };
        self.perform(Action::ItemChanged { index, from, to });
    }

    /// Like [`Parousi::restyle`], as an edit of its own, which typing doesn't
    /// go on with.
    fn restyle_once(&mut self, change: impl FnOnce(&mut Style)) {
        self.history.seal();
        self.restyle(change);
        self.history.seal();
    }

    /// Makes the panel show the selected item's style, keeping what is typed
    /// in where it still holds.
    fn follow_selection(&mut self) {
        if let Some(item) = self.selected.and_then(|index| self.items.get(index)) {
            self.selection_style.follow(&item.shape);
        }
    }

    /// The style the panel shows and changes, see [`Parousi::restyle`], and
    /// which of its settings apply.
    fn panel_style(&self) -> (&Style, Settings) {
        match self.selected.and_then(|index| self.items.get(index)) {
            Some(item) => (&self.selection_style, item.shape.settings()),
            None => (
                &self.style,
                Settings {
                    color: true,
                    fill: self.tool.can_fill(),
                    text: self.tool == Tool::Text,
                },
            ),
        }
    }

    /// Adds `file` to the fonts to pick, unless it already is, returning the
    /// one there.
    fn add_font_file(&mut self, file: Arc<FontFile>) -> Arc<FontFile> {
        let typefaces = self.typefaces.options();

        if let Some(added) = typefaces.iter().find_map(|typeface| match typeface {
            Typeface::File(added) if *added == file => Some(added.clone()),
            _ => None,
        }) {
            return added;
        }

        // After the default font and the other font files.
        let at = typefaces
            .iter()
            .take_while(|typeface| !matches!(typeface, Typeface::System(_)))
            .count();

        let mut typefaces = typefaces.to_vec();
        typefaces.insert(at, Typeface::File(file.clone()));
        self.typefaces = combo_box::State::new(typefaces);
        file
    }

    /// Selects `frame`, moving the camera there with the transition of the
    /// gap between it and the previously selected frame.
    fn select_frame(&mut self, frame: usize) {
        if frame == self.frame {
            return;
        }

        // A frame owns the transition from the frame before it. Moving back
        // crosses the gap after the target instead, so stepping back and
        // forth between two frames plays the same transition both ways.
        let transition = if frame > self.frame {
            &self.frames[frame].transition
        } else {
            &self.frames[frame + 1].transition
        };

        // Starting from the camera shown, rather than the previous frame's,
        // keeps the move smooth when a transition is cut short.
        self.flight = Flight::new(self.camera(), transition);
        self.frame = frame;
    }

    /// Camera the board is shown through: the selected frame's, or one on
    /// its way there while a transition plays.
    fn camera(&self) -> Camera {
        let camera = self.frames[self.frame].camera;
        self.flight.map_or(camera, |flight| flight.camera(camera))
    }

    fn board(&self) -> Board {
        Board {
            size: self.board_size.size,
        }
    }

    fn view(&self, window: window::Id) -> Element<'_, Message> {
        if self.about_window == Some(window) {
            return self.about();
        }

        column![
            row![self.workspace(), self.palette()].height(Fill),
            rule::horizontal(1),
            self.toolbar(),
            rule::horizontal(1),
            self.timeline(),
        ]
        .into()
    }

    /// What Parousi is, and how to use it.
    fn about(&self) -> Element<'_, Message> {
        let sections = USAGE.iter().map(|(title, lines)| {
            column![
                text(*title).size(16),
                Column::with_children(lines.iter().map(|&line| text(line).into())).spacing(6),
            ]
            .spacing(8)
            .into()
        });

        scrollable(
            column![
                column![
                    text("Parousi").size(28),
                    text(concat!("Version ", env!("CARGO_PKG_VERSION")))
                        .size(14)
                        .style(text::secondary),
                ]
                .spacing(4),
                text(
                    "Draw on a board, then present it as a series of views of it, with the \
                     camera moving, zooming and turning from one to the next."
                ),
                rule::horizontal(1),
                Column::with_children(sections).spacing(20),
            ]
            .spacing(16)
            .padding(24),
        )
        .into()
    }

    fn workspace(&self) -> Element<'_, Message> {
        let camera = self.camera();
        let board = self.board();

        // Under the items, the background fills the canvas in place of the
        // board, which nothing marks out then, so only the drawings show over
        // it, as on the exported page.
        let backdrop: Element<'_, Message> = match &self.background {
            Some(background) => Canvas::new(BackgroundView {
                background,
                board,
                camera,
            })
            .width(Fill)
            .height(Fill)
            .into(),
            None => Canvas::new(BoardView { board, camera })
                .width(Fill)
                .height(Fill)
                .into(),
        };

        // A canvas draws its images over its shapes, so the items are split
        // into runs that a canvas each draws in order, see
        // [`Item::shares_canvas`].
        let items = self.items.chunk_by(Item::shares_canvas).map(|items| {
            Canvas::new(ItemsView { camera, items })
                .width(Fill)
                .height(Fill)
                .into()
        });

        // On top, so what is being drawn shows over everything.
        let tool = ToolView {
            board,
            camera,
            items: &self.items,
            selected: self.selected,
            mode: self.mode,
            tool: self.tool,
            style: &self.style,
            corners: self.corners.count,
        };

        let layers = Stack::with_children(
            iter::once(backdrop)
                .chain(items)
                .chain([Canvas::new(tool).width(Fill).height(Fill).into()]),
        )
        .width(Fill)
        .height(Fill);

        sensor(
            container(layers)
                .width(Fill)
                .height(Fill)
                .style(|theme: &Theme| container::background(workspace_color(theme))),
        )
        .on_resize(Message::WorkspaceResized)
        .into()
    }

    fn palette(&self) -> Element<'_, Message> {
        let sections = PALETTE.iter().map(|(title, tools)| {
            let rows = tools.chunks(2).map(|pair| {
                Row::with_children(pair.iter().map(|&tool| self.tool_button(tool)))
                    .spacing(8)
                    .into()
            });

            // Under the tools it sets the corners of.
            let corners = tools.contains(&Tool::Polygon).then(|| self.corners_input());

            column![
                text(*title).size(14),
                Column::with_children(rows.chain(corners)).spacing(8)
            ]
            .spacing(8)
            .into()
        });

        container(scrollable(
            column![
                text("Objects")
                    .size(20)
                    .width(Fill)
                    .align_x(Horizontal::Center),
                Column::with_children(sections.chain([self.color_picker(), self.text_settings()]))
                    .spacing(20),
            ]
            .spacing(16)
            .padding(16),
        ))
        .width(300)
        .height(Fill)
        .style(|theme: &Theme| {
            container::background(theme.extended_palette().background.weak.color)
        })
        .into()
    }

    fn tool_button(&self, tool: Tool) -> Element<'_, Message> {
        button(text(tool.to_string()).width(Fill).center())
            .width(Fill)
            .style(if self.tool == tool {
                button::primary
            } else {
                button::secondary
            })
            .on_press(Message::ToolSelected(tool))
            .into()
    }

    /// The preset colors to draw in, an input for any other, and whether to
    /// fill closed shapes in with it.
    fn color_picker(&self) -> Element<'_, Message> {
        let (style, settings) = self.panel_style();

        let swatches = COLORS.chunks(6).map(|colors| {
            Row::with_children(
                colors
                    .iter()
                    .map(|&color| swatch(color, &style.ink, settings.color)),
            )
            .spacing(8)
            .into()
        });

        let color = style.ink.color;
        let shown = container(space())
            .width(30)
            .height(30)
            .style(move |theme: &Theme| container::Style {
                background: Some(color.into()),
                border: swatch_border(theme),
                ..container::Style::default()
            });

        let input = text_input("#rrggbb", &style.ink.input)
            .on_input_maybe(settings.color.then_some(Message::ColorInputChanged))
            .style(input_style(style.ink.is_valid()));

        let filled = checkbox(style.filled)
            .label("Filled")
            .on_toggle_maybe(settings.fill.then_some(Message::FilledToggled));

        column![
            self.style_title("Color"),
            Column::with_children(swatches).spacing(8),
            row![shown, input].spacing(8).align_y(Center),
            filled,
        ]
        .spacing(8)
        .into()
    }

    /// The size and font to write text in, and a button to load a font from a
    /// file.
    fn text_settings(&self) -> Element<'_, Message> {
        let (style, settings) = self.panel_style();
        let enabled = settings.text;

        let size = text_input("", &style.text_size.input)
            .on_input_maybe(enabled.then_some(Message::TextSizeChanged))
            .style(input_style(style.text_size.is_valid()));

        // A combo box can't be disabled, so a disabled input shows the font
        // instead.
        let font: Element<'_, Message> = if enabled {
            combo_box(
                &self.typefaces,
                "Font",
                Some(&style.typeface),
                Message::TypefacePicked,
            )
            .menu_height(FONT_MENU_HEIGHT)
            .into()
        } else {
            text_input("Font", &style.typeface.to_string()).into()
        };

        let file = button(text("Font file").width(Fill).center())
            .width(Fill)
            .style(button::secondary)
            .on_press_maybe(enabled.then_some(Message::PickFontFile));

        column![
            self.style_title("Text"),
            row![text("Size"), size].spacing(8).align_y(Center),
            font,
            file,
        ]
        .spacing(8)
        .into()
    }

    /// Title of a section of the panel setting the style, which tells when it
    /// is the selected item's.
    fn style_title<'a>(&self, title: &'a str) -> Element<'a, Message> {
        let title = text(title).size(14);

        if self.selected.is_some() {
            row![
                title,
                space::horizontal(),
                text("Selected item").size(14).style(text::secondary),
            ]
            .into()
        } else {
            title.into()
        }
    }

    /// Input of the corners of polygons and points of stars, which only
    /// they use.
    fn corners_input(&self) -> Element<'_, Message> {
        let input = text_input("", &self.corners.input)
            .on_input_maybe(
                matches!(self.tool, Tool::Polygon | Tool::Star).then_some(Message::CornersChanged),
            )
            .style(input_style(self.corners.is_valid()));

        row![text("Corners"), input]
            .spacing(8)
            .align_y(Center)
            .into()
    }

    fn toolbar(&self) -> Element<'_, Message> {
        // The aspect ratio can only be typed in while it is kept, as it
        // follows the size otherwise.
        let keep = self.keep_aspect_ratio;
        let aspect_ratio = row![
            checkbox(keep)
                .label("Aspect ratio:")
                .on_toggle(Message::AspectRatioToggled),
            text_input("", &self.board_size.aspect_ratio.width)
                .on_input_maybe(keep.then_some(Message::AspectWidthChanged))
                .width(60),
            text(":"),
            text_input("", &self.board_size.aspect_ratio.height)
                .on_input_maybe(keep.then_some(Message::AspectHeightChanged))
                .width(60),
        ]
        .spacing(6)
        .align_y(Center);

        let size = row![
            text("Size:"),
            text_input("", &self.board_size.width)
                .on_input(Message::BoardWidthChanged)
                .width(60),
            text("×"),
            text_input("", &self.board_size.height)
                .on_input(Message::BoardHeightChanged)
                .width(60),
        ]
        .spacing(6)
        .align_y(Center);

        let modes = Row::with_children(Mode::ALL.into_iter().map(|mode| {
            button(text(mode.to_string()))
                .style(if self.mode == mode {
                    button::primary
                } else {
                    button::secondary
                })
                .on_press(Message::ModeSelected(mode))
                .into()
        }))
        .spacing(2);

        let history = row![
            action_button("Undo", self.history.can_undo().then_some(Message::Undo)),
            action_button("Redo", self.history.can_redo().then_some(Message::Redo)),
        ]
        .spacing(2);

        let file = row![
            action_button("Open", Message::Open),
            action_button("Save", Message::Save),
            action_button("Export", Message::Export),
        ]
        .spacing(2);

        let has_background = self.background.is_some();
        let parallax = self
            .background
            .as_ref()
            .map_or("", |background| &background.parallax_input);

        let background = row![
            row![
                action_button("Background", Message::PickBackground),
                action_button(
                    "Remove",
                    has_background.then_some(Message::RemoveBackground)
                ),
            ]
            .spacing(2),
            row![
                text("Parallax:"),
                text_input("", parallax)
                    .on_input_maybe(has_background.then_some(Message::BackgroundParallaxChanged))
                    .style(input_style(
                        !has_background || parse_parallax(parallax).is_some()
                    ))
                    .width(50),
                text("%"),
            ]
            .spacing(6)
            .align_y(Center),
        ]
        .spacing(12)
        .align_y(Center);

        row![
            row![aspect_ratio, size].spacing(16).align_y(Center),
            background,
            modes,
            history,
            file,
            action_button("About", Message::About),
        ]
        .spacing(24)
        .padding([8, 16])
        .align_y(Center)
        // Onto more lines in a narrow window, rather than cutting it off.
        .wrap()
        .vertical_spacing(8)
        .into()
    }

    fn timeline(&self) -> Element<'_, Message> {
        let controls = row![
            button(text("Delete").width(Fill).center())
                .width(Fill)
                .style(button::danger)
                .on_press_maybe((self.frames.len() > 1).then_some(Message::DeleteFrame)),
            button(text("+").width(Fill).center())
                .width(Fill)
                .on_press(Message::AddFrame),
        ]
        .spacing(4)
        .width(160);

        let frames = scrollable(
            Row::with_children((0..self.frames.len()).flat_map(|frame| {
                let frame_button = button(
                    text(format!("Frame {}", frame + 1))
                        .width(Fill)
                        .height(Fill)
                        .center(),
                )
                .width(160)
                .height(120)
                .style(if self.frame == frame {
                    button::primary
                } else {
                    button::secondary
                })
                .on_press(Message::FrameSelected(frame))
                .into();

                // The transition into a frame sits between it and the
                // frame before it.
                (frame > 0)
                    .then(|| self.transition_settings(frame))
                    .into_iter()
                    .chain([frame_button])
            }))
            .spacing(8)
            .align_y(Center),
        )
        .horizontal()
        .width(Fill)
        .height(Fill);

        container(row![controls, rule::vertical(1), frames].spacing(8))
            .padding(8)
            .width(Fill)
            .height(240)
            .into()
    }

    /// Settings of the transition into `frame` from the frame before it.
    fn transition_settings(&self, frame: usize) -> Element<'_, Message> {
        let transition = &self.frames[frame].transition;

        let kind = pick_list(TransitionKind::ALL, Some(transition.kind), move |kind| {
            Message::TransitionKindSelected { frame, kind }
        })
        .width(Fill);

        // The time does nothing to an instant transition.
        let duration = text_input("", &transition.duration_input).on_input_maybe(
            (transition.kind != TransitionKind::Instant)
                .then_some(move |value| Message::TransitionDurationChanged { frame, value }),
        );

        column![kind, row![duration, text("s")].spacing(4).align_y(Center)]
            .spacing(4)
            .width(110)
            .into()
    }
}

/// Color around the board, where neither it nor the background is.
fn workspace_color(theme: &Theme) -> Color {
    theme.extended_palette().background.weakest.color
}

/// A button picking `color` to draw in, ringed while `ink` is in it. It
/// shows dimmed, picking nothing, unless `enabled`.
fn swatch<'a>(color: Color, ink: &Ink, enabled: bool) -> Element<'a, Message> {
    let is_picked = enabled && ink.color == color;

    button(space())
        .width(Fill)
        .height(30)
        .padding(0)
        .style(move |theme: &Theme, status| {
            let border = if is_picked {
                // Contrasting with the swatch rather than the panel shows on
                // any color.
                let ring = if color.relative_luminance() > 0.4 {
                    Color::BLACK
                } else {
                    Color::WHITE
                };
                swatch_border(theme).color(ring).width(3)
            } else if status == button::Status::Hovered {
                swatch_border(theme).color(theme.extended_palette().background.base.text)
            } else {
                swatch_border(theme)
            };

            let background = if status == button::Status::Disabled {
                color.scale_alpha(DISABLED_ALPHA)
            } else {
                color
            };

            button::Style {
                background: Some(background.into()),
                border,
                ..button::Style::default()
            }
        })
        .on_press_maybe(enabled.then_some(Message::ColorPicked(color)))
        .into()
}

/// Outline of a color swatch, which shows it apart from the panel even when
/// they are alike.
fn swatch_border(theme: &Theme) -> Border {
    Border {
        color: theme.extended_palette().background.strong.color,
        width: 1.0,
        radius: 4.0.into(),
    }
}

/// Style of a text input, outlined in red while it doesn't hold a valid
/// value.
fn input_style(is_valid: bool) -> impl Fn(&Theme, text_input::Status) -> text_input::Style {
    move |theme, status| {
        let style = text_input::default(theme, status);

        if is_valid {
            style
        } else {
            text_input::Style {
                border: style
                    .border
                    .color(theme.extended_palette().danger.base.color),
                ..style
            }
        }
    }
}

/// A toolbar button, disabled without a message.
fn action_button(label: &str, message: impl Into<Option<Message>>) -> Element<'_, Message> {
    button(text(label))
        .style(button::secondary)
        .on_press_maybe(message.into())
        .into()
}

/// Maps a key press to the message of its shortcut, if it has one.
fn shortcut(event: keyboard::Event) -> Option<Message> {
    let keyboard::Event::KeyPressed {
        key,
        physical_key,
        modifiers,
        ..
    } = event
    else {
        return None;
    };

    // Only keys nothing else took get here, so these don't fire while
    // typing.
    if !modifiers.command() {
        return match key.as_ref() {
            keyboard::Key::Named(Named::Delete | Named::Backspace) => Some(Message::DeleteItem),
            keyboard::Key::Named(Named::Escape) => Some(Message::ItemSelected(None)),
            _ => None,
        };
    }

    // Matching the latin letter keeps the shortcuts on the same keys in
    // other layouts (e.g. Cyrillic).
    match (key.to_latin(physical_key)?, modifiers.shift()) {
        ('z', false) => Some(Message::Undo),
        ('z', true) | ('y', false) => Some(Message::Redo),
        ('s', false) => Some(Message::Save),
        ('s', true) => Some(Message::SaveAs),
        ('o', false) => Some(Message::Open),
        _ => None,
    }
}

/// The board's size and aspect ratio, as typed into their inputs.
#[derive(Debug, Clone, PartialEq)]
struct BoardSize {
    width: String,
    height: String,
    /// In whole board units. The inputs may not form a valid size (e.g.
    /// mid-edit), in which case this keeps the last valid one.
    size: Size,
    /// Kept by the size while keeping the aspect ratio, and following it
    /// otherwise.
    aspect_ratio: AspectRatio,
}

impl Default for BoardSize {
    fn default() -> Self {
        let size = Size::new(640.0, 480.0);

        Self {
            width: size.width.to_string(),
            height: size.height.to_string(),
            size,
            aspect_ratio: AspectRatio::of(size),
        }
    }
}

impl BoardSize {
    /// With the width input set to `input`. Keeping the aspect ratio, the
    /// height follows the width.
    fn with_width(&self, input: String, keep_aspect_ratio: bool) -> Self {
        let mut next = Self {
            width: input,
            ..self.clone()
        };

        if let Some(width) = parse_side(&next.width) {
            next.size.width = width;

            if keep_aspect_ratio {
                next.size.height = round_side(width / self.aspect_ratio.value);
                next.height = next.size.height.to_string();
            } else {
                next.aspect_ratio = AspectRatio::of(next.size);
            }
        }

        next
    }

    /// With the height input set to `input`. Keeping the aspect ratio, the
    /// width follows the height.
    fn with_height(&self, input: String, keep_aspect_ratio: bool) -> Self {
        let mut next = Self {
            height: input,
            ..self.clone()
        };

        if let Some(height) = parse_side(&next.height) {
            next.size.height = height;

            if keep_aspect_ratio {
                next.size.width = round_side(height * self.aspect_ratio.value);
                next.width = next.size.width.to_string();
            } else {
                next.aspect_ratio = AspectRatio::of(next.size);
            }
        }

        next
    }

    /// With the aspect ratio inputs set, which the width follows.
    fn with_aspect_ratio(&self, width: String, height: String) -> Self {
        let mut next = Self {
            aspect_ratio: self.aspect_ratio.with_inputs(width, height),
            ..self.clone()
        };

        next.size.width = round_side(next.size.height * next.aspect_ratio.value);
        next.width = next.size.width.to_string();
        next
    }
}

/// Parses a side of the board, a positive whole number of board units.
fn parse_side(input: &str) -> Option<f32> {
    input
        .parse::<u32>()
        .ok()
        .filter(|&side| side > 0)
        .map(|side| side as f32)
}

/// A background parallax, in percent, up to [`MAX_PARALLAX`].
fn parse_parallax(input: &str) -> Option<f32> {
    input
        .parse::<f32>()
        .ok()
        .filter(|parallax| (0.0..=MAX_PARALLAX).contains(parallax))
}

/// Rounds a side of the board worked out from the other to whole board
/// units, at least one.
fn round_side(side: f32) -> f32 {
    side.round().max(1.0)
}

/// The board's aspect ratio, as typed into its two inputs.
#[derive(Debug, Clone, PartialEq)]
struct AspectRatio {
    width: String,
    height: String,
    /// Width over height. The inputs may not form a valid ratio (e.g.
    /// mid-edit), in which case this keeps the last valid one.
    value: f32,
}

impl AspectRatio {
    /// The aspect ratio of `size`, whose sides are whole, in lowest terms.
    fn of(size: Size) -> Self {
        let (width, height) = (size.width as u32, size.height as u32);
        let divisor = gcd(width, height);

        Self {
            width: (width / divisor).to_string(),
            height: (height / divisor).to_string(),
            value: size.width / size.height,
        }
    }

    fn with_inputs(&self, width: String, height: String) -> Self {
        let value = match (width.parse::<f32>(), height.parse::<f32>()) {
            (Ok(w), Ok(h)) if w > 0.0 && h > 0.0 && (w / h).is_normal() => w / h,
            _ => self.value,
        };

        Self {
            width,
            height,
            value,
        }
    }
}

/// Greatest common divisor of `a` and `b`.
fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// An undoable edit of the presentation, holding enough to apply it either
/// way.
#[derive(Debug, Clone)]
enum Action {
    BoardSize {
        from: BoardSize,
        to: BoardSize,
    },
    Background {
        from: Option<Background>,
        to: Option<Background>,
    },
    Camera {
        frame: usize,
        from: Camera,
        to: Camera,
    },
    Transition {
        frame: usize,
        from: Transition,
        to: Transition,
    },
    ItemAdded {
        index: usize,
        item: Item,
    },
    ItemRemoved {
        index: usize,
        item: Item,
    },
    ItemChanged {
        index: usize,
        from: Item,
        to: Item,
    },
    FrameAdded {
        index: usize,
        frame: Frame,
    },
    FrameDeleted {
        index: usize,
        frame: Frame,
    },
}

impl Action {
    /// The action that takes this one back.
    fn inverse(&self) -> Action {
        match self.clone() {
            Action::BoardSize { from, to } => Action::BoardSize { from: to, to: from },
            Action::Background { from, to } => Action::Background { from: to, to: from },
            Action::Camera { frame, from, to } => Action::Camera {
                frame,
                from: to,
                to: from,
            },
            Action::Transition { frame, from, to } => Action::Transition {
                frame,
                from: to,
                to: from,
            },
            Action::ItemAdded { index, item } => Action::ItemRemoved { index, item },
            Action::ItemRemoved { index, item } => Action::ItemAdded { index, item },
            Action::ItemChanged { index, from, to } => Action::ItemChanged {
                index,
                from: to,
                to: from,
            },
            Action::FrameAdded { index, frame } => Action::FrameDeleted { index, frame },
            Action::FrameDeleted { index, frame } => Action::FrameAdded { index, frame },
        }
    }

    /// Whether applying the action leaves the presentation as it is (e.g.
    /// zooming in past the limit).
    fn is_noop(&self) -> bool {
        match self {
            Action::BoardSize { from, to } => from == to,
            Action::Background { from, to } => from == to,
            Action::Camera { from, to, .. } => from == to,
            Action::Transition { from, to, .. } => from == to,
            Action::ItemChanged { from, to, .. } => from == to,
            Action::ItemAdded { .. }
            | Action::ItemRemoved { .. }
            | Action::FrameAdded { .. }
            | Action::FrameDeleted { .. } => false,
        }
    }

    /// Folds `next` into this action when it continues the same edit (e.g.
    /// a drag, scrolling, typing), so the whole edit undoes in one step.
    /// Returns whether it did.
    fn merge(&mut self, next: &Action) -> bool {
        match (self, next) {
            (Action::BoardSize { to, .. }, Action::BoardSize { to: next, .. }) => {
                *to = next.clone();
            }
            // Typing a background's parallax merges, but picking or removing
            // the background is a step of its own.
            (
                Action::Background {
                    from: Some(from),
                    to: Some(to),
                },
                Action::Background {
                    from: Some(next_from),
                    to: Some(next),
                },
            ) if from.picture == to.picture
                && next_from.picture == next.picture
                && to.picture == next.picture =>
            {
                *to = next.clone();
            }
            (
                Action::Camera { frame, to, .. },
                Action::Camera {
                    frame: next_frame,
                    to: next,
                    ..
                },
            ) if frame == next_frame => *to = *next,
            (
                Action::ItemChanged { index, to, .. },
                Action::ItemChanged {
                    index: next_index,
                    to: next,
                    ..
                },
            ) if index == next_index => *to = next.clone(),
            // Typing a duration merges, but picking a kind is a step of its
            // own.
            (
                Action::Transition { frame, from, to },
                Action::Transition {
                    frame: next_frame,
                    from: next_from,
                    to: next,
                },
            ) if frame == next_frame && from.kind == to.kind && next_from.kind == next.kind => {
                *to = next.clone();
            }
            _ => return false,
        }

        true
    }
}

/// Backlog of done and undone actions.
#[derive(Debug, Default)]
struct History {
    /// Oldest first, so the last one is undone first.
    done: Vec<Action>,
    /// Most recently undone last, so it's redone first.
    undone: Vec<Action>,
    /// Whether the last done action is an edit still going on, which the
    /// next action merges into if it continues it.
    open: bool,
}

impl History {
    /// Records `action`, which was just applied. The undone actions are
    /// dropped, as they were taken back from a state that has now diverged.
    fn record(&mut self, action: Action) {
        self.undone.clear();

        if self.open
            && let Some(last) = self.done.last_mut()
            && last.merge(&action)
        {
            return;
        }

        self.done.push(action);
        self.open = true;
    }

    /// Ends the edit going on, so the next action is a step of its own.
    fn seal(&mut self) {
        self.open = false;
    }

    /// Takes the last done action back, returning the action to apply for
    /// it.
    fn undo(&mut self) -> Option<Action> {
        let action = self.done.pop()?;
        let inverse = action.inverse();
        self.undone.push(action);
        self.open = false;
        Some(inverse)
    }

    /// Takes the last undone action again, returning it to be applied.
    fn redo(&mut self) -> Option<Action> {
        let action = self.undone.pop()?;
        self.done.push(action.clone());
        self.open = false;
        Some(action)
    }

    fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }
}

/// Fill of the board, which only shows without a background.
const BOARD_COLOR: Color = Color::WHITE;

const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 10.0;

/// Zoom factor applied per scrolled line.
const ZOOM_STEP: f32 = 1.1;

/// How many pixels of a pixel-based scroll (e.g. touchpad) count as a line.
const PIXELS_PER_LINE: f32 = 50.0;

#[derive(Debug, Clone, Copy)]
struct Board {
    size: Size,
}

/// How the board is placed on the canvas. It is independent of the canvas
/// size, so resizing the window doesn't resize the board.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Camera {
    /// Screen pixels per board unit.
    zoom: f32,
    /// Clockwise turn of the board on screen, in radians within a full
    /// turn.
    rotation: f32,
    /// Offset of the board center from the canvas center, in screen pixels.
    offset: Vector,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            rotation: 0.0,
            offset: Vector::ZERO,
        }
    }
}

impl Camera {
    /// Turns the board clockwise by `angle` radians around the canvas
    /// center.
    fn rotate(&mut self, angle: f32) {
        self.offset = rotated(self.offset, angle);
        self.rotation = (self.rotation + angle).rem_euclid(TAU);
    }

    /// Zooms by `factor`, keeping the board point under `anchor` (relative to
    /// the canvas center) in place.
    fn zoom_at(&mut self, anchor: Vector, factor: f32) {
        let zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        self.offset = anchor - (anchor - self.offset) * (zoom / self.zoom);
        self.zoom = zoom;
    }

    /// Makes `frame` draw in the coordinates the camera looks at.
    fn apply(self, frame: &mut canvas::Frame) {
        frame.translate(frame.center() - Point::ORIGIN + self.offset);
        frame.rotate(self.rotation);
        frame.scale(self.zoom);
    }

    /// Converts a position relative to the canvas center to board
    /// coordinates.
    fn to_board(self, position: Vector) -> Point {
        Point::ORIGIN + rotated(position - self.offset, -self.rotation) / self.zoom
    }

    /// The camera `amount` (0 to 1) of the way from `self` to `to`. The board
    /// point at the canvas center moves in a straight line, the zoom
    /// changes by the same factor per step, so zooming in looks as fast as
    /// zooming out, and the board turns at a constant speed the short way
    /// round.
    fn interpolate(self, to: Camera, amount: f32) -> Camera {
        let from_center = self.to_board(Vector::ZERO);
        let center = from_center + (to.to_board(Vector::ZERO) - from_center) * amount;
        let zoom = self.zoom * (to.zoom / self.zoom).powf(amount);
        // Between -π and π, as turning past half a turn is the long way.
        let turn = wrapped(to.rotation - self.rotation);
        let rotation = (self.rotation + turn * amount).rem_euclid(TAU);

        Camera {
            zoom,
            rotation,
            offset: rotated((Point::ORIGIN - center) * zoom, rotation),
        }
    }
}

/// Turns `vector` clockwise on screen (where y points down) by `angle`
/// radians.
fn rotated(vector: Vector, angle: f32) -> Vector {
    let (sin, cos) = angle.sin_cos();
    Vector::new(
        vector.x * cos - vector.y * sin,
        vector.x * sin + vector.y * cos,
    )
}

/// `angle` wrapped to between -π and π: the same turn, the short way round.
fn wrapped(angle: f32) -> f32 {
    (angle + PI).rem_euclid(TAU) - PI
}

/// The clockwise angle, between -π and π, that turns the direction of
/// `from` into the direction of `to`. It is 0 if either has no direction.
fn angle_between(from: Vector, to: Vector) -> f32 {
    let cross = from.x * to.y - from.y * to.x;
    let dot = from.x * to.x + from.y * to.y;
    cross.atan2(dot)
}

/// A step of the presentation: where the camera is focused on the board.
/// Items aren't part of a frame, so every frame shows the same drawings.
#[derive(Debug, Clone, Default)]
struct Frame {
    camera: Camera,
    /// How the camera gets here from the previous frame. Unused on the
    /// first frame.
    transition: Transition,
}

/// How the camera moves between two frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransitionKind {
    /// Snaps to the frame.
    Instant,
    /// Moves at a constant speed.
    Linear,
    /// Speeds up then slows down, with the speed following the first half
    /// period of a sine.
    Sine,
}

impl TransitionKind {
    const ALL: [TransitionKind; 3] = [
        TransitionKind::Instant,
        TransitionKind::Linear,
        TransitionKind::Sine,
    ];

    /// Maps the elapsed fraction of the transition time to the fraction of
    /// the way travelled.
    fn ease(self, time: f32) -> f32 {
        match self {
            TransitionKind::Instant => 1.0,
            TransitionKind::Linear => time,
            // The integral of sin(πt), scaled to cover the way in time 1.
            TransitionKind::Sine => (1.0 - (PI * time).cos()) / 2.0,
        }
    }
}

impl fmt::Display for TransitionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TransitionKind::Instant => "Instant",
            TransitionKind::Linear => "Linear",
            TransitionKind::Sine => "Sine",
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Transition {
    kind: TransitionKind,
    duration: Duration,
    /// Duration input in seconds. It may not be a valid duration (e.g.
    /// mid-edit), in which case `duration` keeps the last valid one.
    duration_input: String,
}

impl Default for Transition {
    fn default() -> Self {
        Self {
            kind: TransitionKind::Linear,
            duration: Duration::from_secs(1),
            duration_input: String::from("1"),
        }
    }
}

impl Transition {
    fn set_duration(&mut self, input: String) {
        if let Some(duration) = input
            .parse()
            .ok()
            .and_then(|seconds| Duration::try_from_secs_f32(seconds).ok())
        {
            self.duration = duration;
        }
        self.duration_input = input;
    }
}

/// The camera moving to the selected frame while a transition plays.
#[derive(Debug, Clone, Copy)]
struct Flight {
    from: Camera,
    kind: TransitionKind,
    duration: Duration,
    /// Time of the first window frame of the flight, so it starts when it
    /// is first drawn.
    start: Option<Instant>,
    /// Fraction of the way travelled, from 0 to 1.
    progress: f32,
}

impl Flight {
    /// Returns `None` when the transition has nothing to animate.
    fn new(from: Camera, transition: &Transition) -> Option<Self> {
        (transition.kind != TransitionKind::Instant && !transition.duration.is_zero()).then_some(
            Self {
                from,
                kind: transition.kind,
                duration: transition.duration,
                start: None,
                progress: 0.0,
            },
        )
    }

    /// Advances the flight to `now`, returning whether it has arrived.
    fn advance(&mut self, now: Instant) -> bool {
        let start = *self.start.get_or_insert(now);
        let time = now.saturating_duration_since(start).as_secs_f32() / self.duration.as_secs_f32();
        self.progress = self.kind.ease(time.min(1.0));
        time >= 1.0
    }

    /// Where the camera is on its way to `to`.
    fn camera(self, to: Camera) -> Camera {
        self.from.interpolate(to, self.progress)
    }
}

/// Colors offered in the palette, in rows of six.
const COLORS: [Color; 12] = [
    Color::from_rgb8(0x00, 0x00, 0x00),
    Color::from_rgb8(0x6B, 0x72, 0x80),
    Color::from_rgb8(0xFF, 0xFF, 0xFF),
    Color::from_rgb8(0xDC, 0x26, 0x26),
    Color::from_rgb8(0xEA, 0x58, 0x0C),
    Color::from_rgb8(0xFA, 0xCC, 0x15),
    Color::from_rgb8(0x16, 0xA3, 0x4A),
    Color::from_rgb8(0x0D, 0x94, 0x88),
    DEFAULT_COLOR,
    Color::from_rgb8(0x93, 0x33, 0xEA),
    Color::from_rgb8(0xDB, 0x27, 0x77),
    Color::from_rgb8(0x92, 0x40, 0x0E),
];

const DEFAULT_COLOR: Color = Color::from_rgb8(0x25, 0x63, 0xEB);

/// Opacity of a color swatch that can't be picked.
const DISABLED_ALPHA: f32 = 0.3;

/// How items are drawn, as the panel sets it, for new items or the selected
/// one. Each setting only applies to some items, see [`Settings`].
#[derive(Debug, Clone, PartialEq)]
struct Style {
    ink: Ink,
    /// Whether closed shapes are filled in, rather than outlined.
    filled: bool,
    text_size: TextSize,
    typeface: Typeface,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            ink: Ink::default(),
            filled: false,
            text_size: TextSize::new(TEXT_SIZE),
            typeface: Typeface::Default,
        }
    }
}

impl Style {
    /// Takes on the settings `shape` has, keeping the inputs that still hold
    /// its values.
    fn follow(&mut self, shape: &Shape) {
        if let Some(color) = shape.color()
            && color != self.ink.color
        {
            self.ink = Ink::new(color);
        }

        if let Some(filled) = shape.filled() {
            self.filled = filled;
        }

        if let Some((size, typeface)) = shape.text_style() {
            if size != self.text_size.value {
                self.text_size = TextSize::new(size);
            }
            self.typeface = typeface.clone();
        }
    }
}

/// Which settings of a [`Style`] apply, and can be changed in the panel.
#[derive(Debug, Clone, Copy)]
struct Settings {
    color: bool,
    fill: bool,
    /// The size and font of text.
    text: bool,
}

/// A color, as typed into its input.
#[derive(Debug, Clone, PartialEq)]
struct Ink {
    color: Color,
    /// Color input in hex (e.g. `#2563eb`, or `#2563eb80` with alpha). It
    /// may not be a valid color (e.g. mid-edit), in which case `color` keeps
    /// the last valid one.
    input: String,
}

impl Default for Ink {
    fn default() -> Self {
        Self::new(DEFAULT_COLOR)
    }
}

impl Ink {
    fn new(color: Color) -> Self {
        Self {
            color,
            input: color.to_string(),
        }
    }

    fn set_input(&mut self, input: String) {
        if let Some(color) = parse_color(&input) {
            self.color = color;
        }
        self.input = input;
    }

    fn is_valid(&self) -> bool {
        parse_color(&self.input).is_some()
    }
}

/// Parses a hex color of 3, 4, 6 or 8 digits, with or without a `#`.
fn parse_color(input: &str) -> Option<Color> {
    let input = input.trim();
    let hex = input.strip_prefix('#').unwrap_or(input);

    // `Color`'s parser slices the digits by byte, which panics inside a
    // multi-byte character.
    if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }

    hex.parse().ok()
}

/// Fewest corners a polygon can have, and points a star.
const MIN_CORNERS: usize = 3;
const MAX_CORNERS: usize = 100;

/// The corners new polygons have, and points new stars have.
#[derive(Debug, Clone, PartialEq)]
struct Corners {
    count: usize,
    /// Corners input. It may not be a valid count (e.g. mid-edit), in which
    /// case `count` keeps the last valid one.
    input: String,
}

impl Default for Corners {
    fn default() -> Self {
        Self {
            count: 5,
            input: String::from("5"),
        }
    }
}

impl Corners {
    fn set_input(&mut self, input: String) {
        if let Some(count) = parse_corners(&input) {
            self.count = count;
        }
        self.input = input;
    }

    fn is_valid(&self) -> bool {
        parse_corners(&self.input).is_some()
    }
}

fn parse_corners(input: &str) -> Option<usize> {
    input
        .trim()
        .parse()
        .ok()
        .filter(|count| (MIN_CORNERS..=MAX_CORNERS).contains(count))
}

const LINE_WIDTH: f32 = 3.0;

/// Length of an arrowhead along its arrow, and its width across it. It
/// shrinks to fit an arrow shorter than it.
const ARROW_HEAD_LENGTH: f32 = 15.0;
const ARROW_HEAD_WIDTH: f32 = 12.0;

/// Distance of a star's dents from its center, relative to its points'.
const STAR_DENT: f32 = 0.5;

/// Size of new text, until another is set.
const TEXT_SIZE: f32 = 32.0;

/// Largest size text can be set to. Text can still be resized past it.
const MAX_TEXT_SIZE: f32 = 1000.0;

/// Height of the list of fonts to pick from.
const FONT_MENU_HEIGHT: f32 = 300.0;

/// A text size, as typed into its input.
#[derive(Debug, Clone, PartialEq)]
struct TextSize {
    value: f32,
    /// It may not be a valid size (e.g. mid-edit), in which case `value`
    /// keeps the last valid one.
    input: String,
}

impl TextSize {
    fn new(value: f32) -> Self {
        Self {
            value,
            // Resizing leaves sizes with digits no one would type.
            input: ((value * 100.0).round() / 100.0).to_string(),
        }
    }

    fn set_input(&mut self, input: String) {
        if let Some(value) = parse_text_size(&input) {
            self.value = value;
        }
        self.input = input;
    }

    fn is_valid(&self) -> bool {
        parse_text_size(&self.input).is_some()
    }
}

/// Parses a text size, positive and up to [`MAX_TEXT_SIZE`].
fn parse_text_size(input: &str) -> Option<f32> {
    input
        .trim()
        .parse()
        .ok()
        .filter(|&size| size > 0.0 && size <= MAX_TEXT_SIZE)
}

/// Height of a line of text, relative to its size.
const TEXT_LINE_HEIGHT: f32 = 1.2;

/// Something drawn on the board, so it moves and zooms with it.
#[derive(Debug, Clone, PartialEq)]
struct Item {
    /// In the item's own coordinates: board coordinates turned back by
    /// `rotation` around the board center.
    shape: Shape,
    /// Clockwise turn of the item's own coordinates on the board, in
    /// radians within a full turn.
    rotation: f32,
}

impl Item {
    /// An item with `shape` in board coordinates, not turned.
    fn new(shape: Shape) -> Self {
        Self {
            shape,
            rotation: 0.0,
        }
    }

    fn draw(&self, frame: &mut canvas::Frame) {
        frame.with_save(|frame| {
            frame.rotate(self.rotation);
            self.shape.draw(frame);
        });
    }

    /// Converts a board point to the item's own coordinates.
    fn to_local(&self, point: Point) -> Point {
        Point::ORIGIN + rotated(point - Point::ORIGIN, -self.rotation)
    }

    /// Converts a point in the item's own coordinates to board coordinates.
    fn to_board(&self, point: Point) -> Point {
        Point::ORIGIN + rotated(point - Point::ORIGIN, self.rotation)
    }

    /// Whether `next` can be drawn by the same canvas as this item, after
    /// it. A canvas draws its images over its shapes, and its text over both
    /// (or as shapes, when turned), whatever order they come in, so only a
    /// shape can be followed by another kind of item.
    fn shares_canvas(&self, next: &Item) -> bool {
        match (&self.shape, &next.shape) {
            (Shape::Text { .. }, Shape::Text { .. })
            | (Shape::Image { .. }, Shape::Image { .. }) => true,
            (Shape::Text { .. } | Shape::Image { .. }, _) => false,
            _ => true,
        }
    }

    /// The item moved by `delta` on the board.
    fn translated(&self, delta: Vector) -> Item {
        Item {
            shape: self.shape.translated(rotated(delta, -self.rotation)),
            rotation: self.rotation,
        }
    }

    /// Center of the item's extent on the board, which it turns around.
    fn center(&self) -> Point {
        self.to_board(self.shape.extent().center())
    }

    /// The item turned clockwise by `angle` radians around the board point
    /// `center`.
    fn turned(&self, center: Point, angle: f32) -> Item {
        let rotation = (self.rotation + angle).rem_euclid(TAU);
        // Turning around the board center instead moves `center` by this.
        let center = center - Point::ORIGIN;
        let drift = center - rotated(center, angle);

        Item {
            shape: self.shape.translated(rotated(drift, -rotation)),
            rotation,
        }
    }

    /// Whether `point` is on the drawn item, or within `distance` of it.
    fn hits(&self, point: Point, distance: f32) -> bool {
        self.shape.hits(self.to_local(point), distance)
    }
}

/// What an item draws, in its own coordinates.
#[derive(Debug, Clone, PartialEq)]
enum Shape {
    /// Straight segments through `points`, and back to the first one when
    /// `closed`. Lines, rectangles, polygons, stars, pencil strokes and
    /// paths are all drawn as these.
    Polyline {
        points: Vec<Point>,
        closed: bool,
        /// Whether its inside is filled in rather than outlined, which only
        /// a closed one's can be.
        filled: bool,
        color: Color,
    },
    Arrow {
        from: Point,
        to: Point,
        color: Color,
    },
    Ellipse {
        center: Point,
        radii: Vector,
        /// Whether its inside is filled in rather than outlined.
        filled: bool,
        color: Color,
    },
    /// Lines of text, with the top left corner of the first at `position`.
    Text {
        position: Point,
        content: String,
        size: f32,
        typeface: Typeface,
        color: Color,
    },
    Image {
        bounds: Rectangle,
        picture: Picture,
    },
}

impl Shape {
    fn draw(&self, frame: &mut canvas::Frame) {
        match self {
            Shape::Polyline {
                points,
                closed,
                filled,
                color,
            } => paint(frame, &polyline(points, *closed), *color, *filled),
            Shape::Arrow { from, to, color } => {
                let (base, head) = arrow_head(*from, *to);
                frame.stroke(&canvas::Path::line(*from, base), stroke(*color));
                frame.fill(&polyline(&head, true), *color);
            }
            Shape::Ellipse {
                center,
                radii,
                filled,
                color,
            } => {
                let ellipse = canvas::Path::new(|builder| {
                    builder.ellipse(canvas::path::arc::Elliptical {
                        center: *center,
                        radii: *radii,
                        rotation: Radians(0.0),
                        start_angle: Radians(0.0),
                        end_angle: Radians(TAU),
                    });
                    builder.close();
                });
                paint(frame, &ellipse, *color, *filled);
            }
            Shape::Text {
                position,
                content,
                size,
                typeface,
                color,
            } => frame.fill_text(canvas::Text {
                content: content.clone(),
                position: *position,
                color: *color,
                size: (*size).into(),
                line_height: LineHeight::Relative(TEXT_LINE_HEIGHT),
                font: typeface.font(),
                ..canvas::Text::default()
            }),
            Shape::Image { bounds, picture } => {
                frame.draw_image(*bounds, canvas::Image::new(picture.image.handle().clone()))
            }
        }
    }

    /// The shape moved by `delta`.
    fn translated(&self, delta: Vector) -> Shape {
        self.mapped(|point| point + delta, Vector::new(1.0, 1.0))
    }

    /// The shape stretched by `factors` along each axis, away from `anchor`,
    /// which stays in place. Text only takes the horizontal factor, as it
    /// [keeps its proportions](Self::keeps_proportions).
    fn scaled(&self, anchor: Point, factors: Vector) -> Shape {
        self.mapped(
            |point| {
                Point::new(
                    anchor.x + (point.x - anchor.x) * factors.x,
                    anchor.y + (point.y - anchor.y) * factors.y,
                )
            },
            factors,
        )
    }

    /// The shape with its points moved by `map`, which stretches lengths by
    /// `factors`. Lines stay as thick.
    fn mapped(&self, map: impl Fn(Point) -> Point, factors: Vector) -> Shape {
        match self.clone() {
            Shape::Polyline {
                points,
                closed,
                filled,
                color,
            } => Shape::Polyline {
                points: points.into_iter().map(map).collect(),
                closed,
                filled,
                color,
            },
            Shape::Arrow { from, to, color } => Shape::Arrow {
                from: map(from),
                to: map(to),
                color,
            },
            Shape::Ellipse {
                center,
                radii,
                filled,
                color,
            } => Shape::Ellipse {
                center: map(center),
                radii: Vector::new(radii.x * factors.x, radii.y * factors.y),
                filled,
                color,
            },
            Shape::Text {
                position,
                content,
                size,
                typeface,
                color,
            } => Shape::Text {
                position: map(position),
                content,
                size: size * factors.x,
                typeface,
                color,
            },
            Shape::Image { bounds, picture } => Shape::Image {
                bounds: Rectangle::new(
                    map(bounds.position()),
                    Size::new(bounds.width * factors.x, bounds.height * factors.y),
                ),
                picture,
            },
        }
    }

    /// Whether the shape can only be resized evenly, as stretched text
    /// looks broken.
    fn keeps_proportions(&self) -> bool {
        matches!(self, Shape::Text { .. })
    }

    /// The color the shape is drawn in, unless it is an image.
    fn color(&self) -> Option<Color> {
        match self {
            Shape::Polyline { color, .. }
            | Shape::Arrow { color, .. }
            | Shape::Ellipse { color, .. }
            | Shape::Text { color, .. } => Some(*color),
            Shape::Image { .. } => None,
        }
    }

    /// Whether the shape is filled in, if it is closed.
    fn filled(&self) -> Option<bool> {
        match self {
            Shape::Polyline {
                closed: true,
                filled,
                ..
            }
            | Shape::Ellipse { filled, .. } => Some(*filled),
            _ => None,
        }
    }

    /// The size and font of text.
    fn text_style(&self) -> Option<(f32, &Typeface)> {
        match self {
            Shape::Text { size, typeface, .. } => Some((*size, typeface)),
            _ => None,
        }
    }

    /// Which settings of a [`Style`] apply to the shape.
    fn settings(&self) -> Settings {
        Settings {
            color: self.color().is_some(),
            fill: self.filled().is_some(),
            text: self.text_style().is_some(),
        }
    }

    /// The shape drawn in `style`, as far as its [settings](Self::settings)
    /// go. Text keeps the top left corner of its first line in place.
    fn styled(&self, style: &Style) -> Shape {
        let mut shape = self.clone();

        match &mut shape {
            Shape::Polyline {
                closed,
                filled,
                color,
                ..
            } => {
                *filled = *closed && style.filled;
                *color = style.ink.color;
            }
            Shape::Arrow { color, .. } => *color = style.ink.color,
            Shape::Ellipse { filled, color, .. } => {
                *filled = style.filled;
                *color = style.ink.color;
            }
            Shape::Text {
                size,
                typeface,
                color,
                ..
            } => {
                *size = style.text_size.value;
                *typeface = style.typeface.clone();
                *color = style.ink.color;
            }
            Shape::Image { .. } => {}
        }

        shape
    }

    /// The smallest rectangle the shape's points fit in, not counting the
    /// thickness of lines or arrowheads, so it resizes with the shape.
    fn extent(&self) -> Rectangle {
        match self {
            Shape::Polyline { points, .. } => enclosing(points.iter().copied()),
            Shape::Arrow { from, to, .. } => enclosing([*from, *to]),
            Shape::Ellipse { center, radii, .. } => {
                Rectangle::new(*center - *radii, Size::new(radii.x, radii.y) * 2.0)
            }
            Shape::Text {
                position,
                content,
                size,
                typeface,
                ..
            } => Rectangle::new(*position, text_size(content, *size, typeface.font())),
            Shape::Image { bounds, .. } => *bounds,
        }
    }

    /// Whether `point` is on the drawn shape, or within `distance` of it.
    /// Shapes are hit inside too, as clicking their thin outline is hard.
    fn hits(&self, point: Point, distance: f32) -> bool {
        // Up to the edge of the stroke.
        let reach = distance + LINE_WIDTH / 2.0;

        match self {
            Shape::Polyline { points, closed, .. } => {
                segments(points, *closed).any(|(a, b)| distance_to_segment(point, a, b) <= reach)
                    || *closed && encloses(points, point)
            }
            Shape::Arrow { from, to, .. } => {
                let (_, head) = arrow_head(*from, *to);
                distance_to_segment(point, *from, *to) <= reach || encloses(&head, point)
            }
            Shape::Ellipse { center, radii, .. } => {
                let offset = point - *center;
                (offset.x / (radii.x + reach)).powi(2) + (offset.y / (radii.y + reach)).powi(2)
                    <= 1.0
            }
            Shape::Text { .. } | Shape::Image { .. } => {
                self.extent().expand(distance).contains(point)
            }
        }
    }
}

/// How items draw their outlines.
fn stroke(color: Color) -> canvas::Stroke<'static> {
    canvas::Stroke::default()
        .with_color(color)
        .with_width(LINE_WIDTH)
        .with_line_cap(canvas::LineCap::Round)
        .with_line_join(canvas::LineJoin::Round)
}

/// Fills `path` in with `color` when `filled`, or else outlines it. The fill
/// goes by the even-odd rule, like [`encloses`], so a path crossing itself
/// is hit where it shows filled.
fn paint(frame: &mut canvas::Frame, path: &canvas::Path, color: Color, filled: bool) {
    if filled {
        frame.fill(
            path,
            canvas::Fill {
                style: canvas::Style::Solid(color),
                rule: canvas::fill::Rule::EvenOdd,
            },
        );
    } else {
        frame.stroke(path, stroke(color));
    }
}

/// Straight segments through `points`, and back to the first one when
/// `closed`.
fn polyline(points: &[Point], closed: bool) -> canvas::Path {
    canvas::Path::new(|builder| {
        let Some((first, rest)) = points.split_first() else {
            return;
        };

        builder.move_to(*first);
        for point in rest {
            builder.line_to(*point);
        }
        if closed {
            builder.close();
        }
    })
}

/// The segments of a polyline through `points`, see [`Shape::Polyline`].
fn segments(points: &[Point], closed: bool) -> impl Iterator<Item = (Point, Point)> + '_ {
    let closing = match (points.first(), points.last()) {
        (Some(&first), Some(&last)) if closed => Some((last, first)),
        _ => None,
    };

    points
        .windows(2)
        .map(|pair| (pair[0], pair[1]))
        .chain(closing)
}

fn distance_to_segment(point: Point, a: Point, b: Point) -> f32 {
    let segment = b - a;
    let length_squared = segment.x * segment.x + segment.y * segment.y;
    // How far along the segment the closest point is, from 0 at `a` to 1 at
    // `b`.
    let along = if length_squared > 0.0 {
        let offset = point - a;
        ((offset.x * segment.x + offset.y * segment.y) / length_squared).clamp(0.0, 1.0)
    } else {
        0.0
    };

    point.distance(a + segment * along)
}

/// Whether `point` is inside the polygon with corners `polygon`, by the
/// even-odd rule.
fn encloses(polygon: &[Point], point: Point) -> bool {
    segments(polygon, true)
        .filter(|(a, b)| {
            (a.y > point.y) != (b.y > point.y)
                && point.x < a.x + (point.y - a.y) / (b.y - a.y) * (b.x - a.x)
        })
        .count()
        % 2
        == 1
}

/// The smallest rectangle containing `points`.
fn enclosing(points: impl IntoIterator<Item = Point>) -> Rectangle {
    let (min, max) = points.into_iter().fold(
        (
            Point::new(f32::INFINITY, f32::INFINITY),
            Point::new(f32::NEG_INFINITY, f32::NEG_INFINITY),
        ),
        |(min, max), point| {
            (
                Point::new(min.x.min(point.x), min.y.min(point.y)),
                Point::new(max.x.max(point.x), max.y.max(point.y)),
            )
        },
    );

    Rectangle::new(min, Size::new(max.x - min.x, max.y - min.y))
}

/// The base of the head of an arrow from `from` to `to`, where its shaft
/// ends, and the head's corners, tip first. `from` and `to` must differ.
fn arrow_head(from: Point, to: Point) -> (Point, [Point; 3]) {
    let length = from.distance(to);
    let head_length = ARROW_HEAD_LENGTH.min(length);
    let direction = (to - from) * (1.0 / length);
    let base = to - direction * head_length;
    let side = Vector::new(-direction.y, direction.x)
        * (ARROW_HEAD_WIDTH / 2.0 * head_length / ARROW_HEAD_LENGTH);

    (base, [to, base + side, base - side])
}

/// The shape `tool` draws when dragged from `start` to `end`, unless it would
/// be empty: a line or an arrow between them, a rectangle or an ellipse
/// spanning them, or a polygon or a star centered at `start` with a corner
/// at `end`. All but lines and arrows are filled in when `filled`.
fn shape(
    tool: Tool,
    start: Point,
    end: Point,
    corners: usize,
    color: Color,
    filled: bool,
) -> Option<Shape> {
    let span = enclosing([start, end]);
    let is_empty = match tool {
        Tool::Rectangle | Tool::Ellipse => span.width == 0.0 || span.height == 0.0,
        _ => start == end,
    };

    if is_empty {
        return None;
    }

    let polygon = |dent| Shape::Polyline {
        points: polygon(start, end, corners, dent),
        closed: true,
        filled,
        color,
    };

    Some(match tool {
        Tool::Line => Shape::Polyline {
            points: vec![start, end],
            closed: false,
            filled: false,
            color,
        },
        Tool::Arrow => Shape::Arrow {
            from: start,
            to: end,
            color,
        },
        Tool::Rectangle => Shape::Polyline {
            points: vec![
                span.position(),
                Point::new(span.x + span.width, span.y),
                Point::new(span.x + span.width, span.y + span.height),
                Point::new(span.x, span.y + span.height),
            ],
            closed: true,
            filled,
            color,
        },
        Tool::Ellipse => Shape::Ellipse {
            center: span.center(),
            radii: Vector::new(span.width, span.height) * 0.5,
            filled,
            color,
        },
        Tool::Polygon => polygon(None),
        Tool::Star => polygon(Some(STAR_DENT)),
        _ => return None,
    })
}

/// Corners of a regular polygon centered at `center`, with one at `corner`.
/// With a `dent`, it is a star with as many points, and dents between them
/// at `dent` times their distance from the center.
fn polygon(center: Point, corner: Point, corners: usize, dent: Option<f32>) -> Vec<Point> {
    let radius = corner - center;
    let step = TAU / corners as f32;

    (0..corners)
        .flat_map(|index| {
            let angle = step * index as f32;
            let dent = dent.map(|dent| center + rotated(radius * dent, angle + step / 2.0));
            iter::once(center + rotated(radius, angle)).chain(dent)
        })
        .collect()
}

/// Size of `content` as text of `size` in `font` draws it.
fn text_size(content: &str, size: f32, font: Font) -> Size {
    <Renderer as advanced_text::Renderer>::Paragraph::with_text(advanced_text::Text {
        content,
        bounds: Size::INFINITE,
        size: size.into(),
        line_height: LineHeight::Relative(TEXT_LINE_HEIGHT),
        font,
        align_x: advanced_text::Alignment::Default,
        align_y: Vertical::Top,
        shaping: advanced_text::Shaping::default(),
        wrapping: advanced_text::Wrapping::None,
    })
    .min_bounds()
}

/// The largest rectangle with the aspect ratio of `size` that fits in
/// `area`, centered in it.
fn fit(size: Size, area: Rectangle) -> Rectangle {
    let fitted = size * fit_scale(size, area.size());
    Rectangle::new(
        area.center() - Vector::new(fitted.width, fitted.height) * 0.5,
        fitted,
    )
}

/// Parallax of a background picked while there is none, see
/// [`Background::parallax`].
const DEFAULT_PARALLAX: f32 = 10.0;

/// Highest parallax, in percent.
const MAX_PARALLAX: f32 = 100.0;

/// Power of the camera's zoom the background zooms in by, see
/// [`background_zoom`]. At a half, it zooms in 2× as the board does 4×, so it
/// looks far behind it. At 1, it would zoom in as much as the board.
const BACKGROUND_ZOOM_POWER: f32 = 0.5;

/// Most the background is drawn at however close the camera zooms in, in
/// times the scale that covers the screen, which bounds how large a vector
/// background is rasterized, see [`background_scale`]. An 8K image covering
/// a 4K screen then shows each of its pixels across 4 of the screen's, or 2
/// of a 1080p screen's, and a 4K image twice that: soft, but still fine for
/// a backdrop. It is at least the scale zoomed out at [`MAX_PARALLAX`], 2.
const MAX_BACKGROUND_SCALE: f32 = 8.0;

/// Size, in pixels, of the largest screen a vector background is rasterized
/// to look sharp on: a 4K one.
const SHARPEST_SCREEN: Size = Size::new(3840.0, 2160.0);

/// File extensions of the raster images a picture can be, which the
/// renderer decodes.
const RASTER_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff", "tga", "qoi", "ico", "pnm", "pbm",
    "pgm", "ppm", "pam", "hdr", "exr", "dds", "ff",
];

/// File extensions of the vector images a picture can be, see
/// [`rasterize_svg`].
const VECTOR_EXTENSIONS: &[&str] = &["svg", "svgz"];

/// Longest side, in pixels, of a rasterized vector image, which would take up
/// gigabytes otherwise. A background zoomed in as far as it goes on a 4K
/// screen reaches it, as do very wide or tall images.
const MAX_RASTER_SIDE: f32 = 8192.0;

/// Pixels per unit to rasterize a vector background of `size` at: the most
/// it is drawn at on [`SHARPEST_SCREEN`], so it doesn't show blurry, however
/// the parallax is set and the camera zooms, up to [`MAX_RASTER_SIDE`].
fn background_scale(size: Size) -> f32 {
    cover_scale(size, SHARPEST_SCREEN) * MAX_BACKGROUND_SCALE
}

/// How many times larger than zoomed out the background is drawn, with
/// `parallax` from 0 to 1 and the camera `zoom` times as close as the one
/// that fits the board on screen. Like a far scene, it zooms in by
/// [`BACKGROUND_ZOOM_POWER`] of the camera's zoom. Zoomed out further than
/// the whole board, it shrinks no more, so its edges never show, and zoomed
/// in, it grows up to [`MAX_BACKGROUND_SCALE`].
fn background_zoom(zoom: f32, parallax: f32) -> f32 {
    zoom.powf(BACKGROUND_ZOOM_POWER)
        .min(MAX_BACKGROUND_SCALE / (1.0 + parallax))
        .max(1.0)
}

/// Scale that makes something of `size` cover `area` without stretching.
fn cover_scale(size: Size, area: Size) -> f32 {
    (area.width / size.width).max(area.height / size.height)
}

/// Scale that makes something of `size` fit in `area` without stretching.
fn fit_scale(size: Size, area: Size) -> f32 {
    (area.width / size.width).min(area.height / size.height)
}

/// Asks for an image file and loads it, see [`load_picture`].
fn pick_picture(
    title: &str,
    scale: impl Fn(Size) -> f32 + Copy + Send + 'static,
) -> Task<Result<Picture, image::Error>> {
    let dialog = rfd::AsyncFileDialog::new()
        .set_title(title)
        .add_filter("Images", &[RASTER_EXTENSIONS, VECTOR_EXTENSIONS].concat())
        .pick_file();

    // Loading it up front tells its size, and whether it is an image at all.
    let loaded: Task<Result<_, image::Error>> = Task::future(dialog).and_then(move |file| {
        let path = file.path().to_owned();
        Task::future(async move { load_picture(&path, scale) })
    });

    loaded.and_then(|(handle, file)| {
        image::allocate(handle).map(move |image| {
            image.map(|image| Picture {
                image,
                file: file.clone(),
            })
        })
    })
}

/// Prepares the image at `path` for the renderer, which decodes raster
/// images itself, and keeps its file for exporting. A vector image is
/// rasterized at `scale(size)` pixels per unit of its `size`, see
/// [`rasterize_svg`].
fn load_picture(
    path: &Path,
    scale: impl FnOnce(Size) -> f32,
) -> Result<(image::Handle, ImageFile), image::Error> {
    let is_vector = path
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            VECTOR_EXTENSIONS
                .iter()
                .any(|vector| extension.eq_ignore_ascii_case(vector))
        });

    if is_vector {
        let tree = parse_svg(path)?;
        let size = Size::new(tree.size().width(), tree.size().height());
        Ok((rasterize_svg(&tree, scale(size))?, ImageFile::vector(&tree)))
    } else {
        let file =
            ImageFile::raster(path).map_err(|error| image::Error::Inaccessible(Arc::new(error)))?;
        Ok((image::Handle::from_path(path), file))
    }
}

fn parse_svg(path: &Path) -> Result<usvg::Tree, image::Error> {
    let data = fs::read(path).map_err(|error| image::Error::Inaccessible(Arc::new(error)))?;

    let mut options = usvg::Options {
        // Where images it links to are looked up.
        resources_dir: path.parent().map(Path::to_path_buf),
        ..usvg::Options::default()
    };
    options.fontdb_mut().load_system_fonts();
    resolve_generic_families(options.fontdb_mut());

    usvg::Tree::from_data(&data, &options).map_err(|error| image::Error::Invalid(Arc::new(error)))
}

/// Rasterizes `tree` once, at `scale` pixels per unit, which should be the
/// size it takes up on screen at the highest zoom so it never shows blurry.
/// The renderer could draw it as is, but it would rasterize it again
/// whenever its size on screen changes, which is every frame of a zoom.
fn rasterize_svg(tree: &usvg::Tree, scale: f32) -> Result<image::Handle, image::Error> {
    let svg = Size::new(tree.size().width(), tree.size().height());
    let scale = scale.min(MAX_RASTER_SIDE / svg.width.max(svg.height));

    let mut pixmap = tiny_skia::Pixmap::new(
        (svg.width * scale).ceil() as u32,
        (svg.height * scale).ceil() as u32,
    )
    .ok_or(image::Error::Empty)?;
    resvg::render(
        tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    let (width, height) = (pixmap.width(), pixmap.height());
    let mut pixels = pixmap.take();

    // tiny-skia multiplies the colors by their alpha, but the renderer takes
    // them as they are.
    for [red, green, blue, alpha] in pixels.as_chunks_mut::<4>().0 {
        let alpha = u16::from(*alpha);

        for channel in [red, green, blue] {
            // Transparent pixels have no color to take back.
            if let Some(straight) = (u16::from(*channel) * 255 + alpha / 2).checked_div(alpha) {
                *channel = straight as u8;
            }
        }
    }

    Ok(image::Handle::from_rgba(width, height, pixels))
}

/// Points each generic font family (e.g. sans-serif), which SVG text falls
/// back to, at an installed font. fontdb takes the font fontconfig prefers
/// most for each, even if it isn't installed, so this asks fontconfig which
/// one it would use instead.
fn resolve_generic_families(fontdb: &mut usvg::fontdb::Database) {
    use usvg::fontdb::{Database, Family, Query};

    type Setter = fn(&mut Database, String);

    let generics: [(Family, &str, Setter); 5] = [
        (Family::Serif, "serif", Database::set_serif_family),
        (
            Family::SansSerif,
            "sans-serif",
            Database::set_sans_serif_family,
        ),
        (
            Family::Monospace,
            "monospace",
            Database::set_monospace_family,
        ),
        (Family::Cursive, "cursive", Database::set_cursive_family),
        (Family::Fantasy, "fantasy", Database::set_fantasy_family),
    ];

    for (family, pattern, set) in generics {
        let query = Query {
            families: &[family],
            ..Query::default()
        };

        if fontdb.query(&query).is_some() {
            continue;
        }

        // Without fontconfig (e.g. on Windows), the family stays as it is.
        if let Ok(output) = Command::new("fc-match")
            .args(["--format=%{family[0]}", pattern])
            .output()
            && let Ok(name) = String::from_utf8(output.stdout)
            && !name.is_empty()
        {
            set(fontdb, name);
        }
    }
}

/// A loaded image: the background, or one on the board.
#[derive(Debug, Clone, PartialEq)]
struct Picture {
    /// Keeps the image loaded, so it shows as soon as it is set, or brought
    /// back by undoing.
    image: image::Allocation,
    file: ImageFile,
}

impl Picture {
    /// Size in pixels.
    fn size(&self) -> Size {
        let size = self.image.size();
        Size::new(size.width as f32, size.height as f32)
    }
}

/// The image filling the screen in place of the board, and how far it
/// drifts.
#[derive(Debug, Clone, PartialEq)]
struct Background {
    picture: Picture,
    parallax_input: String,
    /// How far the background drifts as the camera moves from one side of
    /// the board to the other, in percent of the screen. It is drawn that
    /// much larger than it takes to cover the screen, so its edges never
    /// show. The input may not be a valid parallax (e.g. mid-edit), in which
    /// case this keeps the last valid one.
    parallax: f32,
}

impl Background {
    fn new(picture: Picture, parallax: f32) -> Self {
        Self {
            picture,
            parallax_input: parallax.to_string(),
            parallax,
        }
    }

    /// With the parallax input set to `input`.
    fn with_parallax(&self, input: String) -> Self {
        Self {
            parallax: parse_parallax(&input).unwrap_or(self.parallax),
            parallax_input: input,
            ..self.clone()
        }
    }
}

/// Draws the background over the whole canvas, drifting and zooming with the
/// camera the items are drawn through.
struct BackgroundView<'a> {
    background: &'a Background,
    board: Board,
    camera: Camera,
}

impl canvas::Program<Message> for BackgroundView<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let screen = bounds.size();
        let image = self.background.picture.size();

        // Larger than the screen by the parallax, leaving it room to drift
        // either way. Along a side the screen crops more off, it drifts no
        // further, so it drifts alike in any shape of window.
        let parallax = self.background.parallax / 100.0;
        let room = Vector::new(screen.width, screen.height) * (parallax * 0.5);

        // Zoomed in closer than the whole board, it grows too.
        let zoom = background_zoom(
            self.camera.zoom / fit_scale(self.board.size, screen),
            parallax,
        );
        let size = image * (cover_scale(image, screen) * (1.0 + parallax) * zoom);

        // Where the camera looks across the board, from -1 at one side to 1
        // at the other. Looking further out takes it no further.
        let center = self.camera.to_board(Vector::ZERO);
        let half = self.board.size * 0.5;
        let across = Vector::new(
            (center.x / half.width).clamp(-1.0, 1.0),
            (center.y / half.height).clamp(-1.0, 1.0),
        );

        // Like a far scene, it shifts the way the board does. It grows around
        // the middle of the screen, so the point there stays put, and its
        // shift grows with it, which leaves its edges as far out of sight.
        let middle = frame.center() - Vector::new(across.x * room.x, across.y * room.y) * zoom;
        let top_left = middle - Vector::new(size.width, size.height) * 0.5;

        frame.draw_image(
            Rectangle::new(top_left, size),
            canvas::Image::new(self.background.picture.image.handle().clone()),
        );

        vec![frame.into_geometry()]
    }
}

/// Draws the board through the camera, under the items, while there is no
/// background.
struct BoardView {
    board: Board,
    camera: Camera,
}

impl canvas::Program<Message> for BoardView {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        // Draw in board coordinates from here on.
        self.camera.apply(&mut frame);

        let size = self.board.size;
        let top_left = Point::ORIGIN - Vector::new(size.width, size.height) * 0.5;
        // Not `fill_rectangle`, which the wgpu renderer keeps axis-aligned,
        // squashing the board when the camera is turned.
        frame.fill(&canvas::Path::rectangle(top_left, size), BOARD_COLOR);

        vec![frame.into_geometry()]
    }
}

/// Draws a run of items through the camera, see [`Item::shares_canvas`].
struct ItemsView<'a> {
    camera: Camera,
    items: &'a [Item],
}

impl canvas::Program<Message> for ItemsView<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        self.camera.apply(&mut frame);

        for item in self.items {
            item.draw(&mut frame);
        }

        vec![frame.into_geometry()]
    }
}

/// Distance in screen pixels from an item within which the cursor is on it.
const HIT_DISTANCE: f32 = 6.0;

/// Distance in screen pixels between the points of a pencil stroke.
const PENCIL_STEP: f32 = 2.0;

/// Distance in screen pixels from the start of a path within which a click
/// closes it.
const CLOSE_DISTANCE: f32 = 8.0;

/// Shortest side, in screen pixels, of an area dragged out for an image. A
/// smaller drag counts as a click, which places the image in an area
/// [`IMAGE_AREA`] times the size of the board around it.
const MIN_IMAGE_DRAG: f32 = 8.0;
const IMAGE_AREA: f32 = 0.5;

/// Gap and width, in screen pixels, of the outline around the selected item.
const SELECTION_GAP: f32 = 4.0;
const SELECTION_WIDTH: f32 = 2.0;

/// Size in screen pixels of the handles on the outline of the selected item,
/// and how far from their centers they can be grabbed.
const HANDLE_SIZE: f32 = 8.0;
const HANDLE_REACH: f32 = 8.0;

/// Distance in screen pixels of the handle that turns the selected item
/// above its outline.
const TURN_HANDLE_DISTANCE: f32 = 24.0;

/// Angle in radians that items turn by at a time with Shift held.
const TURN_STEP: f32 = PI / 12.0;

/// Speed of the cursor in screen pixels per second below which it counts as
/// moving slowly, so turning the camera or an item stops at right angles
/// (see [`Turn`]).
const SLOW_TURN_SPEED: f32 = 300.0;

/// How far back the speed of the cursor is measured over.
const SPEED_WINDOW: Duration = Duration::from_millis(100);

/// Angle in radians (10°) that a turn stopped at a right angle must be
/// dragged past it to go on.
const RIGHT_ANGLE_HOLD: f32 = PI / 18.0;

/// Size in screen pixels an item can't be resized below, unless it already
/// is, so it doesn't vanish or turn inside out.
const MIN_RESIZE: f32 = 4.0;

/// Directions from an item's center of the sides and corners of its outline
/// it can be resized from, along its own axes.
const RESIZE_SIDES: [Vector; 8] = [
    Vector::new(-1.0, -1.0),
    Vector::new(0.0, -1.0),
    Vector::new(1.0, -1.0),
    Vector::new(1.0, 0.0),
    Vector::new(1.0, 1.0),
    Vector::new(0.0, 1.0),
    Vector::new(-1.0, 1.0),
    Vector::new(-1.0, 0.0),
];

/// Width in screen pixels of the caret of the text being written.
const CARET_WIDTH: f32 = 2.0;

/// Pointer state kept by the tool canvas between events.
#[derive(Debug, Default)]
struct Interaction {
    modifiers: keyboard::Modifiers,
    trail: Trail,
    grab: Option<Grab>,
    /// Item being drawn, previewed until it is finished.
    draft: Option<Draft>,
    /// The last click, to tell double clicks.
    click: Option<Click>,
}

/// Recent cursor positions on screen, oldest first, to tell how fast it
/// moves.
#[derive(Debug, Default)]
struct Trail(VecDeque<(Instant, Point)>);

impl Trail {
    /// Adds the cursor at `position`, returning its speed in screen pixels
    /// per second over the last [`SPEED_WINDOW`].
    fn extend(&mut self, position: Point, now: Instant) -> f32 {
        // The speed is measured from the newest position at least the
        // window old, so after a pause it is slow.
        while self
            .0
            .get(1)
            .is_some_and(|&(time, _)| now.saturating_duration_since(time) >= SPEED_WINDOW)
        {
            self.0.pop_front();
        }

        let speed = self.0.front().map_or(0.0, |&(time, from)| {
            let elapsed = now.saturating_duration_since(time).as_secs_f32();
            if elapsed > 0.0 {
                from.distance(position) / elapsed
            } else {
                0.0
            }
        });

        self.0.push_back((now, position));
        speed
    }
}

/// What is being dragged.
#[derive(Debug, Clone)]
enum Grab {
    /// The camera, with the last cursor position, and its rotation while
    /// turned (see [`Mode::Rotate`]).
    Camera { last: Point, turn: Turn },
    /// The selected item, as it was when grabbed at the board point `start`,
    /// and its rotation while `edit` turns it.
    Item {
        item: Item,
        start: Point,
        edit: Edit,
        turn: Turn,
    },
}

/// A rotation being dragged. Reaching a right angle while the cursor moves
/// slowly, it stops there until dragged [`RIGHT_ANGLE_HOLD`] past it either
/// way, so it is easy to set straight. Moving fast, it goes right through.
#[derive(Debug, Clone, Copy)]
struct Turn {
    /// Rotation the cursor drags to, not stopping. It isn't wrapped to a
    /// full turn, so it changes smoothly.
    free: f32,
    /// The right angle the rotation is stopped at.
    stop: Option<f32>,
}

impl Turn {
    fn new(rotation: f32) -> Self {
        Self {
            free: rotation,
            stop: None,
        }
    }

    /// Drags the rotation to `rotation` (or any whole turns away from it),
    /// returning the rotation to show. Only while the cursor moves `slow`
    /// does it stop at the right angles it reaches.
    fn to(&mut self, rotation: f32, slow: bool) -> f32 {
        let from = self.free;
        let to = from + wrapped(rotation - from);
        self.free = to;

        if let Some(stop) = self.stop {
            if (to - stop).abs() <= RIGHT_ANGLE_HOLD {
                return stop;
            }
            self.stop = None;
        }

        // Reached when dragged onto or across it. Starting from it (give or
        // take rounding) isn't reaching it, so a drag from a right angle
        // turns right away.
        let right = (to / FRAC_PI_2).round() * FRAC_PI_2;
        let reached = (from - right).abs() > 1e-4 && (from - right) * (to - right) <= 0.0;

        if slow && reached && (to - right).abs() <= RIGHT_ANGLE_HOLD {
            self.stop = Some(right);
            right
        } else {
            to
        }
    }
}

/// What dragging the selected item does to it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Edit {
    Move,
    /// Turns it around the center of its outline.
    Turn,
    /// Resizes it from the side or corner of its outline in this direction
    /// (see [`RESIZE_SIDES`]), while the opposite one stays in place.
    Resize(Vector),
}

/// An item being drawn.
#[derive(Debug, Clone)]
enum Draft {
    /// Dragged from `start` to `end` with `tool`, see [`shape`]. For
    /// [`Tool::Image`], the area to place the image in.
    Drag {
        tool: Tool,
        start: Point,
        end: Point,
    },
    /// The points of a pencil stroke, while the button is held.
    Stroke(Vec<Point>),
    /// The points of a path clicked so far, and the cursor, where the next
    /// segment would end.
    Path { points: Vec<Point>, cursor: Point },
    /// Text typed so far.
    Text { position: Point, content: String },
}

/// Turns pointer and key input into camera moves and edits of the items, by
/// tool, and draws what is being drawn and the selection over the items.
struct ToolView<'a> {
    board: Board,
    camera: Camera,
    items: &'a [Item],
    selected: Option<usize>,
    mode: Mode,
    tool: Tool,
    /// How new items are drawn. Closed shapes are filled in as it says, see
    /// [`Tool::can_fill`].
    style: &'a Style,
    /// Corners of new polygons, and points of new stars.
    corners: usize,
}

impl ToolView<'_> {
    /// Converts a cursor position to board coordinates.
    fn to_board(&self, position: Point, bounds: Rectangle) -> Point {
        self.camera.to_board(position - bounds.center())
    }

    /// Index of the topmost item at `point`.
    fn item_at(&self, point: Point) -> Option<usize> {
        let distance = HIT_DISTANCE / self.camera.zoom;
        self.items
            .iter()
            .rposition(|item| item.hits(point, distance))
    }

    /// The item `draft` makes so far, unless it would be empty.
    fn item(&self, draft: &Draft) -> Option<Item> {
        let color = self.style.ink.color;

        let shape = match draft {
            Draft::Drag { tool, start, end } => {
                shape(*tool, *start, *end, self.corners, color, self.style.filled)
            }
            Draft::Stroke(points) | Draft::Path { points, .. } => {
                (points.len() > 1).then(|| Shape::Polyline {
                    points: points.clone(),
                    closed: false,
                    filled: false,
                    color,
                })
            }
            Draft::Text { position, content } => {
                (!content.trim().is_empty()).then(|| Shape::Text {
                    position: *position,
                    content: content.clone(),
                    size: self.style.text_size.value,
                    typeface: self.style.typeface.clone(),
                    color,
                })
            }
        };

        shape.map(Item::new)
    }

    /// The message that finishes `draft`, if anything comes of it.
    fn finish(&self, draft: &Draft) -> Option<Message> {
        match *draft {
            Draft::Drag {
                tool: Tool::Image,
                start,
                end,
            } => Some(Message::PlaceImage(self.image_area(start, end))),
            ref draft => self.item(draft).map(Message::ItemDrawn),
        }
    }

    /// The outline drawn around `item` when it is selected, in its own
    /// coordinates: its extent, with a gap past the thickness of its lines.
    fn outline(&self, item: &Item) -> Rectangle {
        item.shape
            .extent()
            .expand(LINE_WIDTH / 2.0 + SELECTION_GAP / self.camera.zoom)
    }

    /// The handles on the outline of `item`, by what dragging them does, and
    /// where they are in its own coordinates.
    fn handles(&self, item: &Item) -> impl Iterator<Item = (Edit, Point)> {
        let outline = self.outline(item);
        let center = outline.center();
        let keeps_proportions = item.shape.keeps_proportions();
        // Too narrow for handles on both sides and the corners between them
        // to fit.
        let is_flat = |size: f32| size * self.camera.zoom < 2.0 * HANDLE_SIZE;
        let (is_thin, is_flat) = (is_flat(outline.width), is_flat(outline.height));

        let resize = RESIZE_SIDES
            .into_iter()
            // Shapes that keep their proportions resize from their corners,
            // and flat ones (e.g. a level line) along themselves.
            .filter(move |side| {
                let is_corner = side.x != 0.0 && side.y != 0.0;

                match (is_thin, is_flat) {
                    _ if keeps_proportions => is_corner,
                    (true, true) => is_corner,
                    (false, true) => side.y == 0.0,
                    (true, false) => side.x == 0.0,
                    (false, false) => true,
                }
            })
            .map(move |side| {
                let offset = Vector::new(side.x * outline.width, side.y * outline.height) * 0.5;
                (Edit::Resize(side), center + offset)
            });

        let turn = Point::new(
            center.x,
            outline.y - TURN_HANDLE_DISTANCE / self.camera.zoom,
        );

        resize.chain([(Edit::Turn, turn)])
    }

    /// What dragging from the board point `point` with the select tool
    /// edits: a handle of the selected item, else the topmost item there,
    /// else the selected item, if `point` is in its outline.
    fn grabbed_at(&self, point: Point) -> Option<(usize, Edit)> {
        let selected = self
            .selected
            .and_then(|index| Some((index, self.items.get(index)?)));

        let handle = selected.and_then(|(index, item)| {
            let local = item.to_local(point);
            let (edit, _) = self
                .handles(item)
                .find(|(_, at)| at.distance(local) * self.camera.zoom <= HANDLE_REACH)?;
            Some((index, edit))
        });

        handle
            .or_else(|| Some((self.item_at(point)?, Edit::Move)))
            .or_else(|| {
                let (index, item) = selected?;
                self.outline(item)
                    .contains(item.to_local(point))
                    .then_some((index, Edit::Move))
            })
    }

    /// `item` edited by dragging it from the board point `start` to `point`.
    fn edited(&self, item: &Item, edit: Edit, start: Point, point: Point, shift: bool) -> Item {
        match edit {
            Edit::Move => item.translated(point - start),
            Edit::Turn => {
                let center = item.center();
                let mut angle = angle_between(start - center, point - center);

                // Shift turns it in steps.
                if shift {
                    let rotation = ((item.rotation + angle) / TURN_STEP).round() * TURN_STEP;
                    angle = rotation - item.rotation;
                }

                item.turned(center, angle)
            }
            Edit::Resize(side) => {
                let extent = item.shape.extent();
                let half = Vector::new(side.x * extent.width, side.y * extent.height) * 0.5;
                // The side or corner dragged, and the opposite one.
                let dragged = extent.center() + half;
                let anchor = extent.center() - half;
                let delta = rotated(point - start, -item.rotation);

                // The least a size can be stretched by.
                let min = MIN_RESIZE / self.camera.zoom;
                let least = |size: f32| {
                    if size > 0.0 {
                        min.min(size) / size
                    } else {
                        0.0
                    }
                };
                let factor = |side: f32, size: f32, delta: f32| {
                    if side == 0.0 || size == 0.0 {
                        1.0
                    } else {
                        ((size + delta * side) / size).max(least(size))
                    }
                };

                let mut factors = Vector::new(
                    factor(side.x, extent.width, delta.x),
                    factor(side.y, extent.height, delta.y),
                );

                // Shift keeps the proportions.
                if shift || item.shape.keeps_proportions() {
                    let factor = if side.x != 0.0 && side.y != 0.0 {
                        // How far along the diagonal through the corner it
                        // is dragged.
                        let dot = |a: Vector, b: Vector| a.x * b.x + a.y * b.y;
                        let diagonal = dragged - anchor;
                        dot(diagonal + delta, diagonal) / dot(diagonal, diagonal)
                    } else if side.x != 0.0 {
                        factors.x
                    } else {
                        factors.y
                    };
                    let factor = factor.max(least(extent.width)).max(least(extent.height));
                    factors = Vector::new(factor, factor);
                }

                Item {
                    shape: item.shape.scaled(anchor, factors),
                    rotation: item.rotation,
                }
            }
        }
    }

    /// The cursor for dragging `item` with `edit`.
    fn edit_cursor(&self, item: &Item, edit: Edit, is_dragging: bool) -> mouse::Interaction {
        match edit {
            Edit::Move => mouse::Interaction::Move,
            Edit::Turn if is_dragging => mouse::Interaction::Grabbing,
            Edit::Turn => mouse::Interaction::Grab,
            Edit::Resize(side) => {
                // Along the way the side moves on screen, either way.
                let side = rotated(side, item.rotation + self.camera.rotation);
                let angle = side.y.atan2(side.x).rem_euclid(PI);

                match (angle / (PI / 4.0)).round() as u8 % 4 {
                    0 => mouse::Interaction::ResizingHorizontally,
                    1 => mouse::Interaction::ResizingDiagonallyDown,
                    2 => mouse::Interaction::ResizingVertically,
                    _ => mouse::Interaction::ResizingDiagonallyUp,
                }
            }
        }
    }

    /// Area to place an image in, dragged out from `start` to `end`.
    fn image_area(&self, start: Point, end: Point) -> Rectangle {
        let area = enclosing([start, end]);

        if area.width.min(area.height) * self.camera.zoom >= MIN_IMAGE_DRAG {
            area
        } else {
            let size = self.board.size * IMAGE_AREA;
            Rectangle::new(start - Vector::new(size.width, size.height) * 0.5, size)
        }
    }

    fn press(
        &self,
        state: &mut Interaction,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let Some(position) = cursor.position_over(bounds) else {
            // Clicking elsewhere (e.g. on another tool) finishes the path or
            // text being drawn, and goes on to what was clicked.
            let draft = state.draft.take()?;
            return Some(publish_or_redraw(self.finish(&draft)));
        };

        let point = self.to_board(position, bounds);
        let click = Click::new(position, mouse::Button::Left, state.click);
        state.click = Some(click);

        let action = match self.tool {
            Tool::Select => {
                let grabbed = self.grabbed_at(point);
                state.grab = Some(match grabbed {
                    Some((index, edit)) => Grab::Item {
                        item: self.items[index].clone(),
                        start: point,
                        edit,
                        turn: Turn::new(self.items[index].rotation),
                    },
                    None => Grab::Camera {
                        last: position,
                        turn: Turn::new(self.camera.rotation),
                    },
                });
                canvas::Action::publish(Message::ItemSelected(grabbed.map(|(index, _)| index)))
            }
            Tool::Pan => {
                state.grab = Some(Grab::Camera {
                    last: position,
                    turn: Turn::new(self.camera.rotation),
                });
                canvas::Action::publish(Message::CameraGrabbed)
            }
            Tool::Pencil => {
                state.draft = Some(Draft::Stroke(vec![point]));
                canvas::Action::request_redraw()
            }
            Tool::Path => {
                let Some(Draft::Path { points, .. }) = &mut state.draft else {
                    state.draft = Some(Draft::Path {
                        points: vec![point],
                        cursor: point,
                    });
                    return Some(canvas::Action::request_redraw().and_capture());
                };

                // Clicking the start closes the path, and a double click
                // ends it where the first of the two clicks put its last
                // point.
                if points.len() >= MIN_CORNERS
                    && points[0].distance(point) * self.camera.zoom <= CLOSE_DISTANCE
                {
                    let points = std::mem::take(points);
                    state.draft = None;
                    canvas::Action::publish(Message::ItemDrawn(Item::new(Shape::Polyline {
                        points,
                        closed: true,
                        filled: self.style.filled,
                        color: self.style.ink.color,
                    })))
                } else if click.kind() == click::Kind::Double {
                    let draft = state.draft.take()?;
                    publish_or_redraw(self.finish(&draft))
                } else {
                    points.push(point);
                    canvas::Action::request_redraw()
                }
            }
            Tool::Text => {
                // Clicking again finishes the text being written, and starts
                // another where clicked.
                let finished = state.draft.take().and_then(|draft| self.finish(&draft));
                state.draft = Some(Draft::Text {
                    position: point,
                    content: String::new(),
                });
                publish_or_redraw(finished)
            }
            Tool::Line
            | Tool::Arrow
            | Tool::Rectangle
            | Tool::Ellipse
            | Tool::Polygon
            | Tool::Star
            | Tool::Image => {
                state.draft = Some(Draft::Drag {
                    tool: self.tool,
                    start: point,
                    end: point,
                });
                canvas::Action::request_redraw()
            }
        };

        Some(action.and_capture())
    }

    fn cursor_moved(
        &self,
        state: &mut Interaction,
        bounds: Rectangle,
        position: Point,
    ) -> Option<canvas::Action<Message>> {
        let point = self.to_board(position, bounds);
        let slow = state.trail.extend(position, Instant::now()) < SLOW_TURN_SPEED;

        if let Some(grab) = &mut state.grab {
            let message = match grab {
                Grab::Camera { last, turn } => {
                    let message = match self.mode {
                        Mode::Move => Message::CameraMoved(position - *last),
                        // Turns the board like a knob, keeping the cursor on
                        // the same angle around the center, but for stops at
                        // right angles.
                        Mode::Rotate => {
                            let angle =
                                angle_between(*last - bounds.center(), position - bounds.center());
                            Message::CameraRotated(turn.to(turn.free + angle, slow))
                        }
                    };
                    *last = position;
                    message
                }
                // From the item as it was when grabbed, so the edit doesn't
                // drift as it goes.
                Grab::Item {
                    item,
                    start,
                    edit,
                    turn,
                } => {
                    let shift = state.modifiers.shift();
                    let mut edited = self.edited(item, *edit, *start, point, shift);

                    // Turning in steps with Shift doesn't stop at right
                    // angles too.
                    if *edit == Edit::Turn {
                        let rotation = turn.to(edited.rotation, slow && !shift);
                        edited =
                            edited.turned(edited.center(), wrapped(rotation - edited.rotation));
                    }

                    Message::ItemEdited(edited)
                }
            };

            return Some(canvas::Action::publish(message));
        }

        match state.draft.as_mut()? {
            Draft::Drag { end, .. } => *end = point,
            Draft::Stroke(points) => {
                // Leaving out points too close together keeps long strokes
                // light.
                if points
                    .last()
                    .is_some_and(|last| last.distance(point) * self.camera.zoom < PENCIL_STEP)
                {
                    return None;
                }
                points.push(point);
            }
            Draft::Path { cursor, .. } => *cursor = point,
            Draft::Text { .. } => return None,
        }

        Some(canvas::Action::request_redraw())
    }

    fn release(&self, state: &mut Interaction) -> Option<canvas::Action<Message>> {
        if state.grab.take().is_some() {
            return Some(canvas::Action::publish(Message::Released));
        }

        // Paths and text go on after the button is released.
        if !matches!(state.draft, Some(Draft::Drag { .. } | Draft::Stroke(_))) {
            return None;
        }

        let draft = state.draft.take()?;
        Some(publish_or_redraw(self.finish(&draft)))
    }

    fn key_press(
        &self,
        state: &mut Interaction,
        key: &keyboard::Key,
        text: Option<&str>,
        modifiers: keyboard::Modifiers,
    ) -> Option<canvas::Action<Message>> {
        // Shortcuts go on to the app.
        if modifiers.command() {
            return None;
        }

        let key = key.as_ref();

        let action = match state.draft.as_mut()? {
            Draft::Text { content, .. } => match key {
                keyboard::Key::Named(Named::Escape) => {
                    let draft = state.draft.take()?;
                    publish_or_redraw(self.finish(&draft))
                }
                keyboard::Key::Named(Named::Enter) => {
                    content.push('\n');
                    canvas::Action::request_redraw()
                }
                keyboard::Key::Named(Named::Backspace) => {
                    content.pop();
                    canvas::Action::request_redraw()
                }
                _ => {
                    let typed: String = text?.chars().filter(|c| !c.is_control()).collect();

                    if typed.is_empty() {
                        return None;
                    }

                    content.push_str(&typed);
                    canvas::Action::request_redraw()
                }
            },
            Draft::Path { points, .. } => match key {
                keyboard::Key::Named(Named::Enter) => {
                    let draft = state.draft.take()?;
                    publish_or_redraw(self.finish(&draft))
                }
                keyboard::Key::Named(Named::Escape) => {
                    state.draft = None;
                    canvas::Action::request_redraw()
                }
                keyboard::Key::Named(Named::Backspace) => {
                    points.pop();
                    if points.is_empty() {
                        state.draft = None;
                    }
                    canvas::Action::request_redraw()
                }
                _ => return None,
            },
            Draft::Drag { .. } | Draft::Stroke(_) => return None,
        };

        Some(action.and_capture())
    }
}

/// Publishes `message`, or else redraws, e.g. to clear a draft that came to
/// nothing.
fn publish_or_redraw(message: Option<Message>) -> canvas::Action<Message> {
    message.map_or_else(canvas::Action::request_redraw, canvas::Action::publish)
}

impl canvas::Program<Message> for ToolView<'_> {
    type State = Interaction;

    fn update(
        &self,
        state: &mut Self::State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                state.modifiers = *modifiers;
                None
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                modifiers,
                text,
                ..
            }) => self.key_press(state, key, text.as_deref(), *modifiers),
            Event::Mouse(mouse::Event::WheelScrolled { delta }) if state.modifiers.control() => {
                let position = cursor.position_over(bounds)?;
                let lines = match *delta {
                    mouse::ScrollDelta::Lines { y, .. } => y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / PIXELS_PER_LINE,
                };

                Some(
                    canvas::Action::publish(Message::CameraZoomed {
                        anchor: position - bounds.center(),
                        factor: ZOOM_STEP.powf(lines),
                    })
                    .and_capture(),
                )
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                self.press(state, bounds, cursor)
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                self.cursor_moved(state, bounds, *position)
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => self.release(state),
            _ => None,
        }
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        match (&state.grab, &state.draft) {
            (Some(Grab::Camera { .. }), _) => return mouse::Interaction::Grabbing,
            (Some(Grab::Item { item, edit, .. }), _) => {
                return self.edit_cursor(item, *edit, true);
            }
            // Until the button is released, wherever the cursor is.
            (None, Some(Draft::Drag { .. } | Draft::Stroke(_))) => {
                return mouse::Interaction::Crosshair;
            }
            _ => {}
        }

        let Some(position) = cursor.position_over(bounds) else {
            return mouse::Interaction::default();
        };

        match self.tool {
            Tool::Select => match self.grabbed_at(self.to_board(position, bounds)) {
                Some((index, edit)) => self.edit_cursor(&self.items[index], edit, false),
                None => mouse::Interaction::Grab,
            },
            Tool::Pan => mouse::Interaction::Grab,
            Tool::Text => mouse::Interaction::Text,
            _ => mouse::Interaction::Crosshair,
        }
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        self.camera.apply(&mut frame);

        // A screen pixel, for marks as thick at any zoom.
        let pixel = 1.0 / self.camera.zoom;

        if let Some(item) = self.selected.and_then(|index| self.items.get(index)) {
            let outline = self.outline(item);
            let line = canvas::Stroke::default()
                .with_color(theme.extended_palette().primary.base.color)
                .with_width(SELECTION_WIDTH * pixel);

            frame.with_save(|frame| {
                // In the item's own coordinates, so the outline turns with it.
                frame.rotate(item.rotation);
                frame.stroke(
                    &canvas::Path::rectangle(outline.position(), outline.size()),
                    line,
                );

                for (edit, at) in self.handles(item) {
                    let handle = if edit == Edit::Turn {
                        frame.stroke(&canvas::Path::line(Point::new(at.x, outline.y), at), line);
                        canvas::Path::circle(at, HANDLE_SIZE / 2.0 * pixel)
                    } else {
                        let size = Size::new(HANDLE_SIZE, HANDLE_SIZE) * pixel;
                        canvas::Path::rectangle(
                            at - Vector::new(size.width, size.height) * 0.5,
                            size,
                        )
                    };

                    frame.fill(&handle, Color::WHITE);
                    frame.stroke(&handle, line);
                }
            });
        }

        if let Some(draft) = &state.draft {
            if let Some(item) = self.item(draft) {
                item.draw(&mut frame);
            }

            let color = self.style.ink.color;
            let mark = canvas::Stroke::default()
                .with_color(color)
                .with_width(pixel);

            match draft {
                Draft::Drag {
                    tool: Tool::Image,
                    start,
                    end,
                } => {
                    let area = enclosing([*start, *end]);
                    let dash = [4.0 * pixel; 2];
                    frame.stroke(
                        &canvas::Path::rectangle(area.position(), area.size()),
                        canvas::Stroke {
                            line_dash: canvas::LineDash {
                                segments: &dash,
                                offset: 0,
                            },
                            ..mark
                        },
                    );
                }
                Draft::Path { points, cursor } => {
                    if let Some(last) = points.last() {
                        frame.stroke(&canvas::Path::line(*last, *cursor), stroke(color));
                    }

                    // Where clicking closes the path.
                    if points.len() >= MIN_CORNERS {
                        frame.stroke(
                            &canvas::Path::circle(points[0], CLOSE_DISTANCE * pixel),
                            mark,
                        );
                    }
                }
                Draft::Text { position, content } => {
                    // After the last character, as text doesn't wrap.
                    let (row, line) = content.split('\n').enumerate().last().unwrap_or_default();
                    let size = self.style.text_size.value;
                    let line_height = size * TEXT_LINE_HEIGHT;
                    let width = text_size(line, size, self.style.typeface.font()).width;
                    let top = *position + Vector::new(width, row as f32 * line_height);

                    frame.stroke(
                        &canvas::Path::line(top, top + Vector::new(0.0, line_height)),
                        mark.with_width(CARET_WIDTH * pixel),
                    );
                }
                Draft::Drag { .. } | Draft::Stroke(_) => {}
            }
        }

        vec![frame.into_geometry()]
    }
}
