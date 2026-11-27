use crate::reader::{
    ACCENT, BACKGROUND, MUTED, PANEL, SURFACE, TEXT, fill, inside, point_from_mouse, rounded, text,
};
use std::cell::RefCell;
use windows::Win32::Foundation::{
    COLORREF, GlobalFree, HANDLE, HINSTANCE, HWND, LPARAM, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::UI::Controls::{
    DRAWITEMSTRUCT, EM_SETLIMITTEXT, EM_SETSEL, ODS_DISABLED, ODS_FOCUS, ODS_SELECTED, ODT_BUTTON,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Error, HSTRING, Result};

pub const BUTTON: u16 = 0x0080;
pub const EDIT: u16 = 0x0081;
pub const LABEL: u16 = 0x0082;
pub const LIST: u16 = 0x0083;
const FEEDBACK_COPY: u16 = 151;
const FEEDBACK_STATUS: u16 = 152;
const FEEDBACK_ADDRESS: EditField = EditField {
    id: 153,
    bounds: [13, 42, 252, 27],
};

pub struct Control<'a> {
    pub id: u16,
    pub class: u16,
    pub rect: [u16; 4],
    pub title: &'a str,
    pub style: u32,
}

impl<'a> Control<'a> {
    pub fn label(id: u16, rect: [u16; 4], title: &'a str) -> Self {
        const SS_NOPREFIX: u32 = 0x80;
        Self {
            id,
            class: LABEL,
            rect,
            title,
            style: SS_NOPREFIX,
        }
    }

    pub fn button(id: u16, rect: [u16; 4], title: &'a str) -> Self {
        Self {
            id,
            class: BUTTON,
            rect,
            title,
            style: WS_TABSTOP.0,
        }
    }
}

pub fn show(
    owner: HWND,
    title: &str,
    size: [u16; 2],
    controls: &[Control<'_>],
    callback: DLGPROC,
    data: LPARAM,
) -> Result<isize> {
    // A Vec<u32> ensures the dialog starts on a DWORD boundary.
    // Each item is independently DWORD-aligned in the template.
    let mut words = Vec::new();
    let style = (WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN).0
        | DS_MODALFRAME as u32
        | DS_SETFONT as u32
        | DS_CENTER as u32;
    dword(&mut words, style);
    dword(&mut words, 0);
    words.extend_from_slice(&[controls.len() as u16, 0, 0, size[0], size[1], 0, 0]);
    wide(&mut words, title);
    words.push(10);
    wide(&mut words, "Segoe UI");
    for control in controls {
        if words.len() % 2 != 0 {
            words.push(0);
        }
        let kind = control.style & 15;
        let styled = if control.class == BUTTON && kind <= BS_DEFPUSHBUTTON as u32 {
            (control.style & !15) | BS_OWNERDRAW as u32
        } else {
            control.style
        };
        dword(&mut words, (WS_CHILD | WS_VISIBLE).0 | styled);
        dword(&mut words, 0);
        words.extend_from_slice(&control.rect);
        words.extend_from_slice(&[control.id, 0xffff, control.class]);
        wide(&mut words, control.title);
        words.push(0);
    }
    let template: Vec<u32> = words
        .chunks(2)
        .map(|pair| u32::from(pair[0]) | (u32::from(pair.get(1).copied().unwrap_or(0)) << 16))
        .collect();
    let module = unsafe { GetModuleHandleW(None)? };
    let init = (callback, data);
    let result = unsafe {
        DialogBoxIndirectParamW(
            Some(HINSTANCE(module.0)),
            template.as_ptr().cast(),
            Some(owner),
            Some(themed),
            LPARAM((&raw const init) as isize),
        )
    };
    if result == -1 {
        Err(Error::from_thread())
    } else {
        Ok(result)
    }
}

thread_local! {
    static DIALOGS: RefCell<std::collections::HashMap<isize, DLGPROC>> = RefCell::default();
}

pub(crate) fn dark_title(hwnd: HWND) {
    let enabled = 1i32;
    unsafe {
        let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            hwnd,
            windows::Win32::Graphics::Dwm::DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&raw const enabled).cast(),
            size_of::<i32>() as u32,
        );
    }
}

