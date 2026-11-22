use crate::canvas::{self, Canvas, Path};
use windows::Win32::Foundation::{COLORREF, RECT};
use windows::Win32::Graphics::Gdi::HDC;

mod lucide;
pub use lucide::Glyph;

#[derive(Clone, Copy)]
enum Segment {
    Move(f32, f32),
    Line(f32, f32),
    Curve(f32, f32, f32, f32, f32, f32),
    Close,
}

pub fn draw(dc: HDC, glyph: Glyph, rect: RECT, size: f32, color: COLORREF) {
    if let Some(canvas) = Canvas::new(dc) {
        draw_on(&canvas, glyph, rect, size, color);
    }
}

pub fn draw_on(canvas: &Canvas, glyph: Glyph, rect: RECT, size: f32, color: COLORREF) {
    let Some(path) = Path::new() else { return };
    let size = size.min((rect.right - rect.left).min(rect.bottom - rect.top) as f32);
    if size <= 0.0 {
        return;
    }
    let origin = (
        (rect.left + rect.right) as f32 / 2.0 - size / 2.0,
        (rect.top + rect.bottom) as f32 / 2.0 - size / 2.0,
    );
    let point = |x, y| (origin.0 + x * size / 24.0, origin.1 + y * size / 24.0);
    let mut current = (0.0, 0.0);
    for &segment in glyph.path() {
        match segment {
            Segment::Move(x, y) => {
                path.start();
                current = point(x, y);
            }
            Segment::Line(x, y) => {
                let end = point(x, y);
                path.line(current, end);
                current = end;
            }
            Segment::Curve(x1, y1, x2, y2, x3, y3) => {
                let end = point(x3, y3);
                path.curve(current, point(x1, y1), point(x2, y2), end);
                current = end;
            }
            Segment::Close => path.close(),
        }
    }
    let color = canvas::argb(color, 255);
    if matches!(glyph, Glyph::Star) {
        canvas.fill_path(&path, color);
    } else {
        canvas.stroke_path(&path, color, (size / 12.0).max(1.25));
    }
}
