use crate::data::{ScoreEntry, Setlist, Store};
use crate::dialog::{self, Control, LIST};
use crate::icons::{self, Glyph};
use crate::reader::{ACCENT, ALERT, BACKGROUND, MUTED, PANEL, SURFACE, TEXT, Surface, fill, inside, point_from_mouse, rounded, text, text_left};
use std::cell::RefCell;
use std::path::Path;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, HDC, InvalidateRect, PAINTSTRUCT};
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_DISABLED, ODS_FOCUS, ODS_SELECTED, ODT_BUTTON, ODT_LISTBOX, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;

const LISTS: u16 = 101;
const SCORES: u16 = 102;
const STATUS: u16 = 104;
const CREATE: u16 = 110;
const RENAME: u16 = 111;
const DELETE: u16 = 112;
const ADD_PDFS: u16 = 120;
const REMOVE: u16 = 124;
const LOCATE: u16 = 125;
const RANGE: u16 = 126;
const PLAY: u16 = 1;
const CANCEL: u16 = 2;
const LISTS_PANEL: [i32; 4] = [12, 37, 119, 167];
const SCORES_PANEL: [i32; 4] = [137, 37, 277, 167];

pub fn show(owner: HWND, store: &Store) -> Result<Option<Vec<ScoreEntry>>, String> {
    let lists = store.setlists().map_err(|error| error.to_string())?;
    let state = RefCell::new(Library {
        list: (!lists.is_empty()).then_some(0),
        score: None,
        lists,
        store: store.clone(),
        files: None,
        missing: Vec::new(),
        contact: None,
        drag_offsets: Vec::new(),
        drag_buffer: None,
        drag_autoscroll: None,
    });
    let list = |id, rect| Control {
        id,
        class: LIST,
        rect,
        title: "",
        style: (WS_TABSTOP | WS_VSCROLL).0
            | LBS_NOTIFY as u32
            | LBS_NOINTEGRALHEIGHT as u32
            | LBS_OWNERDRAWFIXED as u32
            | LBS_HASSTRINGS as u32,
    };
    let controls = [
        Control::label(200, [15, 18, 57, 13], tr!("Scalette", "Setlists")),
        Control::button(CREATE, [74, 12, 17, 22], tr!("Nuova scaletta", "New setlist")),
        Control::button(RENAME, [93, 12, 17, 22], tr!("Rinomina", "Rename")),
        Control::button(DELETE, [112, 12, 17, 22], tr!("Elimina scaletta", "Delete setlist")),
        list(LISTS, [16, 41, 111, 159]),
        Control::label(201, [141, 18, 168, 13], tr!("Spartiti", "Scores")),
        Control::button(ADD_PDFS, [314, 12, 100, 22], tr!("Aggiungi PDF…", "Add PDFs…")),
        list(SCORES, [141, 41, 269, 159]),
        Control::label(STATUS, [14, 217, 245, 17], ""),
        Control::button(CANCEL, [268, 212, 57, 25], tr!("Chiudi", "Close")),
        Control::button(PLAY, [331, 212, 83, 25], tr!("Avvia concerto", "Start concert")),
    ];
    dialog::show(
        owner,
        tr!("Scalette · ScoreDraw", "Setlists · ScoreDraw"),
        [426, 248],
        &controls,
        Some(procedure),
        LPARAM((&raw const state) as isize),
    )
    .map_err(|e| e.to_string())?;
    Ok(state.into_inner().files)
}

struct Library {
    store: Store,
    lists: Vec<Setlist>,
    list: Option<usize>,
    score: Option<usize>,
    files: Option<Vec<ScoreEntry>>,
    missing: Vec<bool>,
    contact: Option<RowContact>,
    drag_offsets: Vec<f32>,
    drag_buffer: Option<Surface>,
    drag_autoscroll: Option<Instant>,
}

#[derive(Clone, Copy)]
enum RowContact {
    Action { index: usize, action: u16 },
    Drag { from: usize, to: usize, grab_y: i32, y: i32 },
}

impl Library {
    fn scores(&self) -> &[ScoreEntry] {
        self.list
            .map_or(&[], |index| self.lists[index].scores.as_slice())
    }

    fn name_available(&self, name: &str, ignore: Option<usize>) -> bool {
        !self
            .lists
            .iter()
            .enumerate()
            .any(|(index, list)| Some(index) != ignore && list.name.eq_ignore_ascii_case(name))
    }

