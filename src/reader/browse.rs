use super::*;
use std::collections::VecDeque;
use windows::Win32::UI::Controls::{EM_LIMITTEXT, EM_SETCUEBANNER};

pub(super) const SEARCH_ID: u16 = 271;
pub(super) const SEARCH_KEY: u32 = WM_APP + 32;

#[derive(Default)]
pub(super) struct Browser {
    edit: Option<HWND>,
    edit_rect: RECT,
    mode: Option<Mode>,
    home_query: String,
    page_query: String,
    home_scroll: i32,
    page_scroll: i32,
    score: usize,
    hover: Option<POINT>,
    grab: Option<BrowseGrab>,
    previews: HashMap<PageKey, Option<Arc<Frame>>>,
    preview_order: VecDeque<PageKey>,
    requested: Vec<PageKey>,
    cells: RefCell<VecDeque<(GridCell, Surface)>>,
    loading_since: Option<Instant>,
}

#[derive(Clone, Copy, PartialEq)]
struct GridCell {
    key: PageKey,
    width: i32,
    height: i32,
    scale: f32,
    selected: bool,
    hovered: bool,
    ready: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Mode { Home, Pages }

#[derive(Clone, Copy)]
struct BrowseGrab {
    id: u32,
    origin: POINT,
    scroll: i32,
    moved: bool,
    bar: Option<(overlay::Scrollbar, i32)>,
}

#[derive(Clone, Copy)]
struct Home {
    left: RECT,
    panel: RECT,
    search: RECT,
    list: RECT,
    open: RECT,
    setlists: RECT,
    resume: RECT,
    logo: RECT,
    row: i32,
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> RECT {
    RECT { left: x, top: y, right: x + w, bottom: y + h }
}

#[cfg(test)]
pub(super) fn home_logo(layout: &Layout) -> RECT { Home::new(layout).logo }

impl Home {
    fn new(layout: &Layout) -> Self {
        let view = layout.viewport;
        let px = |value| layout.px(value);
        let w = view.right - view.left;
        let split = view.left + w * 48 / 100;
        let left = RECT { left: view.left + px(9), right: split - px(9), ..view };
        let center = (left.left + left.right) / 2;
        let y = ((view.top + view.bottom) / 2 + px(24)).max(view.top + px(175));
        let actions = (left.right - left.left - px(8)).min(px(324)).max(8);
        let action_x = center - actions / 2;
        let half = actions / 2 - px(4);
        let panel = RECT {
            left: split + px(5), right: view.right - px(12),
            top: view.top + px(16), bottom: view.bottom - px(16),
        };
        let search = rect(panel.left + px(13), panel.top + px(45), (panel.right - panel.left - px(26)).max(10), px(29));
        let list = RECT { top: search.bottom + px(8), bottom: panel.bottom - px(9), ..search };
        Self {
            left, panel, search, list,
            open: rect(action_x, y, half, px(43)),
            setlists: rect(center + px(4), y, half, px(43)),
            resume: rect(action_x, y + px(55), actions, px(38)),
            logo: rect(center - px(38), y - px(179), px(76), px(76)),
            row: px(49).max(1),
        }
    }
}

#[derive(Clone, Copy)]
struct Grid {
    search: RECT,
    close: RECT,
    back: RECT,
    next: RECT,
    list: RECT,
    cols: usize,
    cell_width: i32,
    cell_height: i32,
}

impl Grid {
    fn new(layout: &Layout, multi: bool) -> Self {
        let v = layout.viewport;
        let px = |value| layout.px(value);
        let close = rect(v.right - px(50), v.top + px(11), px(40), px(40));
        let search = rect(close.left - px(255), v.top + px(14), px(190), px(33));
        let list = RECT {
            left: v.left + px(14), right: v.right - px(16),
            top: v.top + px(if multi { 152 } else { 103 }), bottom: v.bottom - px(6),
        };
        let width = (list.right - list.left).max(1);
        let cols = (width / px(206).max(1)).max(1) as usize;
        let nav_width = ((width - px(10)) / 2).min(px(268));
        Self {
            close, search, list, cols,
            back: rect(list.left, v.top + px(93), nav_width, px(47)),
            next: rect(list.right - nav_width, v.top + px(93), nav_width, px(47)),
            cell_width: width / cols as i32,
            cell_height: px(262).max(1),
        }
    }

    fn cell(self, index: usize, scroll: i32) -> RECT {
        rect(
            self.list.left + (index % self.cols) as i32 * self.cell_width,
            self.list.top + (index / self.cols) as i32 * self.cell_height - scroll,
            self.cell_width, self.cell_height,
        )
    }

    fn card(self, index: usize, scroll: i32, layout: &Layout) -> RECT {
        let cell = self.cell(index, scroll);
        RECT {
            left: cell.left + layout.px(5),
            top: cell.top + layout.px(5),
            right: cell.right - layout.px(5),
            bottom: cell.bottom - layout.px(6),
        }
    }

