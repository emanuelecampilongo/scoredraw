use std::ptr::null_mut;
use windows::Win32::Foundation::{COLORREF, E_FAIL, RECT};
use windows::Win32::Graphics::Gdi::HDC;
use windows::Win32::Graphics::GdiPlus::{self as gdi, *};

pub struct Startup(usize);

impl Startup {
    pub fn new() -> windows::core::Result<Self> {
        let mut token = 0;
        let input = GdiplusStartupInput {
            GdiplusVersion: 1,
            ..Default::default()
        };
        if unsafe { GdiplusStartup(&mut token, &input, null_mut()) } == gdi::Ok {
            Result::Ok(Self(token))
        } else {
            Err(windows::core::Error::new(
                E_FAIL,
                tr!(
                    "Impossibile inizializzare il disegno vettoriale.",
                    "Couldn’t start the drawing engine."
                ),
            ))
        }
    }
}

impl Drop for Startup {
    fn drop(&mut self) {
        unsafe { GdiplusShutdown(self.0) };
    }
}

pub struct Canvas(*mut GpGraphics);

impl Canvas {
    pub fn new(dc: HDC) -> Option<Self> {
        let mut graphics = null_mut();
        unsafe {
            if GdipCreateFromHDC(dc, &mut graphics) != gdi::Ok {
                return None;
            }
            GdipSetSmoothingMode(graphics, SmoothingModeAntiAlias);
        }
        Some(Self(graphics))
    }

    pub fn clip(&self, rect: RECT) {
        unsafe {
            GdipSetClipRectI(
                self.0,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                CombineModeIntersect,
            );
        }
    }

    pub fn rectangle(&self, x: f32, y: f32, w: f32, h: f32, color: u32) {
        self.brush(color, |brush| unsafe {
            GdipFillRectangle(self.0, brush, x, y, w, h);
        });
    }

    pub fn ellipse(&self, rect: RECT, fill: u32, outline: u32, width: f32) {
        let x = rect.left as f32;
        let y = rect.top as f32;
        let w = (rect.right - rect.left) as f32;
        let h = (rect.bottom - rect.top) as f32;
        self.brush(fill, |brush| unsafe {
            GdipFillEllipse(self.0, brush, x, y, w, h);
        });
        if width > 0.0 {
            let mut pen = null_mut();
            unsafe {
                if GdipCreatePen1(outline, width, UnitPixel, &mut pen) == gdi::Ok {
                    GdipDrawEllipse(
                        self.0,
                        pen,
                        x + width / 2.0,
                        y + width / 2.0,
                        w - width,
                        h - width,
                    );
                    GdipDeletePen(pen);
                }
            }
        }
    }

    pub fn fill_path(&self, path: &Path, color: u32) {
        self.brush(color, |brush| unsafe {
            GdipFillPath(self.0, brush, path.0);
        });
    }

    pub fn stroke_path(&self, path: &Path, color: u32, width: f32) {
        let mut pen = null_mut();
        unsafe {
            if GdipCreatePen1(color, width, UnitPixel, &mut pen) == gdi::Ok {
                GdipSetPenLineJoin(pen, LineJoinRound);
                GdipSetPenStartCap(pen, LineCapRound);
                GdipSetPenEndCap(pen, LineCapRound);
                GdipDrawPath(self.0, pen, path.0);
                GdipDeletePen(pen);
            }
        }
    }

    fn brush(&self, color: u32, action: impl FnOnce(*mut GpBrush)) {
        let mut brush = null_mut();
        unsafe {
            if GdipCreateSolidFill(color, &mut brush) == gdi::Ok {
                action(brush.cast());
                GdipDeleteBrush(brush.cast());
            }
        }
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe { GdipDeleteGraphics(self.0) };
    }
}

pub struct Path(*mut GpPath);

impl Path {
    pub fn new() -> Option<Self> {
        let mut path = null_mut();
        (unsafe { GdipCreatePath(FillModeWinding, &mut path) } == gdi::Ok).then_some(Self(path))
    }

    pub fn polygon(&self, points: &[PointF]) {
        unsafe {
            GdipAddPathPolygon(self.0, points.as_ptr(), points.len() as i32);
        }
    }

    pub fn start(&self) {
        unsafe { GdipStartPathFigure(self.0) };
    }

    pub fn close(&self) {
        unsafe { GdipClosePathFigure(self.0) };
    }

    pub fn line(&self, (x1, y1): (f32, f32), (x2, y2): (f32, f32)) {
        unsafe { GdipAddPathLine(self.0, x1, y1, x2, y2) };
    }

    pub fn curve(
        &self,
        (x1, y1): (f32, f32),
        (x2, y2): (f32, f32),
        (x3, y3): (f32, f32),
        (x4, y4): (f32, f32),
    ) {
        unsafe { GdipAddPathBezier(self.0, x1, y1, x2, y2, x3, y3, x4, y4) };
    }

    pub fn dot(&self, x: f32, y: f32, radius: f32) {
        unsafe {
            GdipAddPathEllipse(self.0, x - radius, y - radius, 2.0 * radius, 2.0 * radius);
        }
    }
}

impl Drop for Path {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { GdipDeletePath(self.0) };
        }
    }
}

pub fn argb(color: COLORREF, opacity: u8) -> u32 {
    let rgb = color.0 & 0x00ff_ffff;
    (u32::from(opacity) << 24) | ((rgb & 255) << 16) | (rgb & 0xff00) | (rgb >> 16)
}
