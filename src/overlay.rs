use crate::data::Style;
use crate::icons::Glyph;
use crate::ink::Tool;
use crate::language::LanguagePreference;
use crate::radial::Command;
use std::time::Instant;
use windows::Win32::Foundation::{COLORREF, POINT, RECT};
pub const FAVORITE_ROWS: usize = 3;
pub const FAVORITE_ROW: i32 = 48;
pub const BOOKMARK_ROWS: usize = 7;

#[derive(Clone, Copy)]
pub struct Scrollbar {
    pub track: RECT,
    pub thumb: RECT,
    max_scroll: f32,
}

impl Scrollbar {
    pub fn new(track: RECT, max_scroll: f32, scroll: f32, min_thumb: i32, scale: f32) -> Option<Self> {
        let height = track.bottom - track.top;
        if max_scroll <= 0.0 || height < 2 { return None; }
        let ratio = height as f32 / (height as f32 + max_scroll * scale);
        let length = (height as f32 * ratio).round() as i32;
        let length = length.clamp(min_thumb.clamp(1, height - 1), height - 1);
        let start = track.top + ((height - length) as f32 * scroll.clamp(0.0, max_scroll) / max_scroll).round() as i32;
        Some(Self { track, thumb: RECT { top: start, bottom: start + length, ..track }, max_scroll })
    }

    pub fn grab(self, y: i32) -> i32 {
        let length = self.thumb.bottom - self.thumb.top;
        if (self.thumb.top..self.thumb.bottom).contains(&y) { y - self.thumb.top } else { length / 2 }
    }

    pub fn scroll(self, y: i32, grab: i32) -> f32 {
        let travel = self.track.bottom - self.track.top - (self.thumb.bottom - self.thumb.top);
        self.max_scroll * ((y - grab - self.track.top) as f32 / travel as f32).clamp(0.0, 1.0)
    }
}