    fn refresh_lists(&self, hwnd: HWND) {
        dialog::rows(
            hwnd,
            LISTS,
            self.lists.iter().map(|list| list.name.clone()),
            self.list,
        );
        show_list(hwnd, LISTS, !self.lists.is_empty());
    }

    fn refresh_scores(&mut self, hwnd: HWND) {
        self.missing = self.scores().iter().map(|path| !path.path().is_file()).collect();
        let scores = self.scores();
        dialog::rows(
            hwnd,
            SCORES,
            scores.iter().map(entry_title),
            self.score,
        );
        show_list(hwnd, SCORES, !scores.is_empty());
        self.update_controls(hwnd);
        let _ = unsafe { InvalidateRect(Some(hwnd), None, true) };
    }

    fn update_controls(&self, hwnd: HWND) {
        let scores = self.scores();
        for id in [RENAME, DELETE, ADD_PDFS] {
            dialog::enable(hwnd, id, self.list.is_some());
        }
        dialog::enable(hwnd, PLAY, !scores.is_empty());
        self.status(hwnd);
    }

    fn status(&self, hwnd: HWND) {
        let label = match (self.list, self.scores().len()) {
            (None, _) => tr!(
                "Crea una scaletta per iniziare.",
                "Create a setlist to get started."
            )
            .into(),
            (Some(_), 0) => tr!("La scaletta è vuota.", "The setlist is empty.").into(),
            (Some(_), 1) => tr!("1 brano", "1 score").into(),
            (Some(_), count) => trf!("{count} brani", "{count} scores"),
        };
        dialog::set_value(hwnd, STATUS, &label);
    }

    fn command(&mut self, hwnd: HWND, id: u16, notification: u16) -> Result<(), String> {
        match id {
            CANCEL => {
                let _ = unsafe { EndDialog(hwnd, 0) };
            }
            LISTS if notification == LBN_SELCHANGE as u16 => {
                self.list = dialog::selection(hwnd, LISTS);
                self.score = None;
                self.refresh_scores(hwnd);
            }
            SCORES if notification == LBN_SELCHANGE as u16 => {
                self.score = dialog::selection(hwnd, SCORES);
                self.update_controls(hwnd);
            }
            CREATE => {
                let Some(name) =
                    dialog::setlist_name(hwnd, None, |name| self.name_available(name, None))
                        .map_err(|e| e.to_string())?
                else {
                    return Ok(());
                };
                let list = Setlist::new(name);
                self.store.save_setlist(&list).map_err(|e| e.to_string())?;
                self.list = Some(self.lists.len());
                self.lists.push(list);
                self.score = None;
                self.refresh_lists(hwnd);
                self.refresh_scores(hwnd);
            }
            RENAME => {
                let Some(index) = self.list else {
                    return Ok(());
                };
                let mut list = self.lists[index].clone();
                let Some(name) = dialog::setlist_name(hwnd, Some(&list.name), |name| {
                    self.name_available(name, Some(index))
                })
                .map_err(|e| e.to_string())?
                else {
                    return Ok(());
                };
                list.name = name;
                self.update(hwnd, index, list, self.score)?;
                self.refresh_lists(hwnd);
            }
            DELETE => {
                let Some(index) = self.list else {
                    return Ok(());
                };
                if dialog::confirm_setlist_delete(hwnd, &self.lists[index].name)
                    .map_err(|e| e.to_string())?
                {
                    self.store
                        .delete_setlist(&self.lists[index].id)
                        .map_err(|e| e.to_string())?;
                    self.lists.remove(index);
                    self.list = (!self.lists.is_empty()).then(|| index.min(self.lists.len() - 1));
                    self.score = None;
                    self.refresh_lists(hwnd);
                    self.refresh_scores(hwnd);
                }
            }
            ADD_PDFS => self.add(hwnd)?,
            REMOVE | LOCATE | RANGE => {
                let (Some(list_index), Some(index)) = (self.list, self.score) else {
                    return Ok(());
                };
                let mut list = self.lists[list_index].clone();
                let mut selected = Some(index);
                match id {
                    REMOVE => {
                        list.scores.remove(index);
                        selected =
                            (!list.scores.is_empty()).then(|| index.min(list.scores.len() - 1));
                    }
                    LOCATE => {
                        let files =
                            crate::reader::choose_files(hwnd, false).map_err(|e| e.to_string())?;
                        let Some(path) = files.into_iter().next() else {
                            return Ok(());
                        };
                        list.scores[index] = ScoreEntry::new(path, list.scores[index].range());
                    }
                    RANGE => {
                        let entry = &list.scores[index];
                        let initial = entry.range().map_or_else(String::new, |(start, end)| format!("{start}-{end}"));
                        let Some(value) = dialog::prompt_text(
                            hwnd, tr!("Intervallo pagine", "Page range"),
                            tr!("Da-a (es. 2-5). Vuoto = tutto il PDF.", "First-last (e.g. 2-5). Blank = entire PDF."),
                            &initial, tr!("Salva", "Save"), |value| parse_range(value).map(|_| ()),
                        ).map_err(|error| error.to_string())? else {
                            return Ok(());
                        };
                        list.scores[index] = ScoreEntry::new(entry.path().to_owned(), parse_range(&value)?);
                    }
                    _ => return Ok(()),
                }
                self.update(hwnd, list_index, list, selected)?;
            }
            PLAY => {
                let Some(index) = self.list else {
                    return Ok(());
                };
                let scores = &self.lists[index].scores;
                if scores.is_empty() {
                    return Ok(());
                }
                let missing: Vec<_> = scores
                    .iter()
                    .filter(|path| !path.path().is_file())
                    .map(entry_title)
                    .collect();
                if !missing.is_empty() {
                    return Err(trf!(
                        "Ricollega questi spartiti prima di avviare il concerto:\n\n{}",
                        "Relink these scores before starting the concert:\n\n{}",
                        missing.join("\n")
                    ));
                }
                self.files = Some(scores.clone());
                let _ = unsafe { EndDialog(hwnd, 1) };
            }
            _ => {}
        }
        Ok(())
    }

