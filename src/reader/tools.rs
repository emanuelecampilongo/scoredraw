use super::*;
use crate::data::Style;
use crate::overlay::{Slider, SliderKind};

impl Reader {
    pub(super) fn commit_settings(&mut self, hwnd: HWND, settings: Settings) -> bool {
        if let Some(store) = &self.store
            && let Err(error) = store.save_settings(&settings)
        {
            self.notify(
                hwnd,
                trf!(
                    "Salvataggio preferenze non riuscito · {error}",
                    "Couldn’t save settings · {error}"
                ),
            );
            return false;
        }
        self.settings = settings;
        true
    }

    fn select_tool(&mut self, hwnd: HWND, tool: Tool) {
        self.tool = tool;
        if self.settings.active_tool != tool {
            self.commit_settings(
                hwnd,
                Settings {
                    active_tool: tool,
                    ..self.settings
                },
            );
        }
    }

    pub(super) fn command(&mut self, hwnd: HWND, command: Command) {
        match command {
            Command::Tool(tool) => {
                self.select_tool(hwnd, tool);
                if self.overlay.drawer && self.overlay.sheet == Some(Sheet::Style) {
                    if !tool.has_options() {
                        self.close_drawer(hwnd);
                    }
                }
            }
            Command::Settings if self.total > 0 && self.tool.has_options() => {
                self.open_sheet(hwnd, Sheet::Style);
            }
            Command::Settings => {}
            Command::Undo => self.undo_ink(hwnd),
            Command::Redo => self.redo_ink(hwnd),
            Command::Fullscreen => {
                self.close_drawer(hwnd);
                self.toggle_fullscreen(hwnd);
            }
            Command::Open => request_file_picker(hwnd),
            Command::Library => post(hwnd, OPEN_LIBRARY),
            Command::Metronome | Command::Tuner if self.visible.is_none() => return,
            Command::Metronome | Command::Tuner => {
                let slot = usize::from(command == Command::Tuner);
                if !self.overlay.toggle_audio(slot) {
                    self.sound.stop(slot);
                    if slot == 0 {
                        self.tempo_taps.clear();
                    }
                }
                self.close_drawer(hwnd);
            }
            Command::ToolOptions(tool) if self.total > 0 => {
                self.select_tool(hwnd, tool);
                self.open_sheet(hwnd, Sheet::Style);
            }
            Command::ToolOptions(_) | Command::Shapes | Command::Back => {}
        }
        repaint(hwnd);
    }

    pub(super) fn open_sheet(&mut self, hwnd: HWND, sheet: Sheet) {
        if self.total == 0 && matches!(sheet, Sheet::Page | Sheet::Bookmarks) {
            return;
        }
        self.hide_utilities(hwnd);
        self.chrome_menu = None;
        if self.page_grid_open() { self.leave_page_grid(); }
        self.overlay.open(sheet);
        if sheet == Sheet::Page { self.start_page_grid(hwnd); }
        if sheet == Sheet::Bookmarks {
            self.load_bookmarks(hwnd);
        }
        self.animate(hwnd);
        repaint(hwnd);
    }

    pub(super) fn close_drawer(&mut self, hwnd: HWND) {
        if self.page_grid_open() { self.leave_page_grid(); }
        self.overlay.close();
        self.reveal_utilities(hwnd);
        self.animate(hwnd);
        repaint(hwnd);
    }

    pub(super) fn cancel_overlay(&mut self, hwnd: HWND, id: u32) {
        if self.overlay.grab.is_some_and(|grab| grab.id == id) {
            self.overlay.grab = None;
            self.bookmark_drag = None;
            self.bookmark_scroll_y = None;
            let _ = unsafe { KillTimer(Some(hwnd), BOOKMARK_DRAG_TIMER) };
            self.finish_slider(hwnd);
        }
    }

    pub(super) fn hide_utilities(&mut self, hwnd: HWND) {
        let _ = unsafe { KillTimer(Some(hwnd), UTILITIES_TIMER) };
        self.utilities_activity = None;
        if self.overlay.utilities {
            self.overlay.utilities = false;
            self.overlay.zoom_slider = false;
            self.repaint_utilities(hwnd);
            self.animate(hwnd);
        }
    }

    pub(super) fn reveal_utilities(&mut self, hwnd: HWND) {
        if self.total == 0 || self.overlay.drawer {
            return;
        }
        self.utilities_activity = Some(Instant::now());
        if !self.overlay.utilities {
            self.overlay.utilities = true;
            self.animate(hwnd);
            self.repaint_utilities(hwnd);
            unsafe {
                SetTimer(
                    Some(hwnd),
                    UTILITIES_TIMER,
                    UTILITIES_IDLE.as_millis() as u32,
                    None,
                )
            };
        }
    }

    pub(super) fn check_utilities_idle(&mut self, hwnd: HWND) {
        let remaining = self.utilities_activity.map_or(Duration::ZERO, |activity| {
            UTILITIES_IDLE.saturating_sub(activity.elapsed())
        });
        if remaining.is_zero() && self.overlay.grab.is_none() {
            self.hide_utilities(hwnd);
        } else {
            let interval = remaining.as_millis().max(200) as u32;
            unsafe { SetTimer(Some(hwnd), UTILITIES_TIMER, interval, None) };
        }
    }

    pub(super) fn repaint_utilities(&self, hwnd: HWND) {
        let layout = Layout::new(hwnd, self.fullscreen.is_some());
        let middle = (layout.viewport.left + layout.viewport.right) / 2;
        repaint_rect(
            hwnd,
            RECT {
                left: (middle - layout.px(314)).max(layout.viewport.left),
                right: (middle + layout.px(314)).min(layout.viewport.right),
                top: (layout.viewport.bottom - layout.px(141)).max(layout.viewport.top),
                bottom: layout.viewport.bottom,
            },
        );
    }

