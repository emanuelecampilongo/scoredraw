use super::*;

const HEIGHT: i32 = 42;
const CAPTION_WIDTH: i32 = 43;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Menu {
    File,
    Edit,
    View,
    Tools,
    Help,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Target {
    Menu(Menu),
    Minimize,
    Maximize,
    Close,
}

#[derive(Clone, Copy)]
pub struct Strip {
    pub rect: RECT,
    scale: f32,
}

impl Strip {
    pub fn new(client: RECT, scale: f32) -> Self {
        let scale = scale.min((client.right - client.left) as f32 / 440.0);
        Self {
            rect: RECT {
                bottom: (HEIGHT as f32 * scale).round() as i32,
                ..client
            },
            scale,
        }
    }

    fn px(self, dp: i32) -> i32 {
        (dp as f32 * self.scale).round() as i32
    }

    fn tabs(self) -> impl Iterator<Item = (Menu, &'static str, RECT)> {
        let menus = [
            (Menu::File, "File", 48),
            (Menu::Edit, tr!("Modifica", "Edit"), 73),
            (Menu::View, tr!("Vista", "View"), 56),
            (Menu::Tools, tr!("Strumenti", "Tools"), 83),
            (Menu::Help, "?", 36),
        ];
        let mut left = self.px(8);
        menus.into_iter().map(move |(menu, title, width)| {
            let right = left + self.px(width);
            let bounds = RECT {
                left,
                right,
                top: self.px(4),
                bottom: self.rect.bottom - self.px(4),
            };
            left = right;
            (menu, title, bounds)
        })
    }

    fn captions(self) -> impl Iterator<Item = (Target, RECT)> {
        [Target::Close, Target::Maximize, Target::Minimize]
            .into_iter()
            .enumerate()
            .map(move |(i, button)| {
                let right = self.rect.right - self.px(i as i32 * CAPTION_WIDTH);
                let left = self.rect.right - self.px((i as i32 + 1) * CAPTION_WIDTH);
                (
                    button,
                    RECT {
                        left,
                        right,
                        ..self.rect
                    },
                )
            })
    }

    pub fn hit(self, point: POINT) -> Option<Target> {
        self.captions()
            .find_map(|(button, rect)| inside(rect, point).then_some(button))
            .or_else(|| {
                self.tabs()
                    .find_map(|(menu, _, rect)| inside(rect, point).then_some(Target::Menu(menu)))
            })
    }

    pub fn draw(
        self,
        dc: HDC,
        caption: &str,
        hover: Option<Target>,
        active: Option<Menu>,
        maximized: bool,
    ) {
        fill(dc, self.rect, PANEL);
        let rule = RECT {
            top: self.rect.bottom - 1,
            ..self.rect
        };
        fill(dc, rule, MENU_BORDER);
        let mut caption_left = 0;
        for (menu, title, bounds) in self.tabs() {
            let selected = active == Some(menu);
            if selected || hover == Some(Target::Menu(menu)) {
                rounded(dc, bounds, SURFACE, self.px(12));
            }
            text(
                dc,
                bounds,
                title,
                if selected { ACCENT } else { TEXT },
                self.px(14),
            );
            caption_left = bounds.right + self.px(13);
        }
        let mut caption_right = self.rect.right;
        for (button, bounds) in self.captions() {
            let hovered = hover == Some(button);
            if hovered {
                fill(
                    dc,
                    bounds,
                    if button == Target::Close {
                        COLORREF(0x3b36b3)
                    } else {
                        SURFACE
                    },
                );
            }
            let glyph = match button {
                Target::Minimize => Glyph::Minus,
                Target::Maximize if maximized => Glyph::Restore,
                Target::Maximize => Glyph::Square,
                Target::Close => Glyph::X,
                Target::Menu(_) => continue,
            };
            icons::draw(dc, glyph, bounds, 15.0 * self.scale, TEXT);
            caption_right = bounds.left - self.px(11);
        }
        if caption_right - caption_left >= self.px(54) {
            text(
                dc,
                RECT {
                    left: caption_left,
                    right: caption_right,
                    ..self.rect
                },
                caption,
                MUTED,
                self.px(13),
            );
        }
    }
}

fn enabled(hwnd: HWND) -> bool {
    unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) as u32 & WS_CAPTION.0 == WS_CAPTION.0 }
}