unsafe extern "system" fn themed(hwnd: HWND, msg: u32, wp: WPARAM, mut lp: LPARAM) -> isize {
    let key = hwnd.0 as isize;
    if msg == WM_INITDIALOG {
        let (callback, payload) = unsafe { *(lp.0 as *const (DLGPROC, LPARAM)) };
        DIALOGS.with(|dialogs| {
            dialogs.borrow_mut().insert(key, callback);
        });
        lp = payload;
        dark_title(hwnd);
        unsafe {
            if GetDlgItem(Some(hwnd), 1).is_ok() {
                SendMessageW(hwnd, DM_SETDEFID, Some(WPARAM(1)), None);
            }
        }
    }
    let callback = DIALOGS.with(|dialogs| {
        let mut dialogs = dialogs.borrow_mut();
        if msg == WM_NCDESTROY {
            dialogs.remove(&key).flatten()
        } else {
            dialogs.get(&key).copied().flatten()
        }
    });
    unsafe {
        match msg {
            WM_CTLCOLORDLG | WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
                let dc = HDC(wp.0 as _);
                let address = msg == WM_CTLCOLORSTATIC
                    && GetDlgCtrlID(HWND(lp.0 as _)) == i32::from(FEEDBACK_ADDRESS.id);
                let color = if address || matches!(msg, WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX) {
                    PANEL
                } else {
                    BACKGROUND
                };
                SetTextColor(dc, TEXT);
                SetBkColor(dc, color);
                let brush = GetStockObject(DC_BRUSH);
                SetDCBrushColor(dc, color);
                return brush.0 as isize;
            }
            WM_DRAWITEM if (*(lp.0 as *const DRAWITEMSTRUCT)).CtlType == ODT_BUTTON => {
                if let Some(callback) = callback
                    && callback(hwnd, msg, wp, lp) != 0
                {
                    return 1;
                }
                let item = &*(lp.0 as *const DRAWITEMSTRUCT);
                let mut title = [0; 256];
                let count = GetWindowTextW(item.hwndItem, &mut title).max(0) as usize;
                let title = String::from_utf16_lossy(&title[..count]);
                let disabled = item.itemState.0 & ODS_DISABLED.0 != 0;
                let pressed = item.itemState.0 & ODS_SELECTED.0 != 0;
                let primary = item.CtlID == 1;
                let brush = HBRUSH(GetStockObject(DC_BRUSH).0);
                SetDCBrushColor(item.hDC, BACKGROUND);
                FillRect(item.hDC, &item.rcItem, brush);
                let surface = if disabled {
                    PANEL
                } else if pressed {
                    COLORREF(0x7b9182)
                } else if primary {
                    ACCENT
                } else {
                    SURFACE
                };
                let radius = ((item.rcItem.bottom - item.rcItem.top) * 3 / 4).max(4);
                rounded(item.hDC, item.rcItem, surface, radius);
                let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96);
                let color = if disabled {
                    MUTED
                } else if primary {
                    BACKGROUND
                } else {
                    TEXT
                };
                if item.CtlID == u32::from(FEEDBACK_COPY) {
                    crate::icons::draw(
                        item.hDC,
                        crate::icons::Glyph::Copy,
                        item.rcItem,
                        18.0 * dpi as f32 / 96.0,
                        color,
                    );
                } else {
                    text(item.hDC, item.rcItem, &title, color, (14 * dpi / 96) as i32);
                }
                if item.itemState.0 & ODS_FOCUS.0 != 0 {
                    let mut focus = item.rcItem;
                    let inset = (3 * dpi / 96) as i32;
                    let _ = InflateRect(&mut focus, -inset, -inset);
                    let pen = CreatePen(
                        PS_SOLID,
                        (dpi / 96).max(1) as i32,
                        if primary { BACKGROUND } else { ACCENT },
                    );
                    if !pen.is_invalid() {
                        let prior_pen = SelectObject(item.hDC, pen.into());
                        let prior_brush = SelectObject(item.hDC, GetStockObject(NULL_BRUSH));
                        let _ = RoundRect(
                            item.hDC,
                            focus.left,
                            focus.top,
                            focus.right,
                            focus.bottom,
                            radius - inset,
                            radius - inset,
                        );
                        SelectObject(item.hDC, prior_brush);
                        SelectObject(item.hDC, prior_pen);
                        let _ = DeleteObject(pen.into());
                    }
                }
                return 1;
            }
            _ => {}
        }
        callback.map_or(0, |callback| callback(hwnd, msg, wp, lp))
    }
}