    pub(super) fn overlay_input(&mut self, hwnd: HWND, msg: u32, id: u32, at: POINT) -> bool {
        let layout = Layout::new(hwnd, self.fullscreen.is_some());
        match msg {
            WM_POINTERDOWN => {
                if let Some(grab) = self.overlay.grab {
                    return grab.id == id;
                }
                let cards = self.cards(&layout);
                let Some(card) = cards.iter().rev().find(|card| (!card.dock || self.overlay.utilities) && card.opacity > 0.04 && inside(card.rect, at)) else {
                    if self.chrome_menu.take().is_some() {
                        self.overlay.grab = Some(overlay::Grab::dismiss(id, at));
                        repaint(hwnd);
                        return true;
                    }
                    return false;
                };
                if !card.interactive {
                    self.overlay.grab = Some(overlay::Grab::dismiss(id, at));
                    return true;
                }
                let mut grab = card.grab(id, at);
                if let Some(bar) = card.favorite_bar && inside(bar.track, at) {
                    let offset = bar.grab(at.y);
                    grab = overlay::Grab::dismiss(id, at);
                    grab.favorite_bar = Some((bar, offset));
                    if !(bar.thumb.top..bar.thumb.bottom).contains(&at.y) {
                        self.set_favorite_scroll(bar.scroll(at.y, offset));
                    }
                } else if card.favorite_clip.is_some_and(|clip| inside(clip, at)) {
                    grab.favorite_scroll = Some((self.overlay.favorite_scroll, card.scale));
                }
                if self.chrome_menu.is_some()
                    && self
                        .chrome_card(&layout)
                        .is_some_and(|menu| !inside(menu.rect, at))
                {
                    self.chrome_menu = None;
                    grab = overlay::Grab::dismiss(id, at);
                }
                self.overlay.grab = Some(grab);
                if matches!(grab.event, Some(Event::BookmarkDrag(_))) {
                    unsafe { SetTimer(Some(hwnd), BOOKMARK_DRAG_TIMER, 170, None) };
                }
                if let Some(slider) = grab.slider {
                    self.slider_original =
                        (slider.kind != SliderKind::Zoom).then_some(self.settings);
                    self.preview_slider(hwnd, slider, at.x);
                }
            }
            WM_POINTERUPDATE if self.overlay.grab.is_some_and(|grab| grab.id == id) => {
                if let Some(grab) = self.overlay.grab {
                    let dy = at.y - grab.origin.y;
                    if let Some((bar, offset)) = grab.favorite_bar {
                        self.set_favorite_scroll(bar.scroll(at.y, offset));
                    } else if let Some((start, scale)) = grab.favorite_scroll
                        && (dy.abs() >= layout.px(5) || self.bookmark_scroll_y.is_some())
                    {
                        self.set_favorite_scroll(start - dy as f32 / scale);
                        self.bookmark_scroll_y = Some(at.y);
                    }
                    match grab.event {
                        Some(Event::BookmarkDrag(_)) => self.bookmark_drag = Some(at),
                        Some(Event::BookmarkSelect(_))
                            if dy.abs() > layout.px(24) || self.bookmark_scroll_y.is_some() =>
                        {
                            let origin = self.bookmark_scroll_y.unwrap_or(grab.origin.y);
                            let step = (origin - at.y) / layout.px(38).max(1);
                            if step != 0 {
                                self.scroll_bookmarks(step);
                                self.bookmark_scroll_y = Some(at.y);
                            }
                        }
                        _ => {}
                    }
                }
                if let Some(slider) = self.overlay.grab.and_then(|grab| grab.slider) {
                    self.preview_slider(hwnd, slider, at.x);
                } else {
                    self.overlay.move_to(id, at, layout.viewport);
                    if self.overlay.audio_visible() {
                        self.animate(hwnd);
                    }
                }
            }
            WM_POINTERUP if self.overlay.grab.is_some_and(|grab| grab.id == id) => {
                    let _ = unsafe { KillTimer(Some(hwnd), BOOKMARK_DRAG_TIMER) };
                let grab = self.overlay.grab.take();
                let swiped = self.bookmark_scroll_y.take().is_some();
                self.bookmark_drag = None;
                if let Some(slider) = grab.and_then(|grab| grab.slider) {
                    self.preview_slider(hwnd, slider, at.x);
                    self.finish_slider(hwnd);
                } else if let Some(Event::BookmarkDrag(index)) = grab.and_then(|grab| grab.event) {
                    self.drop_bookmark(hwnd, &layout, index, at);
                } else if !swiped
                    && let Some(event) = grab.and_then(|grab| grab.event)
                    && self
                        .cards(&layout)
                        .iter()
                        .rev()
                        .find(|card| inside(card.rect, at))
                        .is_some_and(|card| {
                            card.grab(id, at).event == Some(event)
                        })
                {
                    self.chrome_menu = None;
                    self.event(hwnd, event);
                }
            }
            _ => return false,
        }
        if self.overlay.utilities {
            self.utilities_activity = Some(Instant::now());
        }
        repaint(hwnd);
        true
    }

