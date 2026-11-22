use crate::data::PresetTool;
use crate::icons::Glyph;
use crate::ink::Tool;
use std::f32::consts::TAU;
use std::time::Instant;
use windows::Win32::Foundation::{POINT, RECT};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    Tool(Tool),
    Settings,
    Undo,
    Redo,
    Fullscreen,
    Open,
    Library,
    Metronome,
    Tuner,
    Panel,
    Favorite(PresetTool, u8),
    ToolOptions(Tool),
    Shapes,
    Back,
}

impl Command {
    pub fn icon(self) -> Glyph {
        match self {
            Self::Tool(Tool::Hand) => Glyph::Move,
            Self::Tool(Tool::Pen) => Glyph::Pencil,
            Self::Tool(Tool::Highlighter) => Glyph::Highlighter,
            Self::Tool(Tool::Eraser) => Glyph::Eraser,
            Self::Tool(Tool::Staff) => Glyph::Staff,
            Self::Metronome => Glyph::Metronome,
            Self::Tool(Tool::Rectangle) => Glyph::Square,
            Self::Tool(Tool::Select) => Glyph::Pointer,
            Self::Settings => Glyph::Sliders,
            Self::Undo => Glyph::Undo,
            Self::Redo => Glyph::Redo,
            Self::Fullscreen => Glyph::Expand,
            Self::Open | Self::Library => Glyph::BookOpen,
            Self::Tuner => Glyph::Gauge,
            Self::Favorite(tool, _) => Self::Tool(tool.into()).icon(),
            Self::Panel => Glyph::PanelLeft,
            Self::ToolOptions(_) => Glyph::Sliders,
            Self::Shapes => Glyph::Shapes,
            Self::Back => Glyph::ChevronLeft,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Tool(Tool::Hand) => tr!("Sfoglia", "Browse"),
            Self::Tool(Tool::Pen) => tr!("Penna", "Pen"),
            Self::Tool(Tool::Highlighter) => tr!("Evidenzia", "Highlight"),
            Self::Tool(Tool::Eraser) => tr!("Gomma", "Eraser"),
            Self::Tool(Tool::Staff) => tr!("Rigo", "Staff"),
            Self::Tool(Tool::Rectangle) => tr!("Rettangolo", "Rectangle"),
            Self::Tool(Tool::Select) => tr!("Seleziona", "Select"),
            Self::Settings => tr!("Opzioni", "Options"),
            Self::Undo => tr!("Annulla", "Cancel"),
            Self::Redo => tr!("Ripeti", "Redo"),
            Self::Fullscreen => tr!("Schermo", "Screen"),
            Self::Open => tr!("Apri PDF", "Open PDF"),
            Self::Library => tr!("Scalette", "Setlists"),
            Self::Metronome => tr!("Metronomo", "Metronome"),
            Self::Tuner => tr!("Accordatore", "Tuner"),
            Self::Panel => "Menu",
            Self::Favorite(_, index) => ["1", "2", "3"][usize::from(index)],
            Self::ToolOptions(_) => tr!("Opzioni dello strumento", "Tool options"),
            Self::Shapes => tr!("Altro", "More"),
            Self::Back => tr!("Indietro", "Back"),
        }
    }
}

const PEN_ANGLE: f32 = -TAU / 4.0;
const STUDIO: [Command; 8] = [
    Command::Tool(Tool::Pen),
    Command::Tool(Tool::Highlighter),
    Command::Tool(Tool::Eraser),
    Command::Tool(Tool::Select),
    Command::Tool(Tool::Staff),
    Command::Shapes,
    Command::Metronome,
    Command::Tuner,
];

const SHAPES: [Command; 2] = [Command::Tool(Tool::Rectangle), Command::Back];
const OPEN_SPAN: f32 = 158.0;
const PRESET_SPAN: f32 = 180.0;
const RING_RADIUS: f32 = 116.0;

#[derive(Default)]
enum Group {
    #[default]
    Tools,
    Shapes,
}

#[derive(Clone, Copy)]
enum Target {
    Hub,
    Item(Command),
}

struct Grab {
    id: u32,
    target: Target,
    start: POINT,
    start_center: (f32, f32),
    last: POINT,
    stamp: Instant,
    velocity: (f32, f32),
    drag: bool,
}

#[derive(Default)]
pub struct Radial {
    anchor: Option<(f32, f32)>,
    grab: Option<Grab>,
    velocity: (f32, f32),
    frame: Option<Instant>,
    progress: f32,
    pub open: bool,
    group: Group,
}