pub fn area(hwnd: HWND, [x, y, width, height]: [i32; 4]) -> RECT {
    let mut rect = RECT {
        left: x,
        top: y,
        right: x + width,
        bottom: y + height,
    };
    let _ = unsafe { MapDialogRect(hwnd, &mut rect) };
    rect
}

pub fn background(hwnd: HWND, dc: HDC) {
    let mut client = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut client) }.is_ok() {
        fill(dc, client, BACKGROUND);
    }
}

pub fn confirm_setlist_delete(owner: HWND, name: &str) -> Result<bool> {
    let message = trf!(
        "Eliminare «{name}»?\r\nGli spartiti restano disponibili.",
        "Delete “{name}”?\r\nThe scores will still be available."
    );
    let controls = [
        Control::label(100, [14, 12, 245, 37], &message),
        Control::button(1, [99, 51, 74, 23], tr!("Annulla", "Cancel")),
        Control::button(
            3,
            [178, 51, 81, 23],
            tr!("Elimina scaletta", "Delete setlist"),
        ),
    ];
    show(
        owner,
        tr!("Elimina scaletta", "Delete setlist"),
        [273, 86],
        &controls,
        Some(choice_proc),
        LPARAM(0),
    )
    .map(|choice| choice == 3)
}

pub fn notice(owner: HWND, title: &str, message: &str) -> Result<()> {
    if message.chars().count() > 210 || message.lines().count() > 3 {
        return reading(owner, title, message);
    }
    let body_height = if message.lines().count() == 1 && message.chars().count() <= 55 {
        18
    } else {
        40
    };
    let controls = [
        Control::label(100, [14, 12, 254, body_height], message),
        Control::button(1, [193, 16 + body_height, 75, 23], "OK"),
    ];
    show(
        owner,
        title,
        [282, 51 + body_height],
        &controls,
        Some(choice_proc),
        LPARAM(0),
    )
    .map(|_| ())
}

#[derive(Clone, Copy)]
pub(crate) struct EditField {
    pub id: u16,
    pub bounds: [i32; 4],
}

impl EditField {
    pub fn control(self, title: &str, extra: u32) -> Control<'_> {
        let [x, y, width, height] = self.bounds;
        Control {
            id: self.id,
            class: EDIT,
            rect: [
                (x + 7) as u16,
                (y + (height - 10) / 2) as u16,
                (width - 14) as u16,
                10,
            ],
            title,
            style: WS_TABSTOP.0 | ES_AUTOHSCROLL as u32 | extra,
        }
    }
}

pub(crate) fn field_background(hwnd: HWND, dc: HDC, fields: &[EditField]) {
    background(hwnd, dc);
    let radius = area(hwnd, [0, 0, 12, 0]).right;
    for field in fields {
        rounded(dc, area(hwnd, field.bounds), PANEL, radius);
    }
}

pub(crate) fn field_focus(hwnd: HWND, input: LPARAM, fields: &[EditField]) -> bool {
    fields
        .iter()
        .find(|field| inside(area(hwnd, field.bounds), point_from_mouse(input)))
        .and_then(|field| unsafe { GetDlgItem(Some(hwnd), field.id.into()).ok() })
        .is_some_and(|edit| {
            let _ = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(edit)) };
            true
        })
}

const NAME_FIELD: EditField = EditField {
    id: 101,
    bounds: [14, 34, 230, 30],
};
const BOOKMARK_TITLE: EditField = EditField {
    id: 101,
    bounds: [14, 31, 259, 30],
};
const BOOKMARK_PAGE: EditField = EditField {
    id: 102,
    bounds: [14, 86, 83, 30],
};
const BOOKMARK_FIELDS: [EditField; 2] = [BOOKMARK_TITLE, BOOKMARK_PAGE];

