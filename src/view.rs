use windows::Win32::Foundation::{POINT, RECT};

#[derive(Clone, Copy, PartialEq)]
pub struct View {
    zoom: f32,
    offset: (f32, f32),
}

impl Default for View {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            offset: (0.0, 0.0),
        }
    }
}

impl View {
    pub fn zoom(self) -> f32 {
        self.zoom
    }

    pub fn bounds(self, base: RECT, viewport: RECT) -> RECT {
        let (w, h) = dimensions(base);
        let (width, height) = (w * self.zoom(), h * self.zoom());
        let (offset_x, offset_y) = self.bounded_offset(base, viewport);
        let (cx, cy) = center(viewport);
        let left = (cx - width / 2.0 + offset_x).round() as i32;
        let top = (cy - height / 2.0 + offset_y).round() as i32;
        RECT {
            left,
            top,
            right: left + width.round() as i32,
            bottom: top + height.round() as i32,
        }
    }

    pub fn transform(
        &mut self,
        base: RECT,
        viewport: RECT,
        from: (f32, f32),
        to: (f32, f32),
        factor: f32,
    ) {
        let old = self.bounded_offset(base, viewport);
        let zoom = (self.zoom() * factor).clamp(1.0, 4.0);
        let ratio = zoom / self.zoom();
        let middle = center(viewport);
        self.zoom = zoom;
        self.offset = (
            old.0 * ratio + (to.0 - from.0) + (middle.0 - from.0) * (ratio - 1.0),
            old.1 * ratio + (to.1 - from.1) + (middle.1 - from.1) * (ratio - 1.0),
        );
        self.offset = self.bounded_offset(base, viewport);
    }

    pub fn interpolate(self, target: Self, fraction: f32) -> Self {
        let mix = |a, b| a + (b - a) * fraction;
        Self {
            zoom: mix(self.zoom, target.zoom),
            offset: (
                mix(self.offset.0, target.offset.0),
                mix(self.offset.1, target.offset.1),
            ),
        }
    }

    fn bounded_offset(self, base: RECT, viewport: RECT) -> (f32, f32) {
        let (w, h) = dimensions(base);
        let (vw, vh) = dimensions(viewport);
        let limit = |length: f32, available: f32| ((length * self.zoom - available) / 2.0).max(0.0);
        let (x, y) = (limit(w, vw), limit(h, vh));
        (self.offset.0.clamp(-x, x), self.offset.1.clamp(-y, y))
    }
}

#[derive(Default)]
pub struct Touch {
    contacts: Vec<(u32, POINT)>,
    start: POINT,
    transformed: bool,
    sliding: bool,
}

pub enum Motion {
    Transform((f32, f32), (f32, f32), f32),
    Tap,
    Turn(i32),
    Slide(i32),
}

#[derive(Default)]
pub struct PageWheel {
    last: [Option<u32>; 2],
    deltas: [i32; 2],
    turn: Option<WheelTurn>,
}

struct WheelTurn {
    axis: usize,
    time: u32,
    direction: i32,
    tail: i32,
    previous: i32,
}

impl PageWheel {
    pub fn push(&mut self, now: u32, horizontal: bool, delta: i32) -> Option<i32> {
        const IDLE: u32 = 65;
        const DISTANCE: i32 = 65;
        if delta == 0 {
            return None;
        }
        let axis = usize::from(horizontal);
        let direction = delta.signum();
        let magnitude = delta.abs();
        let gap = self.last[axis]
            .replace(now)
            .map_or(u32::MAX, |last| now.wrapping_sub(last));
        if gap >= IDLE {
            self.deltas[axis] = 0;
        }
        if let Some(turn) = &self.turn
            && (axis == turn.axis && gap >= IDLE
                || axis != turn.axis
                    && self.last[turn.axis].is_some_and(|last| now.wrapping_sub(last) >= IDLE))
        {
            self.turn = None;
        }
        if let Some(turn) = &mut self.turn {
            if turn.axis != axis {
                self.deltas[axis] = 0;
                return None;
            }
            if turn.direction == direction {
                self.deltas[axis] = 0;
                // A renewed impulse can arrive before the previous impulse has gone quiet.
                // Conventional wheel detents instead arrive as separate 120-unit pulses.
                let renewed = now.wrapping_sub(turn.time) >= 36
                    && magnitude >= 24
                    && magnitude >= turn.tail * 2 + 8;
                let detent = gap >= 32 && magnitude % 120 == 0 && turn.previous % 120 == 0;
                if !renewed && !detent {
                    turn.tail = turn.tail.min(magnitude);
                    turn.previous = magnitude;
                    return None;
                }
                self.turn = None;
            }
        }
        self.deltas[axis] += delta;
        if self.deltas[axis].abs() < DISTANCE {
            return None;
        }
        self.turn = Some(WheelTurn {
            axis,
            time: now,
            direction,
            tail: magnitude,
            previous: magnitude,
        });
        self.deltas = [0; 2];
        Some(direction)
    }
}