pub fn frame_message(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    if !matches!(msg, WM_NCCALCSIZE | WM_NCHITTEST) || !enabled(hwnd) {
        return None;
    }
    unsafe {
        if msg == WM_NCCALCSIZE {
            // Let Windows reserve the resizable frame, including the insets of a maximized window.
            let rect = if wp.0 == 0 {
                lp.0 as *mut RECT
            } else {
                (lp.0 as *mut NCCALCSIZE_PARAMS).cast::<RECT>()
            };
            let proposed = *rect;
            DefWindowProcW(hwnd, msg, wp, lp);
            let border = ((*rect).left - proposed.left).max(0);
            (*rect).top = proposed.top + border;
            return Some(LRESULT(0));
        }
        let mut point = point_from_mouse(lp);
        let _ = ScreenToClient(hwnd, &mut point);
        let layout = Layout::new(hwnd, false);
        if !inside(layout.client, point) {
            if point.y < layout.client.top && !IsZoomed(hwnd).as_bool() {
                let edge = if point.x < layout.client.left {
                    HTTOPLEFT
                } else if point.x >= layout.client.right {
                    HTTOPRIGHT
                } else {
                    HTTOP
                };
                return Some(LRESULT(edge as isize));
            }
            return None;
        }
        let strip = layout.strip?;
        let code = if inside(strip.rect, point) && strip.hit(point).is_none() {
            HTCAPTION
        } else {
            HTCLIENT
        };
        Some(LRESULT(code as isize))
    }
}

impl Reader {
    pub(super) fn update_chrome_hover(&mut self, hwnd: HWND, point: POINT) {
        // Most pointer events are on the page. Skip building the strip geometry there.
        if self.chrome_hover.is_none() && point.y > 125 {
            return;
        }
        let Some(strip) = Layout::new(hwnd, false).strip else {
            return;
        };
        let hovering = strip.hit(point);
        if self.chrome_hover != hovering {
            if self.chrome_hover.is_none() && hovering.is_some() {
                let mut tracking = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                let _ = unsafe { TrackMouseEvent(&mut tracking) };
            }
            self.chrome_hover = hovering;
            repaint_rect(hwnd, strip.rect);
        }
    }

    pub(super) fn chrome_contact(&mut self, hwnd: HWND, msg: u32, id: u32, point: POINT) -> bool {
        if self.fullscreen.is_some() || (msg == WM_POINTERDOWN && point.y > 125) {
            return false;
        }
        match (msg, self.chrome_press) {
            (WM_POINTERDOWN, None) => {
                let Some(target) = Layout::new(hwnd, false)
                    .strip
                    .and_then(|strip| strip.hit(point))
                else {
                    return false;
                };
                self.chrome_press = Some((id, target));
                true
            }
            (WM_POINTERUPDATE, Some((contact, _))) if contact == id => true,
            (WM_POINTERUP, Some((contact, target))) if contact == id => {
                self.chrome_press = None;
                if Layout::new(hwnd, false)
                    .strip
                    .and_then(|strip| strip.hit(point))
                    == Some(target)
                {
                    self.invoke_chrome(hwnd, target);
                }
                true
            }
            _ => false,
        }
    }

    pub(super) fn cancel_chrome_press(&mut self, id: u32) {
        if self.chrome_press.is_some_and(|(contact, _)| contact == id) {
            self.chrome_press = None;
        }
    }

    fn invoke_chrome(&mut self, hwnd: HWND, target: Target) {
        let system = match target {
            Target::Menu(menu) => {
                self.chrome_menu = (self.chrome_menu != Some(menu)).then_some(menu);
                repaint(hwnd);
                return;
            }
            Target::Minimize => SC_MINIMIZE,
            Target::Maximize if unsafe { IsZoomed(hwnd) }.as_bool() => SC_RESTORE,
            Target::Maximize => SC_MAXIMIZE,
            Target::Close => SC_CLOSE,
        };
        let _ = unsafe {
            PostMessageW(
                Some(hwnd),
                WM_SYSCOMMAND,
                WPARAM(system as usize),
                LPARAM(0),
            )
        };
    }
}

impl Reader {
    pub(super) fn chrome_card(&self, layout: &Layout) -> Option<overlay::Card> {
        self.chrome_card_for(layout, self.chrome_menu?)
    }

