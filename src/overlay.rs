use crate::data::Style;
use crate::icons::Glyph;
use crate::ink::Tool;
use crate::language::LanguagePreference;
use crate::radial::Command;
use crate::render_queue::ScoreInfo;
use std::time::Instant;
use windows::Win32::Foundation::{COLORREF, POINT, RECT};
pub const BOOKMARK_ROWS: usize = 7;
pub const PAGE_SCORE_ROWS: usize = 5;

pub const SWATCHES: [u32; 12] = [
    0x392b22, 0xfdfaf7, 0xef9343, 0x5951e1, 0xc990ae, 0xfea39c, 0x88c148, 0x92e7af, 0x51e1ff,
    0x62c9ff, 0x4569f6, 0xb974ed,
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Sheet {
    Menu,
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
    PageDigit(u8),
    PageErase,
    PageGo,
    PageScores,
    PageScore(usize),
    PageScoreScroll(i32),
    BookmarkSelect(usize),
    BookmarkScroll(i32),
    BookmarkAdd,
    BookmarkEdit(usize),
    BookmarkDelete(usize),
    BookmarkChild(usize),
    BookmarkCollapse(usize),
    BookmarkDrag(usize),
    Finalize,
    DefaultPdf,
    OpenStore,
    DismissStoreUpdate,
    AudioToggle(usize),
    AudioClose(usize),
    Bpm(i16),
    Beats(i8),
    Color(u32),
    Width(i32),
    Opacity(i32),
    Advanced,
    ToggleZoom,
    ZoomStep(i32),
    Turn(i32),
    About,
    Feedback,
    DeleteShape,
    SavePreset(u8),
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
    pub zoom: f32,
    pub tap: bool,
    pub language: LanguagePreference,
    pub page: String,
    pub page_range: String,
    pub page_target_valid: bool,
    pub page_can_erase: bool,
    pub scores: &'a [ScoreInfo],
    pub page_loaded: bool,
    pub page_indicator: String,
    pub undo: bool,
    pub redo: bool,
    pub can_finalize: bool,
    pub can_turn: [bool; 2],
    pub can_add_bookmark: bool,
    pub bpm: u16,
    pub beats: u8,
    pub running: [bool; 2],
    pub audio_status: [&'a str; 2],
    pub pitch: Option<f32>,
    pub pulse: Option<(u8, f32)>,
    pub tool: Tool,
    pub style: Style,
    pub preset_editor: bool,
    pub preset_saved: Option<u8>,
    pub bookmarks: &'a [BookmarkRow<'a>],
    pub selected_bookmark: Option<usize>,
    pub bookmark_status: &'a str,
    pub bookmark_more: [bool; 2],
}

#[derive(Default)]
pub struct Overlay {
    pub drawer: bool,
    pub sheet: Option<Sheet>,
    pub page_input: String,
    pub page_replace: bool,
    pub page_score: usize,
    pub page_scores_open: bool,
    pub page_score_offset: usize,
    pub bookmark_offset: usize,
    pub selected_bookmark: Option<usize>,
    last_frame: Option<Instant>,
    progress: f32,
    floating: [Floating; 2],
    pub grab: Option<Grab>,
    pub utilities: bool,
    pub zoom_slider: bool,
}

#[derive(Default)]
struct Floating {
    visible: bool,
    anchor: Option<(f32, f32)>,
}

#[derive(Clone, Copy)]
pub struct Grab {
    pub id: u32,
    pub event: Option<Event>,
    pub slider: Option<Slider>,
    pub origin: POINT,
    drag: Option<(usize, POINT, POINT)>,
}

impl Grab {
    pub fn dismiss(id: u32, origin: POINT) -> Self {
        Self {
            id,
            origin,
            event: None,
            slider: None,
            drag: None,
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
        self.sheet = Some(sheet);
        self.last_frame = Some(Instant::now());
        if sheet == Sheet::Page {
            self.page_input.clear();
            self.page_scores_open = false;
        }
    }

    pub fn close(&mut self) {
        self.drawer = false;
        self.last_frame = Some(Instant::now());
    }

    pub fn toggle_audio(&mut self, slot: usize) -> bool {
        let panel = &mut self.floating[slot];
        panel.visible = !panel.visible;
        panel.visible
    }

    pub fn audio_visible(&self) -> bool {
        self.floating.iter().any(|panel| panel.visible)
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
        self.progress != old
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

    pub fn cards(&self, viewport: RECT, scale: f32, data: &Model<'_>) -> Vec<Card> {
        let mut cards = Vec::new();
        let px = |x: i32| (x as f32 * scale).round() as i32;
        let vw = viewport.right - viewport.left;
        let vh = viewport.bottom - viewport.top;
        if vw < px(300) || vh < px(160) {
            return cards;
        }
        if data.page_loaded && self.utilities {
            self.utilities_panel(viewport, scale, data, &mut cards);
        }
        for (slot, panel) in self
            .floating
            .iter()
            .enumerate()
            .filter(|(_, panel)| panel.visible)
        {
            let w = px(298);
            let h = px(if slot == 0 { 310 } else { 223 });
            let (ax, ay) = panel.anchor.map_or(
                (
                    viewport.right - w - px(18),
                    viewport.top + px(if slot == 0 { 14 } else { 337 }),
                ),
                |(x, y)| {
                    (
                        viewport.left + (x * vw as f32) as i32,
                        viewport.top + (y * vh as f32) as i32,
                    )
                },
            );
            let x = ax.clamp(viewport.left, viewport.right - w);
            let y = ay.clamp(viewport.top, (viewport.bottom - h).max(viewport.top));
            let mut card = Card::new(x, y, w, h, scale, Some(slot));
            let title = if slot == 0 {
                tr!("Metronomo", "Metronome")
            } else {
                tr!("Accordatore", "Tuner")
            };
            card.label([14, 4, 222, 38], title, 18, false);
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
                (
                    (card.rect([18, 44, 261, 85]), Meter::Pitch(data.pitch)),
                    132,
                )
            };
            card.meter = Some(meter);
            card.label([13, label_y, 272, 36], data.audio_status[slot], 13, true);
            let button = match (slot, data.running[slot]) {
                (_, true) => tr!("Ferma", "Stop"),
                (0, false) => tr!("Avvia", "Start"),
                _ => tr!("Avvia microfono", "Start microphone"),
            };
            card.button(
                [13, label_y + 37, 272, 32],
                button,
                Event::AudioToggle(slot),
                !data.running[slot],
            );
            cards.push(card);
        }
        if self.progress == 0.0 {
            return cards;
        }
        let sheet = self.sheet.unwrap_or(Sheet::Menu);
        let height_dp = match sheet {
            Sheet::Menu => 451,
            Sheet::Page => {
                if data.scores.len() > 1 {
                    395
                } else {
                    345
                }
            }
            Sheet::Bookmarks => 508,
            Sheet::Style => {
                if data.preset_editor {
                    400
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
            .min(vw as f32 / width_dp as f32);
        let width = (width_dp as f32 * scale).round() as i32;
        let position = 1.0 - (1.0 - self.progress).powi(3);
        let x = viewport.left + ((position - 1.0) * width as f32).round() as i32;
        let height = (height_dp as f32 * scale).round() as i32;
        let mut card = Card::new(x, viewport.top, width, height, scale, None);
        let style_title;
        let title = match sheet {
            Sheet::Menu => "ScoreDraw",
            Sheet::Page => tr!("Vai a pagina", "Go to page"),
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
        if sheet != Sheet::Menu {
            card.icon(
                [12, 9, 36, 32],
                Glyph::ChevronLeft,
                Event::Sheet(Sheet::Menu),
                true,
            );
        }
        card.icon(
            [width_dp - 46, 9, 34, 32],
            Glyph::X,
            Event::CloseDrawer,
            true,
        );
        match sheet {
            Sheet::Menu => card.menu(data),
            Sheet::Page => card.pages(self, data),
            Sheet::Bookmarks => card.bookmarks(data),
            Sheet::Style => card.style(data),
            Sheet::Preferences => card.preferences(data),
            Sheet::About => card.about(),
        }
        cards.push(card);
        cards
    }

    fn utilities_panel(&self, viewport: RECT, scale: f32, data: &Model<'_>, cards: &mut Vec<Card>) {
        let width = if data.fullscreen { 558 } else { 506 };
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
        dock.icon(
            [452, 6, 46, 41],
            if data.fullscreen {
                Glyph::Shrink
            } else {
                Glyph::Expand
            },
            Event::Command(Command::Fullscreen),
            true,
        );
        if data.fullscreen {
            dock.icon(
                [504, 6, 46, 41],
                Glyph::PanelLeft,
                Event::Command(Command::Panel),
                true,
            );
        }
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
            .find(|b| b.enabled && contains(b.rect, at))
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

    fn menu(&mut self, data: &Model<'_>) {
        use Command as C;
        use Event as E;
        self.pair(
            55,
            (tr!("Apri PDF", "Open PDF"), E::Command(C::Open)),
            (tr!("Scalette", "Setlists"), E::Command(C::Library)),
        );
        if data.page_loaded {
            self.pair(
                102,
                (tr!("Vai a pagina", "Go to page"), E::Sheet(Sheet::Page)),
                (tr!("Segnalibri", "Bookmarks"), E::Sheet(Sheet::Bookmarks)),
            );
        }
        self.pair(
            149,
            (tr!("Metronomo", "Metronome"), E::Command(C::Metronome)),
            (tr!("Accordatore", "Tuner"), E::Command(C::Tuner)),
        );
        if data.page_loaded {
            self.action(
                [12, 196, 144, 39],
                tr!("Annulla", "Cancel"),
                E::Command(C::Undo),
                false,
                data.undo,
            );
            self.action(
                [164, 196, 144, 39],
                tr!("Ripeti", "Redo"),
                E::Command(C::Redo),
                false,
                data.redo,
            );
            self.action(
                [12, 243, 296, 39],
                tr!("Rendi modifiche permanenti…", "Make changes permanent…"),
                E::Finalize,
                false,
                data.can_finalize,
            );
        }
        self.button(
            [12, 290, 144, 39],
            if data.fullscreen {
                tr!("Torna in finestra", "Exit full screen")
            } else {
                tr!("Schermo intero", "Full screen")
            },
            E::Command(C::Fullscreen),
            false,
        );
        self.action(
            [164, 290, 144, 39],
            tr!("Adatta pagina", "Fit page"),
            E::ResetZoom,
            false,
            data.zoom > 1.0,
        );
        self.button(
            [12, 337, 296, 39],
            tr!("Preferenze", "Settings"),
            E::Advanced,
            false,
        );
        self.label(
            [12, 379, 296, 28],
            format!("Zoom {}%", (data.zoom * 100.0).round()),
            13,
            true,
        );
        self.button(
            [12, 404, 296, 36],
            tr!("Informazioni", "About"),
            E::About,
            false,
        );
    }

    fn pages(&mut self, state: &Overlay, data: &Model<'_>) {
        let mut offset = 0;
        if data.scores.len() > 1 {
            let selected = data
                .scores
                .get(state.page_score)
                .map_or(tr!("Scegli spartito", "Choose score"), |score| {
                    score.title.as_str()
                });
            self.menu_item(
                [20, 49, 280, 42],
                selected,
                if state.page_scores_open {
                    Glyph::ChevronUp
                } else {
                    Glyph::ChevronDown
                },
                Event::PageScores,
                true,
            );
            if state.page_scores_open {
                for (row, (index, score)) in data
                    .scores
                    .iter()
                    .enumerate()
                    .skip(state.page_score_offset)
                    .take(PAGE_SCORE_ROWS)
                    .enumerate()
                {
                    self.button(
                        [20, 99 + row as i32 * 47, 280, 42],
                        format!("{}. {}", index + 1, score.title),
                        Event::PageScore(index),
                        state.page_score == index,
                    );
                }
                let more = state.page_score_offset + PAGE_SCORE_ROWS < data.scores.len();
                if more || state.page_score_offset > 0 {
                    self.icon(
                        [19, 339, 134, 42],
                        Glyph::ChevronUp,
                        Event::PageScoreScroll(-1),
                        state.page_score_offset > 0,
                    );
                    self.icon(
                        [167, 339, 134, 42],
                        Glyph::ChevronDown,
                        Event::PageScoreScroll(1),
                        more,
                    );
                }
                return;
            }
            offset = 50;
        }
        self.label([12, 50 + offset, 296, 33], &data.page, 25, false);
        self.label([12, 81 + offset, 296, 23], &data.page_range, 13, true);
        for (i, digit) in [1, 2, 3, 4, 5, 6, 7, 8, 9, 0].into_iter().enumerate() {
            self.button(
                [
                    20 + (i % 3) as i32 * 96,
                    111 + offset + (i / 3) as i32 * 54,
                    88,
                    46,
                ],
                digit.to_string(),
                Event::PageDigit(digit),
                false,
            );
        }
        self.action(
            [116, 273 + offset, 88, 46],
            "⌫",
            Event::PageErase,
            false,
            data.page_can_erase,
        );
        self.action(
            [212, 273 + offset, 88, 46],
            tr!("Vai", "Go"),
            Event::PageGo,
            true,
            data.page_target_valid,
        );
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

    fn style(&mut self, data: &Model<'_>) {
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
            true,
        );
        if data.preset_editor {
            self.label(
                [12, 328, 296, 25],
                tr!("Salva come preferito", "Save as favorite"),
                13,
                true,
            );
            for slot in 0..3 {
                self.button(
                    [17 + i32::from(slot) * 96, 354, 89, 39],
                    if data.preset_saved == Some(slot) {
                        tr!("✓ Salvato", "✓ Saved").into()
                    } else {
                        format!("Slot {}", slot + 1)
                    },
                    Event::SavePreset(slot),
                    false,
                );
            }
        }
    }
}

pub fn width_ratio(width: f32) -> f32 {
    ((width - 0.0005) / 0.0595).clamp(0.0, 1.0).sqrt()
}

pub fn width_from_ratio(ratio: f32) -> f32 {
    (5.0 + 595.0 * ratio.powi(2)).round() / 10_000.0
}

pub fn contains(rect: RECT, at: POINT) -> bool {
    at.x >= rect.left && at.x < rect.right && at.y >= rect.top && at.y < rect.bottom
}