    fn update(
        &mut self,
        hwnd: HWND,
        index: usize,
        list: Setlist,
        selected: Option<usize>,
    ) -> Result<(), String> {
        self.store.save_setlist(&list).map_err(|e| e.to_string())?;
        self.lists[index] = list;
        self.score = selected;
        self.refresh_scores(hwnd);
        Ok(())
    }

    fn add(&mut self, hwnd: HWND) -> Result<(), String> {
        let Some(index) = self.list else {
            return Ok(());
        };
        let files = crate::reader::choose_files(hwnd, true).map_err(|e| e.to_string())?;
        if files.is_empty() {
            return Ok(());
        }
        let mut list = self.lists[index].clone();
        let first = list.scores.len();
        list.scores.extend(files.into_iter().map(ScoreEntry::Whole));
        self.update(hwnd, index, list, Some(first))
    }

    fn draw_backdrop(&self, hwnd: HWND, dc: HDC) {
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        let px = |dp| (dp * dpi / 96) as i32;
        dialog::background(hwnd, dc);
        for (frame, empty) in [
            (
                LISTS_PANEL,
                self.lists
                    .is_empty()
                    .then_some(tr!("Nessuna scaletta", "No setlists")),
            ),
            (
                SCORES_PANEL,
                self.scores().is_empty().then_some(if self.list.is_some() {
                    tr!("Aggiungi il primo spartito", "Add the first score")
                } else {
                    tr!("Seleziona una scaletta", "Select a setlist")
                }),
            ),
        ] {
            let rect = dialog::area(hwnd, frame);
            rounded(dc, rect, PANEL, px(22));
            if let Some(label) = empty {
                text(dc, rect, label, MUTED, px(14));
            }
        }
        let divider = dialog::area(hwnd, [12, 208, 402, 1]);
        fill(
            dc,
            RECT {
                bottom: divider.top + px(1),
                ..divider
            },
            SURFACE,
        );
    }

