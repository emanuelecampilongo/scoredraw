use crate::data::{Setlist, Store};
use crate::dialog::{self, Control, LIST};
use crate::icons::{self, Glyph};
use crate::reader::{ACCENT, ALERT, MUTED, PANEL, SURFACE, TEXT, fill, rounded, text, text_left};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{HDC, InvalidateRect};
use windows::Win32::UI::Controls::{DRAWITEMSTRUCT, ODS_FOCUS, ODS_SELECTED, ODT_LISTBOX};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

const LISTS: u16 = 101;
const SCORES: u16 = 102;
const STATUS: u16 = 104;
const CREATE: u16 = 110;
const RENAME: u16 = 111;
const DELETE: u16 = 112;
const ADD_PDFS: u16 = 120;
const UP: u16 = 122;
const DOWN: u16 = 123;
const REMOVE: u16 = 124;
const LOCATE: u16 = 125;
const PLAY: u16 = 1;
const CANCEL: u16 = 2;
const LISTS_PANEL: [i32; 4] = [12, 31, 113, 146];
const SCORES_PANEL: [i32; 4] = [131, 31, 231, 146];

pub fn show(owner: HWND, store: &Store) -> Result<Option<Vec<PathBuf>>, String> {
    let lists = store.setlists().map_err(|error| error.to_string())?;
    let state = RefCell::new(Library {
        list: (!lists.is_empty()).then_some(0),
        score: None,
        lists,
        store: store.clone(),
        files: None,
        missing: Vec::new(),
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
        Control::label(
            200,
            [15, 13, 110, 14],
            tr!("Le mie scalette", "My setlists"),
        ),
        list(LISTS, [16, 35, 105, 138]),
        Control::label(
            201,
            [134, 13, 223, 14],
            tr!("Spartiti in scaletta", "Scores in this setlist"),
        ),
        list(SCORES, [135, 35, 223, 138]),
        Control::button(CREATE, [12, 183, 52, 22], tr!("+ Nuova", "+ New")),
        Control::button(RENAME, [69, 183, 56, 22], tr!("Rinomina…", "Rename…")),
        Control::button(
            DELETE,
            [12, 210, 113, 22],
            tr!("Elimina scaletta…", "Delete setlist…"),
        ),
        Control::button(
            ADD_PDFS,
            [131, 183, 113, 22],
            tr!("Aggiungi PDF…", "Add PDFs…"),
        ),
        Control::label(
            203,
            [252, 188, 110, 15],
            tr!("I file restano dove sono.", "Files stay in their folders."),
        ),
        Control::button(LOCATE, [131, 210, 56, 22], tr!("Ricollega…", "Relink…")),
        Control::button(REMOVE, [192, 210, 55, 22], tr!("Rimuovi", "Remove")),
        Control::button(UP, [252, 210, 52, 22], tr!("↑ Sposta su", "↑ Move up")),
        Control::button(DOWN, [309, 210, 53, 22], tr!("↓ Sposta giù", "↓ Move down")),
        Control::label(STATUS, [14, 243, 199, 11], ""),
        Control::label(
            202,
            [14, 256, 199, 16],
            tr!(
                "Modalità concerto · Lettura a schermo intero",
                "Concert mode · Full-screen reading"
            ),
        ),
        Control::button(CANCEL, [221, 243, 53, 24], tr!("Chiudi", "Close")),
        Control::button(
            PLAY,
            [279, 243, 83, 24],
            tr!("Avvia concerto", "Start concert"),
        ),
    ];
    dialog::show(
        owner,
        tr!("Scalette · ScoreDraw", "Setlists · ScoreDraw"),
        [374, 277],
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
    files: Option<Vec<PathBuf>>,
    missing: Vec<bool>,
}

impl Library {
    fn scores(&self) -> &[PathBuf] {
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
        self.missing = self.scores().iter().map(|path| !path.is_file()).collect();
        let scores = self.scores();
        dialog::rows(
            hwnd,
            SCORES,
            scores.iter().map(|path| title(path)),
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
        for id in [LOCATE, REMOVE] {
            dialog::enable(hwnd, id, self.score.is_some());
        }
        dialog::enable(hwnd, UP, self.score.is_some_and(|index| index > 0));
        dialog::enable(
            hwnd,
            DOWN,
            self.score.is_some_and(|index| index + 1 < scores.len()),
        );
        dialog::enable(hwnd, PLAY, !scores.is_empty());
        let label = match (self.list, scores.len()) {
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
            UP | DOWN | REMOVE | LOCATE => {
                let (Some(list_index), Some(index)) = (self.list, self.score) else {
                    return Ok(());
                };
                let mut list = self.lists[list_index].clone();
                let mut selected = Some(index);
                match id {
                    UP if index > 0 => {
                        list.scores.swap(index, index - 1);
                        selected = Some(index - 1);
                    }
                    DOWN if index + 1 < list.scores.len() => {
                        list.scores.swap(index, index + 1);
                        selected = Some(index + 1);
                    }
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
                        list.scores[index] = path;
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
                    .filter(|path| !path.is_file())
                    .map(|path| title(path))
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
        list.scores.extend(files);
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
        let divider = dialog::area(hwnd, [12, 238, 350, 1]);
        fill(
            dc,
            RECT {
                bottom: divider.top + px(1),
                ..divider
            },
            SURFACE,
        );
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
            std::borrow::Cow::Owned(title(path))
        };
        let row = RECT {
            left: item.rcItem.left + px(2),
            right: item.rcItem.right - px(2),
            top: item.rcItem.top + px(2),
            bottom: item.rcItem.bottom - px(2),
        };
        if selected || focused {
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
            text(
                item.hDC,
                leading,
                &format!("{:02}", index + 1),
                tone,
                px(13),
            );
        }
        let missing = !is_list && self.missing.get(index) == Some(&true);
        let title_right = row.right - px(if missing { 74 } else { 9 });
        text_left(
            item.hDC,
            RECT {
                left: leading.right + px(7),
                right: title_right,
                ..row
            },
            &name,
            TEXT,
            px(15),
        );
        if missing {
            text(
                item.hDC,
                RECT {
                    left: title_right,
                    right: row.right - px(5),
                    ..row
                },
                tr!("Mancante", "Missing"),
                ALERT,
                px(12),
            );
        }
    }
}

fn show_list(hwnd: HWND, id: u16, visible: bool) {
    if let Ok(control) = unsafe { GetDlgItem(Some(hwnd), i32::from(id)) } {
        let _ = unsafe { ShowWindow(control, if visible { SW_SHOW } else { SW_HIDE }) };
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