pub const SWATCHES: [u32; 12] = [
    0x392b22, 0xfdfaf7, 0xef9343, 0x5951e1, 0xc990ae, 0xfea39c, 0x88c148, 0x92e7af, 0x51e1ff,
    0x62c9ff, 0x4569f6, 0xb974ed,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Sheet {
    Page,
    Bookmarks,
    Style,
    Preferences,
    About,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    Command(Command),
    Sheet(Sheet),
    CloseDrawer,
    ResetZoom,
    ToggleTap,
    Language(LanguagePreference),
    BookmarkSelect(usize),
    BookmarkScroll(i32),
    BookmarkAdd,
    BookmarkEdit(usize),
    BookmarkDelete(usize),
    BookmarkChild(usize),
    BookmarkCollapse(usize),
    BookmarkDrag(usize),
    Finalize,
    CloseDocument,
    DefaultPdf,
    OpenStore,
    DismissStoreUpdate,
    AudioToggle(usize),
    AudioClose(usize),
    AudioCompact(usize),
    TapTempo,
    EditTuning,
    Bpm(i16),
    Beats(i8),
    Color(u32),
    Width(i32),
    Opacity(i32),
    Advanced,
    ToggleZoom,
    ZoomStep(i32),
    Turn(i32),
    ScoreTurn(i32),
    About,
    Feedback,
    DeleteShape,
    AddFavorite,
    LoadFavorite(usize),
    DeleteFavorite(usize),
    CustomColor,
    Website(&'static str),
    Licenses,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum SliderKind {
    Width,
    Opacity,
    Bpm,
    Beats,
    Zoom,
}

#[derive(Clone, Copy)]
pub struct Slider {
    pub rect: RECT,
    pub kind: SliderKind,
    pub ratio: f32,
}

impl Slider {
    pub fn track(self) -> (i32, i32, i32) {
        let pad = ((self.rect.bottom - self.rect.top) / 3).max(1);
        (
            self.rect.left + pad,
            (self.rect.right - pad).max(self.rect.left + pad + 1),
            (self.rect.top + self.rect.bottom) / 2,
        )
    }

    pub fn position(self, x: i32) -> f32 {
        let (left, right, _) = self.track();
        ((x - left) as f32 / (right - left) as f32).clamp(0.0, 1.0)
    }
}

pub struct BookmarkRow<'a> {
    pub title: &'a str,
    pub detail: String,
    pub index: usize,
    pub depth: usize,
    pub branch: bool,
    pub collapsed: bool,
}

pub struct Model<'a> {
    pub fullscreen: bool,
    pub popup: Option<RECT>,
    pub zoom: f32,
    pub tap: bool,
    pub language: LanguagePreference,
    pub page_loaded: bool,
    pub page_indicator: String,
    pub undo: bool,
    pub redo: bool,
    pub can_turn: [bool; 2],
    pub concert_navigation: Option<[bool; 2]>,
    pub can_add_bookmark: bool,
    pub bpm: u16,
    pub beats: u8,
    pub tuning_hz: u16,
    pub compact_pitch: &'a str,
    pub running: [bool; 2],
    pub audio_status: [&'a str; 2],
    pub pitch: Option<f32>,
    pub pulse: Option<(u8, f32)>,
    pub tool: Tool,
    pub style: Style,
    pub favorites: Option<&'a [Style]>,
    pub bookmarks: &'a [BookmarkRow<'a>],
    pub selected_bookmark: Option<usize>,
    pub bookmark_status: &'a str,
    pub bookmark_more: [bool; 2],
}

#[derive(Default)]
pub struct Overlay {
    pub drawer: bool,
    pub sheet: Option<Sheet>,
    pub bookmark_offset: usize,
    pub selected_bookmark: Option<usize>,
    last_frame: Option<Instant>,
    progress: f32,
    floating: [Floating; 2],
    pub grab: Option<Grab>,
    pub utilities: bool,
    pub utility_opacity: f32,
    pub popup_opacity: f32,
    pub popup_open: bool,
    pub zoom_slider: bool,
    pub favorite_scroll: f32,
}

#[derive(Default)]
struct Floating {
    visible: bool,
    compact: bool,
    anchor: Option<(f32, f32)>,
    visibility: f32,
    compactness: f32,
}

#[derive(Clone, Copy)]
pub struct Grab {
    pub id: u32,
    pub event: Option<Event>,
    pub slider: Option<Slider>,
    pub origin: POINT,
    drag: Option<(usize, POINT, POINT)>,
    pub favorite_scroll: Option<(f32, f32)>,
    pub favorite_bar: Option<(Scrollbar, i32)>,
}

impl Grab {
    pub fn dismiss(id: u32, origin: POINT) -> Self {
        Self {
            id,
            origin,
            event: None,
            slider: None,
            drag: None,
            favorite_scroll: None,
            favorite_bar: None,
        }
    }
}

pub struct Card {
    pub rect: RECT,
    pub header: RECT,
    pub floating: Option<usize>,
    pub labels: Vec<Label>,
    pub buttons: Vec<Button>,
    pub sliders: Vec<Slider>,
    pub separators: Vec<RECT>,
    pub strokes: Vec<(RECT, Style)>,
    pub section: Option<RECT>,
    pub opacity: f32,
    pub dock: bool,
    pub drawer: bool,
    pub interactive: bool,
    pub favorite_clip: Option<RECT>,
    pub favorite_labels_start: usize,
    pub favorite_bar: Option<Scrollbar>,
    pub scale: f32,
    pub meter: Option<(RECT, Meter)>,
}

pub enum Meter {
    Pitch(Option<f32>),
    Pulse { beats: u8, flash: Option<(u8, f32)> },
}

pub struct Label {
    pub rect: RECT,
    pub text: String,
    pub size: i32,
    pub muted: bool,
    pub dark: bool,
}

pub struct Button {
    pub rect: RECT,
    pub text: String,
    pub event: Event,
    pub active: bool,
    pub color: Option<COLORREF>,
    pub icon: Option<Glyph>,
    pub enabled: bool,
}

impl Overlay {
    pub fn open(&mut self, sheet: Sheet) {
        self.drawer = true;
        self.favorite_scroll = 0.0;
        self.sheet = Some(sheet);
        self.last_frame = Some(Instant::now());
    }

    pub fn close(&mut self) {
        self.drawer = false;
        self.last_frame = Some(Instant::now());
    }

    pub fn dismiss_drawer(&mut self) {
        self.drawer = false;
        self.sheet = None;
        self.grab = None;
        self.progress = 0.0;
    }

    pub fn toggle_audio(&mut self, slot: usize) -> bool {
        let panel = &mut self.floating[slot];
        panel.visible = !panel.visible;
        panel.visible
    }

    pub fn toggle_compact(&mut self, slot: usize) {
        let panel = &mut self.floating[slot];
        panel.compact = !panel.compact;
    }

    pub fn close_audio(&mut self, slot: usize) {
        let panel = &mut self.floating[slot];
        panel.visible = false;
    }

    pub fn dismiss_audio(&mut self, slot: usize) {
        let anchor = self.floating[slot].anchor;
        self.floating[slot] = Floating { anchor, ..Floating::default() };
    }

    pub fn audio_visible(&self) -> bool {
        self.floating.iter().any(|panel| panel.visible)
    }

    pub fn reset_frame_time(&mut self) {
        self.last_frame = None;
    }

    pub fn tick(&mut self) -> bool {
        let now = Instant::now();
        let dt = self
            .last_frame
            .replace(now)
            .map_or(0.016, |last| (now - last).as_secs_f32().min(0.06));
        let old = self.progress;
        let sign = if self.drawer { 1.0 } else { -1.0 };
        self.progress = (self.progress + sign * dt / 0.20).clamp(0.0, 1.0);
        let previous = self.utility_opacity;
        let direction = if self.utilities { 1.0 } else { -1.0 };
        self.utility_opacity = (self.utility_opacity + direction * dt / 0.15).clamp(0.0, 1.0);
        let popup = self.popup_opacity;
        let direction = if self.popup_open { 1.0 } else { -1.0 };
        self.popup_opacity = (self.popup_opacity + direction * dt / 0.12).clamp(0.0, 1.0);
        let audio = self.advance_audio(dt);
        self.progress != old || self.utility_opacity != previous || self.popup_opacity != popup || audio
    }

    pub(crate) fn advance_audio(&mut self, dt: f32) -> bool {
        let mut changed = false;
        for panel in &mut self.floating {
            let previous = (panel.visibility, panel.compactness);
            let direction = if panel.visible { 1.0 } else { -1.0 };
            panel.visibility = (panel.visibility + direction * dt / 0.16).clamp(0.0, 1.0);
            if panel.visible {
                let direction = if panel.compact { 1.0 } else { -1.0 };
                panel.compactness = (panel.compactness + direction * dt / 0.19).clamp(0.0, 1.0);
            } else if panel.visibility == 0.0 {
                panel.compact = false;
                panel.compactness = 0.0;
            }
            changed |= previous != (panel.visibility, panel.compactness);
        }
        changed
    }

    pub fn move_to(&mut self, id: u32, at: POINT, viewport: RECT) {
        if let Some(Grab {
            id: active,
            drag: Some((slot, start, origin)),
            ..
        }) = self.grab
            && active == id
        {
            let (w, h) = (
                (viewport.right - viewport.left).max(1),
                (viewport.bottom - viewport.top).max(1),
            );
            self.floating[slot].anchor = Some((
                (origin.x + at.x - start.x - viewport.left) as f32 / w as f32,
                (origin.y + at.y - start.y - viewport.top) as f32 / h as f32,
            ));
        }
    }

    pub fn drawer_reservation(&self, viewport: RECT, scale: f32) -> i32 {
        if (!self.drawer && self.progress == 0.0) || self.sheet == Some(Sheet::Page) {
            return 0;
        }
        let width = if self.sheet == Some(Sheet::Bookmarks) { 386.0 } else { 320.0 };
        (width * scale).round().min((viewport.right - viewport.left) as f32 * 0.64) as i32
    }

    pub fn cards(&self, viewport: RECT, scale: f32, data: &Model<'_>) -> Vec<Card> {
        let mut cards = Vec::new();
        let px = |x: i32| (x as f32 * scale).round() as i32;
        let vw = viewport.right - viewport.left;
        let vh = viewport.bottom - viewport.top;
        if vw < px(300) || vh < px(160) {
            return cards;
        }
        if data.page_loaded && (self.utilities || self.utility_opacity > 0.0) {
            self.utilities_panel(viewport, scale, data, &mut cards);
        }
        let mut obstacles: Vec<_> = data.popup.into_iter()
            .chain(cards.iter().map(|card| card.rect)).collect();
        let drawer = self.drawer_reservation(viewport, scale);
        if drawer > 0 {
            obstacles.push(RECT { right: viewport.left + drawer, ..viewport });
        }
        for (slot, panel) in self
            .floating
            .iter()
            .enumerate()
            .filter(|(_, panel)| panel.visible || panel.visibility > 0.0)
        {
            let ease = |value: f32| value * value * (3.0 - 2.0 * value);
            let morph = ease(panel.compactness);
            let mix = |a: f32, b: f32| (a + (b - a) * morph).round() as i32;
            let dimensions = (mix(298.0, 254.0), mix(if slot == 0 { 310.0 } else { 263.0 }, 78.0));
            let mut preferred = panel.anchor.map_or(
                (viewport.right - px(dimensions.0 + 18),
                 viewport.top + px(if slot == 0 { 14 } else { 108 })),
                |(x, y)| (viewport.left + (x * vw as f32) as i32, viewport.top + (y * vh as f32) as i32),
            );
            preferred.1 -= px((8.0 * (1.0 - ease(panel.visibility))).round() as i32);
            let full = floating_bounds(viewport, &obstacles, preferred, dimensions, scale);
            let compact = panel.compactness >= 0.5 || full.is_none();
            let Some((bounds, local_scale)) = full.or_else(|| {
                floating_bounds(viewport, &obstacles, preferred, (254, 78), scale)
            }) else {
                continue;
            };
            obstacles.push(bounds);
            let mut card = Card::new(bounds.left, bounds.top, bounds.right - bounds.left, bounds.bottom - bounds.top, local_scale, Some(slot));
            card.opacity = ease(panel.visibility) * (0.72 + 0.28 * (2.0 * morph - 1.0).abs());
            card.interactive = panel.visible && (panel.compactness == 0.0 || panel.compactness == 1.0);
            let title = if slot == 0 {
                tr!("Metronomo", "Metronome")
            } else {
                tr!("Accordatore", "Tuner")
            };
            let button = match (slot, data.running[slot]) {
                (_, true) => tr!("Ferma", "Stop"),
                (0, false) => tr!("Avvia", "Start"),
                _ => tr!("Avvia microfono", "Start microphone"),
            };
            if compact {
                let label = if slot == 0 {
                    format!("{} BPM", data.bpm)
                } else if data.compact_pitch.is_empty() {
                    title.into()
                } else {
                    data.compact_pitch.into()
                };
                card.label([9, 4, 161, 38], label, 17, false);
                card.icon([174, 7, 34, 30], Glyph::Expand, Event::AudioCompact(slot), true);
                card.icon([210, 7, 34, 30], Glyph::X, Event::AudioClose(slot), true);
                if slot == 0 {
                    card.button([10, 42, 75, 27], button, Event::AudioToggle(slot), !data.running[slot]);
                    card.meter = Some((card.rect([91, 42, 151, 27]), Meter::Pulse {
                        beats: data.beats, flash: data.pulse,
                    }));
                } else {
                    let compact_button = if data.running[slot] { button } else { tr!("Avvia", "Start") };
                    card.button([10, 42, 75, 27], compact_button, Event::AudioToggle(slot), !data.running[slot]);
                    if data.compact_pitch.is_empty() {
                        card.label([91, 42, 151, 27], data.audio_status[slot], 12, true);
                    } else {
                        card.label([91, 42, 151, 27], trf!("La4 = {} Hz", "A4 = {} Hz", data.tuning_hz), 12, true);
                    }
                }
                cards.push(card);
                continue;
            }
            card.label([14, 4, 194, 38], title, 18, false);
            card.icon([216, 7, 34, 30], Glyph::Minus, Event::AudioCompact(slot), true);
            card.icon([253, 7, 34, 30], Glyph::X, Event::AudioClose(slot), true);
            if slot == 0 {
                card.button([13, 51, 34, 34], "−", Event::Bpm(-1), false);
                card.label([48, 51, 202, 34], format!("{} BPM", data.bpm), 16, false);
                card.button([251, 51, 34, 34], "+", Event::Bpm(1), false);
                card.slider(
                    [12, 85, 274, 37],
                    SliderKind::Bpm,
                    (f32::from(data.bpm) - 30.0) / 270.0,
                );
                card.button([13, 124, 34, 34], "−", Event::Beats(-1), false);
                card.label(
                    [48, 124, 202, 34],
                    trf!("{} battiti per battuta", "{} beats per bar", data.beats),
                    14,
                    false,
                );
                card.button([251, 124, 34, 34], "+", Event::Beats(1), false);
                card.slider(
                    [12, 157, 274, 36],
                    SliderKind::Beats,
                    (f32::from(data.beats) - 1.0) / 11.0,
                );
            }
            let (meter, label_y) = if slot == 0 {
                (
                    (
                        card.rect([15, 195, 268, 33]),
                        Meter::Pulse {
                            beats: data.beats,
                            flash: data.pulse,
                        },
                    ),
                    229,
                )
            } else {
                card.label([48, 48, 202, 32], trf!("La4 = {} Hz", "A4 = {} Hz", data.tuning_hz), 16, false);
                card.icon([251, 48, 34, 32], Glyph::Pencil, Event::EditTuning, true);
                ((card.rect([18, 84, 261, 85]), Meter::Pitch(data.pitch)), 172)
            };
            card.meter = Some(meter);
            card.label([13, label_y, 272, 36], data.audio_status[slot], 13, true);
            if slot == 0 {
                card.button([152, label_y + 37, 133, 32], tr!("Tap Tempo", "Tap Tempo"), Event::TapTempo, false);
            }
            card.button(
                [13, label_y + 37, if slot == 0 { 133 } else { 272 }, 32],
                button,
                Event::AudioToggle(slot),
                !data.running[slot],
            );
            cards.push(card);
        }
        if self.progress == 0.0 {
            return cards;
        }
        let Some(sheet) = self.sheet else { return cards };
        let height_dp = match sheet {
            Sheet::Page => return cards,
            Sheet::Bookmarks => 508,
            Sheet::Style => {
                if let Some(favorites) = data.favorites {
                    favorite_bottom(favorites.len()) + 12
                } else if data.tool == Tool::Rectangle {
                    254
                } else {
                    332
                }
            }
            Sheet::Preferences => 263,
            Sheet::About => 299,
        };
        let width_dp = if sheet == Sheet::Bookmarks { 386 } else { 320 };
        let scale = scale
            .min(vh as f32 / height_dp as f32)
            .min(vw as f32 * 0.64 / width_dp as f32);
        let width = (width_dp as f32 * scale).round() as i32;
        let position = 1.0 - (1.0 - self.progress).powi(3);
        let x = viewport.left + ((position - 1.0) * width as f32).round() as i32;
        let height = (height_dp as f32 * scale).round() as i32;
        let mut card = Card::new(x, viewport.top, width, height, scale, None);
        card.drawer = true;
        let style_title;
        let title = match sheet {
            Sheet::Page => unreachable!(),
            Sheet::Bookmarks => tr!("Segnalibri", "Bookmarks"),
            Sheet::Style => {
                style_title = trf!(
                    "Opzioni · {}",
                    "Options · {}",
                    Command::Tool(data.tool).label()
                );
                &style_title
            }
            Sheet::Preferences => tr!("Preferenze", "Settings"),
            Sheet::About => "ScoreDraw",
        };
        card.label([52, 6, 203, 36], title, 20, false);
        card.icon(
            [width_dp - 46, 9, 34, 32],
            Glyph::X,
            Event::CloseDrawer,
            true,
        );
        match sheet {
            Sheet::Page => unreachable!(),
            Sheet::Bookmarks => card.bookmarks(data),
            Sheet::Style => card.style(self, data),
            Sheet::Preferences => card.preferences(data),
            Sheet::About => card.about(),
        }
        cards.push(card);
        cards
    }

    fn utilities_panel(&self, viewport: RECT, scale: f32, data: &Model<'_>, cards: &mut Vec<Card>) {
        let width = if data.concert_navigation.is_some() { 606 } else { 506 };
        let scale = scale.min((viewport.right - viewport.left) as f32 / (width + 18) as f32);
        let px = |x: i32| (x as f32 * scale).round() as i32;
        let middle = (viewport.left + viewport.right) / 2;
        let bottom = viewport.bottom - px(11);
        let mut dock = Card::new(
            middle - px(width) / 2,
            bottom - px(53),
            px(width),
            px(53),
            scale,
            None,
        );
        dock.dock = true;
        dock.opacity = self.utility_opacity;
        dock.icon(
            [6, 6, 41, 41],
            Glyph::Undo,
            Event::Command(Command::Undo),
            data.undo,
        );
        dock.icon(
            [49, 6, 41, 41],
            Glyph::Redo,
            Event::Command(Command::Redo),
            data.redo,
        );
        dock.separator(99);
        dock.icon(
            [108, 6, 38, 41],
            Glyph::ChevronLeft,
            Event::Turn(-1),
            data.can_turn[0],
        );
        dock.button(
            [148, 6, 77, 41],
            &data.page_indicator,
            Event::Sheet(Sheet::Page),
            false,
        );
        dock.icon(
            [227, 6, 38, 41],
            Glyph::ChevronRight,
            Event::Turn(1),
            data.can_turn[1],
        );
        dock.separator(274);
        dock.icon(
            [284, 6, 39, 41],
            Glyph::Minus,
            Event::ZoomStep(-1),
            data.zoom > 1.0,
        );
        dock.button(
            [325, 6, 66, 41],
            format!("{:.0}%", data.zoom * 100.0),
            Event::ToggleZoom,
            self.zoom_slider,
        );
        dock.icon(
            [393, 6, 39, 41],
            Glyph::Plus,
            Event::ZoomStep(1),
            data.zoom < 4.0,
        );
        dock.separator(442);
        if let Some([previous, next]) = data.concert_navigation {
            dock.icon([451, 6, 43, 41], Glyph::SkipBack, Event::ScoreTurn(-1), previous);
            dock.icon([497, 6, 43, 41], Glyph::SkipForward, Event::ScoreTurn(1), next);
            dock.separator(546);
        }
        dock.icon(
            [width - 54, 6, 46, 41],
            if data.fullscreen {
                Glyph::Shrink
            } else {
                Glyph::Expand
            },
            Event::Command(Command::Fullscreen),
            true,
        );
        cards.push(dock);
        if self.zoom_slider {
            let mut zoom = Card::new(
                middle - px(154),
                bottom - px(127),
                px(308),
                px(65),
                scale,
                None,
            );
            zoom.dock = true;
            zoom.opacity = self.utility_opacity;
            zoom.icon(
                [7, 11, 37, 43],
                Glyph::Minimize,
                Event::ResetZoom,
                data.zoom > 1.0,
            );
            zoom.slider([49, 10, 246, 45], SliderKind::Zoom, (data.zoom - 1.0) / 3.0);
            cards.push(zoom);
        }
    }
}

impl Card {
    pub fn store_update(viewport: RECT, scale: f32) -> Option<Self> {
        let px = |dp: i32| (dp as f32 * scale).round() as i32;
        let w = px(366);
        let h = px(54);
        if viewport.right - viewport.left < w || viewport.bottom - viewport.top < h + px(8) {
            return None;
        }
        let x = viewport.left + (viewport.right - viewport.left - w) / 2;
        let mut card = Self::new(x, viewport.top + px(8), w, h, scale, None);
        card.label(
            [9, 6, 181, 42],
            tr!("È disponibile un aggiornamento", "An update is available"),
            13,
            false,
        );
        card.button(
            [192, 6, 82, 42],
            tr!("Aggiorna", "Update"),
            Event::OpenStore,
            true,
        );
        card.button(
            [280, 6, 78, 42],
            tr!("Più tardi", "Later"),
            Event::DismissStoreUpdate,
            false,
        );
        Some(card)
    }

    pub fn new(x: i32, y: i32, w: i32, h: i32, scale: f32, floating: Option<usize>) -> Self {
        Self {
            rect: RECT {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            },
            header: RECT {
                left: x,
                top: y,
                right: x + w,
                bottom: y + (43.0 * scale) as i32,
            },
            floating,
            labels: Vec::new(),
            buttons: Vec::new(),
            sliders: Vec::new(),
            separators: Vec::new(),
            strokes: Vec::new(),
            section: None,
            opacity: 1.0,
            dock: false,
            drawer: false,
            interactive: true,
            favorite_clip: None,
            favorite_labels_start: usize::MAX,
            favorite_bar: None,
            meter: None,
            scale,
        }
    }

    fn separator(&mut self, x: i32) {
        self.separators.push(self.rect([x, 12, 2, 29]));
    }

    pub fn grab(&self, id: u32, at: POINT) -> Grab {
        let event = self
            .buttons
            .iter()
            .rev()
            .find(|b| {
                b.enabled && contains(b.rect, at)
                    && (!b.event.is_favorite_row() || self.favorite_clip.is_some_and(|clip| contains(clip, at)))
            })
            .map(|b| b.event);
        let slider = self.sliders.iter().copied().find(|s| contains(s.rect, at));
        let drag = self
            .floating
            .filter(|_| contains(self.header, at) && event.is_none())
            .map(|slot| {
                (
                    slot,
                    at,
                    POINT {
                        x: self.rect.left,
                        y: self.rect.top,
                    },
                )
            });
        Grab {
            id,
            origin: at,
            event,
            drag,
            slider,
            favorite_scroll: None,
            favorite_bar: None,
        }
    }

    fn rect(&self, [x, y, w, h]: [i32; 4]) -> RECT {
        let px = |x: i32| (x as f32 * self.scale).round() as i32;
        RECT {
            left: self.rect.left + px(x),
            top: self.rect.top + px(y),
            right: (self.rect.left + px(x + w)).min(self.rect.right),
            bottom: (self.rect.top + px(y + h)).min(self.rect.bottom),
        }
    }

    fn label(&mut self, rect: [i32; 4], text: impl Into<String>, size: i32, muted: bool) {
        let rect = self.rect(rect);
        if rect.top < rect.bottom {
            self.labels.push(Label {
                rect,
                text: text.into(),
                size,
                muted,
                dark: false,
            });
        }
    }

    pub fn button(&mut self, rect: [i32; 4], text: impl Into<String>, event: Event, active: bool) {
        let rect = self.rect(rect);
        if rect.top < rect.bottom {
            self.buttons.push(Button {
                rect,
                text: text.into(),
                event,
                active,
                color: None,
                icon: None,
                enabled: true,
            });
        }
    }

    fn icon(&mut self, rect: [i32; 4], icon: Glyph, event: Event, enabled: bool) {
        self.menu_item(rect, "", icon, event, enabled);
    }

    pub fn menu_item(
        &mut self,
        rect: [i32; 4],
        text: &str,
        icon: Glyph,
        event: Event,
        enabled: bool,
    ) {
        let i = self.buttons.len();
        self.button(rect, text, event, false);
        if let Some(button) = self.buttons.get_mut(i) {
            button.icon = Some(icon);
            button.enabled = enabled;
        }
    }

    fn slider(&mut self, rect: [i32; 4], kind: SliderKind, ratio: f32) {
        let rect = self.rect(rect);
        if rect.top < rect.bottom && rect.left < rect.right {
            self.sliders.push(Slider {
                rect,
                kind,
                ratio: ratio.clamp(0.0, 1.0),
            });
        }
    }

    fn pair(&mut self, y: i32, a: (&str, Event), b: (&str, Event)) {
        self.button([12, y, 144, 39], a.0, a.1, false);
        self.button([164, y, 144, 39], b.0, b.1, false);
    }

    fn action(&mut self, rect: [i32; 4], text: &str, event: Event, active: bool, enabled: bool) {
        let i = self.buttons.len();
        self.button(rect, text, event, active);
        if let Some(button) = self.buttons.get_mut(i) {
            button.enabled = enabled;
        }
    }

    fn bookmarks(&mut self, data: &Model<'_>) {
        use Event as E;
        self.label([11, 46, 188, 34], data.bookmark_status, 12, true);
        self.action(
            [268, 46, 105, 34],
            tr!("+ Aggiungi", "+ Add"),
            E::BookmarkAdd,
            true,
            data.can_add_bookmark,
        );
        for (row, item) in data.bookmarks.iter().enumerate() {
            let y = 88 + row as i32 * 51;
            let indent = item.depth.min(4) as i32 * 9;
            if item.branch {
                self.icon(
                    [7 + indent, y, 29, 42],
                    if item.collapsed {
                        Glyph::ChevronRight
                    } else {
                        Glyph::ChevronDown
                    },
                    E::BookmarkCollapse(item.index),
                    true,
                );
            }
            let left = 12 + indent + if item.branch { 25 } else { 0 };
            self.button(
                [left, y, 207 - left, 42],
                format!("{} {}", item.title, item.detail),
                E::BookmarkSelect(item.index),
                data.selected_bookmark == Some(item.index),
            );
            self.icon(
                [212, y, 38, 42],
                Glyph::Pencil,
                E::BookmarkEdit(item.index),
                true,
            );
            self.icon(
                [253, y, 38, 42],
                Glyph::Plus,
                E::BookmarkChild(item.index),
                true,
            );
            self.icon(
                [294, y, 38, 42],
                Glyph::Trash,
                E::BookmarkDelete(item.index),
                true,
            );
            self.icon(
                [335, y, 38, 42],
                Glyph::Move,
                E::BookmarkDrag(item.index),
                true,
            );
        }
        if data.bookmark_more.iter().any(|&more| more) {
            self.icon(
                [89, 453, 61, 43],
                Glyph::ChevronUp,
                E::BookmarkScroll(-1),
                data.bookmark_more[0],
            );
            self.icon(
                [171, 453, 61, 43],
                Glyph::ChevronDown,
                E::BookmarkScroll(1),
                data.bookmark_more[1],
            );
        }
    }

    fn preferences(&mut self, data: &Model<'_>) {
        use Event as E;
        self.button(
            [16, 59, 288, 48],
            if data.tap {
                tr!("Tocco ai bordi     Attivo", "Edge tap     On")
            } else {
                tr!("Tocco ai bordi     Disattivo", "Edge tap     Off")
            },
            E::ToggleTap,
            data.tap,
        );
        self.button(
            [16, 117, 288, 48],
            tr!(
                "Imposta come app predefinita per i PDF…",
                "Set as default PDF app…"
            ),
            E::DefaultPdf,
            false,
        );
        self.label([18, 175, 282, 28], tr!("Lingua", "Language"), 15, true);
        for (index, (preference, label)) in [
            (LanguagePreference::System, tr!("Automatica", "Automatic")),
            (LanguagePreference::Italian, "Italiano"),
            (LanguagePreference::English, "English"),
        ]
        .into_iter()
        .enumerate()
        {
            self.button(
                [16 + index as i32 * 98, 207, 92, 40],
                label,
                E::Language(preference),
                data.language == preference,
            );
        }

    }

    fn about(&mut self) {
        self.label(
            [12, 54, 296, 32],
            trf!("Versione {}", "Version {}", env!("CARGO_PKG_VERSION")),
            15,
            true,
        );
        self.label(
            [12, 89, 296, 38],
            tr!(
                "Sviluppato da Emanuele Campilongo",
                "Developed by Emanuele Campilongo"
            ),
            16,
            false,
        );
        self.button(
            [16, 134, 288, 44],
            tr!("Sito di ScoreDraw", "ScoreDraw website"),
            Event::Website("https://emanuelecampilongo.com/software/scoredraw/"),
            true,
        );
        self.button(
            [16, 186, 288, 44],
            tr!("Codice sorgente su GitHub", "Source code on GitHub"),
            Event::Website("https://github.com/emanuelecampilongo/scoredraw"),
            false,
        );
        self.pair(
            242,
            (
                tr!("Sito dell’autore", "Author’s website"),
                Event::Website("https://emanuelecampilongo.com"),
            ),
            (
                tr!("Crediti e licenze", "Credits and licenses"),
                Event::Licenses,
            ),
        );
    }

    fn style(&mut self, state: &Overlay, data: &Model<'_>) {
        for (i, color) in SWATCHES.into_iter().enumerate() {
            let len = self.buttons.len();
            self.button(
                [16 + (i % 6) as i32 * 49, 58 + (i / 6) as i32 * 48, 43, 42],
                "",
                Event::Color(color),
                data.style.color == color,
            );
            if let Some(button) = self.buttons.get_mut(len) {
                button.color = Some(COLORREF(color));
            }
        }
        if data.tool != Tool::Rectangle {
            self.action(
                [16, 176, 43, 40],
                "−",
                Event::Width(-1),
                false,
                data.style.width > 0.0005,
            );
            self.label(
                [63, 144, 194, 30],
                trf!(
                    "Spessore {:.1} ‰",
                    "Thickness {:.1} ‰",
                    data.style.width * 1000.0
                ),
                16,
                false,
            );
            self.slider(
                [63, 173, 194, 46],
                SliderKind::Width,
                width_ratio(data.style.width),
            );
            self.action(
                [261, 176, 43, 40],
                "+",
                Event::Width(1),
                false,
                data.style.width < 0.06,
            );
        }
        let y = if data.tool == Tool::Rectangle {
            144
        } else {
            218
        };
        self.action(
            [16, y + 32, 43, 40],
            "−",
            Event::Opacity(-1),
            false,
            data.style.opacity > 0,
        );
        self.label(
            [63, y, 194, 31],
            trf!(
                "Opacità {}%",
                "Opacity {}%",
                (f32::from(data.style.opacity) * 100.0 / 255.0).round()
            ),
            16,
            false,
        );
        self.slider(
            [63, y + 29, 194, 46],
            SliderKind::Opacity,
            f32::from(data.style.opacity) / 255.0,
        );
        self.action(
            [261, y + 32, 43, 40],
            "+",
            Event::Opacity(1),
            false,
            data.style.opacity < 255,
        );
        self.button(
            [16, y + 75, 288, 33],
            tr!("Colore personalizzato…", "Custom color…"),
            Event::CustomColor,
            false,
        );
        if let Some(favorites) = data.favorites {
            let bottom = favorite_bottom(favorites.len());
            self.section = Some(self.rect([12, 345, 296, bottom - 345]));
            self.label([20, 352, 90, 32], tr!("Preferiti", "Favorites"), 16, false);
            self.button(
                [113, 352, 185, 32],
                tr!("Aggiungi ai preferiti", "Add to favorites"),
                Event::AddFavorite,
                false,
            );
            if favorites.is_empty() {
                self.label([23, 393, 274, 32], tr!("Nessun preferito", "No favorites yet"), 13, true);
                return;
            }
            let height = favorites.len().min(FAVORITE_ROWS) as i32 * FAVORITE_ROW;
            let scroll = state.favorite_scroll.clamp(0.0, favorite_max_scroll(favorites.len()));
            let clip = self.rect([18, 393, 286, height]);
            self.favorite_clip = Some(clip);
            self.favorite_labels_start = self.labels.len();
            let start = scroll as usize / FAVORITE_ROW as usize;
            for (index, &style) in favorites.iter().enumerate().skip(start).take(FAVORITE_ROWS + 1) {
                let y = 393 + index as i32 * FAVORITE_ROW - scroll.round() as i32;
                if y >= 393 + height { break; }
                self.button([22, y, 234, 42], "", Event::LoadFavorite(index), style == data.style);
                self.icon([260, y, 38, 42], Glyph::Trash, Event::DeleteFavorite(index), true);
                self.strokes.push((self.rect([32, y + 8, 58, 26]), style));
                self.label([93, y + 2, 160, 18], trf!("Preferito {}", "Favorite {}", index + 1), 13, false);
                self.label(
                    [92, y + 20, 162, 17],
                    trf!("Spess. {:.1}‰ · Opacità {:.0}%", "Width {:.1}‰ · Opacity {:.0}%",
                        style.width * 1000.0, f32::from(style.opacity) * 100.0 / 255.0),
                    11,
                    style != data.style,
                );
                if style == data.style {
                    for label in self.labels.iter_mut().rev().take(2) { label.dark = true; }
                }
            }
            self.favorite_bar = Scrollbar::new(
                self.rect([298, 393, 11, height]), favorite_max_scroll(favorites.len()),
                scroll, (20.0 * self.scale).round() as i32, self.scale,
            );
        }
    }
}

fn favorite_bottom(count: usize) -> i32 {
    393 + count.clamp(1, FAVORITE_ROWS) as i32 * FAVORITE_ROW
}

pub fn favorite_max_scroll(count: usize) -> f32 {
    count.saturating_sub(FAVORITE_ROWS) as f32 * FAVORITE_ROW as f32
}

impl Event {
    pub fn is_favorite_row(self) -> bool {
        matches!(self, Self::LoadFavorite(_) | Self::DeleteFavorite(_))
    }
}

pub fn width_ratio(width: f32) -> f32 {
    ((width - 0.0005) / 0.0595).clamp(0.0, 1.0).sqrt()
}

pub fn width_from_ratio(ratio: f32) -> f32 {
    (5.0 + 595.0 * ratio.powi(2)).round() / 10_000.0
}

fn floating_bounds(
    area: RECT,
    obstacles: &[RECT],
    preferred: (i32, i32),
    (width, height): (i32, i32),
    scale: f32,
) -> Option<(RECT, f32)> {
    let gap = (7.0 * scale).round() as i32;
    for percent in [100, 90, 80, 70, 60, 50, 40, 30] {
        let scale = scale * percent as f32 / 100.0;
        let w = (width as f32 * scale).round() as i32;
        let h = (height as f32 * scale).round() as i32;
        let max_x = area.right - w;
        let max_y = area.bottom - h;
        if max_x < area.left || max_y < area.top { continue; }
        let mut xs = vec![area.left, max_x, preferred.0.clamp(area.left, max_x)];
        let mut ys = vec![area.top, max_y, preferred.1.clamp(area.top, max_y)];
        for obstacle in obstacles {
            xs.extend([obstacle.left - w - gap, obstacle.right + gap]);
            ys.extend([obstacle.top - h - gap, obstacle.bottom + gap]);
        }
        if let Some(rect) = xs.iter().flat_map(|&x| ys.iter().map(move |&y| (x, y)))
            .filter(|&(x, y)| (area.left..=max_x).contains(&x) && (area.top..=max_y).contains(&y))
            .map(|(left, top)| RECT { left, top, right: left + w, bottom: top + h })
            .filter(|r| obstacles.iter().all(|o| r.right <= o.left || r.left >= o.right || r.bottom <= o.top || r.top >= o.bottom))
            .min_by_key(|r| i64::from(r.left - preferred.0).pow(2) + i64::from(r.top - preferred.1).pow(2))
        {
            return Some((rect, scale));
        }
    }
    None
}

pub fn contains(rect: RECT, at: POINT) -> bool {
    at.x >= rect.left && at.x < rect.right && at.y >= rect.top && at.y < rect.bottom
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_floating_card_moves_clear_of_the_radial_and_other_panels() {
        let area = RECT { left: 0, top: 0, right: 420, bottom: 520 };
        let drawer = RECT { right: 220, ..area };
        let radial = RECT { left: 290, top: 340, right: 410, bottom: 470 };
        let (first, _) = floating_bounds(area, &[drawer, radial], (280, 330), (254, 78), 1.0).unwrap();
        let (second, _) = floating_bounds(area, &[drawer, radial, first], (280, 330), (254, 78), 1.0).unwrap();
        let separate = |a: RECT, b: RECT| {
            a.left >= b.right || a.right <= b.left || a.top >= b.bottom || a.bottom <= b.top
        };
        for obstacle in [drawer, radial, first] {
            assert!(separate(second, obstacle));
        }
    }
}