impl Radial {
    pub fn fit_scale(viewport: RECT, scale: f32) -> f32 {
        scale
            .min((viewport.bottom - viewport.top).max(1) as f32 / (PRESET_SPAN + OPEN_SPAN))
            .min((viewport.right - viewport.left).max(1) as f32 / (2.0 * OPEN_SPAN))
    }

    fn margins(&self, scale: f32) -> (f32, f32) {
        let progress = self.expansion();
        let margin = |span| (30.0 + (span - 30.0) * progress) * scale;
        let top = if matches!(self.group, Group::Tools) {
            PRESET_SPAN
        } else {
            OPEN_SPAN
        };
        (margin(top), margin(OPEN_SPAN))
    }

    fn expansion(&self) -> f32 {
        1.0 - (1.0 - self.progress).powi(3)
    }

    fn center(&self, viewport: RECT, scale: f32) -> (f32, f32) {
        let (width, height) = (
            viewport.right - viewport.left,
            viewport.bottom - viewport.top,
        );
        let position = self
            .anchor
            .map(|(x, y)| {
                (
                    viewport.left as f32 + x * width as f32,
                    viewport.top as f32 + y * height as f32,
                )
            })
            .unwrap_or((
                viewport.right as f32 - 35.0 * scale,
                viewport.bottom as f32 - 38.0 * scale,
            ));
        limit(position, viewport, self.margins(scale))
    }

    pub fn hub(&self, viewport: RECT, scale: f32) -> RECT {
        circle(self.center(viewport, scale), 25.0 * scale)
    }

    pub fn footprint(&self, viewport: RECT, scale: f32) -> RECT {
        let (x, y) = self.center(viewport, scale);
        let mut bounds = circle((x, y), 29.0 * scale);
        for rect in self
            .items(viewport, scale)
            .chain(self.options(viewport, scale))
            .map(|(_, rect)| rect)
            .chain(self.presets(viewport, scale).map(|(_, rect)| rect))
        {
            bounds.left = bounds.left.min(rect.left - 2);
            bounds.top = bounds.top.min(rect.top - 2);
            bounds.right = bounds.right.max(rect.right + 2);
            bounds.bottom = bounds.bottom.max(rect.bottom + 2);
        }
        bounds
    }

    pub fn items(&self, viewport: RECT, scale: f32) -> impl Iterator<Item = (Command, RECT)> {
        let center = self.center(viewport, scale);
        let progress = self.expansion();
        let commands: &[Command] = if progress < 0.25 {
            &[]
        } else {
            match self.group {
                Group::Tools => &STUDIO[..],
                Group::Shapes => &SHAPES[..],
            }
        };
        commands
            .iter()
            .copied()
            .enumerate()
            .map(move |(index, command)| {
                let angle = ring_angle(index, commands.len());
                let center = offset_at(center, angle, RING_RADIUS * scale * progress);
                (command, circle(center, (24.0 + 9.0 * progress) * scale))
            })
    }

    pub fn presets(
        &self,
        viewport: RECT,
        scale: f32,
    ) -> impl Iterator<Item = ((PresetTool, u8), RECT)> {
        let center = self.center(viewport, scale);
        let progress = self.expansion();
        let show = progress >= 0.8 && matches!(self.group, Group::Tools);
        [PresetTool::Pen, PresetTool::Highlighter]
            .into_iter()
            .enumerate()
            .filter(move |_| show)
            .flat_map(move |(index, tool)| {
                let ray = ring_angle(index, STUDIO.len());
                let tool_center = offset_at(center, ray, RING_RADIUS * scale * progress);
                [-28.0_f32, 0.0, 28.0]
                    .into_iter()
                    .enumerate()
                    .map(move |(index, angle)| {
                        let dot = offset_at(
                            tool_center,
                            ray + angle.to_radians(),
                            51.0 * scale * progress,
                        );
                        ((tool, index as u8), circle(dot, 11.5 * scale * progress))
                    })
            })
    }

    pub fn options(&self, viewport: RECT, scale: f32) -> impl Iterator<Item = (Command, RECT)> {
        self.items(viewport, scale)
            .filter_map(move |(command, rect)| {
                let Command::Tool(tool) = command else {
                    return None;
                };
                if !tool.has_options() || self.expansion() < 0.8 {
                    return None;
                }
                let radius = (rect.right - rect.left) as f32 / 2.0;
                let overlap = radius * 0.14;
                let center = (rect.right as f32 - overlap, rect.bottom as f32 - overlap);
                Some((Command::ToolOptions(tool), circle(center, 11.0 * scale)))
            })
    }