pub fn setlist_name(
    owner: HWND,
    initial: Option<&str>,
    available: impl Fn(&str) -> bool,
) -> Result<Option<String>> {
    let (caption, action) = if initial.is_some() {
        (
            tr!("Rinomina scaletta", "Rename setlist"),
            tr!("Salva nome", "Save name"),
        )
    } else {
        (
            tr!("Nuova scaletta", "New setlist"),
            tr!("Crea scaletta", "Create setlist"),
        )
    };
    prompt_text(owner, caption, tr!("Nome della scaletta", "Setlist name"),
        initial.unwrap_or_default(), action, |name| {
            if !(1..=100).contains(&name.chars().count()) {
                Err(tr!("Usa un nome tra 1 e 100 caratteri.", "Use a name between 1 and 100 characters long."))
            } else if !available(name) {
                Err(tr!("Esiste già una scaletta con questo nome.", "A setlist already has this name."))
            } else {
                Ok(())
            }
        })
}

pub fn prompt_text(
    owner: HWND, caption: &str, label: &str, initial: &str, action: &str,
    validate: impl Fn(&str) -> std::result::Result<(), &'static str>,
) -> Result<Option<String>> {
    let controls = [
        Control::label(200, [14, 16, 230, 14], label),
        NAME_FIELD.control(initial, 0),
        Control::label(201, [14, 69, 230, 14], ""),
        Control::button(2, [100, 91, 65, 23], tr!("Annulla", "Cancel")),
        Control::button(1, [170, 91, 74, 23], action),
    ];
    let prompt = NamePrompt {
        validate: &validate,
        selected: RefCell::default(),
    };
    show(
        owner,
        caption,
        [258, 128],
        &controls,
        Some(setlist_name_proc),
        LPARAM((&raw const prompt) as isize),
    )?;
    Ok(prompt.selected.into_inner())
}

struct NamePrompt<'a> {
    validate: &'a dyn Fn(&str) -> std::result::Result<(), &'static str>,
    selected: RefCell<Option<String>>,
}

unsafe extern "system" fn setlist_name_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    match msg {
        WM_INITDIALOG => unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, lp.0);
            SendDlgItemMessageW(hwnd, 101, EM_SETLIMITTEXT, WPARAM(200), LPARAM(0));
            SendDlgItemMessageW(hwnd, 101, EM_SETSEL, WPARAM(0), LPARAM(-1));
            if let Ok(edit) = GetDlgItem(Some(hwnd), 101) {
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(edit));
            }
            0
        },
        WM_ERASEBKGND => {
            field_background(hwnd, HDC(wp.0 as _), &[NAME_FIELD]);
            1
        }
        WM_LBUTTONDOWN if field_focus(hwnd, lp, &[NAME_FIELD]) => 1,
        WM_COMMAND if wp.0 as u16 == 1 => {
            let name = value(hwnd, 101).trim().to_owned();
            unsafe {
                let prompt = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const NamePrompt<'_>;
                if let Some(prompt) = prompt.as_ref() {
                    if let Err(error) = (prompt.validate)(&name) {
                        set_value(hwnd, 201, error);
                        return 1;
                    }
                    *prompt.selected.borrow_mut() = Some(name);
                }
                let _ = EndDialog(hwnd, 1);
            }
            1
        }
        _ => unsafe { choice_proc(hwnd, msg, wp, lp) },
    }
}

pub fn bookmark_fields(
    owner: HWND,
    caption: &str,
    initial: &str,
    page: &str,
) -> Result<Option<(String, String)>> {
    let controls = [
        Control::label(200, [14, 13, 259, 14], tr!("Titolo", "Title")),
        BOOKMARK_TITLE.control(initial, 0),
        Control::label(
            201,
            [14, 68, 259, 14],
            tr!("Pagina di destinazione", "Target page"),
        ),
        BOOKMARK_PAGE.control(page, ES_NUMBER as u32),
        Control::button(2, [129, 126, 68, 24], tr!("Annulla", "Cancel")),
        Control::button(1, [203, 126, 70, 24], tr!("Salva", "Save")),
    ];
    let selection: RefCell<Option<(String, String)>> = RefCell::default();
    show(
        owner,
        caption,
        [287, 162],
        &controls,
        Some(bookmark_proc),
        LPARAM((&raw const selection) as isize),
    )?;
    Ok(selection.into_inner())
}