    fn visible(self, count: usize, scroll: i32) -> std::ops::Range<usize> {
        let first = (scroll / self.cell_height) as usize * self.cols;
        let rows = ((self.list.bottom - self.list.top).max(0) / self.cell_height + 2) as usize;
        first.min(count)..(first + rows * self.cols).min(count)
    }
}

fn entry_name(entry: &ReadingPosition) -> String {
    entry.path.file_stem().unwrap_or(entry.path.as_os_str()).to_string_lossy().into_owned()
}

fn max_scroll(bounds: RECT, height: i32) -> i32 {
    (height - (bounds.bottom - bounds.top)).max(0)
}

fn scroll_bar(bounds: RECT, height: i32, scroll: i32, layout: &Layout) -> Option<overlay::Scrollbar> {
    let left = bounds.right + layout.px(1);
    let track = RECT { left, right: left + layout.px(12).max(4), ..bounds };
    overlay::Scrollbar::new(track, max_scroll(bounds, height) as f32, scroll as f32, layout.px(24), 1.0)
}

impl Reader {
    fn browse_mode(&self) -> Option<Mode> {
        if self.overlay.drawer && self.overlay.sheet == Some(Sheet::Page) {
            Some(Mode::Pages)
        } else if self.total == 0 && self.status.is_empty() {
            Some(Mode::Home)
        } else {
            None
        }
    }

    pub(super) fn page_grid_open(&self) -> bool { self.browse_mode() == Some(Mode::Pages) }

    pub(super) fn start_page_grid(&mut self, hwnd: HWND) {
        let _ = unsafe { KillTimer(Some(hwnd), THUMBNAIL_WARMUP_TIMER) };
        self.browser.score = self.scores.iter().rposition(|score| score.first_page <= self.selected).unwrap_or(0);
        self.browser.page_query.clear();
        self.browser.page_scroll = 0;
        let layout = Layout::new(hwnd, self.fullscreen.is_some());
        let grid = Grid::new(&layout, self.scores.len() > 1);
        if let Some(score) = self.scores.get(self.browser.score) {
            let row = (self.selected - score.first_page) as usize / grid.cols;
            self.browser.page_scroll = (row as i32).saturating_mul(grid.cell_height);
        }
        self.radial.open = false;
        self.sync_search(hwnd, &layout);
        self.update_thumbnails(hwnd, &layout);
    }

    pub(super) fn leave_page_grid(&mut self) {
        self.browser.grab = None;
        self.cancel_thumbnails();
    }

    pub(super) fn cancel_thumbnails(&mut self) {
        if !self.browser.requested.is_empty() {
            self.request_thumbnails(Vec::new());
        }
    }

    pub(super) fn reset_thumbnails(&mut self) {
        self.leave_page_grid();
        self.browser.previews.clear();
        self.browser.preview_order.clear();
        self.browser.cells.get_mut().clear();
    }

    fn search_text(&self, mode: Mode) -> &str {
        match mode { Mode::Home => &self.browser.home_query, Mode::Pages => &self.browser.page_query }
    }

    pub(super) fn sync_search(&mut self, hwnd: HWND, layout: &Layout) {
        if hwnd.0.is_null() { return; }
        let mode = self.browse_mode();
        let Some(mode) = mode else {
            if self.browser.mode.take().is_some() && let Some(edit) = self.browser.edit {
                unsafe { let _ = ShowWindow(edit, SW_HIDE); }
            }
            return;
        };
        let bounds = match mode { Mode::Home => Home::new(layout).search, Mode::Pages => Grid::new(layout, self.scores.len() > 1).search };
        unsafe {
            if self.browser.edit.is_none() {
                let Ok(edit) = CreateWindowExW(
                    WINDOW_EX_STYLE::default(), w!("EDIT"), None,
                    WS_CHILD | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                    0, 0, 1, 1, Some(hwnd), Some(HMENU(usize::from(SEARCH_ID) as _)), None, None,
                ) else { return };
                SendMessageW(edit, WM_SETFONT, Some(WPARAM(GetStockObject(DEFAULT_GUI_FONT).0 as usize)), Some(LPARAM(1)));
                SendMessageW(edit, EM_LIMITTEXT, Some(WPARAM(100)), None);
                let _ = windows::Win32::UI::Shell::SetWindowSubclass(edit, Some(search_proc), 1, 0);
                self.browser.edit = Some(edit);
            }
            let edit = self.browser.edit.unwrap();
            let changed = self.browser.mode.replace(mode) != Some(mode);
            if changed {
                let cue = HSTRING::from(match mode {
                    Mode::Home => tr!("Cerca nei recenti", "Search recent PDFs"),
                    Mode::Pages => tr!("Cerca una pagina", "Find a page"),
                });
                SendMessageW(edit, EM_SETCUEBANNER, Some(WPARAM(0)), Some(LPARAM(cue.as_ptr() as isize)));
                let _ = SetWindowTextW(edit, &HSTRING::from(self.search_text(mode)));
            }
            if !changed && self.browser.edit_rect == bounds { return; }
            self.browser.edit_rect = bounds;
            let height = layout.px(18).max(17).min(bounds.bottom - bounds.top);
            let left = layout.px(if mode == Mode::Pages { 33 } else { 8 });
            let _ = SetWindowPos(edit, None, bounds.left + left, (bounds.top + bounds.bottom - height) / 2,
                (bounds.right - bounds.left - left - layout.px(8)).max(1), height, SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }

    pub(super) fn search_command(&mut self, hwnd: HWND, wp: WPARAM) -> bool {
        if wp.0 as u16 != SEARCH_ID || (wp.0 >> 16) as u32 != EN_CHANGE { return false; }
        let value = crate::dialog::value(hwnd, SEARCH_ID);
        match self.browser.mode {
            Some(Mode::Home) => {
                self.browser.home_query = value;
                self.browser.home_scroll = 0;
            }
            Some(Mode::Pages) => {
                self.browser.page_query = value;
                self.browser.page_scroll = 0;
                self.update_thumbnails(hwnd, &Layout::new(hwnd, self.fullscreen.is_some()));
            }
            None => {}
        }
        repaint(hwnd);
        true
    }

    pub(super) fn search_key(&mut self, hwnd: HWND, key: usize) {
        match (self.browse_mode(), key) {
            (Some(Mode::Pages), 13) => {
                if let Some((first, 1)) = self.grid_pages() { self.select_grid_page(hwnd, first); }
            }
            (Some(Mode::Pages), 27) => self.close_drawer(hwnd),
            (Some(Mode::Home), 13) => {
                if let Some(index) = self.recent_indices().first().copied() { self.resume_recent(hwnd, index); }
            }
            _ => {}
        }
    }

    pub(super) fn focus_browse_search(&mut self, hwnd: HWND) -> bool {
        if self.browse_mode().is_none() {
            return false;
        }
        self.sync_search(hwnd, &Layout::new(hwnd, self.fullscreen.is_some()));
        if let Some(edit) = self.browser.edit {
            let _ = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(edit)) };
        }
        true
    }