    fn paint_drag(&mut self, hwnd: HWND, control: HWND, from: usize, top: i32) {
        let mut paint = PAINTSTRUCT::default();
        let mut bounds = RECT::default();
        unsafe {
            let dc = BeginPaint(control, &mut paint);
            let _ = GetClientRect(control, &mut bounds);
            if self.drag_buffer.is_none() {
                self.drag_buffer = Surface::new(dc, bounds.right, bounds.bottom);
            }
            let target = self.drag_buffer.as_ref().map_or(dc, |buffer| buffer.dc);
            let first = SendMessageW(control, LB_GETTOPINDEX, None, None).0.max(0) as usize;
            let height = SendMessageW(control, LB_GETITEMHEIGHT, Some(WPARAM(0)), None).0.max(1) as i32;
            fill(target, bounds, PANEL);
            let draw = |index: usize, y: i32| {
                let item = DRAWITEMSTRUCT {
                    CtlID: u32::from(SCORES),
                    itemID: index as u32,
                    itemState: if index == from { ODS_SELECTED } else { Default::default() },
                    hDC: target,
                    rcItem: RECT { top: y, bottom: y + height, ..bounds },
                    ..Default::default()
                };
                self.draw_row(hwnd, &item);
            };
            let last = first + (bounds.bottom / height + 3) as usize;
            for index in first.saturating_sub(1)..last.min(self.scores().len()) {
                if index != from {
                    let shift = self.drag_offsets.get(index).copied().unwrap_or(0.0);
                    let y = ((index as f32 - first as f32 + shift) * height as f32).round() as i32;
                    draw(index, y);
                }
            }
            draw(from, top.clamp(0, (bounds.bottom - height).max(0)));
            if let Some(buffer) = &self.drag_buffer {
                let rect = paint.rcPaint;
                buffer.copy_to(dc, rect, rect.left, rect.top);
            }
            let _ = EndPaint(control, &paint);
        }
    }

    fn draw_row(&self, hwnd: HWND, item: &DRAWITEMSTRUCT) {
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
        let px = |dp| (dp * dpi / 96) as i32;
        fill(item.hDC, item.rcItem, PANEL);
        let index = item.itemID as usize;
        let selected = item.itemState.0 & ODS_SELECTED.0 != 0;
        let focused = item.itemState.0 & ODS_FOCUS.0 != 0;
        let is_list = item.CtlID == u32::from(LISTS);
        let name = if is_list {
            let Some(list) = self.lists.get(index) else {
                return;
            };
            std::borrow::Cow::Borrowed(list.name.as_str())
        } else {
            let Some(path) = self.scores().get(index) else {
                return;
            };
            std::borrow::Cow::Owned(entry_title(path))
        };
        let row = row_body(item.rcItem, dpi);
        let dragged = !is_list && matches!(self.contact, Some(RowContact::Drag { from, .. }) if from == index);
        if dragged {
            let shadow = RECT { top: row.top + px(3), bottom: row.bottom + px(4), ..row };
            rounded(item.hDC, shadow, BACKGROUND, px(14));
        }
        if selected || focused || dragged {
            rounded(item.hDC, row, SURFACE, px(14));
        }
        if selected {
            let marker = RECT {
                left: row.left + px(3),
                right: row.left + px(6),
                top: row.top + px(9),
                bottom: row.bottom - px(9),
            };
            rounded(item.hDC, marker, ACCENT, px(3));
        }
        let leading = RECT {
            left: row.left + px(12),
            right: row.left + px(41),
            ..row
        };
        let tone = if selected { ACCENT } else { MUTED };
        if is_list {
            icons::draw(item.hDC, Glyph::List, leading, px(18) as f32, tone);
        } else {
            icons::draw(item.hDC, Glyph::Move, leading, px(17) as f32, tone);
        }
        let missing = !is_list && self.missing.get(index) == Some(&true);
        let title_right = if is_list { row.right - px(8) } else { row.right - px(103) };
        text_left(
            item.hDC,
            RECT {
                left: leading.right + px(7),
                right: title_right,
                ..row
            },
            &name,
            if missing { ALERT } else { TEXT },
            px(15),
        );
        if !is_list {
            for (action, glyph, slot) in [(RANGE, Glyph::PageRange, 2), (LOCATE, Glyph::FolderOpen, 1), (REMOVE, Glyph::Trash, 0)] {
                let bounds = row_action_rect(row, dpi, slot);
                let pressed = matches!(self.contact, Some(RowContact::Action { index: i, action: a }) if i == index && a == action);
                rounded(item.hDC, bounds, if pressed { SURFACE } else { PANEL }, px(10));
                icons::draw(item.hDC, glyph, bounds, px(16) as f32, if action == LOCATE && missing { ALERT } else { MUTED });
            }

        }
    }
}

fn show_list(hwnd: HWND, id: u16, visible: bool) {
    if let Ok(control) = unsafe { GetDlgItem(Some(hwnd), i32::from(id)) } {
        let _ = unsafe { ShowWindow(control, if visible { SW_SHOW } else { SW_HIDE }) };
    }
}

