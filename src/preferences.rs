use crate::dialog::{self, Control, EditField};
use crate::pdf::Frame;
use crate::reader::{BACKGROUND, Surface, draw_bitmap, fill, inside, point_from_mouse, rounded};
use std::cell::RefCell;
use std::f32::consts::TAU;
use std::sync::OnceLock;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, ExcludeClipRect, HDC, InvalidateRect, PAINTSTRUCT, RestoreDC, SaveDC,
};
use windows::Win32::UI::Controls::EM_SETLIMITTEXT;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::*;

const HEX: EditField = EditField { id: 101, bounds: [176, 76, 96, 25] };
const RGB: [EditField; 3] = [
    EditField { id: 102, bounds: [176, 121, 30, 25] },
    EditField { id: 103, bounds: [209, 121, 30, 25] },
    EditField { id: 104, bounds: [242, 121, 30, 25] },
];
const FIELDS: [EditField; 4] = [HEX, RGB[0], RGB[1], RGB[2]];
const WHEEL: [i32; 4] = [14, 10, 147, 124];
const SAMPLE: [i32; 4] = [176, 15, 96, 40];
const BRIGHTNESS: [i32; 4] = [19, 153, 137, 15];

pub fn pick(owner: HWND, initial: u32) -> Option<u32> {
    let channels = [initial as u8, (initial >> 8) as u8, (initial >> 16) as u8];
    let color = Hsv::from_rgb(channels, Hsv::default());
    let state = RefCell::new(ColorEditor { color, valid: true, drag: None, buffer: None });
    let [r, g, b] = channels.map(|channel| channel.to_string());
    let hex_value = hex(channels);
    let controls = [
        Control::label(200, [176, 62, 96, 12], tr!("Esadecimale", "Hexadecimal")),
        HEX.control(&hex_value, 0),
        Control::label(201, [176, 107, 30, 12], "R"),
        Control::label(202, [209, 107, 30, 12], "G"),
        Control::label(203, [242, 107, 30, 12], "B"),
        RGB[0].control(&r, ES_NUMBER as u32),
        RGB[1].control(&g, ES_NUMBER as u32),
        RGB[2].control(&b, ES_NUMBER as u32),
        Control::label(204, [19, 138, 137, 13], tr!("Luminosità", "Brightness")),
        Control::label(205, [176, 152, 105, 22], ""),
        Control::button(2, [114, 181, 72, 24], tr!("Annulla", "Cancel")),
        Control::button(1, [192, 181, 80, 24], tr!("Usa colore", "Use color")),
    ];
    let result = dialog::show(
        owner,
        tr!("Colore personalizzato", "Custom color"),
        [286, 217],
        &controls,
        Some(procedure),
        LPARAM((&raw const state) as isize),
    ).ok()?;
    (result == 1).then(|| colorref(state.into_inner().color.rgb()).0)
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

fn parse_hex(value: &str) -> Option<[u8; 3]> {
    let value = value.trim().trim_start_matches('#');
    if value.len() != 6 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let rgb = u32::from_str_radix(value, 16).ok()?;
    Some([(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8])
}

fn colorref([r, g, b]: [u8; 3]) -> COLORREF {
    COLORREF(u32::from(r) | u32::from(g) << 8 | u32::from(b) << 16)
}

#[derive(Clone, Copy, Default)]
struct Hsv {
    hue: f32,
    saturation: f32,
    value: f32,
}

impl Hsv {
    fn from_rgb(rgb: [u8; 3], previous: Self) -> Self {
        let [r, g, b] = rgb.map(|channel| f32::from(channel) / 255.0);
        let value = r.max(g).max(b);
        let delta = value - r.min(g).min(b);
        let hue = if delta == 0.0 {
            previous.hue
        } else if value == r {
            ((g - b) / delta / 6.0).rem_euclid(1.0)
        } else if value == g {
            ((b - r) / delta + 2.0) / 6.0
        } else {
            ((r - g) / delta + 4.0) / 6.0
        };
        let saturation = if value > 0.0 { delta / value } else { previous.saturation };
        Self { hue, saturation, value }
    }

    fn rgb(self) -> [u8; 3] {
        let h = self.hue * 6.0;
        [5.0, 3.0, 1.0].map(|offset| {
            let k = (offset + h).rem_euclid(6.0);
            let channel = self.value * (1.0 - self.saturation * k.min(4.0 - k).clamp(0.0, 1.0));
            (channel * 255.0).round() as u8
        })
    }

    fn at(x: f32, y: f32, value: f32) -> Self {
        Self {
            hue: (y.atan2(x) / TAU).rem_euclid(1.0),
            saturation: x.hypot(y).min(1.0),
            value,
        }
    }
}

fn wheel() -> &'static Frame {
    static FRAME: OnceLock<Frame> = OnceLock::new();
    FRAME.get_or_init(|| {
        const SIZE: i32 = 384;
        let mut pixels = Vec::with_capacity((SIZE * SIZE * 4) as usize);
        let half = SIZE as f32 / 2.0;
        let background = [BACKGROUND.0 as u8, (BACKGROUND.0 >> 8) as u8, (BACKGROUND.0 >> 16) as u8];
        for y in 0..SIZE {
            for x in 0..SIZE {
                let x = (x as f32 + 0.5) / half - 1.0;
                let y = (y as f32 + 0.5) / half - 1.0;
                let coverage = ((1.0 - x.hypot(y)) * half + 0.5).clamp(0.0, 1.0);
                let [r, g, b] = Hsv::at(x, y, 1.0).rgb();
                let blend = |channel, bg| (f32::from(channel) * coverage + f32::from(bg) * (1.0 - coverage)).round() as u8;
                pixels.extend_from_slice(&[blend(b, background[2]), blend(g, background[1]), blend(r, background[0]), 255]);
            }
        }
        Frame { width: SIZE, height: SIZE, pixels }
    })
}

fn wheel_bounds(hwnd: HWND) -> RECT {
    let area = dialog::area(hwnd, WHEEL);
    let side = (area.right - area.left).min(area.bottom - area.top);
    let left = (area.left + area.right - side) / 2;
    let top = (area.top + area.bottom - side) / 2;
    RECT { left, top, right: left + side, bottom: top + side }
}

fn wheel_coordinates(bounds: RECT, point: POINT) -> (f32, f32) {
    let half = ((bounds.right - bounds.left) as f32 / 2.0).max(1.0);
    ((point.x - bounds.left) as f32 / half - 1.0, (point.y - bounds.top) as f32 / half - 1.0)
}

fn paint_bounds(hwnd: HWND, area: [i32; 4]) -> RECT {
    let rect = dialog::area(hwnd, area);
    let pad = dialog::area(hwnd, [0, 0, 3, 0]).right.max(3) + 2;
    let vertical = if area == WHEEL { pad } else { 0 };
    RECT {
        left: rect.left - pad,
        top: rect.top - vertical,
        right: rect.right + pad,
        bottom: rect.bottom + vertical,
    }
}

#[derive(Clone, Copy)]
enum Drag { Wheel, Brightness }

struct ColorEditor {
    color: Hsv,
    valid: bool,
    drag: Option<Drag>,
    buffer: Option<Surface>,
}

impl ColorEditor {
    fn read_inputs(&mut self, hwnd: HWND, id: u16) {
        let value = if id == HEX.id {
            parse_hex(&dialog::value(hwnd, HEX.id))
        } else {
            let [r, g, b] = RGB.map(|field| dialog::value(hwnd, field.id).parse::<u8>());
            r.ok().zip(g.ok()).zip(b.ok()).map(|((r, g), b)| [r, g, b])
        };
        if let Some(channels) = value {
            self.color = Hsv::from_rgb(channels, self.color);
            self.sync_inputs(hwnd, Some(id));
            self.redraw(hwnd);
        }
        self.set_valid(hwnd, value.is_some(), id);
    }

    fn sync_inputs(&self, hwnd: HWND, source: Option<u16>) {
        let channels = self.color.rgb();
        let hex_value = hex(channels);
        if source != Some(HEX.id) && dialog::value(hwnd, HEX.id) != hex_value {
            dialog::set_value(hwnd, HEX.id, &hex_value);
        }
        for (field, channel) in RGB.into_iter().zip(channels) {
            let value = channel.to_string();
            if source != Some(field.id) && dialog::value(hwnd, field.id) != value {
                dialog::set_value(hwnd, field.id, &value);
            }
        }
    }

    fn set_valid(&mut self, hwnd: HWND, valid: bool, source: u16) {
        if self.valid && valid {
            return;
        }
        self.valid = valid;
        dialog::enable(hwnd, 1, valid);
        dialog::set_value(hwnd, 205, if valid {
            ""
        } else if source == HEX.id {
            tr!("Formato: #RRGGBB", "Format: #RRGGBB")
        } else {
            tr!("Valori RGB: 0–255", "RGB values: 0–255")
        });
    }

    fn move_to(&mut self, hwnd: HWND, drag: Drag, point: POINT) {
        match drag {
            Drag::Wheel => {
                let (x, y) = wheel_coordinates(wheel_bounds(hwnd), point);
                self.color = Hsv::at(x, y, self.color.value);
            }
            Drag::Brightness => {
                let bounds = dialog::area(hwnd, BRIGHTNESS);
                self.color.value = ((point.x - bounds.left) as f32 / (bounds.right - bounds.left).max(1) as f32).clamp(0.0, 1.0);
            }
        }
        self.sync_inputs(hwnd, None);
        self.set_valid(hwnd, true, 0);
        self.redraw(hwnd);
    }

    fn redraw(&self, hwnd: HWND) {
        for area in [WHEEL, SAMPLE, BRIGHTNESS] {
            let _ = unsafe { InvalidateRect(Some(hwnd), Some(&paint_bounds(hwnd, area)), false) };
        }
    }

    fn draw(&self, hwnd: HWND, dc: HDC) {
        for area in [WHEEL, SAMPLE, BRIGHTNESS] {
            fill(dc, paint_bounds(hwnd, area), BACKGROUND);
        }
        let wheel_rect = wheel_bounds(hwnd);
        draw_bitmap(dc, wheel_rect, wheel());
        let radius = (wheel_rect.right - wheel_rect.left) as f32 / 2.0;
        let angle = self.color.hue * TAU;
        let mark = radius * self.color.saturation;
        let x = (wheel_rect.left as f32 + radius + angle.cos() * mark).round() as i32;
        let y = (wheel_rect.top as f32 + radius + angle.sin() * mark).round() as i32;
        let dot = dialog::area(hwnd, [0, 0, 3, 0]).right.max(3);
        let sample = colorref(self.color.rgb());
        let square = |radius| RECT { left: x - radius, top: y - radius, right: x + radius, bottom: y + radius };
        rounded(dc, square(dot + 1), BACKGROUND, (dot + 1) * 2);
        rounded(dc, square(dot), COLORREF(0xffffff), dot * 2);
        rounded(dc, square(dot - 2), sample, (dot - 2) * 2);
        rounded(dc, dialog::area(hwnd, SAMPLE), sample, dot * 2);

        let bar = dialog::area(hwnd, BRIGHTNESS);
        let width = bar.right - bar.left;
        for step in 0..48 {
            let left = bar.left + width * step / 48;
            let right = bar.left + width * (step + 1) / 48;
            let color = Hsv { value: step as f32 / 47.0, ..self.color };
            fill(dc, RECT { left, right, ..bar }, colorref(color.rgb()));
        }
        let x = bar.left + (self.color.value * (width - 1).max(0) as f32).round() as i32;
        fill(dc, RECT { left: x - 2, right: x + 2, ..bar }, BACKGROUND);
        fill(dc, RECT { left: x - 1, right: x + 1, ..bar }, COLORREF(0xffffff));
    }

    fn paint(&mut self, hwnd: HWND, dc: HDC) {
        let mut bounds = RECT::default();
        if self.buffer.is_none() && unsafe { GetClientRect(hwnd, &mut bounds) }.is_ok() {
            self.buffer = Surface::new(dc, bounds.right, bounds.bottom);
        }
        if let Some(buffer) = &self.buffer {
            self.draw(hwnd, buffer.dc);
            for area in [WHEEL, SAMPLE, BRIGHTNESS] {
                let rect = paint_bounds(hwnd, area);
                buffer.copy_to(dc, rect, rect.left, rect.top);
            }
        } else {
            self.draw(hwnd, dc);
        }
    }
}

unsafe extern "system" fn procedure(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, lp.0) };
        for field in FIELDS {
            let limit = if field.id == HEX.id { 7 } else { 3 };
            unsafe { SendDlgItemMessageW(hwnd, field.id.into(), EM_SETLIMITTEXT, WPARAM(limit), LPARAM(0)) };
        }
        return 1;
    }
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<ColorEditor>;
    let Some(state) = (unsafe { pointer.as_ref() }) else { return 0 };
    let Ok(mut state) = state.try_borrow_mut() else { return 0 };
    match msg {
        WM_ERASEBKGND => {
            let dc = HDC(wp.0 as _);
            let saved = unsafe { SaveDC(dc) };
            if saved != 0 && state.buffer.is_some() {
                for area in [WHEEL, SAMPLE, BRIGHTNESS] {
                    let rect = paint_bounds(hwnd, area);
                    unsafe { ExcludeClipRect(dc, rect.left, rect.top, rect.right, rect.bottom) };
                }
            }
            dialog::field_background(hwnd, dc, &FIELDS);
            if saved != 0 {
                let _ = unsafe { RestoreDC(dc, saved) };
            }
        }
        WM_PAINT => unsafe {
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut paint);
            if !dc.is_invalid() {
                state.paint(hwnd, dc);
            }
            let _ = EndPaint(hwnd, &paint);
        },
        WM_LBUTTONDOWN => {
            if dialog::field_focus(hwnd, lp, &FIELDS) {
                return 1;
            }
            let point = point_from_mouse(lp);
            let (x, y) = wheel_coordinates(wheel_bounds(hwnd), point);
            let drag = if x.hypot(y) <= 1.0 {
                Drag::Wheel
            } else if inside(dialog::area(hwnd, BRIGHTNESS), point) {
                Drag::Brightness
            } else {
                return 0;
            };
            unsafe { SetCapture(hwnd) };
            state.drag = Some(drag);
            state.move_to(hwnd, drag, point);
        }
        WM_MOUSEMOVE | WM_LBUTTONUP => {
            let Some(drag) = state.drag else { return 0 };
            state.move_to(hwnd, drag, point_from_mouse(lp));
            if msg == WM_LBUTTONUP {
                state.drag = None;
                let _ = unsafe { ReleaseCapture() };
            }
        }
        WM_CAPTURECHANGED => state.drag = None,
        WM_COMMAND if (wp.0 >> 16) as u32 == EN_CHANGE => state.read_inputs(hwnd, wp.0 as u16),
        WM_COMMAND if wp.0 as u16 == 1 && state.valid => {
            let _ = unsafe { EndDialog(hwnd, 1) };
        }
        WM_COMMAND if wp.0 as u16 == 2 => {
            let _ = unsafe { EndDialog(hwnd, 0) };
        }
        WM_CLOSE => {
            let _ = unsafe { EndDialog(hwnd, 0) };
        }
        _ => return 0,
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_and_numeric_inputs_preserve_rgb_channel_order() {
        for rgb in [[255, 0, 0], [0, 255, 0], [0, 0, 255], [13, 42, 221], [0, 0, 0], [128, 128, 128]] {
            assert_eq!(Hsv::from_rgb(rgb, Hsv::default()).rgb(), rgb);
            assert_eq!(parse_hex(&hex(rgb)), Some(rgb));
        }
        assert_eq!(Hsv::at(1.0, 0.0, 1.0).rgb(), [255, 0, 0]);
        assert_eq!(Hsv::at(0.0, 0.0, 0.5).rgb(), [128; 3]);
        assert_eq!(parse_hex("€123"), None);
    }
}