    pub(super) fn cards(&self, layout: &Layout) -> Vec<overlay::Card> {
        if self.page_grid_open() { return self.chrome_card(layout).into_iter().collect(); }
        let page_indicator = self.current_score().map_or_else(String::new, |s| {
            format!("{} / {}", s.pdf_page(self.selected) + 1, s.pdf_count)
        });
        let indices = if self.overlay.drawer && self.overlay.sheet == Some(Sheet::Bookmarks) {
            self.visible_bookmarks()
        } else {
            Vec::new()
        };
        let rows: Vec<_> = self
            .bookmarks
            .as_ref()
            .map(|bookmarks| {
                indices
                    .iter()
                    .copied()
                    .skip(self.overlay.bookmark_offset)
                    .take(overlay::BOOKMARK_ROWS)
                    .map(|index| {
                        let entry = &bookmarks.entries[index];
                        overlay::BookmarkRow {
                            title: &entry.title,
                            detail: entry
                                .page
                                .map_or_else(String::new, |page| format!("· {page}")),
                            index,
                            depth: entry.depth,
                            branch: bookmarks
                                .entries
                                .get(index + 1)
                                .is_some_and(|next| next.depth > entry.depth),
                            collapsed: self.collapsed_bookmarks.contains(&index),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let chrome = self.chrome_card(layout);
        let mut cards = self.overlay.cards(
            layout.viewport,
            layout.scale,
            &overlay::Model {
                fullscreen: self.fullscreen.is_some(),
                popup: chrome.as_ref().map(|card| card.rect).or_else(|| self.chrome_visual.and_then(|menu| self.chrome_card_for(layout, menu)).map(|card| card.rect)),
                zoom: self.view.zoom(),
                tap: self.settings.tap_turn,
                language: self.settings.language,
                page_loaded: self.total > 0,
                page_indicator,
                undo: self.can_undo(),
                redo: self.can_redo(),
                can_turn: [self.selected > 0, self.selected + 1 < self.total],
                concert_navigation: self.concert.then(|| {
                    [self.score_destination(-1).is_some(), self.score_destination(1).is_some()]
                }),
                can_add_bookmark: self.bookmarks.is_some() && !self.original_export,
                bpm: self.settings.bpm,
                beats: self.settings.beats,
                tuning_hz: self.settings.tuning_hz,
                compact_pitch: &self.sound.compact_pitch,
                running: [self.sound.running(0), self.sound.running(1)],
                audio_status: [self.sound.status(0), self.sound.status(1)],
                pitch: self.sound.pitch,
                pulse: self.sound.pulse(),
                tool: self.tool,
                style: self.tool.style(&self.settings),
                favorites: self.tool.favorite_group().map(|tool| self.favorites.styles(tool)),
                bookmarks: &rows,
                selected_bookmark: self.overlay.selected_bookmark,
                bookmark_status: &self.bookmark_status,
                bookmark_more: [
                    self.overlay.bookmark_offset > 0,
                    self.overlay.bookmark_offset + overlay::BOOKMARK_ROWS < indices.len(),
                ],
            },
        );
        if self.store_update_available
            && !self.performing()
            && let Some(banner) = overlay::Card::store_update(layout.viewport, layout.scale)
        {
            cards.insert(0, banner);
        }
        if let Some(card) = chrome {
            cards.push(card);
        }
        cards
    }

    pub(super) fn event(&mut self, hwnd: HWND, event: Event) {
        match event {
            Event::Command(command) => self.command(hwnd, command),
            Event::Sheet(sheet) => self.open_sheet(hwnd, sheet),
            Event::CloseDrawer => self.close_drawer(hwnd),
            Event::ResetZoom => {
                self.overlay.zoom_slider = false;
                self.close_drawer(hwnd);
                self.reset_view(hwnd);
            }
            Event::ToggleZoom => self.overlay.zoom_slider = !self.overlay.zoom_slider,
            Event::ZoomStep(step) => self.zoom_step(hwnd, step),
            Event::Turn(step) => self.turn(hwnd, step),
            Event::ScoreTurn(step) => {
                if let Some(page) = self.score_destination(step) {
                    self.go_to(hwnd, page);
                }
            }
            Event::About => self.open_sheet(hwnd, Sheet::About),
            Event::DeleteShape => self.delete_shape(hwnd),
            Event::ToggleTap => {
                let settings = Settings {
                    tap_turn: !self.settings.tap_turn,
                    ..self.settings
                };
                self.commit_settings(hwnd, settings);
            }
            Event::Language(language) => {
                if self.settings.language != language {
                    let settings = Settings {
                        language,
                        ..self.settings
                    };
                    if self.commit_settings(hwnd, settings) {
                        crate::language::apply(language);
                    }
                }
            }
            Event::BookmarkSelect(index) => {
                self.overlay.selected_bookmark = Some(index);
                self.go_to_bookmark(hwnd);
            }
            Event::BookmarkScroll(direction) => self.scroll_bookmarks(direction),
            Event::BookmarkAdd | Event::BookmarkChild(_) | Event::BookmarkEdit(_) => {
                self.bookmark_edit = Some(event);
                post(hwnd, EDIT_BOOKMARK);
            }
            Event::BookmarkDelete(index) => {
                if let Some(mut bookmarks) = self.bookmarks.clone()
                    && index < bookmarks.entries.len()
                {
                    bookmarks.remove(index);
                    self.save_bookmark_structure(hwnd, bookmarks, None);
                }
            }
            Event::BookmarkCollapse(index) => {
                if !self.collapsed_bookmarks.remove(&index) {
                    self.collapsed_bookmarks.insert(index);
                }
                self.scroll_bookmarks(0);
            }
            Event::BookmarkDrag(_) => {}
            Event::Finalize => post(hwnd, EXPORT_PDF),
            Event::DefaultPdf => {
                let _ = unsafe { PostMessageW(Some(hwnd), DEFAULT_PDF, WPARAM(1), LPARAM(0)) };
            }
            Event::OpenStore => store_update::open_listing(hwnd),
            Event::DismissStoreUpdate => self.store_update_available = false,
            Event::AddFavorite => self.save_favorite(hwnd, None),
            Event::DeleteFavorite(index) => self.save_favorite(hwnd, Some(index)),
            Event::LoadFavorite(index) => {
                if let Some(group) = self.tool.favorite_group()
                    && let Some(&style) = self.favorites.styles(group).get(index)
                {
                    let mut settings = self.settings;
                    if let Some(current) = Self::style_mut(&mut settings, self.tool) {
                        *current = style;
                        self.commit_settings(hwnd, settings);
                    }
                }
            }
            Event::CloseDocument => self.close_document(hwnd),
            Event::CustomColor => post(hwnd, OPEN_PREFERENCES),
            Event::Licenses => post(hwnd, SHOW_ABOUT),
            Event::Feedback => post(hwnd, SHOW_FEEDBACK),
            Event::Website(url) => unsafe {
                windows::Win32::UI::Shell::ShellExecuteW(
                    Some(hwnd),
                    w!("open"),
                    &HSTRING::from(url),
                    None,
                    None,
                    SW_SHOWNORMAL,
                );
            },
            Event::AudioToggle(slot) => {
                self.sound.toggle(slot, self.settings);
                unsafe { SetTimer(Some(hwnd), AUDIO_TIMER, 33, None) };
            }
            Event::AudioClose(slot) => {
                self.overlay.close_audio(slot);
                self.sound.stop(slot);
                self.animate(hwnd);
                if slot == 0 {
                    self.tempo_taps.clear();
                }
            }
            Event::AudioCompact(slot) => {
                self.overlay.toggle_compact(slot);
                self.animate(hwnd);
            }
            Event::TapTempo => self.tap_tempo(hwnd, Instant::now()),
            Event::EditTuning => post(hwnd, EDIT_TUNING),
            Event::Bpm(delta) => {
                self.tempo_taps.clear();
                self.adjust_tempo(
                    hwnd,
                    self.settings
                        .bpm
                        .saturating_add_signed(delta)
                        .clamp(30, 300),
                    self.settings.beats,
                );
            }
            Event::Beats(delta) => {
                self.adjust_tempo(
                    hwnd,
                    self.settings.bpm,
                    self.settings
                        .beats
                        .saturating_add_signed(delta)
                        .clamp(1, 12),
                );
            }
            Event::Color(_) | Event::Width(_) | Event::Opacity(_) => self.update_style(hwnd, event),
            Event::Advanced => self.open_sheet(hwnd, Sheet::Preferences),
        }
        repaint(hwnd);
    }

    fn set_favorite_scroll(&mut self, scroll: f32) {
        let count = self.tool.favorite_group().map_or(0, |tool| self.favorites.styles(tool).len());
        self.overlay.favorite_scroll = scroll.clamp(0.0, overlay::favorite_max_scroll(count));
    }

    pub(super) fn favorite_wheel(&mut self, hwnd: HWND, point: POINT, delta: i32) -> bool {
        if !self.overlay.drawer || self.overlay.sheet != Some(Sheet::Style) {
            return false;
        }
        let layout = Layout::new(hwnd, self.fullscreen.is_some());
        if !self.cards(&layout).iter().any(|card| card.favorite_clip.is_some_and(|clip| inside(clip, point))) {
            return false;
        }
        self.set_favorite_scroll(self.overlay.favorite_scroll - delta as f32 * 0.42);
        repaint(hwnd);
        true
    }

    fn save_favorite(&mut self, hwnd: HWND, delete: Option<usize>) {
        let Some(group) = self.tool.favorite_group() else { return };
        let mut favorites = self.favorites.clone();
        let styles = favorites.styles_mut(group);
        if let Some(index) = delete {
            if index >= styles.len() { return }
            styles.remove(index);
        } else {
            styles.push(self.tool.style(&self.settings));
        }
        let last = overlay::favorite_max_scroll(styles.len());
        if let Some(store) = &self.store
            && let Err(error) = store.save_favorites(&favorites)
        {
            self.notify(hwnd, trf!("Impossibile salvare i preferiti · {error}", "Could not save favorites · {error}"));
            return;
        }
        self.favorites = favorites;
        self.overlay.favorite_scroll = if delete.is_some() {
            self.overlay.favorite_scroll.min(last)
        } else {
            last
        };
    }

    fn tap_tempo(&mut self, hwnd: HWND, now: Instant) {
        if let Some(last) = self.tempo_taps.back() {
            let gap = now.duration_since(*last);
            if gap < Duration::from_millis(180) {
                return;
            }
            if gap > Duration::from_secs(2) {
                self.tempo_taps.clear();
            }
        }
        self.tempo_taps.push_back(now);
        if self.tempo_taps.len() > 5 {
            self.tempo_taps.pop_front();
        }
        if let Some(first) = self.tempo_taps.front()
            && self.tempo_taps.len() > 1
        {
            let bpm = (60.0 * (self.tempo_taps.len() - 1) as f64 / now.duration_since(*first).as_secs_f64())
                .round() as u16;
            let bpm = bpm.clamp(30, 300);
            if bpm != self.settings.bpm {
                self.adjust_tempo(hwnd, bpm, self.settings.beats);
            }
        }
    }

    fn adjust_tempo(&mut self, hwnd: HWND, bpm: u16, beats: u8) {
        let settings = Settings {
            bpm,
            beats,
            ..self.settings
        };
        if self.commit_settings(hwnd, settings) && self.sound.running(0) {
            self.sound.refresh_tempo(settings);
        }
    }

    fn update_style(&mut self, hwnd: HWND, event: Event) {
        let mut settings = self.settings;
        let Some(style) = Self::style_mut(&mut settings, self.tool) else {
            return;
        };
        match event {
            Event::Color(color) => style.color = color,
            Event::Width(delta) => {
                style.width = (style.width + delta as f32 * 0.0005).clamp(0.0005, 0.06)
            }
            Event::Opacity(delta) => {
                style.opacity = (i32::from(style.opacity) + delta * 13).clamp(0, 255) as u8
            }
            _ => return,
        }
        self.commit_settings(hwnd, settings);
    }

    fn style_mut(settings: &mut Settings, tool: Tool) -> Option<&mut Style> {
        match tool {
            Tool::Pen => Some(&mut settings.pen),
            Tool::Highlighter => Some(&mut settings.highlighter),
            Tool::Staff => Some(&mut settings.staff),
            Tool::Rectangle => Some(&mut settings.rectangle),
            _ => None,
        }
    }

    fn preview_slider(&mut self, hwnd: HWND, slider: Slider, x: i32) {
        let ratio = slider.position(x);
        match slider.kind {
            SliderKind::Zoom => self.zoom_to(hwnd, 1.0 + (300.0 * ratio).round() / 100.0),
            SliderKind::Width | SliderKind::Opacity => {
                if let Some(style) = Self::style_mut(&mut self.settings, self.tool) {
                    if slider.kind == SliderKind::Width {
                        style.width = overlay::width_from_ratio(ratio);
                    } else {
                        style.opacity = (255.0 * ratio).round() as u8;
                    }
                }
            }
            SliderKind::Bpm | SliderKind::Beats => {
                if slider.kind == SliderKind::Bpm {
                    self.tempo_taps.clear();
                    self.settings.bpm = (30.0 + 270.0 * ratio).round() as u16;
                } else {
                    self.settings.beats = (1.0 + 11.0 * ratio).round() as u8;
                }
                if self.sound.running(0) {
                    self.sound.refresh_tempo(self.settings);
                }
            }
        }
    }

    fn finish_slider(&mut self, hwnd: HWND) {
        if let Some(original) = self.slider_original.take() {
            let edited = std::mem::replace(&mut self.settings, original);
            self.commit_settings(hwnd, edited);
            if self.sound.running(0) {
                self.sound.refresh_tempo(self.settings);
            }
        }
    }

    fn zoom_to(&mut self, hwnd: HWND, level: f32) {
        let viewport = Layout::new(hwnd, self.fullscreen.is_some()).viewport;
        let center = viewport_center(viewport);
        self.transform_view(hwnd, center, center, level / self.view.zoom());
    }

    pub(super) fn current_score(&self) -> Option<&ScoreInfo> {
        self.scores.iter().rev().find(|score| {
            self.selected >= score.first_page && self.selected - score.first_page < score.count
        })
    }

    pub(super) fn score_destination(&self, step: i32) -> Option<u32> {
        if !self.concert { return None; }
        let current = self.scores.iter().rposition(|score| score.first_page <= self.selected)?;
        let next = current.checked_add_signed(step as isize)?;
        self.scores.get(next).map(|score| score.first_page)
    }

    fn go_to_bookmark(&mut self, hwnd: HWND) {
        let Some(entry) = self
            .overlay
            .selected_bookmark
            .and_then(|index| self.bookmarks.as_ref()?.entries.get(index))
        else {
            return;
        };
        let Some(score) = self.current_score() else {
            return;
        };
        if let Some(page) = entry.page.and_then(|page| score.index(page.checked_sub(1)?)) {
            self.go_to(hwnd, page);
            self.close_drawer(hwnd);
        } else {
            self.notify(
                hwnd,
                tr!(
                    "Questo segnalibro non ha una destinazione interna.",
                    "This bookmark doesn’t point to a page in this PDF."
                )
                .into(),
            );
        }
    }

    pub(super) fn load_bookmarks(&mut self, hwnd: HWND) {
        let Some(source) = self.current_score().map(|score| score.source.clone()) else {
            return;
        };
        if self.bookmark_source.as_ref() == Some(&source) {
            return;
        }
        let Some(store) = self.store.clone() else {
            return;
        };
        self.bookmark_source = Some(source.clone());
        self.bookmarks = None;
        self.collapsed_bookmarks.clear();
        self.overlay.selected_bookmark = None;
        self.overlay.bookmark_offset = 0;
        self.bookmark_wheel_delta = 0;
        self.bookmark_status = tr!("Lettura dei segnalibri…", "Reading bookmarks…").into();
        match background(hwnd, "leggi-segnalibri", move || {
            Bookmarks::read(&source, &store).map_err(|error| error.to_string())
        }) {
            Ok(rx) => self.bookmark_loading = Some(rx),
            Err(error) => {
                self.bookmark_source = None;
                self.bookmark_status = error.to_string();
            }
        }
    }

    fn visible_bookmarks(&self) -> Vec<usize> {
        let mut visible = Vec::new();
        let mut collapsed: Option<usize> = None;
        if let Some(bookmarks) = &self.bookmarks {
            for (index, item) in bookmarks.entries.iter().enumerate() {
                if collapsed.is_some_and(|depth| item.depth > depth) {
                    continue;
                }
                visible.push(index);
                collapsed = self
                    .collapsed_bookmarks
                    .contains(&index)
                    .then_some(item.depth);
            }
        }
        visible
    }

    fn scroll_bookmarks(&mut self, step: i32) {
        let max = self
            .visible_bookmarks()
            .len()
            .saturating_sub(overlay::BOOKMARK_ROWS);
        self.overlay.bookmark_offset = self
            .overlay
            .bookmark_offset
            .saturating_add_signed(step as isize)
            .min(max);
    }

    pub(super) fn bookmark_wheel(&mut self, hwnd: HWND, delta: i32) -> bool {
        if self.overlay.drawer && self.overlay.sheet == Some(Sheet::Bookmarks) {
            self.bookmark_wheel_delta += delta;
            let step = self.bookmark_wheel_delta / 60;
            self.bookmark_wheel_delta %= 60;
            if step != 0 {
                self.scroll_bookmarks(-step);
                repaint(hwnd);
            }
            return true;
        }
        false
    }

    pub(super) fn scroll_dragging_bookmark(&mut self, hwnd: HWND) {
        let Some(at) = self.bookmark_drag else { return };
        let layout = Layout::new(hwnd, self.fullscreen.is_some());
        let cards = self.cards(&layout);
        let Some(card) = cards.iter().rev().find(|card| {
            card.buttons
                .iter()
                .any(|button| matches!(button.event, Event::BookmarkSelect(_)))
        }) else {
            return;
        };
        if at.x < card.rect.left || at.x >= card.rect.right {
            return;
        }
        let top = card.rect.top + (93.0 * card.scale) as i32;
        let bottom = card.rect.bottom - (61.0 * card.scale) as i32;
        let step = if at.y <= top {
            -1
        } else if at.y >= bottom {
            1
        } else {
            0
        };
        let previous = self.overlay.bookmark_offset;
        self.scroll_bookmarks(step);
        if self.overlay.bookmark_offset != previous {
            repaint(hwnd);
        }
    }

    fn drop_bookmark(&mut self, hwnd: HWND, layout: &Layout, from: usize, at: POINT) {
        let Some((target, rect)) = self
            .cards(layout)
            .iter()
            .rev()
            .filter(|card| inside(card.rect, at))
            .flat_map(|card| &card.buttons)
            .find_map(|button| match button.event {
                Event::BookmarkSelect(index)
                    if at.y >= button.rect.top && at.y < button.rect.bottom =>
                {
                    Some((index, button.rect))
                }
                _ => None,
            })
        else {
            return;
        };
        let part = (at.y - rect.top) * 4 / (rect.bottom - rect.top).max(1);
        let pos = match part {
            0 => pdf_edit::DropPosition::Before,
            3 => pdf_edit::DropPosition::After,
            _ => pdf_edit::DropPosition::Child,
        };
        if let Some(mut bookmarks) = self.bookmarks.clone()
            && let Some(index) = bookmarks.move_branch(from, target, pos)
            && self.save_bookmarks(hwnd, bookmarks)
        {
            self.collapsed_bookmarks.clear();
            self.overlay.selected_bookmark = Some(index);
            self.scroll_bookmarks(0);
        }
    }

    fn save_bookmark_structure(&mut self, hwnd: HWND, bookmarks: Bookmarks, next: Option<usize>) {
        if self.save_bookmarks(hwnd, bookmarks) {
            self.collapsed_bookmarks.clear();
            self.overlay.selected_bookmark = next;
            self.scroll_bookmarks(0);
            if let Some(index) = next {
                self.overlay.bookmark_offset = self
                    .overlay
                    .bookmark_offset
                    .clamp(index.saturating_sub(overlay::BOOKMARK_ROWS - 1), index);
            }
        }
    }

    fn save_bookmarks(&mut self, hwnd: HWND, bookmarks: Bookmarks) -> bool {
        if self.original_export {
            self.notify(
                hwnd,
                tr!(
                    "Attendi che ScoreDraw finisca di aggiornare il PDF.",
                    "Wait for ScoreDraw to finish updating the PDF."
                )
                .into(),
            );
            return false;
        }
        let Some((store, source)) = self.store.as_ref().zip(self.bookmark_source.as_ref()) else {
            return false;
        };
        match crate::data::write_json(&store.bookmarks_path(source), &bookmarks) {
            Ok(()) => {
                self.bookmark_status = trf!(
                    "{} · modifiche salvate",
                    "{} · changes saved",
                    bookmark_count(bookmarks.entries.len())
                );
                self.bookmarks = Some(bookmarks);
                true
            }
            Err(error) => {
                self.notify(
                    hwnd,
                    trf!(
                        "Salvataggio segnalibri non riuscito · {error}",
                        "Couldn’t save bookmarks · {error}"
                    ),
                );
                false
            }
        }
    }

    pub(super) fn pdf_edit_result(&mut self, hwnd: HWND) {
        if let Some(rx) = &self.bookmark_loading
            && let Some(result) = try_result(rx)
        {
            self.bookmark_loading = None;
            match result {
                Ok(bookmarks) => {
                    self.bookmark_status = bookmark_count(bookmarks.entries.len());
                    self.bookmarks = Some(bookmarks);
                }
                Err(error) => {
                    self.bookmark_source = None;
                    self.bookmark_status = trf!(
                        "Segnalibri non disponibili · {error}",
                        "Bookmarks unavailable · {error}"
                    );
                }
            }
        }
        if let Some(rx) = &self.exporting
            && let Some(result) = try_result(rx)
        {
            self.exporting = None;
            self.original_export = false;
            match result {
                Ok(result) => {
                    let close = self.close_after_export && result.failed.is_empty();
                    if result.target.is_original()
                        && !close
                        && self
                            .scores
                            .iter()
                            .any(|score| result.written.contains(&score.source.path))
                    {
                        let files = self
                            .scores
                            .iter()
                            .map(|score| score.entry.clone())
                            .collect();
                        let selected = self.selected;
                        let concert = self.concert;
                        let session = self.session;
                        self.open_entries(hwnd, files, false);
                        if self.session != session {
                            self.concert = concert;
                            self.resume_page = Some(selected);
                        }
                    }
                    if !result.failed.is_empty() {
                        self.export_report = Some(result.failure_report());
                        post(hwnd, SHOW_EXPORT_REPORT);
                    } else if close {
                        post(hwnd, WM_CLOSE);
                    } else {
                        self.notify(hwnd, result.success_message());
                    }
                }
                Err(error) => self.notify(
                    hwnd,
                    trf!(
                        "Incorporazione non riuscita · {error}",
                        "Couldn’t update the PDF · {error}"
                    ),
                ),
            }
            self.close_after_export = false;
        }
        repaint(hwnd);
    }

    pub(super) fn notify(&mut self, hwnd: HWND, message: String) {
        self.toast = Some(message);
        unsafe { SetTimer(Some(hwnd), TOAST_TIMER, 6500, None) };
        repaint(hwnd);
    }

    pub(super) fn repaint_audio(&self, hwnd: HWND, layout: &Layout) {
        if self.overlay.audio_visible() {
            for card in self
                .cards(layout)
                .into_iter()
                .filter(|card| card.floating.is_some())
            {
                repaint_rect(hwnd, card.rect);
            }
        }
    }

    pub(super) fn hover_overlays(&mut self, hwnd: HWND, point: Option<POINT>) {
        let previous = std::mem::replace(&mut self.overlay_hover, point);
        if previous == point { return; }
        let layout = Layout::new(hwnd, self.fullscreen.is_some());
        for card in self.cards(&layout) {
            let button = |at| card.grab(MOUSE, at).event;
            if previous.and_then(button) != point.and_then(button) {
                repaint_rect(hwnd, card.rect);
            }
        }
    }

    pub(super) fn draw_overlays(&self, dc: HDC, layout: &Layout, damage: RECT) {
        let mut cards = self.cards(layout);
        if self.chrome_menu.is_none()
            && let Some(menu) = self.chrome_visual
            && let Some(card) = self.chrome_card_for(layout, menu)
        {
            cards.push(card);
        }
        for card in cards {
            let damage = if card.drawer {
                let Some(clip) = intersect(layout.viewport, damage) else { continue };
                clip
            } else { damage };
            let Some(affected) = intersect(card.rect, damage) else { continue };
            if card.opacity <= 0.0 {
                continue;
            }
            with_clip(dc, affected, || {
            let w = affected.right - affected.left;
            let h = affected.bottom - affected.top;
            let mut scratch = self.blend_buffer.borrow_mut();
            let background = if card.opacity < 1.0 {
                if scratch.as_ref().is_none_or(|surface| surface.width < w || surface.height < h) {
                    let (width, height) = scratch.as_ref().map_or((w, h), |surface| {
                        (w.max(surface.width), h.max(surface.height))
                    });
                    *scratch = Surface::new(dc, width, height);
                }
                scratch.as_ref().inspect(|surface| {
                    unsafe {
                        let _ = BitBlt(surface.dc, 0, 0, w, h, Some(dc), affected.left, affected.top, SRCCOPY);
                    }
                })
            } else { None };
            let px = |n: f32| (n * card.scale).round() as i32;
            rounded(dc, card.rect, PANEL, px(28.0));
            if let Some(section) = card.section {
                rounded(dc, section, BACKGROUND, px(20.0));
            }
            for separator in card.separators {
                rounded(dc, separator, MENU_BORDER, px(2.0));
            }
            for control in card.buttons {
                if intersect(control.rect, affected).is_none() {
                    continue;
                }
                let clip = if control.event.is_favorite_row() {
                    card.favorite_clip.unwrap_or(card.rect)
                } else {
                    card.rect
                };
                let hot = control.enabled && self.overlay_hover.is_some_and(|at| inside(clip, at) && inside(control.rect, at));
                with_clip(dc, clip, || {
                if let Some(color) = control.color {
                    if control.active {
                        rounded(dc, control.rect, ACCENT, px(19.0));
                    }
                    let margin = if control.active { 4 } else { 2 };
                    let rect = RECT {
                        left: control.rect.left + margin,
                        top: control.rect.top + margin,
                        right: control.rect.right - margin,
                        bottom: control.rect.bottom - margin,
                    };
                    rounded(dc, rect, color, px(15.0));
                } else if let Some(icon) = control.icon {
                    if control.text.is_empty() {
                        if hot {
                            rounded(dc, control.rect, blend_color(SURFACE, ACCENT, 15), px(19.0));
                            icons::draw(dc, icon, control.rect, 20.0 * card.scale, TEXT);
                        } else {
                            icon_button(dc, control.rect, icon, control.enabled, card.scale);
                        }
                    } else {
                        let foreground = if control.enabled { TEXT } else { MUTED };
                        if control.enabled {
                            rounded(dc, control.rect, if hot { blend_color(SURFACE, ACCENT, 15) } else { SURFACE }, px(16.0));
                        }
                        let glyph = RECT {
                            left: control.rect.left + px(11.0),
                            right: control.rect.left + px(35.0),
                            ..control.rect
                        };
                        icons::draw(dc, icon, glyph, 18.0 * card.scale, foreground);
                        text_left(
                            dc,
                            RECT {
                                left: control.rect.left + px(46.0),
                                right: control.rect.right - px(8.0),
                                ..control.rect
                            },
                            &control.text,
                            foreground,
                            px(14.0),
                        );
                    }
                } else if !control.enabled {
                    text(dc, control.rect, &control.text, MENU_BORDER, px(14.0));
                } else if hot && !control.active {
                    rounded(dc, control.rect, blend_color(SURFACE, ACCENT, 15), px(19.0));
                    text(dc, control.rect, &control.text, TEXT, px(14.0));
                } else {
                    button(dc, control.rect, &control.text, control.active, card.scale);
                }
                });
            }
            for (index, label) in card.labels.into_iter().enumerate() {
                if intersect(label.rect, affected).is_none() {
                    continue;
                }
                let clip = if index >= card.favorite_labels_start {
                    card.favorite_clip.unwrap_or(card.rect)
                } else {
                    card.rect
                };
                with_clip(dc, clip, || text(
                    dc,
                    label.rect,
                    &label.text,
                    if label.dark { BACKGROUND } else if label.muted { MUTED } else { TEXT },
                    px(label.size as f32),
                ));
            }
            with_clip(dc, card.favorite_clip.unwrap_or(card.rect), || for (rect, style) in card.strokes {
                rounded(dc, rect, COLORREF(0x00dedede), px(8.0));
                if let Some(canvas) = Canvas::new(dc)
                    && let Some(path) = canvas::Path::new()
                {
                    let y = (rect.top + rect.bottom) as f32 / 2.0;
                    path.line(((rect.left + px(12.0)) as f32, y), ((rect.right - px(12.0)) as f32, y));
                    canvas.stroke_path(&path,
                        canvas::argb(COLORREF(style.color), style.opacity),
                        (style.width * 220.0).clamp(1.0, 14.0) * card.scale);
                }
            });
            if let Some(bar) = card.favorite_bar {
                draw_scrollbar(dc, bar, card.scale);
            }
            if let Some((rect, meter)) = card.meter {
                draw_meter(dc, rect, meter, card.scale);
            }
            for slider in card.sliders {
                draw_slider(dc, slider, card.scale);
            }
            if let Some(background) = background {
                let w = affected.right - affected.left;
                let h = affected.bottom - affected.top;
                unsafe {
                    let _ = AlphaBlend(dc, affected.left, affected.top, w, h, background.dc, 0, 0, w, h,
                        BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, SourceConstantAlpha: ((1.0 - card.opacity) * 255.0).round() as u8, ..Default::default() });
                }
            }
            });
        }
        if let Some(at) = self.bookmark_drag {
            let mark = RECT {
                left: at.x - layout.px(10),
                top: at.y - layout.px(10),
                right: at.x + layout.px(10),
                bottom: at.y + layout.px(10),
            };
            circle(dc, mark, ACCENT, BACKGROUND, 1.6 * layout.scale);
        }
        if let Some(message) = &self.toast {
            let area = RECT {
                left: layout.viewport.left + layout.px(16),
                right: layout.viewport.right - layout.px(66),
                top: layout.viewport.bottom - layout.px(188),
                bottom: layout.viewport.bottom - layout.px(148),
            };
            rounded(dc, area, PANEL, layout.px(18));
            text(dc, area, message, TEXT, layout.px(14));
        }
    }
}

fn draw_slider(dc: HDC, slider: Slider, scale: f32) {
    let (left, right, y) = slider.track();
    let thumb = left + ((right - left) as f32 * slider.ratio).round() as i32;
    let half = (3.0 * scale).round().max(1.0) as i32;
    let track = RECT {
        left,
        top: y - half,
        right,
        bottom: y + half,
    };
    rounded(dc, track, SURFACE, half * 2);
    rounded(
        dc,
        RECT {
            right: thumb,
            ..track
        },
        ACCENT,
        half * 2,
    );
    let head = (10.0 * scale).round().max(3.0) as i32;
    circle(
        dc,
        RECT {
            left: thumb - head,
            top: y - head,
            right: thumb + head,
            bottom: y + head,
        },
        ACCENT,
        BACKGROUND,
        (1.5 * scale).max(1.0),
    );
}

fn draw_meter(dc: HDC, rect: RECT, meter: overlay::Meter, scale: f32) {
    let w = (rect.right - rect.left) as f32;
    let h = (rect.bottom - rect.top) as f32;
    let center = (rect.left as f32 + w / 2.0, rect.bottom as f32 - 6.0);
    let Some(canvas) = Canvas::new(dc) else {
        return;
    };
    match meter {
        overlay::Meter::Pitch(cents) => {
            let radius = (w / 2.0 - 11.0).min((h - 22.0 * scale).max(1.0));
            let point =
                |angle: f32, r: f32| (center.0 + angle.sin() * r, center.1 - angle.cos() * r);
            for i in -5..=5 {
                let angle = i as f32 * 0.2;
                if let Some(path) = canvas::Path::new() {
                    path.line(
                        point(angle, radius),
                        point(angle, radius - (if i == 0 { 12.0 } else { 8.0 }) * scale),
                    );
                    canvas.stroke_path(
                        &path,
                        canvas::argb(if i == 0 { ACCENT } else { TEXT }, 255),
                        (if i == 0 { 2.6 } else { 1.6 } * scale).max(1.0),
                    );
                }
            }
            for (angle, caption) in [(-1.12, "−50"), (0.0, "0"), (1.12, "+50")] {
                let (x, y) = point(angle, radius + 9.0 * scale);
                let half_width = (20.0 * scale) as i32;
                let half_height = (8.0 * scale) as i32;
                text(
                    dc,
                    RECT {
                        left: x as i32 - half_width,
                        right: x as i32 + half_width,
                        top: y as i32 - half_height,
                        bottom: y as i32 + half_height,
                    },
                    caption,
                    if angle == 0.0 { ACCENT } else { TEXT },
                    (12.0 * scale).round() as i32,
                );
            }
            if let Some(cents) = cents
                && let Some(path) = canvas::Path::new()
            {
                let angle = (cents / 50.0).clamp(-1.0, 1.0);
                path.line(center, point(angle, radius - 3.0));
                canvas.stroke_path(
                    &path,
                    canvas::argb(if cents.abs() <= 3.0 { ACCENT } else { ALERT }, 255),
                    3.0,
                );
            }
        }
        overlay::Meter::Pulse { beats, flash } => {
            let beats = beats.clamp(1, 12);
            let gap = (w / f32::from(beats + 1)).min(34.0);
            let radius = (gap * 0.31).min(10.0);
            for index in 0..beats {
                let x = center.0 + (f32::from(index) - f32::from(beats - 1) / 2.0) * gap;
                let y = rect.top as f32 + h / 2.0;
                let color = flash
                    .filter(|(beat, _)| *beat == index)
                    .map_or(SURFACE, |(_, _)| if index == 0 { ALERT } else { ACCENT });
                let alpha = flash
                    .filter(|(beat, _)| *beat == index)
                    .map_or(255, |(_, strength)| (130.0 + 125.0 * strength) as u8);
                canvas.ellipse(
                    RECT {
                        left: (x - radius) as i32,
                        top: (y - radius) as i32,
                        right: (x + radius) as i32,
                        bottom: (y + radius) as i32,
                    },
                    canvas::argb(color, alpha),
                    canvas::argb(MENU_BORDER, 255),
                    1.2,
                );
            }
        }
    }
}

pub(super) fn show_licenses(hwnd: HWND) {
    let license = include_str!("../../LICENSE")
        .lines()
        .collect::<Vec<_>>()
        .join("\r\n");
    let notices = include_str!("../../THIRD_PARTY_NOTICES.md")
        .lines()
        .filter(|line| !line.starts_with("~~~~"))
        .map(|line| line.trim_start_matches('#').trim_start())
        .collect::<Vec<_>>()
        .join("\r\n");
    let content = trf!(
        "ScoreDraw\r\nCopyright (c) 2026 Emanuele Campilongo\r\nGNU General Public License, versione 3\r\n\r\n{license}\r\n\r\n{notices}",
        "ScoreDraw\r\nCopyright (c) 2026 Emanuele Campilongo\r\nGNU General Public License, version 3\r\n\r\n{license}\r\n\r\n{notices}"
    );
    let _ = crate::dialog::reading(
        hwnd,
        tr!("Crediti e licenze", "Credits and licenses"),
        &content,
    );
}

pub(super) fn show_feedback(hwnd: HWND) {
    const ADDRESS: &str = "info@emanuelecampilongo.com";

    if let Err(error) = crate::dialog::feedback(hwnd, ADDRESS) {
        show_startup_error(&error);
    }
}

fn background<T: Send + 'static>(
    hwnd: HWND,
    name: &str,
    job: impl FnOnce() -> std::result::Result<T, String> + Send + 'static,
) -> std::io::Result<mpsc::Receiver<std::result::Result<T, String>>> {
    let (tx, rx) = mpsc::channel();
    let window = hwnd.0 as isize;
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job))
                .unwrap_or_else(|_| {
                    Err(tr!(
                        "Il PDF contiene dati che non è stato possibile elaborare.",
                        "The PDF contains data that couldn’t be processed."
                    )
                    .into())
                });
            if tx.send(result).is_ok() {
                post(HWND(window as _), PDF_EDIT_RESULT);
            }
        })?;
    Ok(rx)
}