unsafe extern "system" fn bookmark_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    unsafe {
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, lp.0);
                SendDlgItemMessageW(hwnd, 101, EM_SETLIMITTEXT, WPARAM(400), LPARAM(0));
                SendDlgItemMessageW(hwnd, 102, EM_SETLIMITTEXT, WPARAM(10), LPARAM(0));
                if let Ok(edit) = GetDlgItem(Some(hwnd), 101) {
                    let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(edit));
                }
                SendDlgItemMessageW(hwnd, 101, EM_SETSEL, WPARAM(0), LPARAM(-1));
            }
            WM_ERASEBKGND => field_background(hwnd, HDC(wp.0 as _), &BOOKMARK_FIELDS),
            WM_LBUTTONDOWN if field_focus(hwnd, lp, &BOOKMARK_FIELDS) => {}
            WM_COMMAND if wp.0 as u16 == 1 => {
                let title: String = value(hwnd, 101).trim().chars().take(200).collect();
                if !title.is_empty() {
                    let selection = GetWindowLongPtrW(hwnd, GWLP_USERDATA)
                        as *const RefCell<Option<(String, String)>>;
                    if let Some(selection) = selection.as_ref() {
                        *selection.borrow_mut() = Some((title, value(hwnd, 102)));
                    }
                    let _ = EndDialog(hwnd, 1);
                }
            }
            WM_COMMAND if wp.0 as u16 == 2 => {
                let _ = EndDialog(hwnd, 0);
            }
            WM_CLOSE => {
                let _ = EndDialog(hwnd, 0);
            }
            _ => return 0,
        }
        isize::from(msg != WM_INITDIALOG)
    }
}

pub fn permanent_scope(owner: HWND, count: usize) -> Result<Option<bool>> {
    choose(
        owner,
        tr!("Rendi modifiche permanenti", "Make changes permanent"),
        tr!(
            "Su quali PDF vuoi incorporare annotazioni e segnalibri?",
            "Which PDFs should get the annotations and bookmarks?"
        ),
        tr!(
            "Il PDF corrente comprende tutte le sue pagine.",
            "This applies to every page in the current PDF."
        ),
        tr!("PDF corrente", "Current PDF"),
        &trf!("Tutta la scaletta ({count})", "Entire setlist ({count})"),
        tr!("Annulla", "Cancel"),
    )
}

pub fn permanent_target(owner: HWND, count: usize) -> Result<Option<bool>> {
    if count > 1 {
        return choose(
            owner,
            tr!("Rendi modifiche permanenti", "Make changes permanent"),
            &trf!(
                "Incorpora annotazioni e segnalibri nei {count} PDF della scaletta.",
                "Embed annotations and bookmarks in all {count} PDFs in this setlist."
            ),
            tr!(
                "Puoi creare le copie in una cartella. Sovrascrivere gli originali è irreversibile.",
                "You can save the copies in a folder. Overwriting the originals is permanent."
            ),
            tr!("Crea copie…", "Create copies…"),
            tr!("Sovrascrivi gli originali", "Overwrite originals"),
            tr!("Annulla", "Cancel"),
        );
    }
    choose(
        owner,
        tr!("Rendi modifiche permanenti", "Make changes permanent"),
        tr!(
            "Incorpora annotazioni e segnalibri nel PDF corrente.",
            "Embed annotations and bookmarks in the current PDF."
        ),
        tr!(
            "Aggiornare l’originale è irreversibile. Per mantenere il PDF com’è, crea una copia.",
            "Updating the original is permanent. To keep the original PDF intact, create a copy."
        ),
        tr!("Crea una copia…", "Create a copy…"),
        tr!("Sovrascrivi l’originale", "Overwrite original"),
        tr!("Annulla", "Cancel"),
    )
}

pub fn default_pdf(owner: HWND) -> Result<bool> {
    offer(
        owner,
        tr!("App predefinita per i PDF", "Default PDF app"),
        tr!(
            "Vuoi usare ScoreDraw per aprire i PDF?",
            "Use ScoreDraw to open PDFs?"
        ),
        tr!(
            "Si apriranno le Impostazioni di Windows, dove potrai confermare la scelta.",
            "Windows Settings will open so you can confirm your choice."
        ),
        tr!("Apri Impostazioni di Windows", "Open Windows Settings"),
        tr!("No, grazie", "No, thanks"),
    )
}

