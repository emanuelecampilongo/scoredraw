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
const OPEN_SPAN: f32 = 164.0;
const RING_RADIUS: f32 = 116.0;
const RESTITUTION: f32 = 0.68;

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
    last: POINT,
    stamp: Instant,
    velocity: (f32, f32),
    drag: bool,
    pending: Vector,
    contact: Option<Vector>,
}

#[derive(Default)]
pub struct Radial {
    anchor: Option<(f32, f32)>,
    grab: Option<Grab>,
    velocity: (f32, f32),
    frame: Option<Instant>,
    progress: f32,
    display_scale: Option<f32>,
    pub open: bool,
    group: Group,
    next_group: Option<Group>,
    group_progress: f32,
}

impl Radial {
    pub fn fit_scale(viewport: RECT, scale: f32) -> f32 {
        scale
            .min((viewport.bottom - viewport.top - 2).max(1) as f32 / (2.0 * OPEN_SPAN))
            .min((viewport.right - viewport.left - 2).max(1) as f32 / (2.0 * OPEN_SPAN))
    }

    pub fn display_scale(&self, viewport: RECT, scale: f32) -> f32 {
        let base = Self::fit_scale(viewport, scale);
        self.display_scale.unwrap_or(base).min(base)
    }

    fn margin(&self, scale: f32) -> f32 {
        (30.0 + (OPEN_SPAN - 30.0) * self.expansion()) * scale
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
        limit(position, viewport, 30.0 * scale)
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
        let level = if self.next_group.is_some() || self.group_progress > 0.0 {
            1.0 - self.group_progress
        } else { 1.0 };
        let progress = self.expansion() * level;
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

    pub fn options(&self, viewport: RECT, scale: f32) -> impl Iterator<Item = (Command, RECT)> {
        self.items(viewport, scale)
            .filter_map(move |(command, rect)| {
                let Command::Tool(tool) = command else {
                    return None;
                };
                if !tool.has_options() || self.expansion() * (1.0 - self.group_progress) < 0.8 {
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
            .chain(self.items(viewport, scale))
            .filter(|_| self.open && self.next_group.is_none() && self.group_progress == 0.0)
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
            last: point,
            stamp: Instant::now(),
            velocity: (0.0, 0.0),
            drag: false,
            pending: (0.0, 0.0),
            contact: None,
        });
        true
    }

    pub fn grabbed(&self, id: u32) -> bool {
        self.grab.as_ref().is_some_and(|grab| grab.id == id)
    }

    pub fn move_to(&mut self, scale: f32, id: u32, point: POINT) {
        let Some(grab) = self.grab.as_mut().filter(|grab| grab.id == id) else { return };
        if !matches!(grab.target, Target::Hub) { return; }
        let from = if grab.drag { grab.last } else { grab.start };
        let movement = ((point.x - from.x) as f32, (point.y - from.y) as f32);
        grab.drag |= length(movement) >= 6.0 * scale;
        if !grab.drag || point == grab.last { return; }
        let seconds = grab.stamp.elapsed().as_secs_f32().clamp(0.006, 0.12);
        grab.velocity = add(mul(grab.velocity, 0.45), mul(movement, 0.55 / seconds));
        grab.pending = add(grab.pending, movement);
        grab.last = point;
        grab.stamp = Instant::now();
    }

    pub fn finish(&mut self, viewport: RECT, scale: f32, id: u32, point: POINT) -> Option<Command> {
        if !self.grabbed(id) { return None; }
        self.move_to(scale, id, point);
        let grab = self.grab.take().unwrap();
        if grab.drag {
            let gain = (-8.0 * grab.stamp.elapsed().as_secs_f32()).exp();
            let speed = grab.velocity.0.hypot(grab.velocity.1).max(1.0);
            let factor = gain * (2450.0 * scale / speed).min(1.0);
            let velocity = mul(grab.velocity, factor);
            self.velocity = grab.contact.map_or(velocity, |normal| reflect(velocity, normal));
            self.frame = Some(Instant::now());
            return None;
        }
        self.frame = Some(Instant::now());
        match (grab.target, self.target(viewport, scale, point)) {
            (Target::Hub, Some(Target::Hub)) => {
                self.open = !self.open;
                if self.open {
                    self.group = Group::Tools;
                    self.next_group = None;
                    self.group_progress = 0.0;
                }
                None
            }
            (Target::Item(a), Some(Target::Item(b))) if a == b => {
                match a {
                    Command::Shapes => self.next_group = Some(Group::Shapes),
                    Command::Back => self.next_group = Some(Group::Tools),
                    _ => return Some(a),
                }
                None
            }
            _ => None,
        }
    }

    pub fn cancel(&mut self, id: u32) -> bool {
        self.grabbed(id) && self.grab.take().is_some_and(|grab| grab.drag)
    }

    pub fn reset_frame_time(&mut self) {
        self.frame = None;
    }

    pub fn tick(&mut self, viewport: RECT, input_scale: f32, obstacles: &[RECT]) -> bool {
        let now = Instant::now();
        let dt = self.frame.replace(now)
            .map_or(0.016, |last| now.duration_since(last).as_secs_f32().min(0.05));
        let base = Self::fit_scale(viewport, input_scale);
        let previous_scale = self.display_scale(viewport, input_scale);
        let center = self.center(viewport, previous_scale);
        let mut target_scale = base;
        if self.open {
            let fit = [1.0, 0.9, 0.8, 0.7, 0.6, 0.5, 0.4].into_iter()
                .map(|fraction| base * fraction)
                .find(|&scale| clear_position(center, viewport, OPEN_SPAN * scale + 1.0, obstacles).is_some());
            if let Some(fit) = fit {
                target_scale = fit;
            } else {
                self.open = false;
            }
        } else if self.progress > 0.0 {
            target_scale = previous_scale;
        }
        let scale_gap = target_scale - previous_scale;
        let scale = if scale_gap.abs() < 0.002 {
            target_scale
        } else {
            previous_scale + scale_gap * (1.0 - (-18.0 * dt).exp())
        };
        self.display_scale = Some(scale);
        let center = self.center(viewport, scale);
        let previous = self.progress;
        let old_group_progress = self.group_progress;
        if self.next_group.is_some() {
            self.group_progress = (self.group_progress + dt / 0.075).min(1.0);
            if self.group_progress >= 1.0 {
                self.group = self.next_group.take().unwrap();
            }
        } else {
            self.group_progress = (self.group_progress - dt / 0.115).max(0.0);
        }
        let direction = if self.open { 1.0 } else { -1.0 };
        self.progress = (self.progress + direction * dt / 0.18).clamp(0.0, 1.0);
        let radius = self.margin(scale) + 1.0;
        let changing = self.progress != previous || scale != previous_scale || self.group_progress != old_group_progress;

        // A panel can grow into the menu. Resolve the overlap over several frames.
        if !is_clear(center, viewport, radius, obstacles) {
            let target = clear_position(center, viewport, radius, obstacles)
                .or_else(|| clear_position(center, viewport, 31.0 * scale, obstacles));
            if let Some(target) = target {
                let gap = sub(target, center);
                let distance = length(gap);
                let fraction = if distance <= 1.0 {
                    1.0
                } else {
                    (1.0 - (-18.0 * dt).exp()).min(900.0 * scale * dt / distance)
                };
                if let Some(grab) = &mut self.grab {
                    grab.pending = (0.0, 0.0);
                }
                self.velocity = (0.0, 0.0);
                self.place(viewport, scale, add(center, mul(gap, fraction)));
                return true;
            }
            return changing;
        }

        if let Some(grab) = &mut self.grab {
            if !grab.drag || length(grab.pending) < 0.08 { return changing; }
            let motion = mul(grab.pending, 1.0 - (-85.0 * dt).exp());
            grab.pending = sub(grab.pending, motion);
            let next = if let Some(hit) = collision(center, motion, viewport, radius, obstacles) {
                grab.contact = Some(hit.normal);
                grab.pending = without_inward(grab.pending, hit.normal);
                let contact = add(center, mul(motion, hit.time));
                let rest = without_inward(mul(motion, 1.0 - hit.time), hit.normal);
                let fraction = collision(contact, rest, viewport, radius, obstacles).map_or(1.0, |hit| hit.time);
                add(contact, mul(rest, fraction))
            } else {
                grab.contact = None;
                add(center, motion)
            };
            self.place(viewport, scale, next);
            return true;
        }

        if length(self.velocity) < 7.0 * scale { return changing; }
        let mut center = center;
        let damping = (-3.9 * dt).exp();
        let mut time = (1.0 - damping) / 3.9;
        for _ in 0..4 {
            let motion = mul(self.velocity, time);
            if let Some(hit) = collision(center, motion, viewport, radius, obstacles) {
                center = add(center, mul(motion, hit.time));
                self.velocity = reflect(self.velocity, hit.normal);
                time *= 1.0 - hit.time;
            } else {
                center = add(center, motion);
                break;
            }
        }
        self.velocity = mul(self.velocity, damping);
        self.place(viewport, scale, center);
        true
    }

    fn place(&mut self, viewport: RECT, scale: f32, position: (f32, f32)) {
        let place = limit(position, viewport, 30.0 * scale);
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

type Vector = (f32, f32);

fn add(a: Vector, b: Vector) -> Vector { (a.0 + b.0, a.1 + b.1) }
fn sub(a: Vector, b: Vector) -> Vector { (a.0 - b.0, a.1 - b.1) }
fn mul(v: Vector, factor: f32) -> Vector { (v.0 * factor, v.1 * factor) }
fn dot(a: Vector, b: Vector) -> f32 { a.0 * b.0 + a.1 * b.1 }
fn length(v: Vector) -> f32 { v.0.hypot(v.1) }

fn without_inward(v: Vector, normal: Vector) -> Vector {
    sub(v, mul(normal, dot(v, normal).min(0.0)))
}

fn reflect(v: Vector, normal: Vector) -> Vector {
    sub(v, mul(normal, (1.0 + RESTITUTION) * dot(v, normal).min(0.0)))
}

fn nearest(rect: RECT, point: Vector) -> Vector {
    (point.0.clamp(rect.left as f32, rect.right as f32), point.1.clamp(rect.top as f32, rect.bottom as f32))
}

fn is_clear(point: Vector, viewport: RECT, radius: f32, obstacles: &[RECT]) -> bool {
    let gap = sub(point, limit(point, viewport, radius));
    length(gap) <= 0.025
        && obstacles.iter().all(|&rect| length(sub(point, nearest(rect, point))) >= radius - 0.025)
}

fn clear_position(point: Vector, viewport: RECT, radius: f32, obstacles: &[RECT]) -> Option<Vector> {
    if ((viewport.right - viewport.left) as f32) < 2.0 * radius
        || ((viewport.bottom - viewport.top) as f32) < 2.0 * radius
    {
        return None;
    }
    let point = limit(point, viewport, radius);
    if is_clear(point, viewport, radius, obstacles) { return Some(point); }
    let min = limit((f32::NEG_INFINITY, f32::NEG_INFINITY), viewport, radius);
    let max = limit((f32::INFINITY, f32::INFINITY), viewport, radius);
    let mut xs = vec![point.0, min.0, max.0];
    let mut ys = vec![point.1, min.1, max.1];
    let mut candidates = Vec::new();
    for &rect in obstacles {
        xs.extend([rect.left as f32 - radius, rect.right as f32 + radius]);
        ys.extend([rect.top as f32 - radius, rect.bottom as f32 + radius]);
        let contact = nearest(rect, point);
        let normal = sub(point, contact);
        let distance = length(normal);
        if distance > 0.0 && distance < radius {
            candidates.push(add(contact, mul(normal, radius / distance)));
        }
    }
    candidates.into_iter()
        .chain(xs.iter().flat_map(|&x| ys.iter().map(move |&y| (x, y))))
        .filter(|&candidate| is_clear(candidate, viewport, radius, obstacles))
        .min_by(|&a, &b| length(sub(a, point)).total_cmp(&length(sub(b, point))))
}

struct Impact {
    time: f32,
    normal: Vector,
}

fn collision(
    from: Vector, delta: Vector, viewport: RECT, radius: f32, obstacles: &[RECT],
) -> Option<Impact> {
    let mut first: Option<Impact> = None;
    let mut consider = |time: f32, normal: Vector| {
        if (-0.0001..=1.0).contains(&time)
            && dot(delta, normal) < -0.0001
            && first.as_ref().is_none_or(|best| time < best.time)
        {
            first = Some(Impact { time: time.max(0.0), normal });
        }
    };
    let walls = [
        (0, viewport.left as f32 + radius, (1.0, 0.0)),
        (0, viewport.right as f32 - radius, (-1.0, 0.0)),
        (1, viewport.top as f32 + radius, (0.0, 1.0)),
        (1, viewport.bottom as f32 - radius, (0.0, -1.0)),
    ];
    let coord = |(x, y): Vector, axis| if axis == 0 { x } else { y };
    for (axis, edge, normal) in walls {
        let distance = coord(delta, axis);
        if distance != 0.0 {
            consider((edge - coord(from, axis)) / distance, normal);
        }
    }
    let speed = dot(delta, delta);
    if speed < 0.00001 { return first; }
    for rect in obstacles {
        let (left, right, top, bottom) = (rect.left as f32, rect.right as f32, rect.top as f32, rect.bottom as f32);
        let faces = [
            (0, left - radius, top, bottom, (-1.0, 0.0)),
            (0, right + radius, top, bottom, (1.0, 0.0)),
            (1, top - radius, left, right, (0.0, -1.0)),
            (1, bottom + radius, left, right, (0.0, 1.0)),
        ];
        for (axis, edge, min, max, normal) in faces {
            let distance = coord(delta, axis);
            if distance == 0.0 { continue; }
            let time = (edge - coord(from, axis)) / distance;
            let parallel = coord(add(from, mul(delta, time)), 1 - axis);
            if (min..=max).contains(&parallel) { consider(time, normal); }
        }
        for corner in [(left, top), (left, bottom), (right, top), (right, bottom)] {
            let gap = sub(from, corner);
            let toward = dot(gap, delta);
            let discriminant = toward * toward - speed * (dot(gap, gap) - radius * radius);
            if discriminant < 0.0 || toward >= 0.0 { continue; }
            let time = (-toward - discriminant.sqrt()) / speed;
            let normal = mul(add(gap, mul(delta, time)), 1.0 / radius);
            consider(time, normal);
        }
    }
    first
}

fn limit(point: (f32, f32), rect: RECT, margin: f32) -> (f32, f32) {
    let left = rect.left as f32 + margin;
    let ceiling = rect.top as f32 + margin;
    (
        point.0.clamp(left, (rect.right as f32 - margin).max(left)),
        point
            .1
            .clamp(ceiling, (rect.bottom as f32 - margin).max(ceiling)),
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
    fn a_fast_radial_flick_rebounds_from_a_panel() {
        let viewport = RECT { right: 900, bottom: 600, ..RECT::default() };
        let panel = RECT { left: 340, top: 100, right: 440, bottom: 350 };
        let mut radial = Radial {
            anchor: Some((300.0 / 900.0, 0.4)),
            velocity: (2300.0, 0.0),
            frame: Some(Instant::now() - std::time::Duration::from_millis(48)),
            ..Radial::default()
        };
        assert!(radial.tick(viewport, 1.0, &[panel]));
        assert!(radial.velocity.0 < 0.0);
        assert!(radial.footprint(viewport, 1.0).right <= panel.left);
    }

    #[test]
    fn an_expanding_panel_pushes_a_resting_radial_menu_out() {
        let viewport = RECT { right: 900, bottom: 600, ..RECT::default() };
        let panel = RECT { left: 340, top: 100, right: 540, bottom: 350 };
        let mut radial = Radial { anchor: Some((0.4, 0.4)), ..Radial::default() };
        assert!(radial.tick(viewport, 1.0, &[panel]));
        let after = radial.center(viewport, 1.0);
        assert!(after.0 > 310.0 && after.0 < 360.0);
        for _ in 0..20 {
            radial.frame = Some(Instant::now() - std::time::Duration::from_millis(17));
            radial.tick(viewport, 1.0, &[panel]);
        }
        assert!(radial.footprint(viewport, 1.0).right <= panel.left);
    }

    #[test]
    fn circular_contact_clears_the_square_corner_and_reflects_diagonally() {
        let viewport = RECT { right: 900, bottom: 600, ..RECT::default() };
        let panel = RECT { left: 340, top: 100, right: 540, bottom: 350 };
        assert!(collision((250.0, 71.0), (70.0, 0.0), viewport, 31.0, &[panel]).is_none());
        let impact = collision((250.0, 71.0), (105.0, 0.0), viewport, 31.0, &[panel]).unwrap();
        assert!(impact.normal.0 < 0.0 && impact.normal.1 < 0.0);
        assert!(reflect((105.0, 0.0), impact.normal).1 < 0.0);
    }

    #[test]
    fn both_fans_fit_inside_a_compact_window_and_have_distinct_targets() {
        let viewport = RECT {
            left: 9,
            top: 55,
            right: 450,
            bottom: 366,
        };
        let scale = Radial::fit_scale(viewport, 1.0);
        let mut radial = Radial { open: true, ..Radial::default() };
        for _ in 0..45 {
            radial.frame = Some(Instant::now() - std::time::Duration::from_millis(16));
            radial.tick(viewport, 1.0, &[]);
        }
        let buttons: Vec<_> = radial
            .items(viewport, scale)
            .chain(radial.options(viewport, scale))
            .collect();
        assert_eq!(buttons.len(), 11);
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