fn bookmark_count(count: usize) -> String {
    match count {
        0 => tr!("Nessun segnalibro", "No bookmarks").into(),
        1 => tr!("1 segnalibro", "1 bookmark").into(),
        _ => trf!("{count} segnalibri", "{count} bookmarks"),
    }
}

fn try_result<T>(
    rx: &mpsc::Receiver<std::result::Result<T, String>>,
) -> Option<std::result::Result<T, String>> {
    match rx.try_recv() {
        Ok(result) => Some(result),
        Err(mpsc::TryRecvError::Disconnected) => Some(Err(tr!(
            "L’operazione si è interrotta.",
            "The operation stopped."
        )
        .into())),
        Err(mpsc::TryRecvError::Empty) => None,
    }
}

pub(super) struct Finalized {
    target: ExportTarget,
    written: Vec<PathBuf>,
    unchanged: usize,
    failed: Vec<(PathBuf, String)>,
}

enum ExportTarget {
    Original,
    Copy(PathBuf),
    Folder(PathBuf),
}

impl ExportTarget {
    fn is_original(&self) -> bool {
        matches!(self, Self::Original)
    }

    fn write_pdf(
        &self,
        source: &ScoreFile,
        store: &Store,
        snapshots: &pdf_edit::Snapshots,
    ) -> std::result::Result<Option<PathBuf>, Box<dyn std::error::Error>> {
        let bookmarks = if store.bookmarks_path(source).try_exists()? {
            Some(Bookmarks::read(source, store)?)
        } else {
            None
        };
        let bookmarks = bookmarks.as_ref();
        match self {
            Self::Original => {
                let changed = pdf_edit::finalize_original(source, store, bookmarks, snapshots)?;
                Ok(changed.then(|| source.path.clone()))
            }
            Self::Copy(path) => {
                pdf_edit::export(source, path, store, bookmarks, Some(snapshots))?;
                Ok(Some(path.clone()))
            }
            Self::Folder(path) => {
                pdf_edit::export_to_folder(source, path, store, bookmarks, snapshots).map(Some)
            }
        }
    }
}

