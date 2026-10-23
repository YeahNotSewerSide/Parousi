use std::f32::consts::{PI, TAU};
use std::fmt;

use iced::alignment::Horizontal;
use iced::time::{Duration, Instant};
use iced::widget::canvas::{self, Canvas, Event, Geometry};
use iced::widget::{
    Column, Row, button, column, container, pick_list, row, rule, scrollable, text, text_input,
};
use iced::{
    Center, Color, Element, Fill, Point, Rectangle, Renderer, Size, Subscription, Theme, Vector,
    keyboard, mouse, window,
};

fn main() -> iced::Result {
    iced::application(Parousi::default, Parousi::update, Parousi::view)
        .subscription(Parousi::subscription)
        .title("Parousi")
        .theme(Theme::Dark)
        .window_size((1280.0, 800.0))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tool {
    #[default]
    Select,
    Pan,
    Line,
    Arrow,
    Rectangle,
    Ellipse,
    Polygon,
    Star,
    Pencil,
    Path,
    Text,
    Image,
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
    ModeSelected(Mode),
    ToolSelected(Tool),
    FrameSelected(usize),
    /// Moves the selected frame's camera by a screen offset.
    CameraMoved(Vector),
    /// Turns the selected frame's camera, see [`Camera::rotate`].
    CameraRotated(f32),
    /// Zooms the selected frame's camera, see [`Camera::zoom_at`].
    CameraZoomed {
        anchor: Vector,
        factor: f32,
    },
    /// The camera started being dragged.
    CameraGrabbed,
    /// The camera stopped being dragged.
    CameraReleased,
    ItemDrawn(Item),
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
    Undo,
    Redo,
    Preview,
    Save,
    Reload,
    New,
    Settings,
    About,
    DeleteFrame,
    AddFrame,
}

struct Parousi {
    aspect_ratio: AspectRatio,
    mode: Mode,
    tool: Tool,
    /// Never empty, so there is always a camera to edit.
    frames: Vec<Frame>,
    /// Index of the selected frame.
    frame: usize,
    /// Shared by all frames.
    items: Vec<Item>,
    /// Camera move to the selected frame, while a transition plays.
    flight: Option<Flight>,
    history: History,
}

impl Default for Parousi {
    fn default() -> Self {
        Self {
            aspect_ratio: AspectRatio::default(),
            mode: Mode::default(),
            tool: Tool::default(),
            frames: vec![Frame::default()],
            frame: 0,
            items: Vec::new(),
            flight: None,
            history: History::default(),
        }
    }
}

impl Parousi {
    fn update(&mut self, message: Message) {
        match message {
            Message::AspectWidthChanged(width) => {
                let from = self.aspect_ratio.clone();
                let to = from.with_inputs(width, from.height.clone());
                self.perform(Action::AspectRatio { from, to });
            }
            Message::AspectHeightChanged(height) => {
                let from = self.aspect_ratio.clone();
                let to = from.with_inputs(from.width.clone(), height);
                self.perform(Action::AspectRatio { from, to });
            }
            Message::ModeSelected(mode) => self.mode = mode,
            Message::ToolSelected(tool) => self.tool = tool,
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
            Message::CameraRotated(angle) => {
                let from = self.frames[self.frame].camera;
                let mut to = from;
                to.rotate(angle);
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
            Message::CameraGrabbed | Message::CameraReleased => self.history.seal(),
            Message::ItemDrawn(item) => self.perform(Action::ItemAdded {
                index: self.items.len(),
                item,
            }),
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
            Message::Preview
            | Message::Save
            | Message::Reload
            | Message::New
            | Message::Settings
            | Message::About => {}
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let shortcuts = keyboard::listen().filter_map(shortcut);

        if self.flight.is_some() {
            Subscription::batch([shortcuts, window::frames().map(Message::Tick)])
        } else {
            shortcuts
        }
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
            Action::AspectRatio { to, .. } => self.aspect_ratio = to,
            Action::Camera { frame, to, .. } => {
                // Lands any transition, so the camera shows the edit.
                self.flight = None;
                self.frame = frame;
                self.frames[frame].camera = to;
            }
            Action::Transition { frame, to, .. } => self.frames[frame].transition = to,
            Action::ItemAdded { index, item } => self.items.insert(index, item),
            Action::ItemRemoved { index, .. } => {
                self.items.remove(index);
            }
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

    fn view(&self) -> Element<'_, Message> {
        column![
            row![self.workspace(), self.palette()].height(Fill),
            rule::horizontal(1),
            self.toolbar(),
            rule::horizontal(1),
            self.timeline(),
        ]
        .into()
    }

    fn workspace(&self) -> Element<'_, Message> {
        let board = BoardView {
            board: Board::new(self.aspect_ratio.value),
            camera: self.camera(),
            items: &self.items,
            mode: self.mode,
            tool: self.tool,
        };

        container(Canvas::new(board).width(Fill).height(Fill))
            .width(Fill)
            .height(Fill)
            .style(|theme: &Theme| {
                container::background(theme.extended_palette().background.weakest.color)
            })
            .into()
    }

    fn palette(&self) -> Element<'_, Message> {
        let sections = PALETTE.iter().map(|(title, tools)| {
            let rows = tools.chunks(2).map(|pair| {
                Row::with_children(pair.iter().map(|&tool| self.tool_button(tool)))
                    .spacing(8)
                    .into()
            });

            column![
                text(*title).size(14),
                Column::with_children(rows).spacing(8)
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
                Column::with_children(sections).spacing(20),
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

    fn toolbar(&self) -> Element<'_, Message> {
        let aspect_ratio = row![
            text("Aspect ratio:"),
            text_input("", &self.aspect_ratio.width)
                .on_input(Message::AspectWidthChanged)
                .width(60),
            text(":"),
            text_input("", &self.aspect_ratio.height)
                .on_input(Message::AspectHeightChanged)
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

        let presentation = action_button("Preview", Message::Preview);

        let file = row![
            action_button("Save", Message::Save),
            action_button("Reload", Message::Reload),
            action_button("New", Message::New),
        ]
        .spacing(2);

        let misc = row![
            action_button("Settings", Message::Settings),
            action_button("About", Message::About),
        ]
        .spacing(2);

        row![aspect_ratio, modes, history, presentation, file, misc]
            .spacing(24)
            .padding([8, 16])
            .align_y(Center)
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

    if !modifiers.command() {
        return None;
    }

    // Matching the latin letter keeps the shortcuts on the same keys in
    // other layouts (e.g. Cyrillic).
    match (key.to_latin(physical_key)?, modifiers.shift()) {
        ('z', false) => Some(Message::Undo),
        ('z', true) | ('y', false) => Some(Message::Redo),
        _ => None,
    }
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

impl Default for AspectRatio {
    fn default() -> Self {
        Self {
            width: String::from("4"),
            height: String::from("3"),
            value: 4.0 / 3.0,
        }
    }
}

impl AspectRatio {
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

/// An undoable edit of the presentation, holding enough to apply it either
/// way.
#[derive(Debug, Clone)]
enum Action {
    AspectRatio {
        from: AspectRatio,
        to: AspectRatio,
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
            Action::AspectRatio { from, to } => Action::AspectRatio { from: to, to: from },
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
            Action::FrameAdded { index, frame } => Action::FrameDeleted { index, frame },
            Action::FrameDeleted { index, frame } => Action::FrameAdded { index, frame },
        }
    }

    /// Whether applying the action leaves the presentation as it is (e.g.
    /// zooming in past the limit).
    fn is_noop(&self) -> bool {
        match self {
            Action::AspectRatio { from, to } => from == to,
            Action::Camera { from, to, .. } => from == to,
            Action::Transition { from, to, .. } => from == to,
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
            (Action::AspectRatio { to, .. }, Action::AspectRatio { to: next, .. }) => {
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

/// Board height in board units; the width follows the aspect ratio.
const BOARD_HEIGHT: f32 = 480.0;

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

impl Board {
    fn new(aspect_ratio: f32) -> Self {
        Self {
            size: Size::new(BOARD_HEIGHT * aspect_ratio, BOARD_HEIGHT),
        }
    }
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
        let turn = (to.rotation - self.rotation + PI).rem_euclid(TAU) - PI;
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

const LINE_COLOR: Color = Color::from_rgb8(0x25, 0x63, 0xEB);
const LINE_WIDTH: f32 = 3.0;

/// Something drawn on the board. Positions are in board coordinates (board
/// units relative to the board center), so items move and zoom with it.
#[derive(Debug, Clone, Copy)]
enum Item {
    Line { from: Point, to: Point },
}

impl Item {
    fn draw(&self, frame: &mut canvas::Frame) {
        match *self {
            Item::Line { from, to } => frame.stroke(
                &canvas::Path::line(from, to),
                canvas::Stroke::default()
                    .with_color(LINE_COLOR)
                    .with_width(LINE_WIDTH)
                    .with_line_cap(canvas::LineCap::Round),
            ),
        }
    }
}

/// Pointer state kept by the canvas between events.
#[derive(Debug, Default)]
struct Interaction {
    modifiers: keyboard::Modifiers,
    /// Last cursor position while the camera is being dragged.
    drag: Option<Point>,
    /// Item being drawn, previewed until the button is released.
    draft: Option<Item>,
}

/// Draws the board and its items through the selected frame's camera, and
/// turns pointer input into camera moves and new items.
struct BoardView<'a> {
    board: Board,
    camera: Camera,
    items: &'a [Item],
    mode: Mode,
    tool: Tool,
}

impl canvas::Program<Message> for BoardView<'_> {
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
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                if self.tool == Tool::Line =>
            {
                let start = self
                    .camera
                    .to_board(cursor.position_over(bounds)? - bounds.center());
                state.draft = Some(Item::Line {
                    from: start,
                    to: start,
                });
                Some(canvas::Action::capture())
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                state.drag = Some(cursor.position_over(bounds)?);
                Some(canvas::Action::publish(Message::CameraGrabbed).and_capture())
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if let Some(Item::Line { to, .. }) = &mut state.draft {
                    *to = self.camera.to_board(*position - bounds.center());
                    Some(canvas::Action::request_redraw())
                } else {
                    let last = state.drag.as_mut()?;
                    let message = match self.mode {
                        Mode::Move => Message::CameraMoved(*position - *last),
                        // Turns the board like a knob, keeping the cursor
                        // on the same angle around the center.
                        Mode::Rotate => Message::CameraRotated(angle_between(
                            *last - bounds.center(),
                            *position - bounds.center(),
                        )),
                    };
                    *last = *position;
                    Some(canvas::Action::publish(message))
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if state.drag.take().is_some() {
                    return Some(canvas::Action::publish(Message::CameraReleased));
                }

                match state.draft.take()? {
                    // A click without dragging draws nothing.
                    Item::Line { from, to } if from == to => Some(canvas::Action::request_redraw()),
                    item => Some(canvas::Action::publish(Message::ItemDrawn(item))),
                }
            }
            _ => None,
        }
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if state.drag.is_some() {
            mouse::Interaction::Grabbing
        } else if state.draft.is_some() || self.tool == Tool::Line && cursor.is_over(bounds) {
            mouse::Interaction::Crosshair
        } else if cursor.is_over(bounds) {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::default()
        }
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        // Draw in board coordinates from here on.
        frame.translate(frame.center() - Point::ORIGIN + self.camera.offset);
        frame.rotate(self.camera.rotation);
        frame.scale(self.camera.zoom);

        let size = self.board.size;
        let top_left = Point::ORIGIN - Vector::new(size.width, size.height) * 0.5;
        // Not `fill_rectangle`, which the wgpu renderer keeps axis-aligned,
        // squashing the board when the camera is turned.
        frame.fill(&canvas::Path::rectangle(top_left, size), Color::WHITE);

        for item in self.items.iter().chain(&state.draft) {
            item.draw(&mut frame);
        }

        vec![frame.into_geometry()]
    }
}
