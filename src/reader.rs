use crate::audio::Sound;
use crate::data::{ReadingPosition, ScoreEntry};
use crate::canvas::{self, Canvas};
use crate::data::{Favorites, ScoreFile, Settings, Store};
use crate::icons::{self, Glyph};
use crate::ink::{Ink, InkArea, LayerVersion, MOUSE, SAVE_RESULT, SaveStatus, Saver, Tool};
use crate::overlay::{self, Event, Overlay, Sheet};
use crate::pdf::Frame;
use crate::pdf_edit::{self, Bookmarks};
use crate::radial::{Command, Radial};
use crate::render_queue::{self, PageKey, Request, Response, ScoreInfo};
use crate::view::{Motion, PageWheel, Touch, View};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{OsString, c_void};
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{
    COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Power::{ES_CONTINUOUS, ES_DISPLAY_REQUIRED, SetThreadExecutionState};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
    VK_CONTROL,
};
use windows::Win32::UI::Input::Pointer::{
    GetPointerFrameInfo, GetPointerInfo, GetPointerType, POINTER_FLAG_INCONTACT, POINTER_INFO,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT,
    FileOpenDialog, FileSaveDialog, IFileOpenDialog, IFileSaveDialog, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Error, HSTRING, PCWSTR, Result, w};

pub(crate) const BACKGROUND: COLORREF = COLORREF(0x181e1b);
pub(crate) const PANEL: COLORREF = COLORREF(0x232c27);
pub(crate) const TEXT: COLORREF = COLORREF(0xeaf4f2);
pub(crate) const MUTED: COLORREF = COLORREF(0xb5c5be);
pub(crate) const ACCENT: COLORREF = COLORREF(0x90cdba);
pub(crate) const SURFACE: COLORREF = COLORREF(0x333e36);
const MENU_BORDER: COLORREF = COLORREF(0x4c5b4b);
pub(crate) const ALERT: COLORREF = COLORREF(0x9abffb);
const RESIZE_TIMER: usize = 1;
const RADIAL_TIMER: usize = 2;
const AUDIO_TIMER: usize = 3;
const TOAST_TIMER: usize = 4;
const UTILITIES_TIMER: usize = 5;
const UTILITIES_IDLE: Duration = Duration::from_millis(5600);
const PICK_PDF: u32 = WM_APP + 2;
const OPEN_LIBRARY: u32 = WM_APP + 3;
const EDIT_TUNING: u32 = WM_APP + 31;
const OPEN_PREFERENCES: u32 = WM_APP + 4;
const EXPORT_PDF: u32 = WM_APP + 8;
const EDIT_BOOKMARK: u32 = WM_APP + 9;
const PDF_EDIT_RESULT: u32 = WM_APP + 10;
const SHOW_ABOUT: u32 = WM_APP + 12;
const SHOW_EXPORT_REPORT: u32 = WM_APP + 13;
const DEFAULT_PDF: u32 = WM_APP + 14;
const SHOW_FEEDBACK: u32 = WM_APP + 15;
const CHECK_STORE_UPDATE: u32 = WM_APP + 16;
const STORE_UPDATE_AVAILABLE: u32 = WM_APP + 17;
const BOOKMARK_DRAG_TIMER: usize = 7;
const INK_SAVE_TIMER: usize = 9;
const READING_SAVE_TIMER: usize = 10;
const THUMBNAIL_WARMUP_TIMER: usize = 11;
const VIEW_SETTLE_MS: u32 = 130;
const INK_SAVE_MS: u32 = 180;
const ZOOM_TIME: Duration = Duration::from_millis(170);
const WELCOME_LOGO: &[u8] = include_bytes!("../assets/scoredraw.png");

mod animation;
mod direct2d;
mod association;
mod browse;
mod tools;
use animation::{ANIMATION_FRAME, Animation};
mod chrome;
mod store_update;
mod trackpad;

pub fn show_startup_error(error: &Error) {
    let message = HSTRING::from(error.to_string());
    unsafe { MessageBoxW(None, &message, w!("ScoreDraw"), MB_OK | MB_ICONERROR) };
}

pub fn run(files: Vec<PathBuf>) -> Result<()> {
    let _drawing = canvas::Startup::new()?;
    // The UI thread is STA for the Windows file dialog. PDF calls run on a separate thread.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
    }
    let result = run_window(files);
    unsafe { CoUninitialize() };
    result
}