    pub(super) fn grid_key_down(&mut self, hwnd: HWND, key: usize) {
        if key == 13 {
            self.search_key(hwnd, key);
            return;
        }
        let layout = Layout::new(hwnd, self.fullscreen.is_some());
        let grid = Grid::new(&layout, self.scores.len() > 1);
        let distance = match key {
            0x26 => -layout.px(54),
            0x28 => layout.px(54),
            0x21 => grid.list.top - grid.list.bottom,
            0x22 => grid.list.bottom - grid.list.top,
            _ => return,
        };
        self.browser.page_scroll = self.browser.page_scroll.saturating_add(distance);
        self.update_thumbnails(hwnd, &layout);
        repaint(hwnd);
    }

    fn recent_indices(&self) -> Vec<usize> {
        let query = self.browser.home_query.trim().to_lowercase();
        self.recent.iter().enumerate().filter_map(|(index, item)| {
            (query.is_empty() || entry_name(item).to_lowercase().contains(&query))
                .then_some(index)
        }).collect()
    }

    fn home_click(&mut self, hwnd: HWND, home: Home, point: POINT, layout: &Layout) {
        if inside(home.open, point) { request_file_picker(hwnd); }
        else if inside(home.setlists, point) { post(hwnd, OPEN_LIBRARY); }
        else if inside(home.resume, point) { self.resume_recent(hwnd, 0); }
        else if inside(home.list, point) {
            let scroll = self.home_scroll(home);
            let row = (point.y - home.list.top + scroll) / home.row;
            if let Some(index) = self.recent_indices().get(row as usize).copied() {
                if point.x >= home.list.right - layout.px(36) {
                    let mut recent = self.recent.clone();
                    recent.remove(index);
                    if let Some(store) = &self.store
                        && let Err(error) = store.save_recent(&recent)
                    {
                        self.notify(hwnd, error.to_string());
                        return;
                    }
                    self.recent = recent;
                    self.browser.home_scroll = self.home_scroll(home);
                    repaint(hwnd);
                } else {
                    self.resume_recent(hwnd, index);
                }
            }
        }
    }

    fn resume_recent(&mut self, hwnd: HWND, index: usize) {
        let Some(entry) = self.recent.get(index).cloned() else { return };
        let session = self.session;
        self.open(hwnd, vec![entry.path], false);
        if self.session != session { self.resume_page = Some(entry.page); }
    }

    fn home_scroll(&self, home: Home) -> i32 {
        self.browser.home_scroll.clamp(0, max_scroll(home.list, self.recent_indices().len() as i32 * home.row))
    }

    fn grid_pages(&self) -> Option<(u32, usize)> {
        let score = self.scores.get(self.browser.score)?;
        let query = self.browser.page_query.trim();
        if query.is_empty() { return Some((score.first_page, score.count as usize)); }
        let page = query.parse::<u32>().ok()?.checked_sub(1)?;
        Some((score.index(page)?, 1))
    }

    fn grid_scroll(&self, grid: Grid, count: usize) -> i32 {
        let total = (count.div_ceil(grid.cols) as i32).saturating_mul(grid.cell_height);
        self.browser.page_scroll.clamp(0, max_scroll(grid.list, total))
    }

    fn grid_key(&self, index: u32) -> PageKey {
        PageKey { session: self.session, index, width: 300, height: 380 }
    }

    pub(super) fn schedule_thumbnail_warmup(&self, hwnd: HWND) {
        unsafe { SetTimer(Some(hwnd), THUMBNAIL_WARMUP_TIMER, 650, None) };
    }