impl Touch {
    pub fn down(&mut self, id: u32, point: POINT) {
        if self.contacts.is_empty() {
            self.start = point;
            self.transformed = false;
            self.sliding = false;
        } else {
            self.transformed = true;
        }
        self.contacts.push((id, point));
    }

    pub fn moved(&mut self, id: u32, point: POINT, zoomed: bool, scale: f32) -> Option<Motion> {
        let index = self
            .contacts
            .iter()
            .position(|(contact, _)| *contact == id)?;
        let previous = self.contacts[index].1;
        let pair = self.pair();
        if zoomed && !self.transformed && distance(self.start, point) < 6.0 * scale {
            return None;
        }
        self.contacts[index].1 = point;
        if let (Some((from, old)), Some((to, new))) = (pair, self.pair()) {
            return pinch(from, to, old, new);
        }
        if zoomed && self.contacts.len() == 1 {
            self.transformed = true;
            return Some(Motion::Transform(coords(previous), coords(point), 1.0));
        }
        if !self.transformed {
            let dx = point.x - self.start.x;
            let dy = point.y - self.start.y;
            if self.sliding || dx.abs() > (8.0 * scale) as i32 && dx.abs() > dy.abs() * 2 {
                self.sliding = true;
                return Some(Motion::Slide(dx));
            }
        }
        None
    }

    pub fn has_pair(&self) -> bool {
        self.contacts.len() >= 2
    }

    pub fn moved_pair(&mut self, positions: impl Iterator<Item = (u32, POINT)>) -> Option<Motion> {
        let (from, old) = self.pair()?;
        let [(first, a), (second, b), ..] = self.contacts.as_mut_slice() else {
            return None;
        };
        let (mut next_a, mut next_b) = (None, None);
        for (id, point) in positions {
            if id == *first {
                next_a = Some(point);
            } else if id == *second {
                next_b = Some(point);
            }
        }
        (*a, *b) = (next_a?, next_b?);
        let (to, new) = self.pair()?;
        pinch(from, to, old, new)
    }

    pub fn up(&mut self, id: u32, point: POINT, scale: f32) -> Option<Motion> {
        let index = self
            .contacts
            .iter()
            .position(|(contact, _)| *contact == id)?;
        self.contacts.remove(index);
        if self.transformed {
            return None;
        }
        let dx = point.x - self.start.x;
        let dy = point.y - self.start.y;
        if dx.abs() > (48.0 * scale) as i32 && (self.sliding || dx.abs() > dy.abs() * 2) {
            Some(Motion::Turn(if dx < 0 { 1 } else { -1 }))
        } else if !self.sliding && distance(self.start, point) < 12.0 * scale {
            Some(Motion::Tap)
        } else {
            None
        }
    }

    pub fn cancel(&mut self, id: u32) -> bool {
        let count = self.contacts.len();
        self.contacts.retain(|(contact, _)| *contact != id);
        self.contacts.len() != count
    }

    fn pair(&self) -> Option<((f32, f32), f32)> {
        let [(_, a), (_, b), ..] = self.contacts.as_slice() else {
            return None;
        };
        Some((
            ((a.x + b.x) as f32 / 2.0, (a.y + b.y) as f32 / 2.0),
            distance(*a, *b),
        ))
    }
}

fn pinch(from: (f32, f32), to: (f32, f32), old: f32, new: f32) -> Option<Motion> {
    (from != to || old != new).then(|| Motion::Transform(from, to, new.max(12.0) / old.max(12.0)))
}

fn dimensions(rect: RECT) -> (f32, f32) {
    (
        (rect.right - rect.left) as f32,
        (rect.bottom - rect.top) as f32,
    )
}

fn center(rect: RECT) -> (f32, f32) {
    (
        (rect.left + rect.right) as f32 / 2.0,
        (rect.top + rect.bottom) as f32 / 2.0,
    )
}

fn coords(point: POINT) -> (f32, f32) {
    (point.x as f32, point.y as f32)
}