fn parse_range(value: &str) -> Result<Option<(u32, u32)>, &'static str> {
    if value.is_empty() {
        return Ok(None);
    }
    let range = value.split_once('-').and_then(|(start, end)| {
        let start = start.trim().parse::<u32>().ok()?;
        let end = end.trim().parse::<u32>().ok()?;
        (start > 0 && start <= end).then_some((start, end))
    });
    range.map(Some).ok_or(tr!("Usa un intervallo come 2-5.", "Use a range like 2-5."))
}

fn entry_title(entry: &ScoreEntry) -> String {
    let title = title(entry.path());
    match entry.range() {
        Some((start, end)) => format!("{title} · {start}–{end}"),
        None => title,
    }
}

fn title(path: &Path) -> String {
    path.file_stem()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

unsafe extern "system" fn procedure(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    unsafe {
        if msg == WM_INITDIALOG {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, lp.0);
        }
        if msg == WM_DRAWITEM {
            let item = &*(lp.0 as *const DRAWITEMSTRUCT);
            if item.CtlType == ODT_BUTTON {
                return isize::from(draw_toolbar_button(hwnd, item));
            }
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<Library>;
        let Some(cell) = ptr.as_ref() else { return 0 };
        let Ok(mut state) = cell.try_borrow_mut() else {
            return 0;
        };
        match msg {
            WM_INITDIALOG => {
                let row_height = (43 * GetDpiForWindow(hwnd).max(96) / 96) as isize;
                for id in [LISTS, SCORES] {
                    SendDlgItemMessageW(
                        hwnd,
                        id.into(),
                        LB_SETITEMHEIGHT,
                        WPARAM(0),
                        LPARAM(row_height),
                    );
                }
                if let Ok(control) = GetDlgItem(Some(hwnd), SCORES.into()) {
                    let _ = SetWindowSubclass(control, Some(score_list_proc), 1, 0);
                }
                state.refresh_lists(hwnd);
                state.refresh_scores(hwnd);
            }
            WM_COMMAND => {
                if let Err(error) = state.command(hwnd, wp.0 as u16, (wp.0 >> 16) as u16) {
                    let _ = dialog::notice(
                        hwnd,
                        tr!("Scalette · ScoreDraw", "Setlists · ScoreDraw"),
                        &error,
                    );
                }
            }
            WM_ERASEBKGND => state.draw_backdrop(hwnd, HDC(wp.0 as _)),
            WM_DRAWITEM => {
                let item = &*(lp.0 as *const DRAWITEMSTRUCT);
                if item.CtlType != ODT_LISTBOX {
                    return 0;
                }
                state.draw_row(hwnd, item);
            }
            WM_CLOSE => {
                let _ = EndDialog(hwnd, 0);
            }
            _ => return 0,
        }
        1
    }
}

fn row_body(rect: RECT, dpi: u32) -> RECT {
    let inset = (2 * dpi / 96) as i32;
    RECT {
        left: rect.left + inset,
        right: rect.right - inset,
        top: rect.top + inset,
        bottom: rect.bottom - inset,
    }
}

fn row_action_rect(row: RECT, dpi: u32, slot: i32) -> RECT {
    let px = |dp| dp * dpi as i32 / 96;
    let right = row.right - px(4 + slot * 32);
    RECT {
        left: right - px(29),
        right,
        top: row.top + px(4),
        bottom: row.bottom - px(4),
    }
}

fn draw_toolbar_button(hwnd: HWND, item: &DRAWITEMSTRUCT) -> bool {
    let glyph = match item.CtlID as u16 {
        CREATE => Glyph::Plus,
        RENAME => Glyph::Pencil,
        DELETE => Glyph::Trash,
        _ => return false,
    };
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let disabled = item.itemState.0 & ODS_DISABLED.0 != 0;
    let engaged = item.itemState.0 & (ODS_SELECTED.0 | ODS_FOCUS.0) != 0;
    let background = if engaged && !disabled { SURFACE } else { PANEL };
    fill(item.hDC, item.rcItem, BACKGROUND);
    rounded(item.hDC, item.rcItem, background, (15 * dpi / 96) as i32);
    icons::draw(
        item.hDC,
        glyph,
        item.rcItem,
        17.0 * dpi as f32 / 96.0,
        if disabled { MUTED } else if engaged { ACCENT } else { TEXT },
    );
    true
}

fn row_at(control: HWND, point: POINT, count: usize) -> Option<(usize, RECT)> {
    let packed = u32::from(point.x as u16) | (u32::from(point.y as u16) << 16);
    let result = unsafe { SendMessageW(control, LB_ITEMFROMPOINT, None, Some(LPARAM(packed as isize))) }.0 as u32;
    let index = (result & 0xffff) as usize;
    if index >= count {
        return None;
    }
    let mut bounds = RECT::default();
    let found = unsafe { SendMessageW(control, LB_GETITEMRECT, Some(WPARAM(index)), Some(LPARAM((&raw mut bounds) as isize))) };
    (found.0 != LB_ERR as isize).then_some((index, bounds))
}

fn row_action(bounds: RECT, dpi: u32, point: POINT) -> Option<u16> {
    [(RANGE, 2), (LOCATE, 1), (REMOVE, 0)].into_iter().find_map(|(action, slot)| {
        inside(row_action_rect(row_body(bounds, dpi), dpi, slot), point).then_some(action)
    })
}

fn move_score(state: &mut Library, hwnd: HWND, from: usize, to: usize) -> Result<(), String> {
    if let Some(index) = state.list && from != to && from < state.scores().len() && to < state.scores().len() {
        let mut list = state.lists[index].clone();
        let entry = list.scores.remove(from);
        list.scores.insert(to, entry);
        state.update(hwnd, index, list, Some(to))?;
    }
    Ok(())
}

const DRAG_TIMER: usize = 1;

unsafe extern "system" fn score_list_proc(
    control: HWND, message: u32, wp: WPARAM, lp: LPARAM, _: usize, _: usize,
) -> LRESULT {
    unsafe {
        if message == WM_NCDESTROY {
            let _ = windows::Win32::UI::Shell::RemoveWindowSubclass(control, Some(score_list_proc), 1);
            return DefSubclassProc(control, message, wp, lp);
        }
        if !matches!(message, WM_PAINT | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_MOUSEMOVE | WM_MOUSELEAVE | WM_CAPTURECHANGED | WM_TIMER | WM_KEYDOWN) {
            return DefSubclassProc(control, message, wp, lp);
        }
        let Ok(hwnd) = GetParent(control) else { return DefSubclassProc(control, message, wp, lp) };
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<Library>;
        let Some(cell) = ptr.as_ref() else { return DefSubclassProc(control, message, wp, lp) };
        let Ok(mut state) = cell.try_borrow_mut() else { return DefSubclassProc(control, message, wp, lp) };
        if message == WM_PAINT {
            if let Some(RowContact::Drag { from, grab_y, y, .. }) = state.contact {
                state.paint_drag(hwnd, control, from, y - grab_y);
                return LRESULT(0);
            }
            drop(state);
            return DefSubclassProc(control, message, wp, lp);
        }
        let dpi = GetDpiForWindow(hwnd).max(96);
        let count = state.scores().len();
        let mut point = point_from_mouse(lp);
        if message == WM_TIMER {
            if wp.0 != DRAG_TIMER || GetCursorPos(&mut point).is_err() {
                return DefSubclassProc(control, message, wp, lp);
            }
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(control, &mut point);
        }
        let hit = || row_at(control, point, count);
        let action = match message {
            WM_LBUTTONDOWN => {
                if let Some((index, bounds)) = hit().filter(|(_, bounds)| inside(*bounds, point)) {
                    state.contact = if let Some(action) = row_action(bounds, dpi, point) {
                        Some(RowContact::Action { index, action })
                    } else if point.x < bounds.left + (44 * dpi / 96) as i32 && count > 1 {
                        SetTimer(Some(control), DRAG_TIMER, 16, None);
                        state.drag_offsets = vec![0.0; count];
                        state.drag_buffer = None;
                        state.drag_autoscroll = Some(Instant::now());
                        Some(RowContact::Drag { from: index, to: index, grab_y: point.y - bounds.top, y: point.y })
                    } else {
                        None
                    };
                    if state.contact.is_some() {
                        state.score = Some(index);
                        let _ = SetFocus(Some(control));
                        SendMessageW(control, LB_SETCURSEL, Some(WPARAM(index)), None);
                        SetCapture(control);
                        let _ = InvalidateRect(Some(control), None, false);
                        return LRESULT(0);
                    }
                }
                None
            }
            WM_MOUSEMOVE | WM_TIMER => {
                if let Some(RowContact::Drag { from, to, grab_y, .. }) = state.contact {
                    let mut client = RECT::default();
                    let _ = GetClientRect(control, &mut client);
                    let threshold = (19 * dpi / 96) as i32;
                    if message == WM_TIMER
                        && state.drag_autoscroll.is_none_or(|at| at.elapsed() >= Duration::from_millis(95))
                    {
                        state.drag_autoscroll = Some(Instant::now());
                        let top = SendMessageW(control, LB_GETTOPINDEX, None, None).0.max(0);
                        let next = if point.y < threshold {
                            top.saturating_sub(1).max(0)
                        } else if point.y >= client.bottom - threshold {
                            (top + 1).min(count.saturating_sub(1) as isize)
                        } else { top };
                        if next != top {
                            SendMessageW(control, LB_SETTOPINDEX, Some(WPARAM(next as usize)), None);
                        }
                    }
                    let within = POINT { y: point.y.clamp(0, (client.bottom - 1).max(0)), ..point };
                    let to = row_at(control, within, count).map_or(to, |(index, _)| index);
                    state.contact = Some(RowContact::Drag { from, to, grab_y, y: within.y });
                    if message == WM_TIMER {
                        for (index, shift) in state.drag_offsets.iter_mut().enumerate() {
                            let target = if from < index && index <= to {
                                -1.0
                            } else if to <= index && index < from {
                                1.0
                            } else {
                                0.0
                            };
                            let gap = target - *shift;
                            *shift = if gap.abs() < 0.005 { target } else { *shift + gap * 0.28 };
                        }
                    }
                    let _ = InvalidateRect(Some(control), None, false);
                    return LRESULT(0);
                }
                let mut track = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: control,
                    ..TRACKMOUSEEVENT::default()
                };
                let _ = TrackMouseEvent(&mut track);
                if let Some((_, bounds)) = hit().filter(|(_, bounds)| inside(*bounds, point)) {
                    let hint = match row_action(bounds, dpi, point) {
                        Some(RANGE) => Some(tr!("Intervallo pagine", "Page range")),
                        Some(LOCATE) => Some(tr!("Ricollega PDF", "Relink PDF")),
                        Some(REMOVE) => Some(tr!("Rimuovi dalla scaletta", "Remove from setlist")),
                        _ if point.x < bounds.left + (44 * dpi / 96) as i32 && count > 1 =>
                            Some(tr!("Trascina per riordinare", "Drag to reorder")),
                        _ => None,
                    };
                    if let Some(hint) = hint {
                        dialog::set_value(hwnd, STATUS, hint);
                    } else {
                        state.status(hwnd);
                    }
                } else {
                    state.status(hwnd);
                }
                None
            }
            WM_MOUSELEAVE => { state.status(hwnd); None }
            WM_CAPTURECHANGED => {
                let _ = KillTimer(Some(control), DRAG_TIMER);
                if state.contact.take().is_some() {
                    let _ = InvalidateRect(Some(control), None, false);
                }
                None
            }
            WM_LBUTTONUP => {
                let Some(contact) = state.contact.take() else { return DefSubclassProc(control, message, wp, lp) };
                let _ = KillTimer(Some(control), DRAG_TIMER);
                let _ = ReleaseCapture();
                let _ = InvalidateRect(Some(control), None, false);
                let result = match contact {
                    RowContact::Action { index, action }
                        if hit().is_some_and(|(row, bounds)| row == index && row_action(bounds, dpi, point) == Some(action)) =>
                            state.command(hwnd, action, 0),
                    RowContact::Drag { from, to, .. } => {
                        let mut client = RECT::default();
                        let _ = GetClientRect(control, &mut client);
                        if (client.left..client.right).contains(&point.x) {
                            move_score(&mut state, hwnd, from, to)
                        } else { Ok(()) }
                    }
                    _ => Ok(()),
                };
                Some(result)
            }
            WM_KEYDOWN if wp.0 == 0x2e && state.score.is_some() => Some(state.command(hwnd, REMOVE, 0)),
            _ => None,
        };
        if let Some(result) = action {
            if let Err(error) = result {
                let _ = dialog::notice(hwnd, tr!("Scalette · ScoreDraw", "Setlists · ScoreDraw"), &error);
            }
            return LRESULT(0);
        }
        drop(state);
        DefSubclassProc(control, message, wp, lp)
    }
}