    pub(super) fn warm_thumbnails(&mut self, hwnd: HWND) {
        let _ = unsafe { KillTimer(Some(hwnd), THUMBNAIL_WARMUP_TIMER) };
        if self.page_grid_open() || self.editable_frame().is_none() { return; }
        if self.view_moving || self.ink.is_drawing() || self.page_animation.is_some()
            || self.page_drag.is_some() || self.desired.is_some_and(|key| self.queued.contains(&key))
        {
            self.schedule_thumbnail_warmup(hwnd);
            return;
        }
        let Some(score) = self.current_score() else { return };
        let layout = Layout::new(hwnd, self.fullscreen.is_some());
        let columns = Grid::new(&layout, self.scores.len() > 1).cols as u32;
        let row = (self.selected - score.first_page) / columns * columns + score.first_page;
        let end = (row + (columns * 2).min(24)).min(score.first_page + score.count);
        let pages = std::iter::once(self.selected).chain((row..end).filter(|&page| page != self.selected));
        self.request_thumbnails(pages.map(|index| self.grid_key(index)).collect());
    }

    pub(super) fn update_thumbnails(&mut self, hwnd: HWND, layout: &Layout) {
        if !self.page_grid_open() { return; }
        let grid = Grid::new(layout, self.scores.len() > 1);
        let (first, count) = self.grid_pages().unwrap_or_default();
        let scroll = self.grid_scroll(grid, count);
        self.browser.page_scroll = scroll;
        let visible = grid.visible(count, scroll);
        let ahead = visible.end..(visible.end + grid.cols).min(count);
        let behind = visible.start.saturating_sub(grid.cols)..visible.start;
        let selected = self.selected.checked_sub(first).map(|index| index as usize)
            .filter(|index| visible.contains(index));
        let pages = selected.into_iter()
            .chain(visible.filter(|index| Some(*index) != selected))
            .chain(ahead).chain(behind)
            .map(|index| self.grid_key(first + index as u32))
            .collect();
        self.request_thumbnails(pages);
        if !self.browser.requested.is_empty() && self.browser.loading_since.is_none() {
            self.browser.loading_since = Some(Instant::now());
            unsafe { SetTimer(Some(hwnd), GRID_LOADING_TIMER, 85, None) };
        }
    }

    pub(super) fn tick_grid_loading(&mut self, hwnd: HWND) {
        if !self.page_grid_open() || self.browser.requested.is_empty() {
            self.browser.loading_since = None;
            let _ = unsafe { KillTimer(Some(hwnd), GRID_LOADING_TIMER) };
            return;
        }
        let layout = Layout::new(hwnd, self.fullscreen.is_some());
        let grid = Grid::new(&layout, self.scores.len() > 1);
        let (first, count) = self.grid_pages().unwrap_or_default();
        let scroll = self.grid_scroll(grid, count);
        for index in grid.visible(count, scroll) {
            let key = self.grid_key(first + index as u32);
            if self.browser.requested.contains(&key)
                && let Some(rect) = intersect(grid.list, loading_bounds(grid.card(index, scroll, &layout), &layout))
            {
                repaint_rect(hwnd, rect);
            }
        }
    }

    fn request_thumbnails(&mut self, mut missing: Vec<PageKey>) {
        missing.retain(|key| !self.browser.previews.contains_key(key));
        if missing != self.browser.requested {
            self.browser.requested = missing;
            if let Some(worker) = &self.requests {
                let _ = worker.send(Request::Thumbnails(self.browser.requested.clone()));
            }
        }
    }

    pub(super) fn thumbnail(&mut self, hwnd: HWND, key: PageKey, frame: Option<Arc<Frame>>) {
        if key.session != self.session || !self.browser.requested.contains(&key) { return; }
        self.browser.requested.retain(|requested| *requested != key);
        self.browser.preview_order.push_back(key);
        self.browser.previews.insert(key, frame);
        self.browser.cells.get_mut().retain(|(cell, _)| cell.key != key);
        if self.browser.preview_order.len() > 96
            && let Some(oldest) = self.browser.preview_order.pop_front()
        {
            self.browser.previews.remove(&oldest);
        }
        if self.page_grid_open()
            && let Some((first, count)) = self.grid_pages()
            && let Some(index) = key.index.checked_sub(first).map(|index| index as usize)
        {
            let layout = Layout::new(hwnd, self.fullscreen.is_some());
            let grid = Grid::new(&layout, self.scores.len() > 1);
            let scroll = self.grid_scroll(grid, count);
            if index < count && let Some(damage) = intersect(grid.cell(index, scroll), grid.list) {
                repaint_rect(hwnd, damage);
            }
        }
    }