    fn target(&self, viewport: RECT, scale: f32, point: POINT) -> Option<Target> {
        if hit(self.hub(viewport, scale), point) {
            return Some(Target::Hub);
        }
        self.options(viewport, scale)
            .chain(
                self.presets(viewport, scale)
                    .map(|((tool, slot), rect)| (Command::Favorite(tool, slot), rect)),
            )
            .chain(self.items(viewport, scale))
            .filter(|_| self.open)
            .find_map(|(command, rect)| hit(rect, point).then_some(Target::Item(command)))
    }

    pub fn begin(&mut self, viewport: RECT, scale: f32, id: u32, point: POINT) -> bool {
        let Some(target) = self.target(viewport, scale, point) else {
            return false;
        };
        if self.grab.is_some() {
            return false;
        }
        self.velocity = (0.0, 0.0);
        self.frame = Some(Instant::now());
        self.grab = Some(Grab {
            id,
            target,
            start: point,
            start_center: self.center(viewport, scale),
            last: point,
            stamp: Instant::now(),
            velocity: (0.0, 0.0),
            drag: false,
        });
        true
    }

    pub fn grabbed(&self, id: u32) -> bool {
        self.grab.as_ref().is_some_and(|grab| grab.id == id)
    }

    pub fn move_to(&mut self, viewport: RECT, scale: f32, id: u32, point: POINT) {
        let Some(grab) = self.grab.as_mut().filter(|grab| grab.id == id) else {
            return;
        };
        if !matches!(grab.target, Target::Hub) {
            return;
        }
        let (dx, dy) = (
            (point.x - grab.start.x) as f32,
            (point.y - grab.start.y) as f32,
        );
        grab.drag |= dx.hypot(dy) >= 6.0 * scale;
        if !grab.drag {
            return;
        }
        if point != grab.last {
            let seconds = grab.stamp.elapsed().as_secs_f32().clamp(0.006, 0.12);
            let (vx, vy) = (
                (point.x - grab.last.x) as f32 / seconds,
                (point.y - grab.last.y) as f32 / seconds,
            );
            grab.velocity = (
                grab.velocity.0 * 0.4 + vx * 0.6,
                grab.velocity.1 * 0.4 + vy * 0.6,
            );
            grab.last = point;
            grab.stamp = Instant::now();
        }
        let position = (grab.start_center.0 + dx, grab.start_center.1 + dy);
        self.place(viewport, scale, position);
    }

    pub fn finish(&mut self, viewport: RECT, scale: f32, id: u32, point: POINT) -> Option<Command> {
        if !self.grabbed(id) {
            return None;
        }
        self.move_to(viewport, scale, id, point);
        let grab = self.grab.take().unwrap();
        if grab.drag {
            let gain = (-8.0 * grab.stamp.elapsed().as_secs_f32()).exp();
            let speed = grab.velocity.0.hypot(grab.velocity.1).max(1.0);
            let factor = gain * (2450.0 * scale / speed).min(1.0);
            self.velocity = (grab.velocity.0 * factor, grab.velocity.1 * factor);
            self.frame = Some(Instant::now());
            return None;
        }
        self.frame = Some(Instant::now());
        match (grab.target, self.target(viewport, scale, point)) {
            (Target::Hub, Some(Target::Hub)) => {
                self.open = !self.open;
                if self.open {
                    self.group = Group::Tools;
                }
                None
            }
            (Target::Item(a), Some(Target::Item(b))) if a == b => {
                match a {
                    Command::Shapes => self.group = Group::Shapes,
                    Command::Back => self.group = Group::Tools,
                    _ => return Some(a),
                }
                None
            }
            _ => None,
        }
    }

    pub fn cancel(&mut self, id: u32) {
        if self.grabbed(id) {
            self.grab = None;
        }
    }

    pub fn tick(&mut self, viewport: RECT, scale: f32) -> bool {
        let now = Instant::now();
        let dt = self
            .frame
            .replace(now)
            .map_or(0.016, |last| (now - last).as_secs_f32().min(0.055));
        let previous = self.progress;
        let direction = if self.open { 1.0 } else { -1.0 };
        self.progress = (self.progress + direction * dt / 0.18).clamp(0.0, 1.0);
        if self.velocity.0.hypot(self.velocity.1) < 9.0 * scale || self.grab.is_some() {
            return self.progress != previous;
        }
        let center = self.center(viewport, scale);
        let friction = (-3.6 * dt).exp();
        let motion = (1.0 - friction) / 3.6;
        let desired = (
            center.0 + self.velocity.0 * motion,
            center.1 + self.velocity.1 * motion,
        );
        let edge = limit(desired, viewport, self.margins(scale));
        let bounce = |position: f32, clipped: f32, speed: f32| {
            let over = position - clipped;
            if over == 0.0 {
                (position, speed * friction)
            } else {
                // Keep the part of this frame that occurred after contact with the edge.
                (clipped - 0.42 * over, -0.42 * speed * friction)
            }
        };
        let (x, vx) = bounce(desired.0, edge.0, self.velocity.0);
        let (y, vy) = bounce(desired.1, edge.1, self.velocity.1);
        self.place(viewport, scale, (x, y));
        self.velocity = (vx, vy);
        true
    }