impl Finalized {
    fn success_message(&self) -> String {
        match &self.target {
            ExportTarget::Copy(path) => {
                trf!("Copia salvata · {}", "Copy saved · {}", path.display())
            }
            ExportTarget::Original if self.written.is_empty() => {
                tr!("Nessuna modifica da incorporare", "No changes to embed").into()
            }
            ExportTarget::Original if self.written.len() == 1 => {
                tr!("PDF originale aggiornato", "Original PDF updated").into()
            }
            ExportTarget::Original => trf!(
                "{} PDF originali aggiornati",
                "{} original PDFs updated",
                self.written.len()
            ),
            ExportTarget::Folder(_) => trf!(
                "{} copie PDF salvate",
                "{} PDF copies saved",
                self.written.len()
            ),
        }
    }

    fn failure_report(&self) -> String {
        let total = self.written.len() + self.unchanged + self.failed.len();
        let mut report = trf!(
            "Completati {} PDF su {total}.\r\n",
            "Updated {} of {total} PDFs.\r\n",
            self.written.len()
        );
        if self.unchanged > 0 {
            report.push_str(&trf!(
                "PDF senza modifiche da incorporare: {}.\r\n",
                "PDFs with no changes to embed: {}.\r\n",
                self.unchanged
            ));
        }
        for (path, error) in &self.failed {
            report.push_str(&format!("\r\n{}\r\n{error}\r\n", path.display()));
        }
        report
    }
}