pub fn feedback(owner: HWND, address: &str) -> Result<()> {
    let controls = [
        Control::label(
            100,
            [13, 12, 290, 26],
            tr!(
                "Segnala un problema o proponi una modifica via email.\r\nSe puoi, indica la versione di ScoreDraw.",
                "Report a problem or suggest a change by email.\r\nInclude your ScoreDraw version if you can."
            ),
        ),
        FEEDBACK_ADDRESS.control(address, ES_READONLY as u32),
        Control::button(
            FEEDBACK_COPY,
            [271, 42, 32, 27],
            tr!("Copia indirizzo email", "Copy email address"),
        ),
        Control::label(FEEDBACK_STATUS, [13, 84, 209, 16], ""),
        Control::button(2, [228, 77, 75, 25], tr!("Chiudi", "Close")),
    ];
    show(
        owner,
        tr!("Segnalazioni · ScoreDraw", "Feedback · ScoreDraw"),
        [316, 114],
        &controls,
        Some(feedback_proc),
        LPARAM(0),
    )
    .map(|_| ())
}

unsafe extern "system" fn feedback_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    match msg {
        WM_ERASEBKGND => {
            field_background(hwnd, HDC(wp.0 as _), &[FEEDBACK_ADDRESS]);
            1
        }
        WM_LBUTTONDOWN => isize::from(field_focus(hwnd, lp, &[FEEDBACK_ADDRESS])),
        WM_COMMAND if wp.0 as u16 == FEEDBACK_COPY => {
            let address = value(hwnd, FEEDBACK_ADDRESS.id);
            let copied = copy_text(hwnd, &address).is_ok();
            set_value(
                hwnd,
                FEEDBACK_STATUS,
                if copied {
                    tr!("Indirizzo copiato", "Address copied")
                } else {
                    tr!("Copia non riuscita. Riprova.", "Couldn’t copy. Try again.")
                },
            );
            1
        }
        _ => unsafe { choice_proc(hwnd, msg, wp, lp) },
    }
}

fn copy_text(owner: HWND, value: &str) -> Result<()> {
    const CF_UNICODETEXT: u32 = 13;
    let utf16: Vec<_> = value.encode_utf16().chain([0]).collect();
    unsafe {
        let memory = GlobalAlloc(GMEM_MOVEABLE, std::mem::size_of_val(utf16.as_slice()))?;
        let result = (|| {
            let target = GlobalLock(memory).cast::<u16>();
            if target.is_null() {
                return Err(Error::from_thread());
            }
            target.copy_from_nonoverlapping(utf16.as_ptr(), utf16.len());
            let _ = GlobalUnlock(memory);
            OpenClipboard(Some(owner))?;
            let result = EmptyClipboard()
                .and_then(|_| SetClipboardData(CF_UNICODETEXT, Some(HANDLE(memory.0))));
            let _ = CloseClipboard();
            result.map(|_| ())
        })();
        if result.is_err() {
            // On success, Windows owns and releases the handle.
            let _ = GlobalFree(Some(memory));
        }
        result
    }
}

fn offer(
    owner: HWND,
    title: &str,
    prompt: &str,
    detail: &str,
    action: &str,
    cancel: &str,
) -> Result<bool> {
    let message = format!("{prompt}\r\n{detail}");
    let controls = [
        Control::label(101, [13, 12, 271, 35], &message),
        Control::button(1, [13, 50, 158, 28], action),
        Control::button(2, [181, 50, 84, 28], cancel),
    ];
    show(
        owner,
        title,
        [278, 90],
        &controls,
        Some(choice_proc),
        LPARAM(0),
    )
    .map(|result| result == 1)
}

fn choose(
    owner: HWND,
    title: &str,
    prompt: &str,
    detail: &str,
    first: &str,
    second: &str,
    cancel: &str,
) -> Result<Option<bool>> {
    let message = format!("{prompt}\r\n{detail}");
    let controls = [
        Control::label(101, [13, 12, 323, 35], &message),
        Control::button(1, [13, 50, 106, 28], first),
        Control::button(3, [124, 50, 129, 28], second),
        Control::button(2, [258, 50, 78, 28], cancel),
    ];
    match show(
        owner,
        title,
        [349, 90],
        &controls,
        Some(choice_proc),
        LPARAM(0),
    )? {
        1 => Ok(Some(false)),
        3 => Ok(Some(true)),
        _ => Ok(None),
    }
}