    fn place(&mut self, viewport: RECT, scale: f32, position: (f32, f32)) {
        let place = limit(position, viewport, self.margins(scale));
        let (width, height) = (
            (viewport.right - viewport.left).max(1),
            (viewport.bottom - viewport.top).max(1),
        );
        self.anchor = Some((
            (place.0 - viewport.left as f32) / width as f32,
            (place.1 - viewport.top as f32) / height as f32,
        ));
    }
}

fn limit(point: (f32, f32), rect: RECT, (top, other): (f32, f32)) -> (f32, f32) {
    let left = rect.left as f32 + other;
    let ceiling = rect.top as f32 + top;
    (
        point.0.clamp(left, (rect.right as f32 - other).max(left)),
        point
            .1
            .clamp(ceiling, (rect.bottom as f32 - other).max(ceiling)),
    )
}

fn offset_at((x, y): (f32, f32), angle: f32, radius: f32) -> (f32, f32) {
    (x + angle.cos() * radius, y + angle.sin() * radius)
}

fn ring_angle(index: usize, count: usize) -> f32 {
    PEN_ANGLE + TAU * index as f32 / count as f32
}

fn circle(center: (f32, f32), radius: f32) -> RECT {
    RECT {
        left: (center.0 - radius).round() as i32,
        top: (center.1 - radius).round() as i32,
        right: (center.0 + radius).round() as i32,
        bottom: (center.1 + radius).round() as i32,
    }
}

fn hit(rect: RECT, point: POINT) -> bool {
    let cx = (rect.left + rect.right) as f32 / 2.0;
    let cy = (rect.top + rect.bottom) as f32 / 2.0;
    (point.x as f32 - cx).hypot(point.y as f32 - cy) <= (rect.right - rect.left) as f32 / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_fans_fit_inside_a_compact_window_and_have_distinct_targets() {
        let viewport = RECT {
            left: 9,
            top: 55,
            right: 450,
            bottom: 366,
        };
        let scale = Radial::fit_scale(viewport, 1.0);
        let radial = Radial {
            open: true,
            progress: 1.0,
            ..Radial::default()
        };
        let buttons: Vec<_> = radial
            .items(viewport, scale)
            .chain(
                radial
                    .presets(viewport, scale)
                    .map(|((tool, slot), rect)| (Command::Favorite(tool, slot), rect)),
            )
            .chain(radial.options(viewport, scale))
            .collect();
        assert_eq!(buttons.len(), 17);
        for (index, &(command, rect)) in buttons.iter().enumerate() {
            assert!(rect.left >= viewport.left && rect.right <= viewport.right);
            assert!(rect.top >= viewport.top && rect.bottom <= viewport.bottom);
            let point = POINT {
                x: (rect.left + rect.right) / 2,
                y: (rect.top + rect.bottom) / 2,
            };
            assert!(
                matches!(radial.target(viewport, scale, point), Some(Target::Item(target)) if target == command)
            );
            for &(other_cmd, other) in &buttons[index + 1..] {
                if matches!((command, other_cmd), (Command::Tool(tool), Command::ToolOptions(badge)) if tool == badge)
                {
                    continue;
                }
                let distance = (
                    (point.x * 2 - other.left - other.right) as f32 / 2.0,
                    (point.y * 2 - other.top - other.bottom) as f32 / 2.0,
                );
                let radii = (rect.right - rect.left + other.right - other.left) as f32 / 2.0;
                assert!(distance.0.hypot(distance.1) >= radii - 1.0);
            }
        }

        let mut radial = radial;
        let select = |radial: &mut Radial, command, rect: RECT| {
            let point = POINT {
                x: (rect.left + rect.right) / 2,
                y: (rect.top + rect.bottom) / 2,
            };
            assert!(radial.begin(viewport, scale, 7, point));
            assert_eq!(radial.finish(viewport, scale, 7, point), Some(command));
            assert!(radial.open);
        };
        let pen = buttons
            .iter()
            .copied()
            .find(|(command, _)| *command == Command::Tool(Tool::Pen))
            .unwrap();
        select(&mut radial, pen.0, pen.1);
        let options = buttons
            .iter()
            .copied()
            .find(|(command, _)| *command == Command::ToolOptions(Tool::Pen))
            .unwrap();
        select(&mut radial, options.0, options.1);
    }
}