    fn grid_click(&mut self, hwnd: HWND, grid: Grid, point: POINT, layout: &Layout) {
        if inside(grid.close, point) { self.close_drawer(hwnd); return; }
        let step = if self.scores.len() < 2 { 0 } else if inside(grid.back, point) { -1 } else if inside(grid.next, point) { 1 } else { 0 };
        let score = self.browser.score.saturating_add_signed(step);
        if step != 0 && score != self.browser.score && score < self.scores.len() {
            self.browser.score = score;
            self.browser.page_scroll = 0;
            self.browser.page_query.clear();
            if let Some(edit) = self.browser.edit { unsafe { let _ = SetWindowTextW(edit, w!("")); } }
            self.update_thumbnails(hwnd, layout);
            repaint(hwnd);
            return;
        }
        if !inside(grid.list, point) { return; }
        let Some((first, count)) = self.grid_pages() else { return };
        let scroll = self.grid_scroll(grid, count);
        for index in grid.visible(count, scroll) {
            if inside(grid.cell(index, scroll), point) {
                self.select_grid_page(hwnd, first + index as u32);
                break;
            }
        }
    }

    fn select_grid_page(&mut self, hwnd: HWND, index: u32) {
        self.close_drawer(hwnd);
        self.go_to(hwnd, index);
    }

    pub(super) fn browse_wheel(&mut self, hwnd: HWND, point: POINT, delta: i32) -> bool {
        let layout = Layout::new(hwnd, self.fullscreen.is_some());
        match self.browse_mode() {
            Some(Mode::Home) if inside(Home::new(&layout).list, point) => {
                let scroll = self.home_scroll(Home::new(&layout)).saturating_sub(delta);
                self.scroll_browse(hwnd, Mode::Home, &layout, scroll);
            }
            Some(Mode::Pages) if inside(layout.viewport, point) => {
                let grid = Grid::new(&layout, self.scores.len() > 1);
                let scroll = self.grid_scroll(grid, self.grid_pages().map_or(0, |(_, count)| count)).saturating_sub(delta);
                self.scroll_browse(hwnd, Mode::Pages, &layout, scroll);
            }
            _ => return false,
        }
        true
    }

    pub(super) fn browse_cancel(&mut self, id: u32) {
        if self.browser.grab.is_some_and(|grab| grab.id == id) { self.browser.grab = None; }
    }

    pub(super) fn browse_contact(&mut self, hwnd: HWND, msg: u32, id: u32, point: POINT) -> bool {
        let Some(mode) = self.browse_mode() else { return false };
        let layout = Layout::new(hwnd, self.fullscreen.is_some());
        let home = Home::new(&layout);
        let grid = Grid::new(&layout, self.scores.len() > 1);
        if self.browser.grab.is_some_and(|grab| grab.id != id) { return inside(layout.viewport, point); }
        let count = self.grid_pages().map_or(0, |(_, count)| count);
        match msg {
            WM_POINTERDOWN if inside(layout.viewport, point) => {
                // A title-bar menu or floating panel gets first chance at a press.
                let (list, height, scroll) = match mode {
                    Mode::Home => (home.list, self.recent_indices().len() as i32 * home.row, self.home_scroll(home)),
                    Mode::Pages => (
                        grid.list,
                        (count.div_ceil(grid.cols) as i32).saturating_mul(grid.cell_height),
                        self.grid_scroll(grid, count),
                    ),
                };
                let bar = scroll_bar(list, height, scroll, &layout)
                    .filter(|bar| inside(bar.track, point))
                    .map(|bar| (bar, bar.grab(point.y)));
                self.browser.grab = Some(BrowseGrab { id, origin: point, scroll, moved: bar.is_some(), bar });
                if let Some((bar, offset)) = bar
                    && !(bar.thumb.top..bar.thumb.bottom).contains(&point.y)
                {
                    self.scroll_browse(hwnd, mode, &layout, bar.scroll(point.y, offset).round() as i32);
                }
            }
            WM_POINTERUPDATE => {
                let Some(grab) = &mut self.browser.grab else { return false };
                let list = if mode == Mode::Home { home.list } else { grid.list };
                let dy = point.y - grab.origin.y;
                if let Some((bar, offset)) = grab.bar {
                    self.scroll_browse(hwnd, mode, &layout, bar.scroll(point.y, offset).round() as i32);
                } else if inside(list, grab.origin) && (grab.moved || dy.abs() > layout.px(5)) {
                    grab.moved = true;
                    let scroll = grab.scroll.saturating_sub(dy);
                    self.scroll_browse(hwnd, mode, &layout, scroll);
                }
            }
            WM_POINTERUP => {
                let Some(grab) = self.browser.grab.take() else { return false };
                if !grab.moved {
                    match mode {
                        Mode::Home => self.home_click(hwnd, home, point, &layout),
                        Mode::Pages => self.grid_click(hwnd, grid, point, &layout),
                    }
                }
            }
            _ => return false,
        }
        true
    }

    fn scroll_browse(&mut self, hwnd: HWND, mode: Mode, layout: &Layout, scroll: i32) {
        let list = if mode == Mode::Home {
            self.browser.home_scroll = scroll;
            self.browser.home_scroll = self.home_scroll(Home::new(layout));
            Home::new(layout).list
        } else {
            self.browser.page_scroll = scroll;
            self.update_thumbnails(hwnd, layout);
            Grid::new(layout, self.scores.len() > 1).list
        };
        repaint_rect(hwnd, RECT { right: list.right + layout.px(14), ..list });
    }

