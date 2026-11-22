use crate::dialog::{self, Control, EditField};
use crate::reader::{BACKGROUND, SURFACE, TEXT, inside, point_from_mouse, rounded, text};
use std::cell::RefCell;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, EndPaint, HDC, InflateRect, InvalidateRect, PAINTSTRUCT,
};
use windows::Win32::UI::Controls::EM_SETLIMITTEXT;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::*;

const FIELDS: [EditField; 3] = [
    EditField {
        id: 101,
        bounds: [190, 69, 53, 28],
    },
    EditField {
        id: 102,
        bounds: [190, 109, 53, 28],
    },
    EditField {
        id: 103,
        bounds: [190, 149, 53, 28],
    },
];
const CHANNEL_COLORS: [COLORREF; 3] = [COLORREF(0x8578ff), COLORREF(0xa2de79), COLORREF(0xf7b778)];
const SAMPLE: [i32; 4] = [14, 14, 229, 41];

pub fn pick(owner: HWND, initial: u32) -> Option<u32> {
    let channels = [initial as u8, (initial >> 8) as u8, (initial >> 16) as u8];
    let state = RefCell::new(ColorEditor {
        channels,
        drag: None,
    });
    let [r, g, b] = channels.map(|channel| channel.to_string());
    let controls = [
        Control::label(200, [14, 78, 42, 13], tr!("Rosso", "Red")),
        FIELDS[0].control(&r, ES_NUMBER as u32),
        Control::label(201, [14, 118, 42, 13], tr!("Verde", "Green")),
        FIELDS[1].control(&g, ES_NUMBER as u32),
        Control::label(202, [14, 158, 42, 13], tr!("Blu", "Blue")),
        FIELDS[2].control(&b, ES_NUMBER as u32),
        Control::label(203, [14, 183, 229, 15], ""),
        Control::button(2, [84, 205, 68, 24], tr!("Annulla", "Cancel")),
        Control::button(1, [158, 205, 85, 24], tr!("Usa colore", "Use color")),
    ];
    let result = dialog::show(
        owner,
        tr!("Colore personalizzato", "Custom color"),
        [257, 241],
        &controls,
        Some(procedure),
        LPARAM((&raw const state) as isize),
    )
    .ok()?;
    (result == 1).then(|| state.into_inner().color())
}

struct ColorEditor {
    channels: [u8; 3],
    drag: Option<usize>,
}

impl ColorEditor {
    fn color(&self) -> u32 {
        let [r, g, b] = self.channels.map(u32::from);
        r | g << 8 | b << 16
    }

    fn read_inputs(&mut self, hwnd: HWND) {
        let values = FIELDS.map(|field| dialog::value(hwnd, field.id).parse::<u8>());
        let valid = values.iter().all(Result::is_ok);
        for (channel, value) in self.channels.iter_mut().zip(values) {
            if let Ok(value) = value {
                *channel = value;
            }
        }
        dialog::enable(hwnd, 1, valid);
        dialog::set_value(hwnd, 203, if valid { "" } else { "Usa valori da 0 a 255." });
        unsafe {
            let _ = InvalidateRect(Some(hwnd), Some(&dialog::area(hwnd, SAMPLE)), false);
            for index in 0..FIELDS.len() {
                let mut damage = track(hwnd, index);
                let _ = InflateRect(&mut damage, dialog::area(hwnd, [0, 0, 6, 0]).right, 0);
                let _ = InvalidateRect(Some(hwnd), Some(&damage), true);
            }
        }
    }

    fn move_slider(&mut self, hwnd: HWND, index: usize, x: i32) {
        let bounds = track(hwnd, index);
        let range = (bounds.right - bounds.left).max(1);
        let next = ((x - bounds.left).clamp(0, range) * 255 + range / 2) / range;
        if dialog::value(hwnd, FIELDS[index].id).parse::<i32>().ok() != Some(next) {
            dialog::set_value(hwnd, FIELDS[index].id, &next.to_string());
            self.read_inputs(hwnd);
        }
    }