pub(super) fn export_dialog(hwnd: HWND, state: &RefCell<Reader>, store: &Store) {
    let (source, setlist) = {
        let mut app = state.borrow_mut();
        if app.exporting.is_some() {
            app.notify(
                hwnd,
                tr!(
                    "Incorporazione già in corso…",
                    "PDF update already running…"
                )
                .into(),
            );
            return;
        }
        if !app.can_finalize() {
            return;
        }
        let Some(source) = app.current_score().map(|score| score.source.clone()) else {
            return;
        };
        app.ink.cancel_contact();
        if !app.save_ink(hwnd) {
            return;
        }
        let setlist: Vec<_> = if app.concert {
            let mut seen = std::collections::HashSet::new();
            app.scores
                .iter()
                .filter(|score| seen.insert(&score.source.path))
                .map(|score| score.source.clone())
                .collect()
        } else {
            Vec::new()
        };
        (source, setlist)
    };
    let choices = || -> Result<_> {
        let sources = if setlist.len() > 1 {
            match crate::dialog::permanent_scope(hwnd, setlist.len())? {
                Some(true) => setlist,
                Some(false) => vec![source],
                None => return Ok(None),
            }
        } else {
            vec![source]
        };
        let target = match crate::dialog::permanent_target(hwnd, sources.len())? {
            Some(true) => ExportTarget::Original,
            Some(false) if sources.len() > 1 => {
                let Some(folder) = choose_destination(hwnd, None)? else {
                    return Ok(None);
                };
                ExportTarget::Folder(folder.canonicalize().map_err(Error::from)?)
            }
            Some(false) => {
                let stem = sources[0]
                    .path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy();
                let Some(file) = choose_destination(hwnd, Some(&format!("{stem} - annotato.pdf")))?
                else {
                    return Ok(None);
                };
                ExportTarget::Copy(file)
            }
            None => return Ok(None),
        };
        Ok(Some((sources, target)))
    };
    let (sources, target) = match choices() {
        Ok(Some(selection)) => selection,
        Ok(None) => return,
        Err(error) => {
            show_startup_error(&error);
            return;
        }
    };
    let mut app = state.borrow_mut();
    let snapshots = app.saver.as_ref().map(Saver::snapshots).unwrap_or_default();
    let store = store.clone();
    let original = target.is_original();
    match background(hwnd, "scrivi-pdf", move || {
        let mut result = Finalized {
            target,
            written: Vec::new(),
            unchanged: 0,
            failed: Vec::new(),
        };
        for source in sources {
            let written = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                result.target.write_pdf(&source, &store, &snapshots)
            }));
            match written {
                Ok(Ok(Some(path))) => result.written.push(path),
                Ok(Ok(None)) => result.unchanged += 1,
                Ok(Err(error)) => result.failed.push((source.path, error.to_string())),
                Err(_) => result.failed.push((
                    source.path,
                    tr!(
                        "Il PDF contiene dati che non è stato possibile elaborare.",
                        "The PDF contains data that couldn’t be processed."
                    )
                    .into(),
                )),
            }
        }
        Ok(result)
    }) {
        Ok(rx) => {
            app.original_export = original;
            app.exporting = Some(rx);
            app.close_drawer(hwnd);
            app.notify(
                hwnd,
                tr!("Incorporazione nel PDF…", "Updating the PDF…").into(),
            );
        }
        Err(error) => app.notify(hwnd, error.to_string()),
    }
}