fn distance(a: POINT, b: POINT) -> f32 {
    ((b.x - a.x) as f32).hypot((b.y - a.y) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_finger_slide_tracks_reversal_and_only_turns_past_threshold() {
        let mut touch = Touch::default();
        let point = |x| POINT { x, y: 100 };
        touch.down(1, point(300));
        assert!(touch.moved(1, point(296), false, 1.0).is_none());
        assert!(matches!(touch.moved(1, point(220), false, 1.0), Some(Motion::Slide(-80))));
        assert!(matches!(touch.moved(1, point(312), false, 1.0), Some(Motion::Slide(12))));
        assert!(touch.up(1, point(301), 1.0).is_none());
        touch.down(2, point(300));
        assert!(matches!(touch.moved(2, point(200), false, 1.0), Some(Motion::Slide(-100))));
        assert!(matches!(touch.up(2, point(200), 1.0), Some(Motion::Turn(1))));
    }

    #[test]
    fn trackpad_motion_accumulates_and_ignores_inertial_page_turns() {
        let mut wheel = PageWheel::default();
        assert_eq!(wheel.push(0, false, 18), None);
        assert_eq!(wheel.push(10, true, 40), None);
        assert_eq!(wheel.push(17, true, 30), Some(1));
        assert_eq!(wheel.push(36, true, 110), None);
        assert_eq!(wheel.push(45, false, 120), None);
        assert_eq!(wheel.push(51, true, 16), None);
        assert_eq!(wheel.push(61, true, 7), None);
        assert_eq!(wheel.push(69, true, -3), None);
        assert_eq!(wheel.push(77, true, -64), Some(-1));
        assert_eq!(wheel.push(260, false, 120), Some(1));
    }

    #[test]
    fn fast_wheel_bursts_and_consecutive_detents_each_turn_a_page() {
        let mut wheel = PageWheel::default();
        assert_eq!(wheel.push(0, true, 67), Some(1));
        assert_eq!(wheel.push(16, true, 50), None);
        assert_eq!(wheel.push(31, true, 10), None);
        assert_eq!(wheel.push(49, true, 45), None);
        assert_eq!(wheel.push(57, true, 27), Some(1));
        assert_eq!(wheel.push(73, true, 8), None);
        assert_eq!(wheel.push(94, true, 80), Some(1));

        let mut wheel = PageWheel::default();
        assert_eq!(wheel.push(u32::MAX - 12, false, -120), Some(-1));
        assert_eq!(wheel.push(31, false, -120), Some(-1));
        assert_eq!(wheel.push(75, false, -120), Some(-1));
    }

    #[test]
    fn pinch_keeps_the_anchor_under_the_fingers_and_clamps_pan() {
        let page = RECT {
            left: 0,
            top: 0,
            right: 100,
            bottom: 100,
        };
        let mut view = View::default();
        view.transform(page, page, (25.0, 25.0), (25.0, 25.0), 2.0);
        assert_eq!(view.bounds(page, page).left, -25);
        view.transform(page, page, (25.0, 25.0), (999.0, 999.0), 1.0);
        assert_eq!(view.bounds(page, page).left, 0);
        view.transform(page, page, (25.0, 25.0), (25.0, 25.0), 0.1);
        assert_eq!(view.bounds(page, page), page);
    }

    #[test]
    fn fractional_pan_keeps_the_raster_size_at_a_fixed_zoom() {
        let page = RECT { left: 0, top: 0, right: 503, bottom: 629 };
        let center = (250.0, 300.0);
        let mut view = View::default();
        view.transform(page, page, center, center, 1.35);
        let initial = dimensions(view.bounds(page, page));
        for step in 1..40 {
            let distance = step as f32 / 10.0;
            view.transform(page, page, center, (center.0 + distance, center.1 - distance), 1.0);
            assert_eq!(dimensions(view.bounds(page, page)), initial);
        }
    }

    #[test]
    fn pinch_uses_matching_contact_samples_and_keeps_fractional_motion() {
        let mut touch = Touch::default();
        let left = POINT { x: 10, y: 10 };
        let right = POINT { x: 50, y: 10 };
        touch.down(1, left);
        touch.down(2, right);
        let frame = [(2, POINT { x: 63, y: 12 }), (1, POINT { x: 7, y: 12 })];
        let Some(Motion::Transform(from, to, factor)) = touch.moved_pair(frame.into_iter()) else {
            panic!("Expected one transform for the pointer frame");
        };
        assert_eq!(from, (30.0, 10.0));
        assert_eq!(to, (35.0, 12.0));
        assert_eq!(factor, 1.4);
        assert!(touch.moved_pair(frame.into_iter()).is_none());

        let page = RECT {
            right: 100,
            bottom: 100,
            ..Default::default()
        };
        let mut view = View::default();
        view.transform(page, page, (50.0, 50.0), (50.0, 50.0), 2.0);
        for _ in 0..8 {
            view.transform(page, page, (0.0, 0.0), (0.25, 0.25), 1.0);
        }
        let bounds = view.bounds(page, page);
        assert_eq!((bounds.left, bounds.top), (-48, -48));
    }
}