    fn draw(&self, hwnd: HWND, dc: HDC) {
        let sample = dialog::area(hwnd, SAMPLE);
        let radius = dialog::area(hwnd, [0, 0, 12, 0]).right;
        rounded(dc, sample, COLORREF(self.color()), radius);
        let [r, g, b] = self.channels;
        let luma = 299 * u32::from(r) + 587 * u32::from(g) + 114 * u32::from(b);
        let ink = if luma >= 145_000 { BACKGROUND } else { TEXT };
        let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) }.max(96);
        text(
            dc,
            sample,
            &format!("#{r:02X}{g:02X}{b:02X}"),
            ink,
            (15 * scale / 96) as i32,
        );
        let half = (3 * scale / 96).max(2) as i32;
        let thumb = (7 * scale / 96).max(4) as i32;
        for (index, (&channel, &tone)) in self.channels.iter().zip(&CHANNEL_COLORS).enumerate() {
            let rect = track(hwnd, index);
            let y = (rect.top + rect.bottom) / 2;
            let bar = RECT {
                top: y - half,
                bottom: y + half,
                ..rect
            };
            let x = bar.left + (bar.right - bar.left) * i32::from(channel) / 255;
            rounded(dc, bar, SURFACE, half * 2);
            rounded(dc, RECT { right: x, ..bar }, tone, half * 2);
            rounded(
                dc,
                RECT {
                    left: x - thumb,
                    right: x + thumb,
                    top: y - thumb,
                    bottom: y + thumb,
                },
                tone,
                thumb * 2,
            );
        }
    }
}

fn track(hwnd: HWND, index: usize) -> RECT {
    dialog::area(hwnd, [57, 68 + index as i32 * 40, 119, 30])
}

unsafe extern "system" fn procedure(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    if msg == WM_INITDIALOG {
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, lp.0) };
        for field in FIELDS {
            unsafe {
                SendDlgItemMessageW(hwnd, field.id.into(), EM_SETLIMITTEXT, WPARAM(3), LPARAM(0))
            };
        }
        return 1;
    }
    let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const RefCell<ColorEditor>;
    let Some(state) = (unsafe { pointer.as_ref() }) else {
        return 0;
    };
    let Ok(mut state) = state.try_borrow_mut() else {
        return 0;
    };
    match msg {
        WM_ERASEBKGND => dialog::field_background(hwnd, HDC(wp.0 as _), &FIELDS),
        WM_PAINT => unsafe {
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut paint);
            if !dc.is_invalid() {
                state.draw(hwnd, dc);
            }
            let _ = EndPaint(hwnd, &paint);
        },
        WM_LBUTTONDOWN => {
            if dialog::field_focus(hwnd, lp, &FIELDS) {
                return 1;
            }
            let point = point_from_mouse(lp);
            let Some(index) = (0..FIELDS.len()).find(|index| inside(track(hwnd, *index), point))
            else {
                return 0;
            };
            unsafe { SetCapture(hwnd) };
            state.drag = Some(index);
            state.move_slider(hwnd, index, point.x);
        }
        WM_MOUSEMOVE | WM_LBUTTONUP => {
            let Some(index) = state.drag else { return 0 };
            state.move_slider(hwnd, index, point_from_mouse(lp).x);
            if msg == WM_LBUTTONUP {
                state.drag = None;
                let _ = unsafe { ReleaseCapture() };
            }
        }
        WM_CAPTURECHANGED => state.drag = None,
        WM_COMMAND if (wp.0 >> 16) as u32 == EN_CHANGE => state.read_inputs(hwnd),
        WM_COMMAND if wp.0 as u16 == 1 => {
            if FIELDS
                .iter()
                .all(|field| dialog::value(hwnd, field.id).parse::<u8>().is_ok())
            {
                let _ = unsafe { EndDialog(hwnd, 1) };
            }
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