fn choose_destination(hwnd: HWND, file: Option<&str>) -> Result<Option<PathBuf>> {
    use windows::Win32::UI::Shell::{FOS_PICKFOLDERS, IFileDialog};

    unsafe {
        let save: IFileSaveDialog;
        let folder: IFileOpenDialog;
        let picker: &IFileDialog = if let Some(file) = file {
            save = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?;
            save.SetOptions(save.GetOptions()? | FOS_FORCEFILESYSTEM | FOS_OVERWRITEPROMPT)?;
            save.SetFileTypes(&[COMDLG_FILTERSPEC {
                pszName: trw!("Documento PDF", "PDF document"),
                pszSpec: w!("*.pdf"),
            }])?;
            save.SetDefaultExtension(w!("pdf"))?;
            save.SetFileName(&HSTRING::from(file))?;
            &save
        } else {
            folder = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            folder.SetOptions(folder.GetOptions()? | FOS_FORCEFILESYSTEM | FOS_PICKFOLDERS)?;
            folder.SetTitle(trw!(
                "Cartella per le copie annotate",
                "Folder for annotated copies"
            ))?;
            &folder
        };
        if let Err(error) = picker.Show(Some(hwnd)) {
            return if error.code().0 == 0x800704c7u32 as i32 {
                Ok(None)
            } else {
                Err(error)
            };
        }
        let raw = picker.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
        let path = OsString::from_wide(raw.as_wide());
        CoTaskMemFree(Some(raw.0.cast()));
        Ok(Some(path.into()))
    }
}