    pub(super) fn chrome_card_for(&self, layout: &Layout, menu: Menu) -> Option<overlay::Card> {
        use Command as C;
        use Event as E;
        let strip = layout.strip?;
        let (_, _, anchor) = strip.tabs().find(|(id, _, _)| *id == menu)?;
        let s = self.total > 0;
        let has_options = s && self.tool.has_options();
        let tool = if has_options { trf!(
            "Opzioni · {}",
            "Options · {}",
            Command::Tool(self.tool).label()
        ) } else { tr!("Opzioni strumento corrente", "Current tool options").into() };
        let entries: &[(&str, Glyph, Event, bool)] = match menu {
            Menu::File => &[
                (
                    tr!("Apri PDF…", "Open PDF…"),
                    Glyph::FolderOpen,
                    E::Command(C::Open),
                    true,
                ),
                (
                    tr!("Scalette…", "Setlists…"),
                    Glyph::List,
                    E::Command(C::Library),
                    true,
                ),
                (
                    tr!("Rendi modifiche permanenti…", "Make changes permanent…"),
                    Glyph::Save,
                    E::Finalize,
                    self.can_finalize(),
                ),
                (
                    tr!("Home", "Home"),
                    Glyph::Home,
                    E::CloseDocument,
                    self.total > 0 && self.exporting.is_none(),
                ),
            ],
            Menu::Edit => &[
                (
                    tr!("Annulla", "Cancel"),
                    Glyph::Undo,
                    E::Command(C::Undo),
                    self.can_undo(),
                ),
                (
                    tr!("Ripeti", "Redo"),
                    Glyph::Redo,
                    E::Command(C::Redo),
                    self.can_redo(),
                ),
                (
                    tr!("Elimina oggetto selezionato", "Delete selected object"),
                    Glyph::Trash,
                    E::DeleteShape,
                    self.can_delete_shape(),
                ),
            ],
            Menu::View => &[
                (
                    tr!("Schermo intero", "Full screen"),
                    Glyph::Expand,
                    E::Command(C::Fullscreen),
                    s,
                ),
                (
                    tr!("Vai a pagina…", "Go to page…"),
                    Glyph::FileText,
                    E::Sheet(Sheet::Page),
                    s,
                ),
                (
                    tr!("Segnalibri", "Bookmarks"),
                    Glyph::Bookmark,
                    E::Sheet(Sheet::Bookmarks),
                    s,
                ),
                (
                    tr!("Adatta pagina", "Fit page"),
                    Glyph::Minimize,
                    E::ResetZoom,
                    s && self.view.zoom() > 1.0,
                ),
            ],
            Menu::Tools => &[
                (
                    tr!("Metronomo", "Metronome"),
                    Glyph::Metronome,
                    E::Command(C::Metronome),
                    self.visible.is_some(),
                ),
                (
                    tr!("Accordatore", "Tuner"),
                    Glyph::Gauge,
                    E::Command(C::Tuner),
                    self.visible.is_some(),
                ),
                (
                    &tool,
                    C::Tool(self.tool).icon(),
                    E::Command(C::Settings),
                    has_options,
                ),
                (
                    tr!("Preferenze…", "Settings…"),
                    Glyph::Sliders,
                    E::Advanced,
                    true,
                ),
            ],
            Menu::Help => &[
                (tr!("Informazioni", "About"), Glyph::Info, E::About, true),
                (
                    tr!("Segnalazioni…", "Feedback…"),
                    Glyph::FileText,
                    E::Feedback,
                    true,
                ),
            ],
        };
        let px = |x| strip.px(x);
        let width = px(300);
        let x = anchor.left.min((layout.client.right - width).max(0));
        let mut card = overlay::Card::new(
            x,
            strip.rect.bottom + px(3) - px((4.0 * (1.0 - self.overlay.popup_opacity)).round() as i32),
            width,
            px(16 + 53 * entries.len() as i32),
            strip.scale,
            None,
        );
        card.opacity = self.overlay.popup_opacity;
        for (i, &(label, icon, event, enabled)) in entries.iter().enumerate() {
            card.menu_item([8, 8 + 53 * i as i32, 284, 48], label, icon, event, enabled);
        }
        Some(card)
    }
}