fn run_window(files: Vec<PathBuf>) -> Result<()> {
    let store = Store::open().map_err(Error::from)?;
    let settings = store.settings().unwrap_or_default();
    crate::language::apply(settings.language);
    let state = RefCell::new(Reader {
        welcome_logo: canvas::Bitmap::from_png(WELCOME_LOGO),
        store: Some(store.clone()),
        recent: store.recent().unwrap_or_default(),
        favorites: store.favorites().unwrap_or_default(),
        tool: settings.active_tool,
        settings,
        frame_timing: std::env::var_os("SCOREDRAW_RENDER_TRACE").map(|_| animation::FrameTiming::default()),
        ..Reader::default()
    });
    // The stack-owned state outlives the window and is removed from its data slot on destruction.
    unsafe {
        let instance = HINSTANCE(GetModuleHandleW(None)?.0);
        let icon = LoadImageW(
            Some(instance),
            PCWSTR(std::ptr::without_provenance(101)),
            IMAGE_ICON,
            128,
            128,
            LR_SHARED,
        )
        .map_or(HICON::default(), |handle| HICON(handle.0));
        let class_name = w!("ScoreDraw.Reader");
        let class = WNDCLASSW {
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hIcon: icon,
            hInstance: instance,
            lpszClassName: class_name,
            lpfnWndProc: Some(window_proc),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err(Error::from_thread());
        }
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class_name,
            w!("ScoreDraw"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            940,
            880,
            None,
            None,
            Some(instance),
            Some((&raw const state).cast::<c_void>()),
        )?;
        crate::dialog::dark_title(hwnd);
        if let Ok(mut app) = state.try_borrow_mut() {
            app.animation = Animation::new(hwnd).ok();
            app.trackpad = trackpad::Trackpad::register(hwnd);
            let (requests, responses) = match render_queue::spawn(hwnd, store.thumbnail_cache()) {
                Ok(channels) => channels,
                Err(error) => {
                    let _ = DestroyWindow(hwnd);
                    return Err(Error::from(error));
                }
            };
            app.requests = Some(requests);
            app.responses = Some(responses);
            match Saver::start(Some(hwnd)) {
                Ok(saver) => app.saver = Some(saver),
                Err(error) => {
                    let _ = DestroyWindow(hwnd);
                    return Err(Error::from(error));
                }
            }
        }
        let launched_with_pdf = !files.is_empty();
        state.borrow_mut().open(hwnd, files, false);
        let _ = ShowWindow(hwnd, SW_SHOWMAXIMIZED);
        // The launching shell can override the first ShowWindow call.
        if !IsZoomed(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_MAXIMIZE);
        }
        post(hwnd, CHECK_STORE_UPDATE);
        if !launched_with_pdf {
            post(hwnd, DEFAULT_PDF);
        }
        let mut _store_check = None;
        let mut message = MSG::default();
        loop {
            match GetMessageW(&mut message, None, 0, 0).0 {
                -1 => {
                    let _ = DestroyWindow(hwnd);
                    return Err(Error::from_thread());
                }
                0 => return Ok(()),
                _ if message.hwnd == hwnd && message.message == PICK_PDF => {
                    // The dialog pumps messages. Leave reader state unborrowed while it runs.
                    match choose_files(hwnd, true) {
                        Ok(files) => state.borrow_mut().open(hwnd, files, false),
                        Err(error) => {
                            let text = HSTRING::from(error.to_string());
                            MessageBoxW(Some(hwnd), &text, w!("ScoreDraw"), MB_OK | MB_ICONWARNING);
                        }
                    }
                }
                _ if message.hwnd == hwnd && message.message == OPEN_LIBRARY => {
                    match crate::library::show(hwnd, &store) {
                        Ok(Some(files)) => {
                            let mut app = state.borrow_mut();
                            app.open_entries(hwnd, files, true);
                        }
                        Ok(None) => {}
                        Err(error) => {
                            crate::dialog::message(
                                hwnd,
                                tr!("Scalette · ScoreDraw", "Setlists · ScoreDraw"),
                                &error,
                                MB_OK | MB_ICONWARNING,
                            );
                        }
                    }
                }
                _ if message.hwnd == hwnd && message.message == OPEN_PREFERENCES => {
                    let initial = {
                        let app = state.borrow();
                        app.tool.style(&app.settings).color
                    };
                    if let Some(color) = crate::preferences::pick(hwnd, initial) {
                        state.borrow_mut().event(hwnd, Event::Color(color));
                    }
                }
                _ if message.hwnd == hwnd && message.message == EDIT_TUNING => {
                    let initial = state.borrow().settings.tuning_hz.to_string();
                    if let Ok(Some(value)) = crate::dialog::prompt_text(
                        hwnd, tr!("Accordatura di riferimento", "Reference tuning"),
                        tr!("Frequenza del La in Hz (400–480)", "A frequency in Hz (400–480)"),
                        &initial, tr!("Applica", "Apply"),
                        |text| text.parse::<u16>().ok().filter(|hz| (400..=480).contains(hz))
                            .map(|_| ()).ok_or(tr!("Inserisci un valore da 400 a 480.", "Enter a value from 400 to 480.")),
                    ) && let Ok(tuning_hz) = value.parse() {
                        let mut app = state.borrow_mut();
                        let settings = Settings { tuning_hz, ..app.settings };
                        if app.commit_settings(hwnd, settings) {
                            app.sound.tick(tuning_hz);
                            repaint(hwnd);
                        }
                    }
                }
                _ if message.hwnd == hwnd && message.message == EXPORT_PDF => {
                    tools::export_dialog(hwnd, &state, &store);
                }
                _ if message.hwnd == hwnd && message.message == EDIT_BOOKMARK => {
                    tools::bookmark_dialog(hwnd, &state);
                }
                _ if message.hwnd == hwnd && message.message == SHOW_ABOUT => {
                    tools::show_licenses(hwnd);
                }
                _ if message.hwnd == hwnd && message.message == SHOW_FEEDBACK => {
                    tools::show_feedback(hwnd);
                }
                _ if message.hwnd == hwnd && message.message == SHOW_EXPORT_REPORT => {
                    let report = state.borrow_mut().export_report.take();
                    if let Some(report) = report {
                        let _ = crate::dialog::reading(
                            hwnd,
                            tr!("Esito dell’incorporazione", "PDF update results"),
                            &report,
                        );
                    }
                }
                _ if message.hwnd == hwnd && message.message == DEFAULT_PDF => {
                    association::prompt(hwnd, &state, message.wParam.0 != 0);
                }
                _ if message.hwnd == hwnd && message.message == CHECK_STORE_UPDATE => {
                    _store_check = store_update::Check::start(hwnd).ok();
                }
                _ => {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        }
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let create = &*(lp.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        if msg == WM_NCDESTROY {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        }
        if msg == WM_DESTROY {
            SetThreadExecutionState(ES_CONTINUOUS);
            PostQuitMessage(0);
            return LRESULT(0);
        }
        if let Some(result) = chrome::frame_message(hwnd, msg, wp, lp) {
            return result;
        }
        let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const RefCell<Reader>;
        // Some Win32 APIs dispatch messages synchronously. Avoid a second mutable borrow.
        if let Some(state) = state.as_ref()
            && let Ok(mut app) = state.try_borrow_mut()
            && let Some(result) = app.handle(hwnd, msg, wp, lp)
        {
            return result;
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

#[derive(Default)]
struct Reader {
    welcome_logo: Option<canvas::Bitmap>,
    recent: Vec<ReadingPosition>,
    reading_dirty: bool,
    browser: browse::Browser,
    session: u64,
    requests: Option<mpsc::Sender<Request>>,
    responses: Option<mpsc::Receiver<Response>>,
    scores: Vec<ScoreInfo>,
    total: u32,
    selected: u32,
    concert: bool,
    store_update_available: bool,
    desired: Option<PageKey>,
    visible: Option<(PageKey, Arc<Frame>)>,
    cache: HashMap<PageKey, Arc<Frame>>,
    queued: Vec<PageKey>,
    status: String,
    fullscreen: Option<WINDOWPLACEMENT>,
    touch: Touch,
    wheel: PageWheel,
    trackpad: Option<trackpad::Trackpad>,
    view: View,
    view_moving: bool,
    view_panning: bool,
    zoom_animation: Option<ZoomAnimation>,
    mouse_pan: Option<POINT>,
    settings: Settings,
    store: Option<Store>,
    ink: Ink,
    tool: Tool,
    radial: Radial,
    overlay: Overlay,
    sound: Sound,
    tempo_taps: std::collections::VecDeque<Instant>,
    favorites: Favorites,
    collapsed_bookmarks: std::collections::HashSet<usize>,
    bookmark_edit: Option<Event>,
    bookmark_drag: Option<POINT>,
    bookmark_scroll_y: Option<i32>,
    bookmark_wheel_delta: i32,
    resume_page: Option<u32>,
    original_export: bool,
    utilities_activity: Option<Instant>,
    bookmarks: Option<Bookmarks>,
    bookmark_source: Option<ScoreFile>,
    bookmark_status: String,
    bookmark_loading: Option<mpsc::Receiver<std::result::Result<Bookmarks, String>>>,
    exporting: Option<mpsc::Receiver<std::result::Result<tools::Finalized, String>>>,
    export_report: Option<String>,
    toast: Option<String>,
    close_after_export: bool,
    animation: Option<Animation>,
    frame_timing: Option<animation::FrameTiming>,
    slider_original: Option<Settings>,
    chrome_menu: Option<chrome::Menu>,
    chrome_visual: Option<chrome::Menu>,
    overlay_hover: Option<POINT>,
    chrome_hover: Option<chrome::Target>,
    chrome_press: Option<(u32, chrome::Target)>,
    saver: Option<Saver>,
    ink_save_pending: bool,
    close_after_save: bool,
    close_document_after_save: bool,
    recovered_page: bool,
    paint_buffer: Option<Surface>,
    direct2d: Option<direct2d::Renderer>,
    direct2d_unavailable: bool,
    blend_buffer: RefCell<Option<Surface>>,
    page_surface: Option<PageSurface>,
    animate_next_turn: bool,
    page_animation: Option<PageAnimation>,
    page_drag: Option<PageDrag>,
}

impl Reader {
    fn handle(&mut self, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        if matches!(
            msg,
            WM_MOUSEMOVE | WM_NCMOUSEMOVE | WM_MOUSEWHEEL | WM_MOUSEHWHEEL
        ) && !self.ink.is_drawing()
        {
            self.reveal_utilities(hwnd);
        }
        if msg == WM_MOUSEMOVE && self.fullscreen.is_none() {
            self.update_chrome_hover(hwnd, point_from_mouse(lp));
        } else if matches!(msg, WM_MOUSELEAVE | WM_NCMOUSEMOVE)
            && self.chrome_hover.take().is_some()
            && let Some(strip) = Layout::new(hwnd, false).strip
        {
            repaint_rect(hwnd, strip.rect);
        }
        if msg == WM_MOUSEMOVE {
            self.browse_hover(hwnd, Some(point_from_mouse(lp)));
            self.hover_overlays(hwnd, Some(point_from_mouse(lp)));
        } else if matches!(msg, WM_MOUSELEAVE | WM_NCMOUSEMOVE) {
            self.browse_hover(hwnd, None);
            self.hover_overlays(hwnd, None);
        }
        match msg {
            WM_COMMAND if self.search_command(hwnd, wp) => {}
            browse::SEARCH_KEY => self.search_key(hwnd, wp.0),
            WM_CTLCOLOREDIT if unsafe { GetDlgCtrlID(HWND(lp.0 as _)) } == i32::from(browse::SEARCH_ID) => unsafe {
                let dc = HDC(wp.0 as _);
                SetTextColor(dc, TEXT);
                SetBkColor(dc, SURFACE);
                SetDCBrushColor(dc, SURFACE);
                return Some(LRESULT(GetStockObject(DC_BRUSH).0 as isize));
            },
            WM_PAINT => self.paint(hwnd),
            WM_ERASEBKGND => return Some(LRESULT(1)),
            WM_SIZE => unsafe {
                self.page_animation = None;
                self.zoom_animation = None;
                SetTimer(Some(hwnd), RESIZE_TIMER, VIEW_SETTLE_MS, None);
                if self.visible.is_some() {
                    self.animate(hwnd);
                }
                repaint(hwnd);
            },
            WM_TIMER if wp.0 == RESIZE_TIMER => {
                let _ = unsafe { KillTimer(Some(hwnd), RESIZE_TIMER) };
                if self.zoom_animation.is_none() {
                    if self.ink.is_drawing() {
                        self.defer_view_refresh(hwnd);
                    } else {
                        self.view_moving = false;
                        self.view_panning = false;
                        self.refresh(hwnd);
                    }
                }
            }
            WM_TIMER if wp.0 == INK_SAVE_TIMER => {
                self.save_ink(hwnd);
            }
            WM_TIMER if wp.0 == READING_SAVE_TIMER => self.save_reading_position(hwnd),
            WM_TIMER if wp.0 == THUMBNAIL_WARMUP_TIMER => self.warm_thumbnails(hwnd),
            WM_TIMER | ANIMATION_FRAME if msg == ANIMATION_FRAME || wp.0 == RADIAL_TIMER => {
                let finished = self.animate_frame(hwnd);
                // WM_PAINT is normally generated only after the input queue drains.
                // Render the invalidated region now, even during continuous pointer input.
                if unsafe { GetUpdateRect(hwnd, None, false) }.as_bool() {
                    self.paint(hwnd);
                    if let Some(timing) = &mut self.frame_timing {
                        timing.painted(Instant::now());
                    }
                }
                if let Some(clock) = &self.animation {
                    clock.presented();
                }
                if finished && let Some(timing) = &mut self.frame_timing {
                    timing.finish();
                }
            }
            WM_TIMER if wp.0 == AUDIO_TIMER => {
                let playing = self.sound.running(0);
                if self.sound.tick(self.settings.tuning_hz) {
                    let layout = Layout::new(hwnd, self.fullscreen.is_some());
                    self.repaint_audio(hwnd, &layout);
                    if playing != self.sound.running(0) {
                        let (bounds, scale) = self.radial_area(&layout);
                        repaint_rect(hwnd, self.radial.footprint(bounds, scale));
                    }
                }
                if !self.sound.running(0) && !self.sound.running(1) {
                    let _ = unsafe { KillTimer(Some(hwnd), AUDIO_TIMER) };
                }
            }
            WM_TIMER if wp.0 == TOAST_TIMER => {
                let _ = unsafe { KillTimer(Some(hwnd), TOAST_TIMER) };
                self.toast = None;
                repaint(hwnd);
            }
            WM_TIMER if wp.0 == UTILITIES_TIMER => self.check_utilities_idle(hwnd),
            WM_TIMER if wp.0 == BOOKMARK_DRAG_TIMER => self.scroll_dragging_bookmark(hwnd),
            WM_DPICHANGED => unsafe {
                let rect = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            },
            WM_GETMINMAXINFO => unsafe {
                let minimum = &mut *(lp.0 as *mut MINMAXINFO);
                minimum.ptMinTrackSize = POINT { x: 410, y: 430 };
            },
            render_queue::RENDER_RESULT => self.receive(hwnd),
            STORE_UPDATE_AVAILABLE => {
                self.store_update_available = true;
                if !self.performing() {
                    repaint(hwnd);
                }
            }
            PDF_EDIT_RESULT => self.pdf_edit_result(hwnd),
            SAVE_RESULT => {
                if self.close_after_save && !matches!(self.save_status(), SaveStatus::Saving) {
                    self.close_after_save = false;
                    post(hwnd, WM_CLOSE);
                } else if self.close_document_after_save && !matches!(self.save_status(), SaveStatus::Saving) {
                    self.close_document_after_save = false;
                    self.close_document(hwnd);
                }
                let layout = Layout::new(hwnd, self.fullscreen.is_some());
                repaint_rect(hwnd, layout.save_notice());
                let (bounds, scale) = self.radial_area(&layout);
                repaint_rect(hwnd, self.radial.hub(bounds, scale));
            }
            WM_CLOSE => {
                self.save_reading_position(hwnd);
                self.ink.cancel_contact();
                if self.save_ink(hwnd) {
                    if self.exporting.is_some() {
                        self.close_after_export = true;
                        return Some(LRESULT(0));
                    }
                    match self.save_status() {
                        SaveStatus::Idle => return None,
                        SaveStatus::Saving => {
                            self.close_after_save = true;
                            repaint(hwnd);
                        }
                        SaveStatus::Recovered(paths) => {
                            self.show_recovery(hwnd, &paths);
                            return None;
                        }
                        SaveStatus::Failed(error) => {
                            let message = trf!(
                                "Le modifiche sono ancora aperte.\n\n{error}",
                                "Your changes are still open.\n\n{error}"
                            );
                            if crate::dialog::message(
                                hwnd,
                                tr!("Annotazioni · ScoreDraw", "Annotations · ScoreDraw"),
                                &message,
                                MB_RETRYCANCEL | MB_ICONWARNING,
                            ) == IDRETRY
                                && self.retry_save(hwnd)
                            {
                                self.close_after_save = true;
                            }
                        }
                    }
                }
            }
            WM_KEYDOWN if wp.0 == 0x46 && unsafe { GetKeyState(VK_CONTROL.0.into()) } < 0
                && self.focus_browse_search(hwnd) => {}
            WM_KEYDOWN if self.page_grid_open() && wp.0 != 0x1b => self.grid_key_down(hwnd, wp.0),
            WM_KEYDOWN => match wp.0 {
                0x25 | 0x21 => self.turn(hwnd, -1),
                0x27 | 0x22 | 0x20 => self.turn(hwnd, 1),
                0x7a => self.toggle_fullscreen(hwnd),
                0x2e if self.can_delete_shape() => self.delete_shape(hwnd),
                0x1b if self.chrome_menu.take().is_some() => repaint(hwnd),
                0x1b if self.overlay.drawer => self.close_drawer(hwnd),
                0x1b if self.radial.open => {
                    self.radial.open = false;
                    self.animate(hwnd);
                }
                0x1b if self.fullscreen.is_some() => self.toggle_fullscreen(hwnd),
                0x30 if unsafe { GetKeyState(VK_CONTROL.0.into()) } < 0 => {
                    self.reset_view(hwnd);
                }
                0xbb | 0x6b | 0xbd | 0x6d if !self.ink.is_drawing() => {
                    self.zoom_step(hwnd, if matches!(wp.0, 0xbb | 0x6b) { 1 } else { -1 });
                }
                0x4f if unsafe { GetKeyState(VK_CONTROL.0.into()) } < 0 => {
                    request_file_picker(hwnd)
                }
                0x57 if unsafe { GetKeyState(VK_CONTROL.0.into()) } < 0 => self.close_document(hwnd),
                0x4c if unsafe { GetKeyState(VK_CONTROL.0.into()) } < 0 => post(hwnd, OPEN_LIBRARY),
                0x47 if unsafe { GetKeyState(VK_CONTROL.0.into()) } < 0 => {
                    self.open_sheet(hwnd, Sheet::Page)
                }
                0x42 if unsafe { GetKeyState(VK_CONTROL.0.into()) } < 0 => {
                    self.open_sheet(hwnd, Sheet::Bookmarks)
                }
                0x4d if unsafe { GetKeyState(VK_CONTROL.0.into()) } < 0 => {
                    self.command(hwnd, Command::Metronome)
                }
                0x54 if unsafe { GetKeyState(VK_CONTROL.0.into()) } < 0 => {
                    self.command(hwnd, Command::Tuner)
                }
                0x5a if unsafe { GetKeyState(VK_CONTROL.0.into()) } < 0 => self.undo_ink(hwnd),
                0x59 if unsafe { GetKeyState(VK_CONTROL.0.into()) } < 0 => self.redo_ink(hwnd),
                _ => return None,
            },
            WM_MOUSEWHEEL if {
                let mut point = point_from_mouse(lp);
                let _ = unsafe { ScreenToClient(hwnd, &mut point) };
                self.browse_wheel(hwnd, point, (wp.0 >> 16) as i16 as i32)
            } => {}
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL if !self.ink.is_drawing() && self.total > 0 => {
                let mut point = point_from_mouse(lp);
                let _ = unsafe { ScreenToClient(hwnd, &mut point) };
                let delta = (wp.0 >> 16) as i16 as i32;
                let horizontal = msg == WM_MOUSEHWHEEL;
                if !horizontal && wp.0 & 8 != 0 {
                    let at = (point.x as f32, point.y as f32);
                    self.transform_view(hwnd, at, at, 1.25_f32.powf(delta as f32 / 120.0));
                } else if !horizontal
                    && (self.favorite_wheel(hwnd, point, delta) || self.bookmark_wheel(hwnd, delta))
                {
                } else if self.overlay.drawer
                    || self.overlay.grab.is_some()
                    || self.chrome_menu.is_some()
                {
                    return None;
                } else if !horizontal && self.view.zoom() > 1.0 {
                    self.transform_view(hwnd, (0.0, 0.0), (0.0, delta as f32 * 0.55), 1.0);
                } else if let Some(step) = self.wheel.push(
                    unsafe { GetMessageTime() } as u32,
                    horizontal,
                    if horizontal { delta } else { -delta },
                ) {
                    self.turn_by_gesture(hwnd, step);
                }
            }
            WM_LBUTTONDOWN => self.mouse_down(hwnd, point_from_mouse(lp)),
            WM_MBUTTONDOWN if self.view.zoom() > 1.0 && !self.ink.is_drawing() => {
                self.interrupt_zoom(hwnd);
                self.mouse_pan = Some(point_from_mouse(lp));
                unsafe {
                    SetCapture(hwnd);
                }
            }
            WM_MOUSEMOVE if self.mouse_pan.is_some() => {
                let at = point_from_mouse(lp);
                if let Some(before) = self.mouse_pan.replace(at) {
                    self.transform_view(
                        hwnd,
                        (before.x as f32, before.y as f32),
                        (at.x as f32, at.y as f32),
                        1.0,
                    );
                }
            }
            WM_MBUTTONUP if self.mouse_pan.take().is_some() => {
                let _ = unsafe { ReleaseCapture() };
            }
            WM_MOUSEMOVE | WM_LBUTTONUP
                if self.browse_contact(hwnd, if msg == WM_LBUTTONUP { WM_POINTERUP } else { WM_POINTERUPDATE }, MOUSE, point_from_mouse(lp)) =>
            {
                if msg == WM_LBUTTONUP { let _ = unsafe { ReleaseCapture() }; }
            }
            WM_MOUSEMOVE | WM_LBUTTONUP
                if self.overlay.grab.is_some_and(|grab| grab.id == MOUSE) =>
            {
                let pointer_msg = if msg == WM_LBUTTONUP {
                    WM_POINTERUP
                } else {
                    WM_POINTERUPDATE
                };
                self.overlay_input(hwnd, pointer_msg, MOUSE, point_from_mouse(lp));
                if msg == WM_LBUTTONUP {
                    let _ = unsafe { ReleaseCapture() };
                }
            }
            WM_MOUSEMOVE if self.radial.grabbed(MOUSE) => {
                self.radial_input(hwnd, WM_POINTERUPDATE, MOUSE, point_from_mouse(lp));
            }
            WM_MOUSEMOVE if self.ink.has_contact(MOUSE) => {
                if let Some(area) = self.page_area(hwnd) {
                    self.ink.mouse_update(area, point_from_mouse(lp));
                    self.repaint_ink(hwnd, area);
                }
            }
            WM_LBUTTONUP => {
                let point = point_from_mouse(lp);
                if self.chrome_contact(hwnd, WM_POINTERUP, MOUSE, point) {
                    let _ = unsafe { ReleaseCapture() };
                } else if self.radial.grabbed(MOUSE) {
                    self.radial_input(hwnd, WM_POINTERUP, MOUSE, point_from_mouse(lp));
                    let _ = unsafe { ReleaseCapture() };
                } else if self.ink.has_contact(MOUSE) {
                    if let Some(area) = self.page_area(hwnd) {
                        self.ink.mouse_update(area, point_from_mouse(lp));
                        self.repaint_ink(hwnd, area);
                    }
                    self.complete_ink_contact(hwnd);
                    let _ = unsafe { ReleaseCapture() };
                } else {
                    self.click(hwnd, point_from_mouse(lp));
                }
            }
            WM_CAPTURECHANGED if self.ink.has_contact(MOUSE) => {
                self.ink.cancel_contact();
                self.schedule_ink_save(hwnd);
            }
            WM_CAPTURECHANGED => {
                self.browse_cancel(MOUSE);
                self.cancel_chrome_press(MOUSE);
                if self.radial.cancel(MOUSE) {
                    repaint(hwnd);
                }
                self.cancel_overlay(hwnd, MOUSE);
                self.mouse_pan = None;
            }
            WM_POINTERENTER | WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP => {
                if !self.pointer_message(hwnd, msg, (wp.0 & 0xffff) as u32) {
                    return None;
                }
            }
            WM_KILLFOCUS => {
                self.wheel = PageWheel::default();
                if let Some(trackpad) = &mut self.trackpad {
                    trackpad.cancel();
                }
                return None;
            }
            WM_POINTERCAPTURECHANGED => {
                let id = (wp.0 & 0xffff) as u32;
                self.browse_cancel(id);
                self.cancel_chrome_press(id);
                if self.ink.has_contact(id) {
                    self.ink.cancel_contact();
                    self.schedule_ink_save(hwnd);
                }
                if self.radial.cancel(id) {
                    repaint(hwnd);
                }
                self.cancel_overlay(hwnd, id);
                if self.touch.cancel(id) {
                    self.release_page_drag(hwnd);
                }
            }
            _ => return None,
        }
        Some(LRESULT(0))
    }

    fn open(&mut self, hwnd: HWND, files: Vec<PathBuf>, concert: bool) {
        self.open_entries(
            hwnd,
            files.into_iter().map(ScoreEntry::Whole).collect(),
            concert,
        );
    }

    fn open_entries(&mut self, hwnd: HWND, files: Vec<ScoreEntry>, concert: bool) {
        if files.is_empty() {
            return;
        }
        self.close_document_after_save = false;
        if !self.reset_document(hwnd, concert) {
            return;
        }
        self.status = tr!("Apertura degli spartiti…", "Opening scores…").into();
        if let Some(worker) = &self.requests
            && worker.send(Request::Open(self.session, files)).is_err()
        {
            self.status = tr!(
                "Il motore PDF non è disponibile.",
                "The PDF engine is unavailable."
            )
            .into();
        }
        repaint(hwnd);
    }

    fn reset_document(&mut self, hwnd: HWND, concert: bool) -> bool {
        self.ink.cancel_contact();
        if !self.save_ink(hwnd) {
            return false;
        }
        self.session += 1;
        self.resume_page = None;
        self.concert = concert;
        if self.fullscreen.is_none() {
            unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
        }
        self.total = 0;
        self.selected = 0;
        self.view = View::default();
        self.view_moving = false;
        self.zoom_animation = None;
        self.touch = Touch::default();
        self.wheel = PageWheel::default();
        self.desired = None;
        self.visible = None;
        self.page_surface = None;
        self.page_drag = None;
        self.page_animation = None;
        self.animate_next_turn = false;
        self.cache.clear();
        self.queued.clear();
        self.scores.clear();
        self.bookmarks = None;
        self.bookmark_loading = None;
        self.bookmark_source = None;
        self.ink = Ink::default();
        self.recovered_page = false;
        self.radial.open = false;
        self.overlay.dismiss_drawer();
        self.chrome_menu = None;
        self.chrome_visual = None;
        self.overlay.popup_open = false;
        self.overlay.popup_opacity = 0.0;
        self.reset_thumbnails();
        let _ = unsafe { KillTimer(Some(hwnd), THUMBNAIL_WARMUP_TIMER) };
        self.hide_utilities(hwnd);
        self.animate(hwnd);
        self.status.clear();
        true
    }

    fn close_document(&mut self, hwnd: HWND) {
        if self.total == 0 || self.exporting.is_some() {
            return;
        }
        self.ink.cancel_contact();
        if !self.save_ink(hwnd) {
            return;
        }
        match self.save_status() {
            SaveStatus::Saving => {
                self.close_document_after_save = true;
                repaint(hwnd);
                return;
            }
            SaveStatus::Failed(error) => {
                self.show_error(hwnd, &error);
                return;
            }
            SaveStatus::Recovered(paths) => self.show_recovery(hwnd, &paths),
            SaveStatus::Idle => {}
        }
        if self.reset_document(hwnd, false) {
            if let Some(worker) = &self.requests {
                let _ = worker.send(Request::Close(self.session));
            }
            self.stop_audio_panels();
            if self.fullscreen.is_some() {
                self.toggle_fullscreen(hwnd);
            }
            repaint(hwnd);
        }
    }

    fn stop_audio_panels(&mut self) {
        for slot in 0..2 {
            self.overlay.dismiss_audio(slot);
            self.sound.stop(slot);
        }
        self.tempo_taps.clear();
    }

    fn receive(&mut self, hwnd: HWND) {
        let mut changed = false;
        while let Some(response) = self.responses.as_ref().and_then(|rx| rx.try_recv().ok()) {
            match response {
                Response::Opened(session, scores, total) if session == self.session => {
                    self.scores = scores;
                    self.total = total;
                    self.selected = self
                        .resume_page
                        .take()
                        .unwrap_or(0)
                        .min(total.saturating_sub(1));
                    self.status.clear();
                    if self.concert {
                        unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED) };
                    }
                    self.reveal_utilities(hwnd);
                    self.refresh(hwnd);
                }
                Response::Preview(key, frame)
                    if self.desired == Some(key)
                        && self.queued.contains(&key)
                        && self.visible.is_none() =>
                {
                    self.present(hwnd, key, frame);
                    changed = true;
                }
                Response::Thumbnail(key, frame) => self.thumbnail(hwnd, key, frame),
                Response::Rendered(key, frame) if self.queued.contains(&key) => {
                    self.queued.retain(|queued| *queued != key);
                    self.cache.insert(key, frame.clone());
                    changed |= self.page_drag.as_ref().and_then(|drag| drag.neighbor) == Some(key.index);
                    if self.desired == Some(key) {
                        if self.editable_frame().is_some()
                            && (self.view_moving || self.ink.is_drawing())
                        {
                            self.defer_view_refresh(hwnd);
                        } else {
                            self.present(hwnd, key, frame);
                            changed = true;
                        }
                    }
                }
                Response::OpenError(session, error) if session == self.session => {
                    self.stop_audio_panels();
                    self.status.clear();
                    self.resume_page = None;
                    self.concert = false;
                    if self.fullscreen.is_some() {
                        self.toggle_fullscreen(hwnd);
                    }
                    self.show_error(hwnd, &error);
                    changed = true;
                }
                Response::RenderError(key, error) => {
                    self.queued.retain(|queued| *queued != key);
                    if self.desired == Some(key) {
                        self.release_page_drag(hwnd);
                        if self
                            .visible
                            .as_ref()
                            .is_some_and(|(visible, _)| visible.index == key.index)
                        {
                            self.status = tr!(
                                "Definizione completa non disponibile.",
                                "Full text unavailable."
                            )
                            .into();
                        } else {
                            self.status =
                                tr!("Pagina non disponibile.", "Page unavailable.").into();
                            self.show_error(hwnd, &error);
                        }
                        changed = true;
                    }
                }
                Response::Unavailable(error) => {
                    self.status = tr!(
                        "Il motore PDF non è disponibile.",
                        "The PDF engine is unavailable."
                    )
                    .into();
                    self.show_error(hwnd, &error);
                    changed = true;
                }
                _ => {}
            }
        }
        if changed {
            repaint(hwnd);
        }
    }

    fn refresh(&mut self, hwnd: HWND) {
        if self.total == 0 {
            return;
        }
        let layout = Layout::new(hwnd, self.fullscreen.is_some());
        if layout.viewport.right <= layout.viewport.left
            || layout.viewport.bottom <= layout.viewport.top
        {
            return;
        }
        let base = PageKey {
            session: self.session,
            index: self.selected,
            width: render_dimension(layout.viewport.right - layout.viewport.left),
            height: render_dimension(layout.viewport.bottom - layout.viewport.top),
        };
        let zoom = self.view.zoom();
        // Cap the current page raster independently from the fit-sized neighbors.
        let key = PageKey {
            width: ((base.width as f32 * zoom) as u32).min(3200),
            height: ((base.height as f32 * zoom) as u32).min(3200),
            ..base
        };
        self.desired = Some(key);
        if let Some(frame) = self.cache.get(&key) {
            self.present(hwnd, key, frame.clone());
        } else if self
            .visible
            .as_ref()
            .is_none_or(|(visible, _)| visible.index != key.index)
        {
            self.status = tr!("Caricamento pagina…", "Loading page…").into();
        }
        let mut neighbors = render_queue::render_plan(key, base, self.total, &self.scores);
        self.cache.retain(|page, _| neighbors.contains(page));
        neighbors.retain(|page| !self.cache.contains_key(page));
        if neighbors != self.queued {
            self.queued = neighbors;
            if let Some(worker) = &self.requests
                && worker
                    .send(Request::Render {
                        first_page_preview: self.visible.is_none()
                            && self.queued.first() == Some(&key),
                        pages: self.queued.clone(),
                    })
                    .is_err()
            {
                self.status = tr!(
                    "Il motore PDF non è disponibile.",
                    "The PDF engine is unavailable."
                )
                .into();
            }
        }
        repaint(hwnd);
    }

    fn turn(&mut self, hwnd: HWND, step: i32) {
        let next = self.selected.saturating_add_signed(step);
        self.navigate(hwnd, next, true);
    }

    fn turn_by_gesture(&mut self, hwnd: HWND, step: i32) {
        let next = self.selected.saturating_add_signed(step);
        self.navigate(hwnd, next, true);
    }

    fn go_to(&mut self, hwnd: HWND, next: u32) {
        self.navigate(hwnd, next, false);
    }

    fn navigate(&mut self, hwnd: HWND, next: u32, animated: bool) {
        if next < self.total && next != self.selected {
            self.ink.cancel_contact();
            if !self.save_ink(hwnd) {
                return;
            }
            if !self.page_grid_open() {
                self.cancel_thumbnails();
            }
            self.selected = next;
            self.animate_next_turn = animated && !self.view_moving;
            self.page_animation = None;
            self.view = View::default();
            self.view_moving = false;
            self.zoom_animation = None;
            if self.overlay.drawer && self.overlay.sheet == Some(Sheet::Bookmarks) {
                self.load_bookmarks(hwnd);
            }
            self.refresh(hwnd);
        }
    }

    fn present(&mut self, hwnd: HWND, key: PageKey, frame: Arc<Frame>) {
        let previous = self
            .visible
            .as_ref()
            .map(|(key, _)| (key.session, key.index));
        if previous != Some((key.session, key.index)) {
            self.schedule_thumbnail_warmup(hwnd);
            self.ink.cancel_contact();
            if !self.save_ink(hwnd) {
                return;
            }
            let drag_start = self.page_drag.take().map_or(0, |drag| drag.dx.abs());
            if std::mem::take(&mut self.animate_next_turn)
                && let Some((session, index)) = previous
                && session == key.session
                && let Some((_, frame)) = &self.visible
                && self.page_surface.as_ref().is_some_and(|page| {
                    Arc::ptr_eq(&page.frame, frame)
                        && page.ink_version == Some(self.ink.layer_version())
                })
                && let Some(page) = self.page_surface.take()
            {
                self.page_animation = Some(PageAnimation {
                    outgoing: page.composite.unwrap_or(page.raster),
                    clip: page.clip,
                    forward: key.index > index,
                    distance: drag_start,
                    viewport: Layout::new(hwnd, self.fullscreen.is_some()).viewport,
                    started: None,
                });
            }
            let score = self
                .scores
                .iter()
                .rev()
                .find(|score| score.first_page <= key.index);
            let path = self.store.as_ref().zip(score).map(|(store, score)| {
                store.page_path(&score.source, score.pdf_page(key.index))
            });
            if let Some(score) = score {
                let position = ReadingPosition {
                    path: score.source.path.clone(),
                    page: score.pdf_page(key.index),
                };
                if self.recent.first() != Some(&position) {
                    position.move_to_front(&mut self.recent);
                    self.reading_dirty = true;
                    unsafe { SetTimer(Some(hwnd), READING_SAVE_TIMER, 750, None) };
                }
            }
            self.recovered_page = false;
            if let Some(saver) = &self.saver {
                match self.ink.bind(path, saver) {
                    Ok(recovered) => self.recovered_page = recovered,
                    Err(error) => self.show_error(
                        hwnd,
                        &trf!(
                            "Impossibile aprire le annotazioni della pagina. {error}",
                            "Couldn’t open the annotations for this page. {error}"
                        ),
                    ),
                }
            }
            if self.recovered_page {
                self.save_ink(hwnd);
            }
        }
        self.visible = Some((key, frame));
        self.status.clear();
    }

    fn save_reading_position(&mut self, hwnd: HWND) {
        let _ = unsafe { KillTimer(Some(hwnd), READING_SAVE_TIMER) };
        if self.reading_dirty
            && let Some(store) = &self.store
            && store.save_recent(&self.recent).is_ok()
        {
            self.reading_dirty = false;
        }
    }

    fn save_ink(&mut self, hwnd: HWND) -> bool {
        if std::mem::take(&mut self.ink_save_pending) {
            let _ = unsafe { KillTimer(Some(hwnd), INK_SAVE_TIMER) };
        }
        if let Some(saver) = &self.saver
            && let Err(error) = self.ink.save(saver)
        {
            self.show_error(
                hwnd,
                &trf!(
                    "Impossibile salvare le annotazioni: {error}",
                    "Couldn’t save annotations: {error}"
                ),
            );
            return false;
        }
        true
    }

    fn schedule_ink_save(&mut self, hwnd: HWND) {
        if self.ink_save_pending || !self.ink.has_unsaved() {
            return;
        }
        self.ink_save_pending =
            unsafe { SetTimer(Some(hwnd), INK_SAVE_TIMER, INK_SAVE_MS, None) } != 0;
        if !self.ink_save_pending {
            self.save_ink(hwnd);
        }
    }

    fn complete_ink_contact(&mut self, hwnd: HWND) {
        let editing = (self.ink.can_undo(), self.ink.can_redo());
        if self.ink.complete_contact() {
            self.enter_selection(hwnd);
            repaint(hwnd);
        }
        if self.overlay.utilities && editing != (self.ink.can_undo(), self.ink.can_redo()) {
            self.repaint_utilities(hwnd);
        }
        self.schedule_ink_save(hwnd);
    }

    fn enter_selection(&mut self, hwnd: HWND) {
        self.tool = Tool::Select;
        if self.overlay.drawer && self.overlay.sheet == Some(Sheet::Style) {
            self.close_drawer(hwnd);
        }
    }

    fn undo_ink(&mut self, hwnd: HWND) {
        if !self.can_undo() {
            return;
        }
        self.ink.undo();
        self.save_ink(hwnd);
        repaint(hwnd);
    }

    fn can_undo(&self) -> bool {
        self.editable_frame().is_some() && self.ink.can_undo()
    }

    fn can_redo(&self) -> bool {
        self.editable_frame().is_some() && self.ink.can_redo()
    }

    fn can_finalize(&self) -> bool {
        self.editable_frame().is_some() && self.exporting.is_none()
    }

    fn can_delete_shape(&self) -> bool {
        self.tool == Tool::Select
            && self.editable_frame().is_some()
            && self.ink.has_selected_shape()
    }

    fn delete_shape(&mut self, hwnd: HWND) {
        if self.can_delete_shape()
            && let Some(area) = self.page_area(hwnd)
            && self.ink.delete_selected_shape()
        {
            self.save_ink(hwnd);
            self.repaint_ink(hwnd, area);
        }
    }

    fn redo_ink(&mut self, hwnd: HWND) {
        if !self.can_redo() {
            return;
        }
        self.ink.redo();
        self.save_ink(hwnd);
        repaint(hwnd);
    }

    fn save_status(&self) -> SaveStatus {
        self.saver.as_ref().map(Saver::status).unwrap_or_default()
    }

    fn retry_save(&self, hwnd: HWND) -> bool {
        if let Some(saver) = &self.saver
            && let Err(error) = saver.retry()
        {
            self.show_error(
                hwnd,
                &trf!(
                    "Impossibile riprovare il salvataggio. {error}",
                    "Couldn’t retry saving. {error}"
                ),
            );
            return false;
        }
        repaint(hwnd);
        true
    }

    fn show_recovery(&self, hwnd: HWND, paths: &[PathBuf]) {
        let files = paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        self.show_error(hwnd, &trf!("Un’altra istanza ha salvato alcune pagine. Le tue modifiche si trovano nei file di recupero.\n\n{files}", "Another instance saved some pages. Your changes are in the recovery files.\n\n{files}"));
    }

    fn editable_frame(&self) -> Option<&Frame> {
        if self.original_export {
            return None;
        }
        let (key, frame) = self.visible.as_ref()?;
        (key.session == self.session && key.index == self.selected).then_some(frame.as_ref())
    }

    fn page_area(&self, hwnd: HWND) -> Option<RECT> {
        let frame = self.editable_frame()?;
        let viewport = Layout::new(hwnd, self.fullscreen.is_some()).viewport;
        Some(self.view.bounds(fit(frame, viewport), viewport))
    }

    fn repaint_ink(&mut self, hwnd: HWND, area: RECT) {
        let viewport = Layout::new(hwnd, self.fullscreen.is_some()).viewport;
        if let Some(damage) = self.ink.take_damage(area, viewport) {
            repaint_rect(hwnd, damage);
        }
    }

    fn animate(&self, hwnd: HWND) {
        if let Some(clock) = &self.animation {
            clock.start();
        } else {
            unsafe { SetTimer(Some(hwnd), RADIAL_TIMER, 16, None) };
        }
    }

    fn animate_frame(&mut self, hwnd: HWND) -> bool {
        let layout = Layout::new(hwnd, self.fullscreen.is_some());
        let (bounds, scale) = self.radial_area(&layout);
        let old = self.radial.footprint(bounds, scale);
        let previous_cards = self.animated_card_bounds(&layout);
        let overlay = self.overlay.tick();
        if !self.overlay.popup_open && self.overlay.popup_opacity == 0.0 { self.chrome_visual = None; }
        let obstacles = self.radial_obstacles(&layout);
        let radial = self.radial.tick(bounds, layout.scale, &obstacles);
        let zooming = self.zoom_animation.is_some();
        if let Some(zoom) = self.zoom_animation {
            let progress = zoom.started.elapsed().as_secs_f32() / ZOOM_TIME.as_secs_f32();
            if progress >= 1.0 {
                self.view = zoom.target;
                self.zoom_animation = None;
                self.defer_view_refresh(hwnd);
            } else {
                self.view = zoom
                    .from
                    .interpolate(zoom.target, 1.0 - (1.0 - progress).powi(3));
            }
            self.view_moving = true;
            repaint_rect(hwnd, layout.viewport);
        }
        let dragging_page = self.page_drag.as_ref().is_some_and(|drag| {
            drag.released.is_some()
                || self.visible.as_ref().is_some_and(|(key, _)| key.index == self.selected)
        });
        if let Some(drag) = &self.page_drag
            && let Some(start) = drag.released
        {
            if start.elapsed() >= PageDrag::RETURN_TIME {
                self.page_drag = None;
            }
            repaint_rect(hwnd, layout.viewport);
        }
        let turn_frame = self
            .page_animation
            .as_ref()
            .and_then(|animation| animation.started.map(|start| (animation.viewport, start, animation.duration())));
        if let Some((damage, start, duration)) = turn_frame {
            if start.elapsed() >= duration {
                self.page_animation = None;
            }
            repaint_rect(hwnd, damage);
        }
        if radial {
            let (bounds, scale) = self.radial_area(&layout);
            let next = self.radial.footprint(bounds, scale);
            repaint_rect(hwnd, union(old, next));
        }
        if overlay {
            let next_cards = self.animated_card_bounds(&layout);
            for &(rect, opacity) in &previous_cards {
                if !next_cards.contains(&(rect, opacity)) {
                    repaint_rect(hwnd, rect);
                }
            }
            for &(rect, opacity) in &next_cards {
                if !previous_cards.contains(&(rect, opacity)) {
                    repaint_rect(hwnd, rect);
                }
            }
        }
        if !radial && !overlay && !zooming && !self.view_moving && !dragging_page && turn_frame.is_none() {
            if let Some(clock) = &self.animation {
                clock.stop();
            }
            self.radial.reset_frame_time();
            self.overlay.reset_frame_time();
            let _ = unsafe { KillTimer(Some(hwnd), RADIAL_TIMER) };
            true
        } else {
            false
        }
    }

    fn transform_view(&mut self, hwnd: HWND, from: (f32, f32), to: (f32, f32), factor: f32) {
        self.interrupt_zoom(hwnd);
        let viewport = Layout::new(hwnd, self.fullscreen.is_some()).viewport;
        if let Some(frame) = self.editable_frame() {
            let previous = self.view;
            self.view
                .transform(fit(frame, viewport), viewport, from, to, factor);
            if self.view != previous {
                self.page_animation = None;
                self.view_moving = true;
                self.view_panning = self.view.zoom() == previous.zoom();
                self.defer_view_refresh(hwnd);
                repaint_rect(hwnd, viewport);
                self.animate(hwnd);
            }
        }
    }

    fn defer_view_refresh(&self, hwnd: HWND) {
        unsafe { SetTimer(Some(hwnd), RESIZE_TIMER, VIEW_SETTLE_MS, None) };
    }

    fn interrupt_zoom(&mut self, hwnd: HWND) {
        if self.zoom_animation.take().is_some() && self.view_moving {
            self.defer_view_refresh(hwnd);
        }
    }

    fn reset_view(&mut self, hwnd: HWND) {
        self.zoom_animation = None;
        self.view_moving = false;
        self.page_animation = None;
        self.view = View::default();
        self.refresh(hwnd);
    }

    fn zoom_step(&mut self, hwnd: HWND, step: i32) {
        let Some(frame) = self.editable_frame() else {
            return;
        };
        if self.ink.is_drawing() {
            return;
        }
        let level = self
            .zoom_animation
            .map_or(self.view.zoom(), |zoom| zoom.target.zoom());
        let next = (level * 1.25_f32.powi(step)).clamp(1.0, 4.0);
        if next == level {
            return;
        }
        let viewport = Layout::new(hwnd, self.fullscreen.is_some()).viewport;
        let center = viewport_center(viewport);
        let mut target = self.view;
        target.transform(
            fit(frame, viewport),
            viewport,
            center,
            center,
            next / self.view.zoom(),
        );
        self.page_animation = None;
        self.view_panning = false;
        self.zoom_animation = Some(ZoomAnimation {
            from: self.view,
            target,
            started: Instant::now(),
        });
        self.animate(hwnd);
    }

    fn mouse_down(&mut self, hwnd: HWND, point: POINT) {
        if self.chrome_contact(hwnd, WM_POINTERDOWN, MOUSE, point)
            || self.overlay_input(hwnd, WM_POINTERDOWN, MOUSE, point)
            || self.browse_contact(hwnd, WM_POINTERDOWN, MOUSE, point)
            || self.radial_input(hwnd, WM_POINTERDOWN, MOUSE, point)
        {
            unsafe {
                SetCapture(hwnd);
            }
            return;
        }
        if self.page_animation.take().is_some() {
            repaint(hwnd);
        }
        self.interrupt_zoom(hwnd);
        let layout = Layout::new(hwnd, self.fullscreen.is_some());
        if self.settings.tap_turn
            && self.view.zoom() == 1.0
            && (point.x < layout.client.right / 5 || point.x > layout.client.right * 4 / 5)
        {
            return;
        }
        let settings = self.settings;
        if inside(layout.viewport, point)
            && let Some(area) = self.page_area(hwnd)
            && self.ink.begin(
                MOUSE,
                layout.ink_area(area),
                point,
                0.5,
                self.tool,
                &settings,
            )
        {
            unsafe {
                SetCapture(hwnd);
            }
            self.repaint_ink(hwnd, area);
        }
    }

    fn pointer_message(&mut self, hwnd: HWND, msg: u32, id: u32) -> bool {
        let mut pointer_type = PT_POINTER;
        if unsafe { GetPointerType(id, &mut pointer_type) }.is_err() {
            return false;
        }
        if pointer_type == PT_TOUCHPAD {
            return self.trackpad_message(hwnd, msg, id);
        }
        if pointer_type == PT_PEN
            && matches!(msg, WM_POINTERENTER | WM_POINTERUPDATE)
            && !self.ink.is_drawing()
        {
            self.reveal_utilities(hwnd);
        }
        if msg == WM_POINTERENTER {
            return false;
        }
        if (pointer_type == PT_PEN || pointer_type == PT_TOUCH)
            && !self.ink.is_drawing()
            && (msg == WM_POINTERDOWN
                || self.radial.grabbed(id)
                || self.chrome_press.is_some_and(|(contact, _)| contact == id)
                || self.overlay.grab.is_some_and(|grab| grab.id == id))
        {
            let mut info = POINTER_INFO::default();
            if unsafe { GetPointerInfo(id, &mut info) }.is_ok() {
                let mut point = info.ptPixelLocation;
                let _ = unsafe { ScreenToClient(hwnd, &mut point) };
                if (self.radial.grabbed(id) && self.radial_input(hwnd, msg, id, point))
                    || self.chrome_contact(hwnd, msg, id, point)
                    || self.overlay_input(hwnd, msg, id, point)
                    || (!self.page_grid_open() && self.radial_input(hwnd, msg, id, point))
                {
                    return true;
                }
            }
        }
        if matches!(pointer_type, PT_PEN | PT_TOUCH)
            && let Some(point) = pointer_position(hwnd, id)
            && self.browse_contact(hwnd, msg, id, point)
        {
            return true;
        }
        match pointer_type {
            PT_PEN => {
                if msg == WM_POINTERDOWN {
                    self.interrupt_zoom(hwnd);
                }
                let Some(frame) = self.editable_frame() else {
                    return false;
                };
                let layout = Layout::new(hwnd, self.fullscreen.is_some());
                let area = self
                    .view
                    .bounds(fit(frame, layout.viewport), layout.viewport);
                if msg == WM_POINTERDOWN {
                    let mut info = POINTER_INFO::default();
                    if unsafe { GetPointerInfo(id, &mut info) }.is_ok() {
                        let mut point = info.ptPixelLocation;
                        let _ = unsafe { ScreenToClient(hwnd, &mut point) };
                        if !inside(layout.viewport, point) {
                            return false;
                        }
                    }
                    if self.page_animation.take().is_some() {
                        repaint(hwnd);
                    }
                }
                if self.ink.pen(
                    hwnd,
                    layout.ink_area(area),
                    msg,
                    id,
                    self.tool,
                    &self.settings,
                ) {
                    self.touch = Touch::default();
                    if msg == WM_POINTERUP {
                        self.complete_ink_contact(hwnd);
                    } else if !self.ink.is_drawing() {
                        self.schedule_ink_save(hwnd);
                    }
                    self.repaint_ink(hwnd, area);
                    return true;
                }
                false
            }
            PT_TOUCH => {
                if msg == WM_POINTERDOWN {
                    // Reveal after hit testing, so a tap on an empty score area cannot activate
                    // a control that was hidden when the user touched the screen.
                    self.reveal_utilities(hwnd);
                }
                self.touch_message(hwnd, msg, id)
            }
            _ => false,
        }
    }

    fn trackpad_message(&mut self, hwnd: HWND, msg: u32, id: u32) -> bool {
        if !matches!(msg, WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP) {
            return false;
        }
        let available = self.total > 0
            && !self.ink.is_drawing()
            && !self.overlay.drawer
            && self.overlay.grab.is_none()
            && self.chrome_menu.is_none();
        let zoomed = self.view.zoom() > 1.0;
        let layout = Layout::new(hwnd, self.fullscreen.is_some());
        let Some(movement) = self.trackpad.as_mut().and_then(|trackpad| {
            trackpad.input(id, msg == WM_POINTERDOWN, available, zoomed, layout.dpi)
        }) else {
            return false;
        };
        if available {
            self.reveal_utilities(hwnd);
            if movement.pages != 0 {
                self.turn_by_gesture(hwnd, movement.pages);
            }
            if movement.pan != 0.0 || movement.zoom != 1.0 {
                let mut anchor = movement.anchor;
                let _ = unsafe { ScreenToClient(hwnd, &mut anchor) };
                let from = (anchor.x as f32, anchor.y as f32);
                self.transform_view(hwnd, from, (from.0, from.1 + movement.pan), movement.zoom);
            }
        }
        true
    }

    fn radial_input(&mut self, hwnd: HWND, msg: u32, id: u32, point: POINT) -> bool {
        if self.visible.is_none() && self.fullscreen.is_none() {
            return false;
        }
        let layout = Layout::new(hwnd, self.fullscreen.is_some());
        let (bounds, scale) = self.radial_area(&layout);
        let old = self.radial.footprint(bounds, scale);
        match msg {
            WM_POINTERDOWN if !self.ink.is_drawing() => {
                return self.radial.begin(bounds, scale, id, point);
            }
            WM_POINTERUPDATE if self.radial.grabbed(id) => {
                self.radial.move_to(scale, id, point);
                self.animate(hwnd);
                return true;
            }
            WM_POINTERUP if self.radial.grabbed(id) => {
                let command = self.radial.finish(bounds, scale, id, point);
                if let Some(command) = command {
                    self.command(hwnd, command);
                }
                self.animate(hwnd);
            }
            _ => return false,
        }
        repaint_rect(hwnd, union(old, self.radial.footprint(bounds, scale)));
        true
    }

    fn touch_message(&mut self, hwnd: HWND, msg: u32, id: u32) -> bool {
        if self.ink.is_drawing() && !self.ink.has_contact(id) {
            return true;
        }
        let Some(point) = pointer_position(hwnd, id) else {
            return false;
        };
        let layout = Layout::new(hwnd, self.fullscreen.is_some());
        if self.ink.has_contact(id) {
            if let Some(area) = self.page_area(hwnd) {
                self.ink.contact_update(id, area, point);
                self.repaint_ink(hwnd, area);
            }
            if msg == WM_POINTERUP {
                self.complete_ink_contact(hwnd);
            }
            return true;
        }
        if msg == WM_POINTERDOWN {
            if self.page_drag.take().is_some() || self.page_animation.take().is_some() {
                repaint(hwnd);
            }
            self.interrupt_zoom(hwnd);
            if self.tool == Tool::Select
                && inside(layout.viewport, point)
                && let Some(area) = self.page_area(hwnd)
                && self.ink.selected_hit(layout.ink_area(area), point)
                && self.ink.begin(
                    id,
                    layout.ink_area(area),
                    point,
                    0.5,
                    self.tool,
                    &self.settings,
                )
            {
                self.repaint_ink(hwnd, area);
                return true;
            }
            self.touch.down(id, point);
            return true;
        }
        let motion = if msg == WM_POINTERUPDATE {
            self.touch_move(hwnd, id, point, layout.scale)
        } else {
            self.touch.up(id, point, layout.scale)
        };
        match motion {
            Some(Motion::Transform(from, to, factor)) => {
                if self.page_drag.take().is_some() { repaint(hwnd); }
                self.transform_view(hwnd, from, to, factor);
            }
            Some(Motion::Slide(dx)) => self.slide_with_touch(hwnd, &layout, dx),
            Some(Motion::Turn(step)) => {
                let selected = self.selected;
                self.turn_by_gesture(hwnd, step);
                if self.selected == selected {
                    self.release_page_drag(hwnd);
                }
            }
            Some(Motion::Tap) => {
                if matches!(self.tool, Tool::Staff | Tool::Rectangle)
                    && let Some(area) = self.page_area(hwnd)
                    && self.ink.begin(
                        id,
                        layout.ink_area(area),
                        point,
                        0.5,
                        self.tool,
                        &self.settings,
                    )
                {
                    self.complete_ink_contact(hwnd);
                    self.repaint_ink(hwnd, area);
                    return true;
                }
                if inside(layout.viewport, point)
                    && let Some(area) = self.page_area(hwnd)
                    && self.ink.select_shape(area, point)
                {
                    self.enter_selection(hwnd);
                    repaint(hwnd);
                } else {
                    self.click(hwnd, point);
                    repaint(hwnd);
                }
            }
            None if msg == WM_POINTERUP => self.release_page_drag(hwnd),
            None => {}
        }
        true
    }

    fn slide_with_touch(&mut self, hwnd: HWND, layout: &Layout, dx: i32) {
        if self.editable_frame().is_none() { return; }
        let width = (layout.viewport.right - layout.viewport.left).max(1);
        let neighbor = if dx < 0 {
            (self.selected + 1 < self.total).then_some(self.selected + 1)
        } else {
            self.selected.checked_sub(1)
        };
        let dx = if neighbor.is_some() {
            dx.clamp(-width, width)
        } else {
            (dx / 4).clamp(-width / 8, width / 8)
        };
        let drag = self.page_drag.get_or_insert_with(PageDrag::default);
        if drag.neighbor != neighbor {
            drag.neighbor = neighbor;
            drag.ink = Ink::default();
            drag.preview = None;
            if let Some(index) = neighbor
                && let Some(store) = &self.store
                && let Some(saver) = &self.saver
                && let Some(score) = self.scores.iter().rev().find(|score| score.first_page <= index)
            {
                let _ = drag.ink.bind(Some(store.page_path(&score.source, score.pdf_page(index))), saver);
            }
        }
        drag.dx = dx;
        drag.released = None;
        repaint_rect(hwnd, layout.viewport);
        self.animate(hwnd);
    }

    fn release_page_drag(&mut self, hwnd: HWND) {
        if let Some(drag) = &mut self.page_drag && drag.released.is_none() {
            drag.released = Some(Instant::now());
            self.animate(hwnd);
            repaint(hwnd);
        }
    }

    fn draw_page_drag(&self, dc: HDC, layout: &Layout, clip: RECT, frame: &Frame, drag: &PageDrag) {
        let dx = drag.offset();
        let current = offset(fit(frame, layout.viewport), dx, 0);
        if let Some(page) = &self.page_surface {
            let source = page.composite.as_ref().unwrap_or(&page.raster);
            PageAnimation::copy_shifted(dc, source, page.clip, clip, dx);
            if page.ink_version.is_none() {
                self.ink.draw_committed(dc, current, clip, 0);
            }
        } else {
            with_clip(dc, clip, || draw_bitmap(dc, current, frame));
            self.ink.draw_committed(dc, current, clip, 0);
        }
        if let Some(preview) = &drag.preview && dx != 0 {
            let width = layout.viewport.right - layout.viewport.left;
            PageAnimation::copy_shifted(
                dc, &preview.surface, preview.bounds, clip, dx - dx.signum() * width,
            );
        }
    }

    fn touch_move(&mut self, hwnd: HWND, id: u32, point: POINT, scale: f32) -> Option<Motion> {
        if self.touch.has_pair() {
            let mut frame = [POINTER_INFO::default(); 16];
            let mut count = frame.len() as u32;
            if unsafe { GetPointerFrameInfo(id, &mut count, Some(frame.as_mut_ptr())) }.is_ok()
                && let Some(motion) = self.touch.moved_pair(
                    frame[..count as usize]
                        .iter()
                        .filter(|info| info.pointerFlags.contains(POINTER_FLAG_INCONTACT))
                        .map(|info| {
                            let mut at = info.ptPixelLocation;
                            let _ = unsafe { ScreenToClient(hwnd, &mut at) };
                            (info.pointerId, at)
                        }),
                )
            {
                return Some(motion);
            }
        }
        self.touch.moved(id, point, self.view.zoom() > 1.0, scale)
    }

    fn click(&mut self, hwnd: HWND, point: POINT) {
        let layout = Layout::new(hwnd, self.fullscreen.is_some());
        if self.settings.tap_turn && self.total > 0 && !self.page_grid_open()
            && self.view.zoom() == 1.0
            && inside(layout.viewport, point)
        {
            let width = layout.client.right;
            if point.x < width / 5 {
                self.turn(hwnd, -1);
            } else if point.x > width * 4 / 5 {
                self.turn(hwnd, 1);
            }
        }
    }

    fn performing(&self) -> bool {
        self.concert && self.fullscreen.is_some()
    }

    fn toggle_fullscreen(&mut self, hwnd: HWND) {
        if self.fullscreen.is_none() && self.total == 0 { return; }
        self.chrome_menu = None;
        self.chrome_hover = None;
        self.chrome_press = None;
        unsafe {
            if let Some(placement) = self.fullscreen.take() {
                if !self.concert || self.total == 0 {
                    SetThreadExecutionState(ES_CONTINUOUS);
                }
                SetWindowLongPtrW(
                    hwnd,
                    GWL_STYLE,
                    (WS_OVERLAPPEDWINDOW | WS_VISIBLE | WS_CLIPCHILDREN).0 as isize,
                );
                let _ = SetWindowPlacement(hwnd, &placement);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
                );
            } else {
                let mut placement = WINDOWPLACEMENT {
                    length: size_of::<WINDOWPLACEMENT>() as u32,
                    ..Default::default()
                };
                let mut monitor = MONITORINFO {
                    cbSize: size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                let display = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                if GetWindowPlacement(hwnd, &mut placement).is_err()
                    || !GetMonitorInfoW(display, &mut monitor).as_bool()
                {
                    return;
                }
                self.fullscreen = Some(placement);
                SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED);
                SetWindowLongPtrW(
                    hwnd,
                    GWL_STYLE,
                    (WS_POPUP | WS_VISIBLE | WS_CLIPCHILDREN).0 as isize,
                );
                let bounds = monitor.rcMonitor;
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    bounds.left,
                    bounds.top,
                    bounds.right - bounds.left,
                    bounds.bottom - bounds.top,
                    SWP_FRAMECHANGED | SWP_NOZORDER,
                );
            }
        }
        self.refresh(hwnd);
        self.reveal_utilities(hwnd);
        repaint(hwnd);
    }

    fn show_error(&self, hwnd: HWND, text: &str) {
        let text = HSTRING::from(text);
        unsafe { MessageBoxW(Some(hwnd), &text, w!("ScoreDraw"), MB_OK | MB_ICONWARNING) };
    }

    fn paint(&mut self, hwnd: HWND) {
        let layout = Layout::new(hwnd, self.fullscreen.is_some());
        if let Some(menu) = self.chrome_menu {
            self.chrome_visual = Some(menu);
        }
        let popup_open = self.chrome_menu.is_some();
        if popup_open != self.overlay.popup_open {
            self.overlay.popup_open = popup_open;
            self.animate(hwnd);
        }
        self.sync_search(hwnd, &layout);
        self.update_thumbnails(hwnd, &layout);
        let mut paint = PAINTSTRUCT::default();
        unsafe {
            let dc = BeginPaint(hwnd, &mut paint);
            if !dc.is_invalid() {
                if !self.page_grid_open() && intersect(layout.viewport, paint.rcPaint).is_some() {
                    self.prepare_page_surface(dc, &layout);
                }
                if let Some(drag) = &mut self.page_drag
                    && let Some(index) = drag.neighbor
                    && let Some((_, frame)) = self.cache.iter().find(|(key, _)| key.index == index)
                {
                    drag.prepare(dc, layout.viewport, frame);
                }
                if let Some(animation) = &mut self.page_animation
                    && animation.started.is_none()
                {
                    if animation.prepare(self.page_surface.as_ref()) {
                        self.animate(hwnd);
                    } else {
                        self.page_animation = None;
                    }
                }
                if self.paint_direct2d(hwnd, dc, &layout, paint.rcPaint) {
                    let _ = EndPaint(hwnd, &paint);
                    return;
                }
                let (width, height) = (layout.client.right, layout.client.bottom);
                if self
                    .paint_buffer
                    .as_ref()
                    .is_none_or(|surface| !surface.fits(width, height))
                {
                    self.paint_buffer = Surface::new(dc, width, height);
                }
                if let Some(surface) = &self.paint_buffer {
                    let damage = paint.rcPaint;
                    with_clip(surface.dc, damage, || {
                        self.draw(hwnd, surface.dc, &layout, damage)
                    });
                    let area = paint.rcPaint;
                    let _ = BitBlt(
                        dc,
                        area.left,
                        area.top,
                        area.right - area.left,
                        area.bottom - area.top,
                        Some(surface.dc),
                        area.left,
                        area.top,
                        SRCCOPY,
                    );
                } else {
                    self.draw(hwnd, dc, &layout, paint.rcPaint);
                }
            }
            let _ = EndPaint(hwnd, &paint);
        }
    }

    fn prepare_page_surface(&mut self, target: HDC, layout: &Layout) {
        let Some((key, frame)) = &self.visible else {
            self.page_surface = None;
            return;
        };
        let base = fit(frame, layout.viewport);
        let awaiting_detail = self.desired.is_some_and(|desired| {
            key.index == desired.index && *key != desired && self.queued.contains(&desired)
        });
        let view_bounds = self.view.bounds(base, layout.viewport);
        let (bounds, clip) = if self.view_moving || (awaiting_detail && self.view.zoom() > 1.0) {
            // Keep all source pixels and newly exposed edges available during the gesture.
            let full_page = RECT {
                right: frame.width,
                bottom: frame.height,
                ..Default::default()
            };
            (full_page, full_page)
        } else {
            let bounds = view_bounds;
            let Some(clip) = intersect(bounds, layout.viewport) else {
                self.page_surface = None;
                return;
            };
            (bounds, clip)
        };
        let changed = self.page_surface.as_ref().is_none_or(|page| {
            !Arc::ptr_eq(&page.frame, frame) || page.bounds != bounds || page.clip != clip
        });
        if changed {
            let (w, h) = (clip.right - clip.left, clip.bottom - clip.top);
            let cached = self
                .page_surface
                .take()
                .filter(|page| page.raster.fits(w, h));
            let (raster, composite) = match cached {
                Some(page) => (Some(page.raster), page.composite),
                None => (Surface::new(target, w, h), None),
            };
            self.page_surface = raster.map(|mut raster| {
                draw_bitmap(raster.dc, offset(bounds, -clip.left, -clip.top), frame);
                raster.revision += 1;
                PageSurface {
                    frame: frame.clone(),
                    bounds,
                    clip,
                    raster,
                    composite,
                    ink_version: None,
                    pan_raster: None,
                }
            });
        }
        if let Some(page) = &mut self.page_surface {
            page.update_marks(target, &self.ink);
            if self.direct2d.is_none() && self.view_moving && self.view_panning {
                page.prepare_pan(target, view_bounds);
            }
        }
    }

    fn draw(&self, hwnd: HWND, dc: HDC, layout: &Layout, damage: RECT) {
        fill(dc, damage, BACKGROUND);
        if self.page_grid_open() {
            self.draw_page_grid(dc, layout, damage);
        } else if let Some((_, frame)) = &self.visible {
            let bounds = self
                .view
                .bounds(fit(frame, layout.viewport), layout.viewport);
            if let Some(clip) = intersect(layout.viewport, damage) {
                if let Some(drag) = &self.page_drag {
                    self.draw_page_drag(dc, layout, clip, frame, drag);
                } else if !self.page_animation.as_ref().is_some_and(|animation| {
                    animation.draw(dc, clip, self.page_surface.as_ref())
                })
                {
                    if let Some(page) = &self.page_surface {
                        page.draw(dc, bounds, clip);
                    } else {
                        with_clip(dc, clip, || draw_bitmap(dc, bounds, frame));
                    }
                    if self.page_surface.as_ref().is_none_or(|page| page.ink_version.is_none()) {
                        self.ink.draw_committed(dc, bounds, clip, 0);
                    }
                    self.ink
                        .draw_live(dc, layout.ink_area(bounds), clip, self.tool);
                }
            }
        } else {
            self.draw_home(dc, layout, damage);
        }
        self.draw_chrome(hwnd, dc, layout, damage);
    }

    fn draw_chrome(&self, hwnd: HWND, dc: HDC, layout: &Layout, damage: RECT) {
        if let Some(strip) = layout.strip
            && intersect(strip.rect, damage).is_some()
        {
            let caption = if !self.status.is_empty() {
                self.status.as_str()
            } else {
                self.current_score()
                    .map_or("ScoreDraw", |score| score.title.as_str())
            };
            strip.draw(
                dc,
                caption,
                self.chrome_hover,
                self.chrome_menu,
                unsafe { IsZoomed(hwnd) }.as_bool(),
            );
        }
        let save = self.save_status();
        if !self.page_grid_open() && !self.radial.dragging() {
            self.draw_radial(dc, layout, damage, &save);
        }
        let notice = match &save {
            SaveStatus::Failed(_) => tr!("Salvataggio sospeso", "Saving paused"),
            SaveStatus::Recovered(paths) if paths.len() > 1 => {
                tr!("Copie di recupero", "Recovery copies")
            }
            SaveStatus::Recovered(_) => tr!("Copia di recupero", "Recovery copy"),
            SaveStatus::Saving if self.fullscreen.is_none() => tr!("Salvataggio…", "Saving…"),
            _ if self.recovered_page => tr!("Annotazioni recuperate", "Recovered annotations"),
            _ => "",
        };
        let bounds = layout.save_notice();
        if !notice.is_empty() && intersect(bounds, damage).is_some() {
            button(dc, bounds, notice, false, layout.scale);
        }
        self.draw_overlays(dc, layout, damage);
        if !self.page_grid_open() && self.radial.dragging() {
            self.draw_radial(dc, layout, damage, &save);
        }
    }

    fn radial_area(&self, layout: &Layout) -> (RECT, f32) {
        let area = layout.viewport;
        (area, self.radial.display_scale(area, layout.scale))
    }

    fn animated_card_bounds(&self, layout: &Layout) -> Vec<(RECT, f32)> {
        let closing_popup = self.chrome_menu.is_none()
            .then(|| self.chrome_visual.and_then(|menu| self.chrome_card_for(layout, menu)))
            .flatten();
        self.cards(layout).into_iter().chain(closing_popup)
            .map(|card| (card.rect, card.opacity))
            .collect()
    }

    fn radial_obstacles(&self, layout: &Layout) -> Vec<RECT> {
        if self.visible.is_none() || self.page_grid_open() {
            return Vec::new();
        }
        let closing_popup = if self.chrome_menu.is_none() {
            self.chrome_visual.and_then(|menu| self.chrome_card_for(layout, menu))
        } else {
            None
        };
        self.cards(layout).into_iter().chain(closing_popup)
            .filter_map(|card| intersect(card.rect, layout.viewport))
            .collect()
    }

    fn draw_radial(&self, dc: HDC, layout: &Layout, damage: RECT, save: &SaveStatus) {
        let (radial_bounds, scale) = self.radial_area(&layout);
        if (self.visible.is_some() || self.fullscreen.is_some())
            && intersect(self.radial.footprint(radial_bounds, scale), damage).is_some()
        {
            let Some(canvas) = Canvas::new(dc) else { return };
            canvas.clip(radial_bounds);
            let disc = |rect, fill, border, width| {
                canvas.ellipse(rect, canvas::argb(fill, 255), canvas::argb(border, 255), width);
            };
            let mut labels = Vec::with_capacity(12);
            let hub = self.radial.hub(radial_bounds, scale);
            for (command, rect) in self.radial.items(radial_bounds, scale) {
                let selected = command == Command::Tool(self.tool);
                let tone = radial_tint(command);
                disc(
                    rect,
                    blend_color(PANEL, tone, if selected { 28 } else { 9 }),
                    if selected {
                        tone
                    } else {
                        blend_color(MENU_BORDER, tone, 32)
                    },
                    scale * 1.7,
                );
                let radius = (rect.bottom - rect.top) as f32 / 2.0;
                let center = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
                let icon_box = RECT {
                    top: center.1 - (radius * 0.66).round() as i32,
                    bottom: center.1 + 1,
                    ..rect
                };
                let label_bottom = (radius * 0.49).floor() as i32;
                // Fit the entire text box inside the chord at its lowest point.
                let inset = (radius.powi(2) - (label_bottom as f32).powi(2)).sqrt() as i32 - 2;
                let label_box = RECT {
                    left: center.0 - inset,
                    right: center.0 + inset,
                    top: icon_box.bottom,
                    bottom: center.1 + label_bottom,
                };
                icons::draw_on(&canvas, command.icon(), icon_box, radius * 0.72, tone);
                labels.push((label_box, command.label(), (radius * 0.36).round() as i32));
            }
            for (_, rect) in self.radial.options(radial_bounds, scale) {
                disc(rect, SURFACE, ACCENT, 1.1 * scale);
                labels.push((rect, "⋯", (15.0 * scale).round() as i32));
            }
            let glyph = if self.radial.open {
                Glyph::X
            } else if self.sound.running(0) {
                Glyph::Music
            } else {
                Command::Tool(self.tool).icon()
            };
            disc(
                hub,
                ACCENT,
                if matches!(save, SaveStatus::Failed(_) | SaveStatus::Recovered(_)) {
                    ALERT
                } else {
                    ACCENT
                },
                scale * 2.0,
            );
            icons::draw_on(&canvas, glyph, hub, 25.0 * scale, BACKGROUND);
            if !self.radial.open && !self.sound.running(0) {
                let settings = self.settings;
                let accent = match self.tool {
                    Tool::Pen => Some(settings.pen.color),
                    Tool::Highlighter => Some(settings.highlighter.color),
                    Tool::Staff => Some(settings.staff.color),
                    Tool::Rectangle => Some(settings.rectangle.color),
                    _ => None,
                };
                if let Some(accent) = accent {
                    let dot = (11.0 * scale).round() as i32;
                    let rect = RECT {
                        left: hub.right - dot * 2,
                        top: hub.bottom - dot * 2,
                        right: hub.right - dot,
                        bottom: hub.bottom - dot,
                    };
                    disc(rect, COLORREF(accent), PANEL, scale);
                }
            }
            drop(canvas);
            with_clip(dc, radial_bounds, || {
                for (bounds, label, size) in labels {
                    draw_text(dc, bounds, label, TEXT, size, true, DT_CENTER);
                }
            });
        }
    }
}

#[derive(Clone, Copy)]
struct ZoomAnimation {
    from: View,
    target: View,
    started: Instant,
}

struct PageSurface {
    frame: Arc<Frame>,
    bounds: RECT,
    clip: RECT,
    raster: Surface,
    composite: Option<Surface>,
    ink_version: Option<LayerVersion>,
    pan_raster: Option<Surface>,
}

#[derive(Default)]
struct PageDrag {
    dx: i32,
    neighbor: Option<u32>,
    ink: Ink,
    preview: Option<DragPreview>,
    released: Option<Instant>,
}

struct DragPreview {
    frame: Arc<Frame>,
    bounds: RECT,
    surface: Surface,
}

impl PageDrag {
    const RETURN_TIME: Duration = Duration::from_millis(135);

    fn prepare(&mut self, target: HDC, viewport: RECT, frame: &Arc<Frame>) {
        let bounds = fit(frame, viewport);
        if self.preview.as_ref().is_some_and(|cached| {
            Arc::ptr_eq(&cached.frame, frame) && cached.bounds == bounds
        }) {
            return;
        }
        let rect = offset(bounds, -bounds.left, -bounds.top);
        let Some(surface) = Surface::new(target, rect.right, rect.bottom) else { return };
        draw_bitmap(surface.dc, rect, frame);
        self.ink.draw_committed(surface.dc, rect, rect, 0);
        self.preview = Some(DragPreview { frame: frame.clone(), bounds, surface });
    }

    fn offset(&self) -> i32 {
        let remaining = self.released.map_or(1.0, |start| {
            1.0 - (start.elapsed().as_secs_f32() / Self::RETURN_TIME.as_secs_f32()).min(1.0)
        });
        (self.dx as f32 * remaining.powi(2)).round() as i32
    }
}

struct PageAnimation {
    outgoing: Surface,
    clip: RECT,
    forward: bool,
    distance: i32,
    viewport: RECT,
    started: Option<Instant>,
}

impl PageAnimation {
    fn duration(&self) -> Duration {
        let width = (self.viewport.right - self.viewport.left).max(1);
        Duration::from_millis((235 - 145 * i64::from(self.distance.min(width)) / i64::from(width)) as u64)
    }

    fn prepare(&mut self, incoming: Option<&PageSurface>) -> bool {
        if incoming.is_none() { return false; }
        self.started = Some(Instant::now());
        true
    }

    fn draw(&self, dc: HDC, damage: RECT, incoming: Option<&PageSurface>) -> bool {
        let Some((travel, width, direction, edge, progress)) = self.position() else { return false };
        let Some(damage) = intersect(self.viewport, damage) else { return false };
        let Some(incoming) = incoming else { return false };
        Self::copy_shifted(dc, &self.outgoing, self.clip, damage, travel);
        let source = incoming.composite.as_ref().unwrap_or(&incoming.raster);
        Self::copy_shifted(dc, source, incoming.clip, damage, travel - width * direction);
        with_clip(dc, damage, || self.draw_edge(dc, edge, direction, progress));
        true
    }

    fn position(&self) -> Option<(i32, i32, i32, i32, f32)> {
        let progress = self.started?.elapsed().as_secs_f32() / self.duration().as_secs_f32();
        if progress >= 1.0 {
            return None;
        }
        let width = self.viewport.right - self.viewport.left;
        let eased = 1.0 - (1.0 - progress).powi(2);
        let start = self.distance.clamp(0, width);
        let distance = start + ((width - start) as f32 * eased).round() as i32;
        let direction = if self.forward { -1 } else { 1 };
        let edge = if self.forward { self.viewport.right - distance } else { self.viewport.left + distance };
        Some((distance * direction, width, direction, edge, progress))
    }

    fn copy_shifted(dc: HDC, source: &Surface, bounds: RECT, damage: RECT, dx: i32) {
        let shifted = offset(bounds, dx, 0);
        if let Some(rect) = intersect(shifted, damage) {
            source.copy_to(dc, rect, rect.left - shifted.left, rect.top - shifted.top);
        }
    }

    fn draw_edge(&self, dc: HDC, edge: i32, direction: i32, progress: f32) {
        let Some(canvas) = Canvas::new(dc) else { return };
        let top = self.clip.top as f32;
        let height = (self.clip.bottom - self.clip.top) as f32;
        let strength = (progress * 10.0).min((1.0 - progress) * 10.0).min(1.0);
        let side = -direction;
        for stripe in 0..10 {
            let x = edge + if side > 0 { stripe } else { -stripe - 1 };
            let alpha = ((10 - stripe) as f32 * 5.0 * strength) as u8;
            canvas.rectangle(x as f32, top, 1.0, height, canvas::argb(COLORREF(0), alpha));
        }
    }
}

impl PageSurface {
    fn update_marks(&mut self, target: HDC, ink: &Ink) {
        let version = ink.layer_version();
        if self.ink_version == Some(version) {
            return;
        }
        self.pan_raster = None;
        if version.count == 0 {
            self.composite = None;
            self.ink_version = Some(version);
            return;
        }
        let (w, h) = (self.raster.width, self.raster.height);
        if self.composite.is_none() {
            self.composite = Surface::new(target, w, h);
        }
        if let Some(composite) = &mut self.composite {
            let local = RECT {
                left: 0,
                top: 0,
                right: w,
                bottom: h,
            };
            let first = version
                .append_from(self.ink_version)
                .filter(|first| *first > 0);
            if first.is_none() {
                self.raster.copy_to(composite.dc, local, 0, 0);
            }
            ink.draw_committed(
                composite.dc,
                offset(self.bounds, -self.clip.left, -self.clip.top),
                local,
                first.unwrap_or(0),
            );
            composite.revision += 1;
            self.ink_version = Some(version);
        }
    }

    fn draw(&self, dc: HDC, bounds: RECT, clip: RECT) {
        let (width, height) = (bounds.right - bounds.left, bounds.bottom - bounds.top);
        if let Some(pan) = self.pan_raster.as_ref().filter(|surface| surface.fits(width, height)) {
            if let Some(to) = intersect(bounds, clip) {
                pan.copy_to(dc, to, to.left - bounds.left, to.top - bounds.top);
            }
            return;
        }
        let source = self.composite.as_ref().unwrap_or(&self.raster);
        if self.bounds != bounds
            && self.bounds == self.clip
            && source.fits(bounds.right - bounds.left, bounds.bottom - bounds.top)
        {
            if let Some(to) = intersect(bounds, clip) {
                source.copy_to(dc, to, to.left - bounds.left, to.top - bounds.top);
            }
        } else if self.bounds != bounds {
            with_clip(dc, clip, || source.stretch_to(dc, bounds));
        } else if let Some(to) = intersect(self.clip, clip) {
            source.copy_to(dc, to, to.left - self.clip.left, to.top - self.clip.top);
        }
    }

    fn prepare_pan(&mut self, target: HDC, bounds: RECT) {
        let (width, height) = (bounds.right - bounds.left, bounds.bottom - bounds.top);
        // Keep one scaled page for panning at a fixed zoom. Bound its memory
        // independently of display size and the maximum zoom.
        if i64::from(width) * i64::from(height) > 8_000_000 {
            self.pan_raster = None;
            return;
        }
        let source = self.composite.as_ref().unwrap_or(&self.raster);
        if source.fits(width, height)
            || self.pan_raster.as_ref().is_some_and(|surface| surface.fits(width, height))
        {
            return;
        }
        self.pan_raster = Surface::new(target, width, height);
        if let Some(pan) = &self.pan_raster {
            source.stretch_to(pan.dc, RECT { right: width, bottom: height, ..Default::default() });
        }
    }
}

pub(crate) struct Surface {
    pub(crate) dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    width: i32,
    height: i32,
    id: u64,
    revision: u64,
}

impl Surface {
    pub(crate) fn new(target: HDC, width: i32, height: i32) -> Option<Self> {
        if width <= 0 || height <= 0 {
            return None;
        }
        unsafe {
            let dc = CreateCompatibleDC(Some(target));
            if dc.is_invalid() {
                return None;
            }
            let bitmap = CreateCompatibleBitmap(target, width, height);
            if bitmap.is_invalid() {
                let _ = DeleteDC(dc);
                return None;
            }
            let previous = SelectObject(dc, bitmap.into());
            if previous.is_invalid() {
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(dc);
                return None;
            }
            Some(Self {
                dc,
                bitmap,
                previous,
                width,
                height,
                id: {
                    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
                    NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                },
                revision: 0,
            })
        }
    }

    fn fits(&self, width: i32, height: i32) -> bool {
        self.width == width && self.height == height
    }

    pub(crate) fn copy_to(&self, dc: HDC, to: RECT, x: i32, y: i32) {
        unsafe {
            let _ = BitBlt(
                dc,
                to.left,
                to.top,
                to.right - to.left,
                to.bottom - to.top,
                Some(self.dc),
                x,
                y,
                SRCCOPY,
            );
        }
    }

    fn stretch_to(&self, dc: HDC, to: RECT) {
        unsafe {
            // Interactive scaling uses the cached surface. Once the view settles,
            // prepare_page_surface rebuilds the visible region with HALFTONE.
            SetStretchBltMode(dc, COLORONCOLOR);
            let _ = StretchBlt(
                dc,
                to.left,
                to.top,
                to.right - to.left,
                to.bottom - to.top,
                Some(self.dc),
                0,
                0,
                self.width,
                self.height,
                SRCCOPY,
            );
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

struct Layout {
    client: RECT,
    viewport: RECT,
    strip: Option<chrome::Strip>,
    scale: f32,
    dpi: f32,
}

impl Layout {
    fn new(hwnd: HWND, full: bool) -> Self {
        let mut client = RECT::default();
        let dpi = unsafe {
            let _ = GetClientRect(hwnd, &mut client);
            GetDpiForWindow(hwnd).max(96) as f32 / 96.0
        };
        let scale = dpi.min((client.right as f32 / 413.0).max(0.35));
        let px = |dp: i32| (dp as f32 * scale).round() as i32;
        let strip = (!full).then(|| chrome::Strip::new(client, scale));
        let viewport = strip.map_or(client, |title| RECT {
            left: px(9),
            right: client.right - px(9),
            top: title.rect.bottom + px(7),
            bottom: client.bottom - px(9),
        });
        Self {
            client,
            viewport,
            strip,
            scale,
            dpi,
        }
    }

    fn ink_area(&self, page: RECT) -> InkArea {
        InkArea {
            page,
            viewport: self.viewport,
            dpi: self.dpi,
        }
    }

    fn save_notice(&self) -> RECT {
        let left = self.client.left + self.px(15);
        let bottom = self.client.bottom - self.px(14);
        RECT {
            left,
            right: left + self.px(206),
            top: bottom - self.px(34),
            bottom,
        }
    }

    fn px(&self, dp: i32) -> i32 {
        (dp as f32 * self.scale).round() as i32
    }


}

fn viewport_center(viewport: RECT) -> (f32, f32) {
    (
        (viewport.left + viewport.right) as f32 / 2.0,
        (viewport.top + viewport.bottom) as f32 / 2.0,
    )
}

fn fit(frame: &Frame, bounds: RECT) -> RECT {
    let width = bounds.right - bounds.left;
    let height = bounds.bottom - bounds.top;
    let scale = (width as f32 / frame.width as f32).min(height as f32 / frame.height as f32);
    let w = (frame.width as f32 * scale).round() as i32;
    let h = (frame.height as f32 * scale).round() as i32;
    let x = bounds.left + (width - w) / 2;
    let y = bounds.top + (height - h) / 2;
    RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    }
}

fn render_dimension(px: i32) -> u32 {
    let px = px.clamp(1, 3072) as u32;
    px.div_ceil(128).max(2) * 128
}

fn repaint(hwnd: HWND) {
    let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
}

fn repaint_rect(hwnd: HWND, rect: RECT) {
    if rect.right > rect.left && rect.bottom > rect.top {
        let _ = unsafe { InvalidateRect(Some(hwnd), Some(&rect), false) };
    }
}

fn intersect(a: RECT, b: RECT) -> Option<RECT> {
    let rect = RECT {
        left: a.left.max(b.left),
        top: a.top.max(b.top),
        right: a.right.min(b.right),
        bottom: a.bottom.min(b.bottom),
    };
    (rect.right > rect.left && rect.bottom > rect.top).then_some(rect)
}

fn union(a: RECT, b: RECT) -> RECT {
    RECT {
        left: a.left.min(b.left),
        top: a.top.min(b.top),
        right: a.right.max(b.right),
        bottom: a.bottom.max(b.bottom),
    }
}

fn offset(rect: RECT, dx: i32, dy: i32) -> RECT {
    RECT {
        left: rect.left + dx,
        top: rect.top + dy,
        right: rect.right + dx,
        bottom: rect.bottom + dy,
    }
}

fn with_clip(dc: HDC, clip: RECT, draw: impl FnOnce()) {
    unsafe {
        let saved = SaveDC(dc);
        if saved != 0 {
            IntersectClipRect(dc, clip.left, clip.top, clip.right, clip.bottom);
        }
        draw();
        if saved != 0 {
            let _ = RestoreDC(dc, saved);
        }
    }
}

fn request_file_picker(hwnd: HWND) {
    post(hwnd, PICK_PDF);
}

fn post(hwnd: HWND, message: u32) {
    let _ = unsafe { PostMessageW(Some(hwnd), message, WPARAM(0), LPARAM(0)) };
}

pub(crate) fn inside(rect: RECT, p: POINT) -> bool {
    p.x >= rect.left && p.x < rect.right && p.y >= rect.top && p.y < rect.bottom
}

pub(crate) fn point_from_mouse(data: LPARAM) -> POINT {
    POINT {
        x: data.0 as i16 as i32,
        y: (data.0 >> 16) as i16 as i32,
    }
}

fn pointer_position(hwnd: HWND, id: u32) -> Option<POINT> {
    let mut pointer = POINTER_INFO::default();
    unsafe {
        GetPointerInfo(id, &mut pointer).ok()?;
        let mut point = pointer.ptPixelLocation;
        let _ = ScreenToClient(hwnd, &mut point);
        Some(point)
    }
}

pub(crate) fn draw_bitmap(dc: HDC, rect: RECT, frame: &Frame) {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: frame.width,
            biHeight: -frame.height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    unsafe {
        SetStretchBltMode(dc, HALFTONE);
        StretchDIBits(
            dc,
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            0,
            0,
            frame.width,
            frame.height,
            Some(frame.pixels.as_ptr().cast()),
            &info,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
    }
}

pub(crate) fn fill(dc: HDC, rect: RECT, color: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(color);
        FillRect(dc, &rect, brush);
        let _ = DeleteObject(brush.into());
    }
}

pub(crate) fn text(dc: HDC, rect: RECT, label: &str, color: COLORREF, size: i32) {
    draw_text(dc, rect, label, color, size, false, DT_CENTER);
}

pub(crate) fn text_left(dc: HDC, rect: RECT, label: &str, color: COLORREF, size: i32) {
    draw_text(dc, rect, label, color, size, false, DT_LEFT);
}

#[derive(Default)]
struct TextFonts(HashMap<i32, HFONT>);

impl Drop for TextFonts {
    fn drop(&mut self) {
        for font in self.0.values() {
            let _ = unsafe { DeleteObject((*font).into()) };
        }
    }
}

thread_local! {
    static TEXT_FONTS: RefCell<TextFonts> = RefCell::default();
}

fn draw_text(
    dc: HDC,
    mut rect: RECT,
    label: &str,
    color: COLORREF,
    size: i32,
    fit: bool,
    alignment: DRAW_TEXT_FORMAT,
) {
    // An empty Vec has no backing buffer to pass to DrawTextW.
    if label.is_empty() || rect.right <= rect.left || size < 1 {
        return;
    }
    let mut utf16: Vec<u16> = label.encode_utf16().collect();
    unsafe {
        let font_at = |height: i32| TEXT_FONTS.with(|fonts| {
            *fonts.borrow_mut().0.entry(height).or_insert_with(|| CreateFontW(
                -height,
                0,
                0,
                0,
                FW_MEDIUM.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                DEFAULT_PITCH.0 as u32,
                w!("Segoe UI"),
            ))
        });
        let mut font_size = size;
        let prior = SelectObject(dc, font_at(font_size).into());
        if fit {
            let mut extent = SIZE::default();
            let width = (rect.right - rect.left - 3).max(1);
            while font_size > 1
                && GetTextExtentPoint32W(dc, &utf16, &mut extent).as_bool()
                && extent.cx > width
            {
                font_size = (font_size * width / extent.cx).clamp(1, font_size - 1);
                SelectObject(dc, font_at(font_size).into());
            }
        }
        SetTextColor(dc, color);
        SetBkMode(dc, TRANSPARENT);
        DrawTextW(
            dc,
            &mut utf16,
            &mut rect,
            alignment
                | DT_VCENTER
                | DT_SINGLELINE
                | DT_NOPREFIX
                | if fit {
                    DRAW_TEXT_FORMAT(0)
                } else {
                    DT_END_ELLIPSIS
                },
        );
        SelectObject(dc, prior);
    }
}

pub(crate) fn rounded(dc: HDC, rect: RECT, color: COLORREF, round: i32) {
    if rect.right <= rect.left || rect.bottom <= rect.top {
        return;
    }
    if let Some(canvas) = Canvas::new(dc)
        && canvas.rounded_rectangle(rect, round, canvas::argb(color, 255))
    {
        return;
    }
    unsafe {
        let brush = CreateSolidBrush(color);
        let prior_brush = SelectObject(dc, brush.into());
        let prior_pen = SelectObject(dc, GetStockObject(NULL_PEN));
        let _ = RoundRect(
            dc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            round,
            round,
        );
        SelectObject(dc, prior_pen);
        SelectObject(dc, prior_brush);
        let _ = DeleteObject(brush.into());
    }
}

fn draw_scrollbar(dc: HDC, bar: overlay::Scrollbar, scale: f32) {
    let width = (6.0 * scale).round().max(3.0) as i32;
    let left = (bar.track.left + bar.track.right - width) / 2;
    let track = RECT { left, right: left + width, ..bar.track };
    rounded(dc, track, MENU_BORDER, width);
    rounded(dc, RECT { top: bar.thumb.top, bottom: bar.thumb.bottom, ..track }, MUTED, width);
}

fn button(dc: HDC, rect: RECT, label: &str, primary: bool, scale: f32) {
    let background = if primary { ACCENT } else { SURFACE };
    let foreground = if primary { BACKGROUND } else { TEXT };
    let round = (scale * 25.0).round() as i32;
    rounded(dc, rect, background, round);
    let size = if label.chars().count() == 1 {
        25.0
    } else {
        15.0
    };
    text(dc, rect, label, foreground, (size * scale).round() as i32);
}

fn icon_button(dc: HDC, rect: RECT, icon: Glyph, enabled: bool, scale: f32) {
    rounded(
        dc,
        rect,
        if enabled { SURFACE } else { PANEL },
        (19.0 * scale).round() as i32,
    );
    icons::draw(
        dc,
        icon,
        rect,
        20.0 * scale,
        if enabled { TEXT } else { MENU_BORDER },
    );
}

fn radial_tint(command: Command) -> COLORREF {
    COLORREF(match command {
        Command::Tool(Tool::Pen) => 0xffae63,
        Command::Tool(Tool::Highlighter) => 0x4bceff,
        Command::Tool(Tool::Eraser) => 0xa673ff,
        Command::Tool(Tool::Select) => 0xa4e25a,
        Command::Tool(Tool::Staff) => 0xffa0bb,
        Command::Tool(Tool::Rectangle) | Command::Shapes => 0x70dcce,
        Command::Metronome => 0x569cff,
        Command::Tuner => 0xe4d94a,
        _ => ACCENT.0,
    })
}

fn blend_color(base: COLORREF, tint: COLORREF, percent: u32) -> COLORREF {
    let channel = |shift: u32| {
        let a = (base.0 >> shift) & 255;
        let b = (tint.0 >> shift) & 255;
        ((a * (100 - percent) + b * percent) / 100) << shift
    };
    COLORREF(channel(0) | channel(8) | channel(16))
}

fn circle(dc: HDC, rect: RECT, color: COLORREF, border: COLORREF, width: f32) {
    if let Some(canvas) = Canvas::new(dc) {
        canvas.ellipse(
            rect,
            canvas::argb(color, 255),
            canvas::argb(border, 255),
            width,
        );
    }
}

pub(crate) fn choose_files(hwnd: HWND, multiple: bool) -> Result<Vec<PathBuf>> {
    unsafe {
        let picker: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        let mut options = picker.GetOptions()? | FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM;
        if multiple {
            options |= FOS_ALLOWMULTISELECT;
        }
        picker.SetOptions(options)?;
        picker.SetFileTypes(&[COMDLG_FILTERSPEC {
            pszName: trw!("Spartiti PDF", "PDF scores"),
            pszSpec: w!("*.pdf"),
        }])?;
        if let Err(error) = picker.Show(Some(hwnd)) {
            if error.code().0 == 0x800704c7u32 as i32 {
                return Ok(Vec::new());
            }
            return Err(error);
        }
        let items = picker.GetResults()?;
        let mut paths = Vec::new();
        for index in 0..items.GetCount()? {
            let raw = items.GetItemAt(index)?.GetDisplayName(SIGDN_FILESYSPATH)?;
            let path = OsString::from_wide(raw.as_wide());
            CoTaskMemFree(Some(raw.0.cast()));
            paths.push(PathBuf::from(path));
        }
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_embeds_separate_icons_for_the_app_and_pdfs() {
        let module = unsafe { GetModuleHandleW(None) }.unwrap();
        let icon = |id| {
            unsafe {
                LoadImageW(
                    Some(HINSTANCE(module.0)),
                    PCWSTR(std::ptr::without_provenance(id)),
                    IMAGE_ICON,
                    48,
                    48,
                    LR_SHARED,
                )
            }
            .unwrap()
        };
        assert_ne!(icon(101), icon(102));
    }

    #[test]
    fn zoomed_ink_reuses_the_scaled_viewport_and_saved_marks() {
        let drawing = canvas::Startup::new().unwrap();
        let dc = unsafe { CreateCompatibleDC(None) };
        assert!(!dc.is_invalid());
        let viewport = RECT {
            left: 0,
            top: 0,
            right: 240,
            bottom: 300,
        };
        let layout = Layout {
            client: viewport,
            viewport,
            strip: None,
            scale: 1.0,
            dpi: 1.0,
        };
        let key = PageKey {
            session: 1,
            index: 0,
            width: 400,
            height: 500,
        };
        let mut reader = Reader {
            session: key.session,
            visible: Some((
                key,
                Arc::new(Frame {
                    width: 4,
                    height: 5,
                    pixels: vec![255; 80],
                }),
            )),
            ..Reader::default()
        };
        let frame = reader.editable_frame().unwrap();
        reader.view.transform(
            fit(frame, viewport),
            viewport,
            (120.0, 150.0),
            (120.0, 150.0),
            3.0,
        );
        let bounds = reader
            .view
            .bounds(fit(reader.editable_frame().unwrap(), viewport), viewport);

        let scratch = crate::data::Scratch::new();
        let saver = Saver::start(None).unwrap();
        reader
            .ink
            .bind(Some(scratch.0.join("zoom.json")), &saver)
            .unwrap();
        reader.ink.begin(
            1,
            bounds,
            POINT { x: 100, y: 120 },
            0.55,
            Tool::Pen,
            &reader.settings,
        );
        reader.ink.cancel_contact();
        reader.prepare_page_surface(dc, &layout);
        let page = reader.page_surface.as_ref().unwrap();
        assert_eq!(page.clip, viewport);
        assert!(page.raster.fits(240, 300));
        let raster = page.raster.bitmap;
        let composite = page.composite.as_ref().unwrap().bitmap;
        let ink_version = page.ink_version;

        reader.ink.begin(
            2,
            bounds,
            POINT { x: 145, y: 155 },
            0.75,
            Tool::Pen,
            &reader.settings,
        );
        reader
            .ink
            .contact_update(2, bounds, POINT { x: 169, y: 160 });
        reader.prepare_page_surface(dc, &layout);
        let page = reader.page_surface.as_ref().unwrap();
        assert_eq!(page.raster.bitmap, raster);
        assert_eq!(page.composite.as_ref().unwrap().bitmap, composite);
        assert_eq!(page.ink_version, ink_version);

        reader.ink.cancel_contact();
        reader.prepare_page_surface(dc, &layout);
        let page = reader.page_surface.as_ref().unwrap();
        assert_eq!(page.raster.bitmap, raster);
        assert_eq!(page.composite.as_ref().unwrap().bitmap, composite);
        assert_eq!(page.ink_version.unwrap().append_from(ink_version), Some(1));
        let committed = page.ink_version;
        reader.ink.undo();
        reader.prepare_page_surface(dc, &layout);
        let page = reader.page_surface.as_ref().unwrap();
        assert_eq!(page.ink_version.unwrap().count, 1);
        assert_eq!(page.ink_version.unwrap().append_from(committed), None);

        reader.view_moving = true;
        reader.prepare_page_surface(dc, &layout);
        let preview = reader.page_surface.as_ref().unwrap();
        assert_eq!(preview.bounds, preview.clip);
        assert!(preview.raster.fits(preview.frame.width, preview.frame.height));
        let preview_bounds = preview.bounds;
        let preview_bitmap = preview.composite.as_ref().unwrap().bitmap;
        for factor in [0.8, 1.1, 1.15, 0.6] {
            reader
                .view
                .transform(viewport, viewport, (125.0, 135.0), (130.0, 128.0), factor);
            reader.prepare_page_surface(dc, &layout);
            let preview = reader.page_surface.as_ref().unwrap();
            assert_eq!(preview.bounds, preview_bounds);
            assert_eq!(preview.composite.as_ref().unwrap().bitmap, preview_bitmap);
        }
        reader.view_moving = false;
        let detail = PageKey { width: key.width * 2, height: key.height * 2, ..key };
        reader.desired = Some(detail);
        reader.queued.push(detail);
        reader.prepare_page_surface(dc, &layout);
        assert_eq!(
            reader.page_surface.as_ref().unwrap().composite.as_ref().unwrap().bitmap,
            preview_bitmap
        );
        reader.queued.clear();
        reader.prepare_page_surface(dc, &layout);
        let settled = reader.page_surface.as_ref().unwrap();
        assert_eq!(settled.bounds, reader.view.bounds(viewport, viewport));
        drop(reader);
        assert!(unsafe { DeleteDC(dc) }.as_bool());
        drop(drawing);
    }

    #[test]
    fn skips_empty_labels_when_drawing_the_welcome_screen() {
        let dc = unsafe { CreateCompatibleDC(None) };
        assert!(!dc.is_invalid());
        let bounds = RECT {
            left: 0,
            top: 0,
            right: 120,
            bottom: 40,
        };
        text(dc, bounds, "", TEXT, 16);
        assert!(unsafe { DeleteDC(dc) }.as_bool());
    }

    #[test]
    fn welcome_logo_keeps_smooth_edges_at_display_scales() {
        let _drawing = canvas::Startup::new().unwrap();
        let reader = Reader {
            welcome_logo: Some(canvas::Bitmap::from_png(WELCOME_LOGO).unwrap()),
            ..Reader::default()
        };
        let screen = unsafe { GetDC(None) };
        assert!(!screen.is_invalid());
        for scale in [1.0, 1.5, 2.0, 3.0] {
            let client = RECT {
                right: (420.0 * scale) as i32,
                bottom: (485.0 * scale) as i32,
                ..RECT::default()
            };
            let layout = Layout {
                client,
                viewport: client,
                strip: None,
                scale,
                dpi: scale,
            };
            let surface = Surface::new(screen, client.right, client.bottom).unwrap();
            reader.draw(HWND::default(), surface.dc, &layout, client);

            let logo = browse::home_logo(&layout);
            let center = (logo.left + logo.right) / 2;
            let top = logo.top;
            let pixel = |y| unsafe { GetPixel(surface.dc, center, top + y) };
            assert_eq!(pixel(0), BACKGROUND);
            assert_eq!(pixel(layout.px(19)), ACCENT);
            assert!((1..layout.px(19)).any(|y| {
                let color = pixel(y);
                color != BACKGROUND && color != ACCENT
            }));
        }
        assert_ne!(unsafe { ReleaseDC(None, screen) }, 0);
    }

    #[test]
    fn compact_audio_cards_keep_the_primary_control_and_can_expand() {
        let viewport = RECT {
            right: 960,
            bottom: 720,
            ..RECT::default()
        };
        let layout = Layout {
            client: viewport,
            viewport,
            strip: None,
            scale: 1.0,
            dpi: 1.0,
        };
        let mut reader = Reader::default();
        for slot in [0, 1] {
            reader.overlay.toggle_audio(slot);
            reader.overlay.advance_audio(0.2);
            let expanded = reader
                .cards(&layout)
                .into_iter()
                .find(|card| card.floating == Some(slot))
                .unwrap();
            if slot == 0 {
                assert!(
                    expanded.buttons.iter().any(|button| button.event == Event::TapTempo)
                );
            }
            reader.overlay.toggle_compact(slot);
            reader.overlay.advance_audio(0.09);
            let midway = reader.cards(&layout).into_iter()
                .find(|card| card.floating == Some(slot)).unwrap();
            assert!(midway.rect.bottom - midway.rect.top > 78);
            assert!(midway.rect.bottom - midway.rect.top < expanded.rect.bottom - expanded.rect.top);
            assert!(midway.opacity < 1.0);
            assert!(!midway.interactive);
            reader.overlay.advance_audio(0.12);
            let compact = reader
                .cards(&layout)
                .into_iter()
                .find(|card| card.floating == Some(slot))
                .unwrap();
            assert!(
                compact.rect.bottom - compact.rect.top < expanded.rect.bottom - expanded.rect.top
            );
            for event in [
                Event::AudioCompact(slot),
                Event::AudioToggle(slot),
                Event::AudioClose(slot),
            ] {
                let control = compact.buttons.iter().find(|button| button.event == event).unwrap();
                let hit = POINT {
                    x: (control.rect.left + control.rect.right) / 2,
                    y: (control.rect.top + control.rect.bottom) / 2,
                };
                assert_eq!(compact.grab(0, hit).event, Some(event));
            }
            reader.overlay.close_audio(slot);
            assert!(!reader.overlay.audio_visible());
            reader.overlay.advance_audio(0.2);
        }
    }

    #[test]
    fn concert_controls_jump_to_the_first_playable_page_of_each_neighbor() {
        let scratch = crate::data::Scratch::new();
        let path = scratch.0.join("score.pdf");
        std::fs::write(&path, b"sample").unwrap();
        let (source, _) = ScoreFile::open(&path).unwrap();
        let score = |first_page, count, pdf_start| ScoreInfo {
            title: String::new(),
            source: source.clone(),
            first_page,
            count,
            pdf_start,
            pdf_count: 20,
            entry: ScoreEntry::Whole(path.clone()),
        };
        let mut reader = Reader {
            concert: true,
            selected: 2,
            scores: vec![score(0, 4, 6), score(4, 3, 10)],
            ..Reader::default()
        };
        assert_eq!(reader.score_destination(-1), None);
        assert_eq!(reader.score_destination(1), Some(4));
        reader.selected = 6;
        assert_eq!(reader.score_destination(-1), Some(0));
        assert_eq!(reader.score_destination(1), None);
    }

    #[test]
    fn rounded_buttons_and_circles_blend_their_edges() {
        let _graphics = canvas::Startup::new().unwrap();
        let screen = unsafe { GetDC(None) };
        let surface = Surface::new(screen, 90, 50).unwrap();
        let bounds = RECT { right: 90, bottom: 50, ..RECT::default() };
        fill(surface.dc, bounds, BACKGROUND);
        let button = RECT { left: 4, top: 4, right: 43, bottom: 42 };
        let disc = offset(button, 43, 0);
        rounded(surface.dc, button, ACCENT, 24);
        circle(surface.dc, disc, ACCENT, ACCENT, 0.0);
        for bounds in [button, disc] {
            assert!((bounds.top..bounds.top + 10).any(|y| {
                (bounds.left..bounds.left + 15).any(|x| {
                    let pixel = unsafe { GetPixel(surface.dc, x, y) };
                    pixel != BACKGROUND && pixel != ACCENT
                })
            }));
        }
        drop(surface);
        let _ = unsafe { ReleaseDC(None, screen) };
    }

    #[test]
    fn closing_a_document_returns_to_start_and_preserves_history() {
        let mut reader = Reader {
            total: 3,
            selected: 2,
            recent: vec![ReadingPosition { path: PathBuf::from("score.pdf"), page: 2 }],
            ..Reader::default()
        };
        reader.overlay.open(Sheet::Preferences);
        reader.overlay.tick();
        reader.close_document(HWND::default());
        assert_eq!(reader.total, 0);
        assert_eq!(reader.selected, 0);
        assert!(reader.status.is_empty());
        assert!(!reader.overlay.drawer);
        assert_eq!(reader.overlay.sheet, None);
        assert_eq!(reader.recent.len(), 1);
        assert_eq!(reader.recent[0].page, 2);
    }

    #[test]
    fn only_the_currently_selected_page_is_editable() {
        let key = PageKey {
            session: 3,
            index: 6,
            width: 3,
            height: 3,
        };
        let frame = Arc::new(Frame {
            width: 3,
            height: 3,
            pixels: vec![0; 36],
        });
        let mut reader = Reader {
            session: key.session,
            selected: key.index,
            visible: Some((key, frame)),
            ..Reader::default()
        };
        assert!(reader.editable_frame().is_some());
        reader.selected = key.index + 1;
        assert!(reader.editable_frame().is_none());
        reader.selected = key.index;
        reader.session = key.session + 1;
        assert!(reader.editable_frame().is_none());
    }
}