pub(super) fn bookmark_dialog(hwnd: HWND, state: &RefCell<Reader>) {
    let (event, caption, title, original_page, maximum) = {
        let mut app = state.borrow_mut();
        let Some(event) = app.bookmark_edit.take() else {
            return;
        };
        let Some(bookmarks) = &app.bookmarks else {
            return;
        };
        let Some(score) = app.current_score() else {
            return;
        };
        let (caption, title, page) = if let Event::BookmarkEdit(index) = event {
            let Some(entry) = bookmarks.entries.get(index) else {
                return;
            };
            (
                tr!("Modifica segnalibro", "Edit bookmark"),
                entry.title.clone(),
                entry.page,
            )
        } else {
            let page = score.pdf_page(app.selected) + 1;
            (
                tr!("Nuovo segnalibro", "New bookmark"),
                trf!("Pagina {page}", "Page {page}"),
                Some(page),
            )
        };
        (event, caption, title, page, score.pdf_count)
    };
    let page = original_page.map_or_else(String::new, |page| page.to_string());
    match crate::dialog::bookmark_fields(hwnd, caption, &title, &page) {
        Ok(Some((value, target))) => {
            let mut app = state.borrow_mut();
            let target = if target.is_empty() {
                original_page
            } else {
                let page = target
                    .parse::<u32>()
                    .ok()
                    .filter(|page| (1..=maximum).contains(page));
                if page.is_none() {
                    app.notify(
                        hwnd,
                        trf!(
                            "Scegli una pagina da 1 a {maximum}.",
                            "Choose a page from 1 to {maximum}."
                        ),
                    );
                    return;
                }
                page
            };
            let Some(mut bookmarks) = app.bookmarks.clone() else {
                return;
            };
            match (event, target) {
                (Event::BookmarkEdit(index), target) => {
                    let Some(entry) = bookmarks.entries.get_mut(index) else {
                        return;
                    };
                    entry.title = value;
                    if let Some(page) = target.filter(|_| target != original_page) {
                        bookmarks.retarget(index, page);
                    }
                    app.save_bookmarks(hwnd, bookmarks);
                }
                (Event::BookmarkAdd, Some(page)) => {
                    let index = bookmarks.add(value, page);
                    app.save_bookmark_structure(hwnd, bookmarks, Some(index));
                }
                (Event::BookmarkChild(parent), Some(page)) => {
                    if let Some(index) = bookmarks.add_child(parent, value, page) {
                        app.save_bookmark_structure(hwnd, bookmarks, Some(index));
                    }
                }
                _ => return,
            }
            repaint(hwnd);
        }
        Ok(None) => {}
        Err(error) => show_startup_error(&error),
    }
}
