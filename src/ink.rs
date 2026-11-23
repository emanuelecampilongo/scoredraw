use crate::canvas::{self, Canvas, Path};
use crate::data::{self, PresetTool, Settings, Style};
use crate::icons::{self, Glyph};
use serde::{Deserialize, Serialize};
use std::{io, path::PathBuf, sync::Arc};
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::GdiPlus::PointF;
use windows::Win32::UI::Input::Pointer::{
    GetPointerPenInfo, GetPointerPenInfoHistory, POINTER_PEN_INFO,
};
use windows::Win32::UI::WindowsAndMessaging::{
    PEN_FLAG_BARREL, PEN_FLAG_ERASER, PEN_FLAG_INVERTED, PEN_MASK_PRESSURE, WM_POINTERDOWN,
};

mod saves;
pub use saves::{SAVE_RESULT, SaveStatus, Saver};

pub const MOUSE: u32 = u32::MAX;

#[derive(Clone, Copy)]
pub struct InkArea {
    pub page: RECT,
    pub viewport: RECT,
    pub dpi: f32,
}

impl From<RECT> for InkArea {
    fn from(page: RECT) -> Self {
        Self {
            page,
            viewport: page,
            dpi: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tool {
    Hand,
    #[default]
    Pen,
    Highlighter,
    Eraser,
    Staff,
    Rectangle,
    Select,
}

impl Tool {
    pub fn preset_group(self) -> Option<PresetTool> {
        match self {
            Self::Pen => Some(PresetTool::Pen),
            Self::Highlighter => Some(PresetTool::Highlighter),
            _ => None,
        }
    }

    pub fn has_options(self) -> bool {
        matches!(
            self,
            Self::Pen | Self::Highlighter | Self::Staff | Self::Rectangle
        )
    }

    pub fn style(self, settings: &Settings) -> Style {
        match self {
            Self::Highlighter => settings.highlighter,
            Self::Staff => settings.staff,
            Self::Rectangle => settings.rectangle,
            _ => settings.pen,
        }
    }
}

impl From<PresetTool> for Tool {
    fn from(tool: PresetTool) -> Self {
        match tool {
            PresetTool::Pen => Self::Pen,
            PresetTool::Highlighter => Self::Highlighter,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
pub(crate) struct Point {
    x: f32,
    y: f32,
    pressure: f32,
}

impl Point {
    fn from_client(position: POINT, area: RECT, pressure: f32) -> Self {
        Self {
            x: ((position.x - area.left) as f32 / (area.right - area.left) as f32).clamp(0.0, 1.0),
            y: ((position.y - area.top) as f32 / (area.bottom - area.top) as f32).clamp(0.0, 1.0),
            pressure,
        }
    }

    fn client(self, area: RECT) -> POINT {
        pixel(
            area.left as f32 + self.x * (area.right - area.left) as f32,
            area.top as f32 + self.y * (area.bottom - area.top) as f32,
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
pub(crate) struct Bounds {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Bounds {
    fn between(start: Point, end: Point) -> Self {
        let x = start.x.min(end.x);
        let y = start.y.min(end.y);
        Self {
            x,
            y,
            w: (start.x - end.x).abs(),
            h: (start.y - end.y).abs(),
        }
    }

    fn pixels(self, area: RECT) -> RECT {
        let a = Point {
            x: self.x,
            y: self.y,
            pressure: 0.0,
        }
        .client(area);
        let b = Point {
            x: self.x + self.w,
            y: self.y + self.h,
            pressure: 0.0,
        }
        .client(area);
        RECT {
            left: a.x,
            top: a.y,
            right: b.x.max(a.x + 1),
            bottom: b.y.max(a.y + 1),
        }
    }

    fn contains(self, point: Point, margin: f32) -> bool {
        point.x >= self.x - margin
            && point.x <= self.x + self.w + margin
            && point.y >= self.y - margin
            && point.y <= self.y + self.h + margin
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Mark {
    Stroke {
        style: Style,
        variable: bool,
        points: Arc<Vec<Point>>,
    },
    Shape {
        style: Style,
        staff: bool,
        bounds: Bounds,
    },
}

impl Mark {
    fn hit(&self, point: Point) -> bool {
        match self {
            Self::Shape { .. } => false,
            Self::Stroke { points, style, .. } => {
                let radius = (style.width * 0.75 + 0.012).clamp(0.01, 0.065);
                points
                    .first()
                    .is_some_and(|p| distance(point, *p, *p) <= radius)
                    || points
                        .windows(2)
                        .any(|segment| distance(point, segment[0], segment[1]) <= radius)
            }
        }
    }
}

struct Gesture {
    contact: u32,
    before: Vec<Mark>,
    action: Action,
    changed: bool,
    last_sample: u64,
}

enum Action {
    Stroke,
    Erase,
    Remove,
    Shape {
        start: Point,
        default: Bounds,
    },
    Move {
        start: Point,
        original: Bounds,
        index: usize,
        handle: (i32, i32),
    },
}

#[derive(Default)]
pub struct Ink {
    path: Option<PathBuf>,
    marks: Vec<Mark>,
    undo: Vec<Vec<Mark>>,
    redo: Vec<Vec<Mark>>,
    gesture: Option<Gesture>,
    selected: Option<usize>,
    dirty: bool,
    revision: data::Revision,
    layer_generation: u64,
    damage: Damage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LayerVersion {
    generation: u64,
    pub count: usize,
}

impl LayerVersion {
    pub fn append_from(self, previous: Option<Self>) -> Option<usize> {
        let previous = previous?;
        (self.generation == previous.generation && self.count >= previous.count)
            .then_some(previous.count)
    }
}

#[derive(Default)]
enum Damage {
    #[default]
    None,
    Stroke {
        bounds: Bounds,
        width: f32,
    },
    Full,
}

impl Damage {
    fn points(&mut self, points: &[Point], width: f32) {
        if matches!(self, Self::Full) {
            return;
        }
        // A new sample can change the smoothed curve around the previous endpoint.
        for point in points.iter().rev().take(3) {
            *self = match *self {
                Self::Stroke { bounds, width } => {
                    let x = bounds.x.min(point.x);
                    let y = bounds.y.min(point.y);
                    Self::Stroke {
                        width,
                        bounds: Bounds {
                            x,
                            y,
                            w: (bounds.x + bounds.w).max(point.x) - x,
                            h: (bounds.y + bounds.h).max(point.y) - y,
                        },
                    }
                }
                _ => Self::Stroke {
                    bounds: Bounds {
                        x: point.x,
                        y: point.y,
                        w: 0.0,
                        h: 0.0,
                    },
                    width,
                },
            };
        }
    }
}

impl Ink {
    pub fn bind(&mut self, path: Option<PathBuf>, saver: &Saver) -> io::Result<bool> {
        if self.path == path {
            return Ok(false);
        }
        self.save(saver)?;
        *self = Self::default();
        if let Some(path) = path {
            let (marks, revision, recovered) = saver.load(&path)?;
            self.marks = marks;
            self.revision = revision;
            self.path = Some(path);
            self.dirty = recovered;
            return Ok(recovered);
        }
        Ok(false)
    }

    pub fn save(&mut self, saver: &Saver) -> io::Result<()> {
        if self.dirty {
            if let Some(path) = &self.path {
                let committed = self
                    .gesture
                    .as_ref()
                    .map_or(&self.marks, |gesture| &gesture.before);
                saver.submit(path.clone(), self.revision, committed.clone())?;
            }
            self.dirty = false;
        }
        Ok(())
    }

    pub fn is_drawing(&self) -> bool {
        self.gesture.is_some()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn has_unsaved(&self) -> bool {
        self.dirty
    }

    pub fn layer_version(&self) -> LayerVersion {
        LayerVersion {
            generation: self.layer_generation,
            count: self.marks.len() - usize::from(self.active_stroke().is_some()),
        }
    }

    pub fn take_damage(&mut self, page: RECT, viewport: RECT) -> Option<RECT> {
        match std::mem::take(&mut self.damage) {
            Damage::None => None,
            Damage::Full => Some(viewport),
            Damage::Stroke { bounds, width } => {
                let zone = bounds.pixels(page);
                let short = (page.right - page.left).min(page.bottom - page.top) as f32;
                let pad = (width * short).ceil() as i32 + 5;
                Some(RECT {
                    left: (zone.left - pad).max(viewport.left),
                    top: (zone.top - pad).max(viewport.top),
                    right: (zone.right + pad).min(viewport.right),
                    bottom: (zone.bottom + pad).min(viewport.bottom),
                })
            }
        }
    }

    fn change_layer(&mut self) {
        self.layer_generation = self.layer_generation.wrapping_add(1);
    }

    pub fn has_contact(&self, contact: u32) -> bool {
        self.gesture
            .as_ref()
            .is_some_and(|gesture| gesture.contact == contact)
    }

    pub fn select_shape(&mut self, area: RECT, position: POINT) -> bool {
        let point = Point::from_client(position, area, 0.5);
        self.selected = contains(area, position)
            .then(|| self.shape_at(point))
            .flatten()
            .map(|(i, _)| i);
        self.selected.is_some()
    }

    fn shape_at(&self, point: Point) -> Option<(usize, Bounds)> {
        self.marks
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, mark)| match mark {
                Mark::Shape { bounds, .. } if bounds.contains(point, 0.006) => {
                    Some((index, *bounds))
                }
                _ => None,
            })
    }

    pub fn selected_hit(&self, geometry: InkArea, position: POINT) -> bool {
        let Some(Mark::Shape { bounds, .. }) = self.selected.and_then(|i| self.marks.get(i)) else {
            return false;
        };
        let rect = bounds.pixels(geometry.page);
        contains(geometry.selection_rect(rect), position)
            || geometry.resize_hit(rect, position).is_some()
            || geometry
                .trash(rect)
                .is_some_and(|button| contains(button, position))
    }

    pub fn has_selected_shape(&self) -> bool {
        self.selected
            .and_then(|index| self.marks.get(index))
            .is_some_and(|mark| matches!(mark, Mark::Shape { .. }))
    }

    pub fn delete_selected_shape(&mut self) -> bool {
        self.cancel_contact();
        if !self.has_selected_shape() {
            return false;
        }
        let before = self.marks.clone();
        self.remove_selected();
        self.commit_edit(before);
        true
    }

    fn remove_selected(&mut self) {
        if let Some(index) = self.selected.take() {
            self.marks.remove(index);
            self.change_layer();
            self.damage = Damage::Full;
        }
    }

    fn commit_edit(&mut self, before: Vec<Mark>) {
        if self.undo.len() == 20 {
            self.undo.remove(0);
        }
        self.undo.push(before);
        self.redo.clear();
        self.dirty = true;
    }

    pub fn undo(&mut self) {
        self.cancel_contact();
        if let Some(before) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.marks, before));
            self.selected = None;
            self.dirty = true;
            self.change_layer();
        }
    }

    pub fn redo(&mut self) {
        self.cancel_contact();
        if let Some(after) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.marks, after));
            self.selected = None;
            self.dirty = true;
            self.change_layer();
        }
    }

    pub fn cancel_contact(&mut self) {
        if let Some(gesture) = self.gesture.take()
            && gesture.changed
        {
            self.commit_edit(gesture.before);
        }
    }

    pub fn complete_contact(&mut self) -> bool {
        let placed_staff = self.gesture.as_ref().is_some_and(|gesture| {
            matches!(gesture.action, Action::Shape { .. })
                && matches!(self.marks.last(), Some(Mark::Shape { staff: true, .. }))
        });
        self.cancel_contact();
        if placed_staff {
            self.selected = self.marks.len().checked_sub(1);
            self.damage = Damage::Full;
        }
        placed_staff
    }

    pub fn pen(
        &mut self,
        hwnd: HWND,
        geometry: InkArea,
        msg: u32,
        id: u32,
        tool: Tool,
        settings: &Settings,
    ) -> bool {
        let area = geometry.page;
        if msg != WM_POINTERDOWN && !self.has_contact(id) {
            return false;
        }
        let mut current = POINTER_PEN_INFO::default();
        if unsafe { GetPointerPenInfo(id, &mut current) }.is_err() {
            return false;
        }
        let (position, pressure) = pen_sample(hwnd, &current);
        if msg == WM_POINTERDOWN {
            let tool = if current.penFlags & (PEN_FLAG_ERASER | PEN_FLAG_INVERTED | PEN_FLAG_BARREL)
                != 0
            {
                Tool::Eraser
            } else {
                tool
            };
            let handled = self.begin(id, geometry, position, pressure, tool, settings);
            if handled && let Some(gesture) = &mut self.gesture {
                gesture.last_sample = current.pointerInfo.PerformanceCount;
            }
            return handled;
        }
        if current.pointerInfo.historyCount > 1 {
            let mut history = [POINTER_PEN_INFO::default(); 64];
            let mut count = history.len() as u32;
            if unsafe { GetPointerPenInfoHistory(id, &mut count, Some(history.as_mut_ptr())) }
                .is_ok()
            {
                for entry in history.iter().take(count as usize).rev() {
                    self.apply_pen_sample(hwnd, area, entry);
                }
            } else {
                self.apply_pen_sample(hwnd, area, &current);
            }
        } else {
            self.apply_pen_sample(hwnd, area, &current);
        }
        true
    }

    fn apply_pen_sample(&mut self, hwnd: HWND, area: RECT, entry: &POINTER_PEN_INFO) {
        let Some(gesture) = &mut self.gesture else {
            return;
        };
        let stamp = entry.pointerInfo.PerformanceCount;
        if stamp > 0 {
            if stamp <= gesture.last_sample {
                return;
            }
            gesture.last_sample = stamp;
        }
        let (position, mut pressure) = pen_sample(hwnd, entry);
        if entry.pressure == 0
            && let Some(Mark::Stroke { points, .. }) = self.marks.last()
        {
            let Some(last) = points.last() else { return };
            pressure = last.pressure;
        }
        self.update(Point::from_client(position, area, pressure));
    }

    pub fn mouse_update(&mut self, area: RECT, position: POINT) {
        self.contact_update(MOUSE, area, position);
    }

    pub fn contact_update(&mut self, id: u32, area: RECT, position: POINT) {
        if self.has_contact(id) {
            self.update(Point::from_client(position, area, 0.5));
        }
    }

    pub fn begin(
        &mut self,
        contact: u32,
        geometry: impl Into<InkArea>,
        position: POINT,
        pressure: f32,
        tool: Tool,
        settings: &Settings,
    ) -> bool {
        let geometry = geometry.into();
        let area = geometry.page;
        if self.path.is_none()
            || tool == Tool::Hand
            || self.gesture.is_some()
            || !(contains(area, position)
                || tool == Tool::Select && self.selected_hit(geometry, position))
        {
            return false;
        }
        let point = Point::from_client(position, area, pressure);
        let before = self.marks.clone();
        let style = tool.style(settings);
        let mut changed = false;
        let action = match tool {
            Tool::Pen | Tool::Highlighter => {
                if self.selected.take().is_some() {
                    self.damage = Damage::Full;
                }
                self.marks.push(Mark::Stroke {
                    style,
                    variable: tool == Tool::Pen,
                    points: Arc::new(vec![point]),
                });
                self.damage.points(&[point], style.width);
                changed = true;
                Action::Stroke
            }
            Tool::Eraser => {
                self.selected = None;
                Action::Erase
            }
            Tool::Staff | Tool::Rectangle => {
                self.selected = None;
                let staff = tool == Tool::Staff;
                let (w, h) = if staff { (0.32, 0.045) } else { (0.18, 0.065) };
                let default = Bounds {
                    x: point.x.min(1.0 - w),
                    y: point.y.min(1.0 - h),
                    w,
                    h,
                };
                self.marks.push(Mark::Shape {
                    style,
                    staff,
                    bounds: default,
                });
                self.change_layer();
                self.damage = Damage::Full;
                changed = true;
                Action::Shape {
                    start: point,
                    default,
                }
            }
            Tool::Select => {
                let selected = self.selected.and_then(|index| match self.marks.get(index) {
                    Some(Mark::Shape { bounds, .. }) => Some((index, *bounds)),
                    _ => None,
                });
                if let Some((_, bounds)) = selected
                    && geometry
                        .trash(bounds.pixels(area))
                        .is_some_and(|button| contains(button, position))
                {
                    self.remove_selected();
                    self.gesture = Some(Gesture {
                        contact,
                        before,
                        action: Action::Remove,
                        changed: true,
                        last_sample: 0,
                    });
                    return true;
                }
                let selected = selected
                    .and_then(|(index, bounds)| {
                        let rect = bounds.pixels(area);
                        geometry
                            .resize_hit(rect, position)
                            .or_else(|| {
                                contains(geometry.selection_rect(rect), position).then_some((0, 0))
                            })
                            .map(|handle| (index, bounds, handle))
                    })
                    .or_else(|| {
                        self.shape_at(point)
                            .map(|(index, bounds)| (index, bounds, (0, 0)))
                    });
                self.selected = selected.map(|(index, _, _)| index);
                self.damage = Damage::Full;
                let Some((index, original, handle)) = selected else {
                    return true;
                };
                Action::Move {
                    start: point,
                    original,
                    index,
                    handle,
                }
            }
            Tool::Hand => return false,
        };
        self.gesture = Some(Gesture {
            contact,
            before,
            action,
            changed,
            last_sample: 0,
        });
        if tool == Tool::Eraser {
            self.update(point);
        }
        true
    }

    fn update(&mut self, point: Point) {
        let Some(gesture) = &mut self.gesture else {
            return;
        };
        match gesture.action {
            Action::Remove => {}
            Action::Stroke => {
                if let Some(Mark::Stroke { points, style, .. }) = self.marks.last_mut()
                    && points.last() != Some(&point)
                {
                    Arc::make_mut(points).push(point);
                    self.damage.points(points, style.width);
                }
            }
            Action::Erase => {
                if let Some(index) = self.marks.iter().rposition(|mark| mark.hit(point)) {
                    self.marks.remove(index);
                    gesture.changed = true;
                    self.layer_generation = self.layer_generation.wrapping_add(1);
                    self.damage = Damage::Full;
                }
            }
            Action::Shape { start, default } => {
                if let Some(Mark::Shape { bounds, .. }) = self.marks.last_mut() {
                    let drawn = Bounds::between(start, point);
                    let resized = if drawn.w >= 0.016 && drawn.h >= 0.009 {
                        drawn
                    } else {
                        default
                    };
                    if *bounds != resized {
                        *bounds = resized;
                        self.layer_generation = self.layer_generation.wrapping_add(1);
                        self.damage = Damage::Full;
                    }
                }
            }
            Action::Move {
                start,
                original,
                index,
                handle,
            } => {
                if let Some(Mark::Shape { bounds, .. }) = self.marks.get_mut(index) {
                    let previous = *bounds;
                    let (dx, dy) = (point.x - start.x, point.y - start.y);
                    *bounds = if handle != (0, 0) {
                        let (x, w) = resize_axis(original.x, original.w, dx, handle.0, 0.012);
                        let (y, h) = resize_axis(original.y, original.h, dy, handle.1, 0.009);
                        Bounds { x, y, w, h }
                    } else {
                        Bounds {
                            x: (original.x + dx).clamp(0.0, (1.0 - original.w).max(0.0)),
                            y: (original.y + dy).clamp(0.0, (1.0 - original.h).max(0.0)),
                            ..original
                        }
                    };
                    gesture.changed |= *bounds != original;
                    if previous != *bounds {
                        self.layer_generation = self.layer_generation.wrapping_add(1);
                        self.damage = Damage::Full;
                    }
                }
            }
        }
    }

    pub fn draw_committed(&self, dc: HDC, area: RECT, clip: RECT, from: usize) {
        let committed = &self.marks[from..self.layer_version().count];
        if !committed.is_empty()
            && let Some(canvas) = Canvas::new(dc)
        {
            canvas.clip(clip);
            canvas.clip(area);
            for mark in committed {
                draw_mark(&canvas, area, mark, clip);
            }
        }
    }

    fn active_stroke(&self) -> Option<&Mark> {
        if matches!(
            self.gesture.as_ref().map(|gesture| &gesture.action),
            Some(Action::Stroke)
        ) {
            self.marks.last()
        } else {
            None
        }
    }

    pub fn draw_live(&self, dc: HDC, geometry: InkArea, clip: RECT, tool: Tool) {
        let area = geometry.page;
        if let Some(mark) = self.active_stroke()
            && let Some(canvas) = Canvas::new(dc)
        {
            canvas.clip(clip);
            canvas.clip(area);
            draw_mark(&canvas, area, mark, clip);
        }
        if tool == Tool::Select
            && let Some(Mark::Shape { bounds, .. }) = self.selected.and_then(|i| self.marks.get(i))
            && let Some(canvas) = Canvas::new(dc)
        {
            canvas.clip(clip);
            let shape = bounds.pixels(area);
            let rect = geometry.selection_rect(shape);
            let (x, y) = (rect.left as f32, rect.top as f32);
            let (w, h) = (
                (rect.right - rect.left) as f32,
                (rect.bottom - rect.top) as f32,
            );
            let outline = 0xff176889;
            let line = 1.6 * geometry.dpi;
            canvas.rectangle(x, y, w, line, outline);
            canvas.rectangle(x, y + h - line, w, line, outline);
            canvas.rectangle(x, y, line, h, outline);
            canvas.rectangle(x + w - line, y, line, h, outline);
            for (_, point) in handles(rect) {
                canvas.ellipse(
                    around(point, geometry.px(9.0)),
                    0xfff6fbff,
                    outline,
                    2.4 * geometry.dpi,
                );
            }
            if let Some(bin) = geometry.trash(shape) {
                canvas.ellipse(bin, 0xffffede9, 0xffb94142, 1.8 * geometry.dpi);
                icons::draw_on(
                    &canvas,
                    Glyph::Trash,
                    bin,
                    23.0 * geometry.dpi,
                    COLORREF(0x4535ad),
                );
            }
        }
    }
}

fn draw_mark(canvas: &Canvas, area: RECT, mark: &Mark, clip: RECT) {
    match mark {
        Mark::Stroke {
            style,
            variable,
            points,
        } => {
            let scale = (area.right - area.left).min(area.bottom - area.top) as f32;
            let samples = stroke_samples(points, area, style.width * scale, *variable);
            draw_stroke_clipped(
                canvas,
                &samples,
                canvas::argb(COLORREF(style.color), style.opacity),
                clip,
                *variable,
            );
        }
        Mark::Shape {
            style,
            staff,
            bounds,
        } => {
            let rect = bounds.pixels(area);
            let (x, y) = (rect.left as f32, rect.top as f32);
            let (w, h) = (
                (rect.right - rect.left) as f32,
                (rect.bottom - rect.top) as f32,
            );
            let color = canvas::argb(COLORREF(style.color), style.opacity);
            if *staff {
                let line_height = ((area.right - area.left) as f32 * style.width)
                    .clamp(0.65, h / 5.0 + 0.65)
                    .min(h);
                for line in 0..5 {
                    canvas.rectangle(
                        x,
                        y + (h - line_height) * line as f32 / 4.0,
                        w,
                        line_height,
                        color,
                    );
                }
            } else {
                canvas.rectangle(x, y, w, h, color);
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct StrokeSample {
    x: f32,
    y: f32,
    radius: f32,
}

impl StrokeSample {
    fn towards(self, end: Self, t: f32) -> Self {
        Self {
            x: self.x + (end.x - self.x) * t,
            y: self.y + (end.y - self.y) * t,
            radius: self.radius + (end.radius - self.radius) * t,
        }
    }

    fn distance(self, end: Self) -> f32 {
        (self.x - end.x).hypot(self.y - end.y)
    }
}

fn stroke_samples(points: &[Point], area: RECT, width: f32, variable: bool) -> Vec<StrokeSample> {
    let (w, h) = (
        (area.right - area.left) as f32,
        (area.bottom - area.top) as f32,
    );
    let raw: Vec<_> = points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let factor = if variable {
                let before = points[i.saturating_sub(1)].pressure;
                let after = points[(i + 1).min(points.len() - 1)].pressure;
                let pressure = ((before + 2.0 * p.pressure + after) / 4.0).clamp(0.0, 1.0);
                0.18 + 1.72 * pressure
            } else {
                1.0
            };
            StrokeSample {
                x: area.left as f32 + p.x * w,
                y: area.top as f32 + p.y * h,
                radius: (width * factor / 2.0).max(0.4),
            }
        })
        .collect();
    if raw.len() < 3 {
        return raw;
    }

    let mut smoothed = Vec::with_capacity(raw.len() * 2);
    smoothed.push(raw[0]);
    for window in raw.windows(3) {
        let [previous, center, next] = [window[0], window[1], window[2]];
        let incoming = (center.x - previous.x, center.y - previous.y);
        let outgoing = (next.x - center.x, next.y - center.y);
        let cosine_scaled = incoming.0 * outgoing.0 + incoming.1 * outgoing.1;
        if cosine_scaled < 0.15 * previous.distance(center) * center.distance(next) {
            // Keep intentional corners and pen reversals, which carry meaning in musical notation.
            smoothed.push(center);
            continue;
        }
        let start = previous.towards(center, 0.5);
        let end = center.towards(next, 0.5);
        let segments = ((start.distance(center) + center.distance(end)) / 3.5)
            .ceil()
            .clamp(1.0, 12.0) as u32;
        for segment in 0..=segments {
            let t = segment as f32 / segments as f32;
            smoothed.push(start.towards(center, t).towards(center.towards(end, t), t));
        }
    }
    smoothed.push(raw[raw.len() - 1]);
    smoothed
}

fn stroke_quads(samples: &[StrokeSample]) -> impl Iterator<Item = [PointF; 4]> + '_ {
    samples.windows(2).filter_map(|segment| {
        let [a, b] = [segment[0], segment[1]];
        let length = a.distance(b);
        if length < 0.01 {
            return None;
        }
        let (nx, ny) = (-(b.y - a.y) / length, (b.x - a.x) / length);
        let point = |p: StrokeSample, sign: f32| PointF {
            X: p.x + nx * p.radius * sign,
            Y: p.y + ny * p.radius * sign,
        };
        Some([point(a, 1.0), point(a, -1.0), point(b, -1.0), point(b, 1.0)])
    })
}

fn highlighter_polygons(samples: &[StrokeSample]) -> impl Iterator<Item = [PointF; 8]> + '_ {
    samples
        .iter()
        .enumerate()
        .map(|(index, &end)| {
            let start = samples[index.saturating_sub(1)];
            let nib = |sample: StrokeSample| {
                let r = sample.radius;
                let c = r * 0.65;
                [
                    (-c, -r),
                    (c, -r),
                    (r, -c),
                    (r, c),
                    (c, r),
                    (-c, r),
                    (-r, c),
                    (-r, -c),
                ]
                .map(|(x, y)| PointF {
                    X: sample.x + x,
                    Y: sample.y + y,
                })
            };
            (nib(start), nib(end))
        })
        .flat_map(|(start, end)| {
            // Sweep the beveled nib along the segment, keeping all figures wound alike.
            let dx = end[0].X - start[0].X;
            let dy = end[0].Y - start[0].Y;
            std::iter::once(end).chain((0..8).filter_map(move |i| {
                let next = (i + 1) % 8;
                let edge_x = start[next].X - start[i].X;
                let edge_y = start[next].Y - start[i].Y;
                (edge_x * dy - edge_y * dx > 0.0).then_some([
                    start[i],
                    start[next],
                    end[next],
                    end[i],
                    end[i],
                    end[i],
                    end[i],
                    end[i],
                ])
            }))
        })
}

fn draw_stroke_clipped(
    canvas: &Canvas,
    samples: &[StrokeSample],
    color: u32,
    clip: RECT,
    variable: bool,
) {
    if samples.is_empty() || color >> 24 == 0 {
        return;
    }
    let Some(path) = Path::new() else { return };
    if !variable {
        for polygon in highlighter_polygons(samples) {
            if polygon_overlaps(&polygon, clip) {
                path.polygon(&polygon);
            }
        }
        canvas.fill_path(&path, color);
        return;
    }
    for quad in stroke_quads(samples) {
        if polygon_overlaps(&quad, clip) {
            path.polygon(&quad);
        }
    }
    for sample in samples {
        if sample.x + sample.radius >= clip.left as f32
            && sample.x - sample.radius <= clip.right as f32
            && sample.y + sample.radius >= clip.top as f32
            && sample.y - sample.radius <= clip.bottom as f32
        {
            path.dot(sample.x, sample.y, sample.radius);
        }
    }
    // Fill all segments together so a translucent stroke has uniform opacity at its joins.
    canvas.fill_path(&path, color);
}

fn polygon_overlaps(points: &[PointF], clip: RECT) -> bool {
    points.iter().any(|p| p.X >= clip.left as f32)
        && points.iter().any(|p| p.X <= clip.right as f32)
        && points.iter().any(|p| p.Y >= clip.top as f32)
        && points.iter().any(|p| p.Y <= clip.bottom as f32)
}

impl Mark {
    pub(crate) fn append_pdf(&self, out: &mut String, area: RECT) -> std::fmt::Result {
        use std::fmt::Write;
        let style = match self {
            Self::Stroke { style, .. } | Self::Shape { style, .. } => style,
        };
        if style.opacity == 0 {
            return Ok(());
        }
        let channel = |shift| ((style.color >> shift) & 255u32) as f32 / 255.0;
        writeln!(
            out,
            "/SD{} gs {:.5} {:.5} {:.5} rg",
            style.opacity,
            channel(0),
            channel(8),
            channel(16)
        )?;
        match self {
            Self::Stroke {
                variable, points, ..
            } => {
                let scale = (area.right - area.left).min(area.bottom - area.top) as f32;
                let samples = stroke_samples(points, area, style.width * scale, *variable);
                if !variable {
                    for polygon in highlighter_polygons(&samples) {
                        let [first, rest @ ..] = polygon;
                        writeln!(out, "{:.3} {:.3} m", first.X, first.Y)?;
                        for point in rest {
                            writeln!(out, "{:.3} {:.3} l", point.X, point.Y)?;
                        }
                        writeln!(out, "h")?;
                    }
                    return writeln!(out, "f");
                }
                for [a, b, c, d] in stroke_quads(&samples) {
                    writeln!(
                        out,
                        "{:.3} {:.3} m {:.3} {:.3} l {:.3} {:.3} l {:.3} {:.3} l h",
                        a.X, a.Y, b.X, b.Y, c.X, c.Y, d.X, d.Y
                    )?;
                }
                for StrokeSample { x, y, radius: r } in samples {
                    let k = r * 0.552_284_8;
                    writeln!(out, "{:.3} {:.3} m", x + r, y)?;
                    for (a, b, c, d, e, f) in [
                        (x + r, y + k, x + k, y + r, x, y + r),
                        (x - k, y + r, x - r, y + k, x - r, y),
                        (x - r, y - k, x - k, y - r, x, y - r),
                        (x + k, y - r, x + r, y - k, x + r, y),
                    ] {
                        writeln!(out, "{a:.3} {b:.3} {c:.3} {d:.3} {e:.3} {f:.3} c")?;
                    }
                    writeln!(out, "h")?;
                }
            }
            Self::Shape { staff, bounds, .. } => {
                let rect = bounds.pixels(area);
                let (x, y) = (rect.left, rect.top as f32);
                let (w, h) = (rect.right - rect.left, (rect.bottom - rect.top) as f32);
                if *staff {
                    let line = ((area.right - area.left) as f32 * style.width)
                        .clamp(0.65, h / 5.0 + 0.65)
                        .min(h);
                    for number in 0..5 {
                        let top = y + (h - line) * number as f32 / 4.0;
                        writeln!(out, "{x} {top:.3} {w} {line:.3} re")?;
                    }
                } else {
                    writeln!(out, "{x} {y:.3} {w} {h:.3} re")?;
                }
            }
        }
        writeln!(out, "f")
    }

    pub(crate) fn opacity(&self) -> u8 {
        match self {
            Self::Stroke { style, .. } | Self::Shape { style, .. } => style.opacity,
        }
    }
}

fn distance(point: Point, start: Point, end: Point) -> f32 {
    let (dx, dy) = (end.x - start.x, end.y - start.y);
    let length = dx * dx + dy * dy;
    let t = if length > 0.0 {
        (((point.x - start.x) * dx + (point.y - start.y) * dy) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (point.x - (start.x + t * dx)).hypot(point.y - (start.y + t * dy))
}

fn resize_axis(start: f32, length: f32, delta: f32, handle: i32, min: f32) -> (f32, f32) {
    let end = start + length;
    match handle {
        -1 => {
            let low = (start + delta).clamp(0.0, (end - min).max(0.0));
            (low, end - low)
        }
        1 => (start, (length + delta).clamp(min, (1.0 - start).max(min))),
        _ => (start, length),
    }
}

fn handles(rect: RECT) -> impl Iterator<Item = ((i32, i32), POINT)> {
    let x = [rect.left, (rect.left + rect.right) / 2, rect.right];
    let y = [rect.top, (rect.top + rect.bottom) / 2, rect.bottom];
    (-1i32..=1).flat_map(move |dy| {
        (-1i32..=1).filter_map(move |dx| {
            (dx != 0 || dy != 0).then_some((
                (dx, dy),
                POINT {
                    x: x[(dx + 1) as usize],
                    y: y[(dy + 1) as usize],
                },
            ))
        })
    })
}

fn around(point: POINT, radius: i32) -> RECT {
    RECT {
        left: point.x - radius,
        top: point.y - radius,
        right: point.x + radius,
        bottom: point.y + radius,
    }
}

impl InkArea {
    fn px(self, dip: f32) -> i32 {
        (dip * self.dpi).round().max(1.0) as i32
    }

    fn selection_rect(self, rect: RECT) -> RECT {
        let minimum = self.px(54.0);
        let extend = |start: i32, end: i32| {
            let space = (minimum - (end - start)).max(0);
            (start - space / 2, end + space - space / 2)
        };
        let (left, right) = extend(rect.left, rect.right);
        let (top, bottom) = extend(rect.top, rect.bottom);
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    fn trash(self, rect: RECT) -> Option<RECT> {
        let visible = self.viewport;
        if rect.bottom <= visible.top
            || rect.top >= visible.bottom
            || rect.right <= visible.left
            || rect.left >= visible.right
        {
            return None;
        }
        let rect = self.selection_rect(rect);
        let radius = self.px(22.0);
        let gap = self.px(15.0);
        let above = rect.top - radius - gap;
        let below = rect.bottom + radius + gap;
        let x = (rect.left + rect.right) / 2;
        let y = if above - radius >= visible.top {
            above
        } else {
            below
        };
        let x = x.clamp(
            visible.left + radius,
            (visible.right - radius).max(visible.left + radius),
        );
        let y = y.clamp(
            visible.top + radius,
            (visible.bottom - radius).max(visible.top + radius),
        );
        Some(around(POINT { x, y }, radius))
    }

    fn resize_hit(self, rect: RECT, position: POINT) -> Option<(i32, i32)> {
        let radius = self.px(21.0);
        handles(self.selection_rect(rect))
            .map(|(handle, point)| {
                (
                    handle,
                    (point.x - position.x).pow(2) + (point.y - position.y).pow(2),
                )
            })
            .filter(|(_, distance)| *distance <= radius * radius)
            .min_by_key(|(_, distance)| *distance)
            .map(|(handle, _)| handle)
    }
}

fn contains(area: RECT, point: POINT) -> bool {
    point.x >= area.left && point.x < area.right && point.y >= area.top && point.y < area.bottom
}

fn pen_sample(hwnd: HWND, info: &POINTER_PEN_INFO) -> (POINT, f32) {
    let mut position = info.pointerInfo.ptPixelLocation;
    let _ = unsafe { ScreenToClient(hwnd, &mut position) };
    let pressure = if info.penMask & PEN_MASK_PRESSURE != 0 {
        (info.pressure as f32 / 1024.0).clamp(0.0, 1.0)
    } else {
        0.5
    };
    (position, pressure)
}

fn pixel(x: f32, y: f32) -> POINT {
    POINT {
        x: x.round() as i32,
        y: y.round() as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thin_staff_keeps_large_handles_separate_at_high_dpi() {
        let page = RECT {
            left: 0,
            top: 0,
            right: 900,
            bottom: 700,
        };
        let geometry = InkArea {
            page,
            viewport: page,
            dpi: 1.5,
        };
        let staff = RECT {
            left: 250,
            top: 300,
            right: 510,
            bottom: 321,
        };
        let control = geometry.selection_rect(staff);
        assert!(control.bottom - control.top >= 81);
        let top = POINT {
            x: control.left,
            y: control.top,
        };
        let bottom = POINT {
            x: control.left,
            y: control.bottom,
        };
        assert_eq!(geometry.resize_hit(staff, top), Some((-1, -1)));
        assert_eq!(geometry.resize_hit(staff, bottom), Some((-1, 1)));
        let trash = geometry.trash(staff).unwrap();
        assert_eq!(trash.right - trash.left, 66);
        assert!(trash.bottom + geometry.px(9.0) < control.top);
    }

    #[test]
    fn active_stroke_damages_only_the_recent_segment_at_high_zoom() {
        let scratch = data::Scratch::new();
        let saver = Saver::start(None).unwrap();
        let mut ink = Ink::default();
        ink.bind(Some(scratch.0.join("stroke.json")), &saver)
            .unwrap();
        let page = RECT {
            left: -800,
            top: -1100,
            right: 1800,
            bottom: 2300,
        };
        let viewport = RECT {
            left: 0,
            top: 0,
            right: 900,
            bottom: 1200,
        };
        let revision = ink.layer_version();
        ink.begin(
            1,
            page,
            POINT { x: 120, y: 150 },
            0.6,
            Tool::Pen,
            &Settings::default(),
        );
        let first = ink.take_damage(page, viewport).unwrap();
        assert!(first.right - first.left < 40);
        ink.contact_update(1, page, POINT { x: 140, y: 158 });
        ink.contact_update(1, page, POINT { x: 160, y: 170 });
        ink.contact_update(1, page, POINT { x: 165, y: 170 });
        let next = ink.take_damage(page, viewport).unwrap();
        assert!(next.left < 120 && next.right > 165);
        assert!(next.right - next.left < 90);
        assert_eq!(ink.layer_version(), revision);
        assert_eq!(revision.count, 0);
        ink.cancel_contact();
        assert_eq!(ink.layer_version().append_from(Some(revision)), Some(0));
        assert_eq!(ink.layer_version().count, 1);
    }

    #[test]
    fn eraser_removes_strokes_and_selected_shapes_have_independent_handles() {
        let scratch = data::Scratch::new();
        let area = RECT {
            left: 0,
            top: 0,
            right: 800,
            bottom: 1200,
        };
        let settings = Settings::default();
        let saver = Saver::start(None).unwrap();
        let mut ink = Ink::default();
        ink.bind(Some(scratch.0.join("shape.json")), &saver)
            .unwrap();
        let point = POINT { x: 200, y: 300 };
        ink.begin(1, area, point, 0.5, Tool::Staff, &settings);
        assert!(ink.complete_contact());
        assert!(ink.has_selected_shape());
        let inside = POINT { x: 250, y: 325 };
        ink.begin(1, area, inside, 0.6, Tool::Pen, &settings);
        ink.cancel_contact();
        ink.begin(1, area, inside, 0.6, Tool::Eraser, &settings);
        ink.cancel_contact();
        assert!(ink.select_shape(area, inside));
        assert_eq!(ink.marks.len(), 1);
        let Mark::Shape {
            bounds: original, ..
        } = ink.marks[0]
        else {
            panic!("missing shape")
        };
        let rect = original.pixels(area);
        assert!(ink.begin(
            1,
            area,
            POINT {
                x: rect.left,
                y: rect.top
            },
            0.5,
            Tool::Select,
            &settings
        ));
        ink.contact_update(
            1,
            area,
            POINT {
                x: rect.left - 30,
                y: rect.top - 20,
            },
        );
        ink.cancel_contact();
        let Mark::Shape {
            bounds: resized, ..
        } = ink.marks[0]
        else {
            panic!("missing resized shape")
        };
        assert!(resized.w > original.w && resized.h > original.h && resized.x < original.x);
        let bin = InkArea::from(area).trash(resized.pixels(area)).unwrap();
        ink.begin(
            1,
            area,
            POINT {
                x: (bin.left + bin.right) / 2,
                y: (bin.top + bin.bottom) / 2,
            },
            0.5,
            Tool::Select,
            &settings,
        );
        ink.cancel_contact();
        assert!(ink.marks.is_empty());
        ink.undo();
        assert_eq!(ink.marks.len(), 1);
        assert!(ink.select_shape(area, inside));
        assert!(ink.delete_selected_shape());
        assert!(ink.marks.is_empty());
        ink.undo();
        assert_eq!(ink.marks.len(), 1);
    }

    #[test]
    fn reloads_marks_with_pressure_and_preserves_shape_edits() {
        let scratch = data::Scratch::new();
        let path = scratch.0.join("page.json");
        let area = RECT {
            left: 0,
            top: 0,
            right: 800,
            bottom: 1200,
        };
        let settings = Settings::default();
        let saver = Saver::start(None).unwrap();
        let mut ink = Ink::default();
        ink.bind(Some(path.clone()), &saver).unwrap();
        ink.begin(1, area, POINT { x: 200, y: 90 }, 0.15, Tool::Pen, &settings);
        ink.update(Point {
            x: 0.4,
            y: 0.12,
            pressure: 0.8,
        });
        ink.cancel_contact();
        ink.begin(
            1,
            area,
            POINT { x: 320, y: 360 },
            0.5,
            Tool::Staff,
            &settings,
        );
        ink.update(Point {
            x: 0.76,
            y: 0.38,
            pressure: 0.5,
        });
        ink.cancel_contact();
        ink.save(&saver).unwrap();

        let mut reloaded = Ink::default();
        reloaded.bind(Some(path.clone()), &saver).unwrap();
        assert!(matches!(&reloaded.marks[0], Mark::Stroke { points, .. }
            if points[0].pressure == 0.15 && points[1].pressure == 0.8));
        reloaded.begin(
            1,
            area,
            POINT { x: 395, y: 405 },
            0.5,
            Tool::Select,
            &settings,
        );
        reloaded.update(Point {
            x: 0.57,
            y: 0.5,
            pressure: 0.5,
        });
        reloaded.cancel_contact();
        reloaded.save(&saver).unwrap();
        assert!(
            matches!(reloaded.marks[1], Mark::Shape { staff: true, bounds, .. } if bounds.y > 0.4)
        );
        reloaded.undo();
        assert!(
            matches!(reloaded.marks[1], Mark::Shape { staff: true, bounds, .. } if bounds.y < 0.4)
        );
        reloaded.redo();
        assert!(matches!(reloaded.marks[1], Mark::Shape { bounds, .. } if bounds.y > 0.4));
        reloaded.save(&saver).unwrap();
        let start = std::time::Instant::now();
        while matches!(saver.status(), SaveStatus::Saving) {
            assert!(start.elapsed().as_secs() < 5);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(matches!(saver.status(), SaveStatus::Idle));
        let (saved, _, _) = data::read_page::<Vec<Mark>>(&path).unwrap();
        assert!(matches!(saved[1], Mark::Shape { bounds, .. } if bounds.y > 0.4));
    }

    #[test]
    fn pressure_curve_is_smooth_and_keeps_the_input_endpoints() {
        let points = [
            Point {
                x: 0.1,
                y: 0.2,
                pressure: 0.05,
            },
            Point {
                x: 0.18,
                y: 0.25,
                pressure: 0.2,
            },
            Point {
                x: 0.25,
                y: 0.26,
                pressure: 0.85,
            },
            Point {
                x: 0.5,
                y: 0.3,
                pressure: 1.0,
            },
        ];
        let rect = RECT {
            left: 0,
            top: 0,
            right: 800,
            bottom: 1200,
        };
        let sample = stroke_samples(&points, rect, 4.0, true);
        assert!(sample.len() > points.len());
        assert_eq!(sample.first().unwrap().x, 80.0);
        assert_eq!(sample.last().unwrap().x, 400.0);
        assert!(
            sample
                .windows(2)
                .all(|pair| pair[0].radius <= pair[1].radius)
        );
    }

    #[test]
    fn translucent_stroke_has_no_seams_at_joins() {
        use windows::Win32::Graphics::Gdi::*;
        let _startup = canvas::Startup::new().unwrap();
        let (width, height) = (80, 48);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        unsafe {
            let dc = CreateCompatibleDC(None);
            let mut bits = std::ptr::null_mut();
            let dib = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
            let original = SelectObject(dc, dib.into());
            let rect = RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            };
            for variable in [true, false] {
                FillRect(dc, &rect, HBRUSH(GetStockObject(WHITE_BRUSH).0));
                {
                    let canvas = Canvas::new(dc).unwrap();
                    let stroke = [10.0, 24.0, 40.0, 60.0].map(|x| StrokeSample {
                        x,
                        y: 20.0,
                        radius: 6.0,
                    });
                    draw_stroke_clipped(&canvas, &stroke, 0x80ffb400, rect, variable);
                }
                let _ = GdiFlush();
                let sample = |x, y| *bits.cast::<u32>().add((y * width + x) as usize) & 0x00ff_ffff;
                let center = sample(17, 20);
                assert_eq!(sample(24, 20), center);
                assert_eq!(sample(17, 4), 0xffffff);
                assert!((110..=145).contains(&(center & 255)));
                // Near the first endpoint the beveled nib reaches farther than a round tip.
                assert_eq!(sample(5, 16) != 0xffffff, !variable);
            }
            SelectObject(dc, original);
            let _ = DeleteObject(dib.into());
            let _ = DeleteDC(dc);
        }
    }
}