pub fn reading(owner: HWND, title: &str, content: &str) -> Result<()> {
    let controls = [
        Control {
            id: 101,
            class: EDIT,
            rect: [13, 13, 315, 200],
            title: content,
            style: (WS_TABSTOP | WS_VSCROLL).0
                | (ES_MULTILINE | ES_AUTOVSCROLL | ES_READONLY) as u32,
        },
        Control {
            id: 2,
            class: BUTTON,
            rect: [270, 223, 58, 26],
            title: "×",
            style: WS_TABSTOP.0,
        },
    ];
    show(
        owner,
        title,
        [341, 261],
        &controls,
        Some(choice_proc),
        LPARAM(0),
    )
    .map(|_| ())
}

unsafe extern "system" fn choice_proc(hwnd: HWND, msg: u32, wp: WPARAM, _lp: LPARAM) -> isize {
    let id = wp.0 as u16;
    if msg == WM_CLOSE || msg == WM_COMMAND && (1..=3).contains(&id) {
        let _ = unsafe { EndDialog(hwnd, if msg == WM_CLOSE { 0 } else { id as isize }) };
        1
    } else {
        0
    }
}

fn dword(words: &mut Vec<u16>, value: u32) {
    words.extend_from_slice(&[value as u16, (value >> 16) as u16]);
}

fn wide(words: &mut Vec<u16>, text: &str) {
    words.extend(text.encode_utf16());
    words.push(0);
}

pub fn value(hwnd: HWND, id: u16) -> String {
    let Ok(control) = (unsafe { GetDlgItem(Some(hwnd), id.into()) }) else {
        return String::new();
    };
    let length = unsafe { GetWindowTextLengthW(control) };
    let mut text = vec![0u16; length.max(0) as usize + 1];
    let read = unsafe { GetWindowTextW(control, &mut text) };
    String::from_utf16_lossy(&text[..read.max(0) as usize])
}

pub fn set_value(hwnd: HWND, id: u16, value: &str) {
    let text = HSTRING::from(value);
    let _ = unsafe { SetDlgItemTextW(hwnd, id.into(), &text) };
}

pub fn enable(hwnd: HWND, id: u16, enabled: bool) {
    use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
    if let Ok(control) = unsafe { GetDlgItem(Some(hwnd), id.into()) } {
        let _ = unsafe { EnableWindow(control, enabled) };
    }
}

pub fn selection(hwnd: HWND, id: u16) -> Option<usize> {
    let result =
        unsafe { SendDlgItemMessageW(hwnd, id.into(), LB_GETCURSEL, WPARAM(0), LPARAM(0)) };
    usize::try_from(result.0).ok()
}

pub fn rows(
    hwnd: HWND,
    id: u16,
    labels: impl IntoIterator<Item = String>,
    selected: Option<usize>,
) {
    let Ok(list) = (unsafe { GetDlgItem(Some(hwnd), i32::from(id)) }) else {
        return;
    };
    unsafe {
        let first = SendMessageW(list, LB_GETTOPINDEX, None, None).0.max(0) as usize;
        SendMessageW(list, WM_SETREDRAW, Some(WPARAM(0)), None);
        SendDlgItemMessageW(hwnd, id.into(), LB_RESETCONTENT, WPARAM(0), LPARAM(0));
        for label in labels {
            let text = HSTRING::from(label);
            SendDlgItemMessageW(
                hwnd,
                id.into(),
                LB_ADDSTRING,
                WPARAM(0),
                LPARAM(text.as_ptr() as isize),
            );
        }
        if let Some(index) = selected {
            SendMessageW(list, LB_SETTOPINDEX, Some(WPARAM(first.min(index))), None);
            SendDlgItemMessageW(hwnd, id.into(), LB_SETCURSEL, WPARAM(index), LPARAM(0));
        }
        SendMessageW(list, WM_SETREDRAW, Some(WPARAM(1)), None);
        let _ = InvalidateRect(Some(list), None, true);
    }
}

pub fn message(hwnd: HWND, title: &str, text: &str, kind: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    unsafe {
        MessageBoxW(
            Some(hwnd),
            &HSTRING::from(text),
            &HSTRING::from(title),
            kind,
        )
    }
}