    pub(super) fn browse_hover(&mut self, hwnd: HWND, point: Option<POINT>) {
        if self.browse_mode().is_some() && self.browser.hover != point {
            let previous = std::mem::replace(&mut self.browser.hover, point);
            let layout = Layout::new(hwnd, self.fullscreen.is_some());
            if self.page_grid_open() {
                let grid = Grid::new(&layout, self.scores.len() > 1);
                let count = self.grid_pages().map_or(0, |(_, count)| count);
                let scroll = self.grid_scroll(grid, count);
                let hit = |point: POINT| {
                    [grid.back, grid.next].into_iter()
                        .filter(|_| self.scores.len() > 1)
                        .chain(grid.visible(count, scroll)
                            .filter_map(|index| intersect(grid.card(index, scroll, &layout), grid.list)))
                        .find(|rect| inside(*rect, point))
                };
                let old = previous.and_then(hit);
                let new = point.and_then(hit);
                if old != new {
                    for rect in old.into_iter().chain(new) {
                        repaint_rect(hwnd, rect);
                    }
                }
            } else {
                repaint_rect(hwnd, Home::new(&layout).list);
            }
        }
    }

    pub(super) fn draw_home(&self, dc: HDC, layout: &Layout, damage: RECT) {
        let home = Home::new(layout);
        let px = |value| layout.px(value);
        if let Some(logo) = &self.welcome_logo
            && let Some(clip) = intersect(home.logo, damage)
            && let Some(canvas) = Canvas::new(dc)
        {
            canvas.clip(clip);
            canvas.image(logo, home.logo);
        }
        text(dc, RECT { top: home.open.top - px(99), bottom: home.open.top - px(41), ..home.left }, "ScoreDraw", TEXT, px(42));
        text(dc, RECT { top: home.open.top - px(44), bottom: home.open.top - px(12), ..home.left },
            tr!("I tuoi spartiti. Il tuo spazio.", "Your scores. Your space."), ACCENT, px(18));
        if !self.status.is_empty() {
            text(dc, RECT { top: home.open.top, bottom: home.open.bottom, ..home.left }, &self.status, MUTED, px(16));
            return;
        }
        button(dc, home.open, tr!("Apri PDF", "Open PDF"), true, layout.scale);
        button(dc, home.setlists, tr!("Scalette", "Setlists"), false, layout.scale);
        if let Some(entry) = self.recent.first() {
            button(dc, home.resume, tr!("Riprendi lettura", "Resume reading"), false, layout.scale);
            text(dc, RECT { top: home.resume.bottom + px(3), bottom: home.resume.bottom + px(34), ..home.resume },
                &trf!("{} · Pagina {}", "{} · Page {}", entry_name(entry), entry.page + 1), MUTED, px(13));
        }
        rounded(dc, home.panel, PANEL, px(22));
        text_left(dc, RECT { left: home.search.left, top: home.panel.top + px(8), bottom: home.search.top, ..home.search },
            tr!("PDF recenti", "Recent PDFs"), TEXT, px(20));
        rounded(dc, home.search, SURFACE, px(12));
        let hits = self.recent_indices();
        if hits.is_empty() {
            text(dc, home.list, if self.recent.is_empty() { tr!("Nessun PDF recente", "No recent PDFs") }
                else { tr!("Nessun risultato", "No results") }, MUTED, px(14));
            return;
        }
        let scroll = self.home_scroll(home);
        with_clip(dc, home.list, || {
            for (row, index) in hits.iter().enumerate().skip((scroll / home.row) as usize) {
                let top = home.list.top + row as i32 * home.row - scroll;
                if top >= home.list.bottom { break; }
                let bounds = RECT { top, bottom: top + home.row - px(2), ..home.list };
                let hovered = self.browser.hover.is_some_and(|point| inside(bounds, point));
                if hovered { rounded(dc, bounds, SURFACE, px(12)); }
                icons::draw(dc, Glyph::FileText, RECT { right: bounds.left + px(28), ..bounds }, px(16) as f32, ACCENT);
                let body = RECT { left: bounds.left + px(30), right: bounds.right - px(37), ..bounds };
                let entry = &self.recent[*index];
                text_left(dc, RECT { top: top + px(4), bottom: top + px(25), ..body }, &entry_name(entry), TEXT, px(14));
                text_left(dc, RECT { top: top + px(24), bottom: bounds.bottom, ..body },
                    &trf!("Pagina {}", "Page {}", entry.page + 1), MUTED, px(12));
                icon_button(dc, RECT { left: bounds.right - px(35), top: top + px(7), bottom: bounds.bottom - px(6), ..bounds }, Glyph::Trash, true, layout.scale);
                fill(dc, RECT { top: bounds.bottom, bottom: bounds.bottom + 1, ..body }, SURFACE);
            }
        });
        if let Some(bar) = scroll_bar(home.list, hits.len() as i32 * home.row, scroll, layout) {
            draw_scrollbar(dc, bar, layout.scale);
        }
    }

    pub(super) fn draw_page_grid(&self, dc: HDC, layout: &Layout, damage: RECT) {
        let grid = Grid::new(layout, self.scores.len() > 1);
        let v = layout.viewport;
        let px = |value| layout.px(value);
        let Some(score) = self.scores.get(self.browser.score) else { return };
        if damage.top < grid.list.top {
            text_left(dc, RECT { left: v.left + px(15), right: grid.search.left - px(5), top: v.top + px(6), bottom: v.top + px(54) },
                tr!("Vai a pagina", "Go to page"), TEXT, px(23));
            rounded(dc, grid.search, SURFACE, px(14));
            icons::draw(dc, Glyph::Search, RECT { right: grid.search.left + px(31), ..grid.search },
                px(17) as f32, MUTED);
            icon_button(dc, grid.close, Glyph::X, true, layout.scale);
            text(dc, RECT { top: v.top + px(56), bottom: v.top + px(89), ..grid.list },
                &score.title, MUTED, px(15));
            fill(dc, RECT { top: grid.list.top - px(5), bottom: grid.list.top - px(4), ..grid.list }, SURFACE);
            if self.scores.len() > 1 {
                let previous = self.browser.score.checked_sub(1).and_then(|index| self.scores.get(index));
                let next = self.scores.get(self.browser.score + 1);
                for (bounds, glyph, label, neighbor) in [
                    (grid.back, Glyph::ChevronLeft, tr!("Spartito precedente", "Previous score"), previous),
                    (grid.next, Glyph::ChevronRight, tr!("Spartito successivo", "Next score"), next),
                ] {
                    let enabled = neighbor.is_some();
                    let hot = enabled && self.browser.hover.is_some_and(|point| inside(bounds, point));
                    rounded(dc, bounds, if hot { blend_color(SURFACE, ACCENT, 15) } else if enabled { SURFACE } else { PANEL }, px(15));
                    let glyph_bounds = RECT { right: bounds.left + px(35), ..bounds };
                    icons::draw(dc, glyph, glyph_bounds, px(19) as f32, if enabled { ACCENT } else { MUTED });
                    let side = RECT { left: glyph_bounds.right, right: bounds.right - px(8), ..bounds };
                    let label_bounds = if enabled { RECT { bottom: bounds.top + px(26), ..side } } else { side };
                    text_left(dc, label_bounds, label, if enabled { TEXT } else { MUTED }, px(14));
                    if let Some(neighbor) = neighbor {
                        text_left(dc, RECT { top: bounds.top + px(24), bottom: bounds.bottom - px(2), ..side },
                            &neighbor.title, MUTED, px(12));
                    }
                }
            }
        }
        let Some((first, count)) = self.grid_pages() else {
            text(dc, grid.list, tr!("Nessuna pagina corrispondente", "No matching page"), MUTED, px(16));
            return;
        };
        let scroll = self.grid_scroll(grid, count);
        with_clip(dc, grid.list, || {
            for index in grid.visible(count, scroll) {
                let global = first + index as u32;
                let cell = grid.cell(index, scroll);
                if intersect(cell, damage).is_none() { continue; }
                let card = grid.card(index, scroll, layout);
                let key = self.grid_key(global);
                let frame = self.browser.previews.get(&key).and_then(Option::as_deref);
                let style = GridCell {
                    key,
                    width: card.right - card.left,
                    height: card.bottom - card.top,
                    scale: layout.scale,
                    selected: self.selected == global,
                    hovered: self.browser.hover.is_some_and(|point| inside(card, point)),
                    ready: frame.is_some(),
                };
                self.browser.paint_cell(dc, layout, card, style, frame, score.pdf_page(global) + 1);
                if frame.is_none() {
                    let area = loading_bounds(card, layout);
                    if intersect(area, damage).is_some() {
                        let color = if style.selected { BACKGROUND } else { MUTED };
                        if self.browser.requested.contains(&key) && let Some(start) = self.browser.loading_since {
                            draw_grid_loading(dc, area, color, start.elapsed());
                        } else {
                            icons::draw(dc, Glyph::FileText, area, px(22) as f32, color);
                        }
                    }
                }
            }
        });
        if let Some(bar) = scroll_bar(grid.list, (count.div_ceil(grid.cols) as i32).saturating_mul(grid.cell_height), scroll, layout) {
            draw_scrollbar(dc, bar, layout.scale);
        }
    }
}

impl Browser {
    fn paint_cell(&self, dc: HDC, layout: &Layout, card: RECT, style: GridCell, frame: Option<&Frame>, page: u32) {
        let px = |value| layout.px(value);
        let draw = |dc, card: RECT| {
            fill(dc, card, BACKGROUND);
            rounded(dc, card, if style.selected { ACCENT } else if style.hovered { SURFACE } else { PANEL }, px(15));
            let preview = RECT { left: card.left + px(11), right: card.right - px(11),
                top: card.top + px(10), bottom: card.bottom - px(31) };
            if let Some(frame) = frame {
                draw_bitmap(dc, fit(frame, preview), frame);
            }
            text(dc, RECT { top: preview.bottom, ..card }, &page.to_string(),
                if style.selected { BACKGROUND } else { TEXT }, px(15));
        };
        let mut cells = self.cells.borrow_mut();
        let surface = if let Some(index) = cells.iter().position(|(key, _)| *key == style) {
            cells.remove(index).unwrap().1
        } else if let Some(surface) = Surface::new(dc, style.width, style.height) {
            draw(surface.dc, rect(0, 0, style.width, style.height));
            surface
        } else {
            draw(dc, card);
            return;
        };
        surface.copy_to(dc, card, 0, 0);
        cells.push_back((style, surface));
        if cells.len() > 64 {
            cells.pop_front();
        }
    }
}

fn loading_bounds(card: RECT, layout: &Layout) -> RECT {
    let radius = layout.px(15);
    let x = (card.left + card.right) / 2;
    let y = (card.top + card.bottom - layout.px(21)) / 2;
    rect(x - radius, y - radius, 2 * radius, 2 * radius)
}

fn draw_grid_loading(dc: HDC, area: RECT, color: COLORREF, elapsed: Duration) {
    let Some(canvas) = Canvas::new(dc) else { return };
    let radius = (area.right - area.left) as f32 * 0.34;
    let x = (area.left + area.right) as f32 / 2.0;
    let y = (area.top + area.bottom) as f32 / 2.0;
    let step = elapsed.as_millis() / 85 % 8;
    for dot in 0..8 {
        let angle = dot as f32 * std::f32::consts::TAU / 8.0;
        let alpha = 32 + ((dot + 8 - step) % 8) as u8 * 23;
        let cx = (x + radius * angle.cos()).round() as i32;
        let cy = (y + radius * angle.sin()).round() as i32;
        let size = (radius * 0.22).round().max(1.0) as i32;
        canvas.ellipse(rect(cx - size, cy - size, size * 2, size * 2), canvas::argb(color, alpha), 0, 0.0);
    }
}

unsafe extern "system" fn search_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _: usize, _: usize) -> LRESULT {
    unsafe {
        if msg == WM_KEYDOWN && matches!(wp.0, 13 | 27) {
            if let Ok(parent) = GetParent(hwnd) {
                let _ = PostMessageW(Some(parent), SEARCH_KEY, wp, LPARAM(0));
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(parent));
            }
            return LRESULT(0);
        }
        if msg == WM_CHAR && matches!(wp.0, 13 | 27) { return LRESULT(0); }
        if msg == WM_NCDESTROY {
            let _ = windows::Win32::UI::Shell::RemoveWindowSubclass(hwnd, Some(search_proc), 1);
        }
        windows::Win32::UI::Shell::DefSubclassProc(hwnd, msg, wp, lp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_search_preserves_order_and_matches_file_names() {
        let mut reader = Reader {
            recent: ["Sonata.pdf", "Suite.pdf", "Sonata breve.pdf"].map(|name| ReadingPosition {
                path: PathBuf::from(name),
                page: 0,
            }).into(),
            ..Default::default()
        };
        reader.browser.home_query = " SONATA ".into();
        assert_eq!(reader.recent_indices(), [0, 2]);
        reader.browser.home_query = "assente".into();
        assert!(reader.recent_indices().is_empty());
    }

    #[test]
    fn scrollbar_drag_reaches_both_ends_and_keeps_the_grab_offset() {
        let track = rect(0, 20, 12, 200);
        let bar = overlay::Scrollbar::new(track, 900.0, 450.0, 24, 1.0).unwrap();
        let y = bar.thumb.top + 8;
        let grab = bar.grab(y);
        assert_eq!(grab, 8);
        assert!((bar.scroll(y, grab) - 450.0).abs() < 3.0);
        assert_eq!(bar.scroll(track.top - 10, grab), 0.0);
        assert_eq!(bar.scroll(track.bottom + 10, grab), 900.0);
    }

    #[test]
    fn grid_scroll_reuses_a_composed_thumbnail_cell() {
        let _drawing = canvas::Startup::new().unwrap();
        let screen = unsafe { GetDC(None) };
        let area = rect(0, 0, 320, 400);
        let target = Surface::new(screen, 320, 400).unwrap();
        unsafe { ReleaseDC(None, screen); }
        let layout = Layout { client: area, viewport: area, strip: None, scale: 1.0, dpi: 1.0 };
        let style = GridCell { key: PageKey { session: 1, index: 0, width: 16, height: 16 },
            width: 170, height: 210, scale: 1.0, selected: false, hovered: false, ready: true };
        let frame = Frame { width: 16, height: 16, pixels: vec![255; 16 * 16 * 4] };
        let browser = Browser::default();
        browser.paint_cell(target.dc, &layout, rect(6, 100, 170, 210), style, Some(&frame), 1);
        let first = browser.cells.borrow().front().unwrap().1.id;
        browser.paint_cell(target.dc, &layout, rect(6, 88, 170, 210), style, Some(&frame), 1);
        assert_eq!(browser.cells.borrow().front().unwrap().1.id, first);
    }

    #[test]
    fn thumbnail_grid_only_requests_rows_near_the_viewport() {
        let viewport = rect(0, 0, 940, 740);
        let layout = Layout { client: viewport, viewport, strip: None, scale: 1.0, dpi: 1.0 };
        let grid = Grid::new(&layout, false);
        let pages = grid.visible(1000, 4 * grid.cell_height);
        assert_eq!(pages.start, 4 * grid.cols);
        assert!(pages.len() < 30);
        assert_eq!(grid.cell(pages.start, 4 * grid.cell_height).top, grid.list.top);
    }
}
