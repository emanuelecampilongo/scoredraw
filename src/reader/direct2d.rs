use super::*;
use std::collections::VecDeque;
use std::io::Write;
use windows::Win32::Graphics::Direct2D::{Common::*, *};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::core::{Interface, Result};

const PIXEL_FORMAT: D2D1_PIXEL_FORMAT = D2D1_PIXEL_FORMAT {
    format: DXGI_FORMAT_B8G8R8A8_UNORM,
    alphaMode: D2D1_ALPHA_MODE_IGNORE,
};

pub(super) struct Renderer {
    target: ID2D1HwndRenderTarget,
    interop: ID2D1GdiInteropRenderTarget,
    size: D2D_SIZE_U,
    textures: VecDeque<(u64, u64, ID2D1Bitmap)>,
    chrome_buffer: Option<Surface>,
    hardware: bool,
    presented: bool,
}

impl Renderer {
    fn new(hwnd: HWND, size: D2D_SIZE_U) -> Result<Self> {
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let window = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd,
                pixelSize: size,
                presentOptions: D2D1_PRESENT_OPTIONS_RETAIN_CONTENTS,
            };
            let mut properties = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_HARDWARE,
                pixelFormat: PIXEL_FORMAT,
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_GDI_COMPATIBLE,
                ..Default::default()
            };
            let (target, hardware) = match factory.CreateHwndRenderTarget(&properties, &window) {
                Ok(target) => (target, true),
                Err(_) => {
                    properties.r#type = D2D1_RENDER_TARGET_TYPE_SOFTWARE;
                    (factory.CreateHwndRenderTarget(&properties, &window)?, false)
                }
            };
            let interop = target.cast()?;
            Ok(Self {
                target,
                interop,
                size,
                textures: VecDeque::new(),
                chrome_buffer: None,
                hardware,
                presented: false,
            })
        }
    }

    fn paint(&mut self, reader: &Reader, hwnd: HWND, window_dc: HDC, layout: &Layout, mut damage: RECT) -> Result<()> {
        let size = client_size(layout);
        if size.width != self.size.width || size.height != self.size.height {
            unsafe { self.target.Resize(&size)? };
            self.size = size;
            self.chrome_buffer = None;
            damage = layout.client;
        }
        unsafe {
            self.target.BeginDraw();
            self.target.PushAxisAlignedClip(&rectangle(damage), D2D1_ANTIALIAS_MODE_ALIASED);
            self.target.Clear(Some(&background()));
            self.target.PopAxisAlignedClip();
        }
        let result = self.scene(reader, hwnd, window_dc, layout, damage);
        let presented = unsafe { self.target.EndDraw(None, None) };
        result.and(presented)
    }

    fn scene(&mut self, reader: &Reader, hwnd: HWND, window_dc: HDC, layout: &Layout, damage: RECT) -> Result<()> {
        let page = reader.page_surface.as_ref().expect("page already prepared");
        let source = page.composite.as_ref().unwrap_or(&page.raster);
        let bounds = reader.view.bounds(fit(&page.frame, layout.viewport), layout.viewport);
        let turn = reader.page_animation.as_ref().and_then(PageAnimation::position);
        let clip = intersect(layout.viewport, damage);
        if let Some(clip) = clip {
            unsafe {
                self.target.PushAxisAlignedClip(&rectangle(clip), D2D1_ANTIALIAS_MODE_ALIASED);
            }
            let drawn: Result<()> = (|| {
                if let Some(drag) = &reader.page_drag {
                    let dx = drag.offset();
                    self.bitmap(source, offset(page.clip, dx, 0), clip)?;
                    if let Some(preview) = &drag.preview && dx != 0 {
                        let width = layout.viewport.right - layout.viewport.left;
                        self.bitmap(&preview.surface, offset(preview.bounds, dx - dx.signum() * width, 0), clip)?;
                    }
                } else if let Some((travel, width, direction, _, _)) = turn
                    && let Some(animation) = &reader.page_animation
                {
                    self.bitmap(&animation.outgoing, offset(animation.clip, travel, 0), clip)?;
                    self.bitmap(source, offset(page.clip, travel - width * direction, 0), clip)?;
                } else {
                    self.bitmap(source, if page.bounds == bounds { page.clip } else { bounds }, clip)?;
                }
                Ok(())
            })();
            unsafe { self.target.PopAxisAlignedClip() };
            drawn?;
        }
        // Some display drivers drop GDI glyphs drawn straight to the interop DC.
        // Compose the dirty region on a normal GDI surface and copy opaque pixels.
        let (width, height) = (damage.right - damage.left, damage.bottom - damage.top);
        if self.chrome_buffer.is_none() {
            self.chrome_buffer = Surface::new(window_dc, self.size.width as i32, self.size.height as i32);
        }
        let buffer = self.chrome_buffer.as_ref().ok_or_else(|| {
            windows::core::Error::from_hresult(windows::Win32::Foundation::E_OUTOFMEMORY)
        })?;
        unsafe {
            let dc = self.interop.GetDC(D2D1_DC_INITIALIZE_MODE_COPY)?;
            let copied = BitBlt(buffer.dc, damage.left, damage.top, width, height, Some(dc), damage.left, damage.top, SRCCOPY)
                .and_then(|()| {
                    with_clip(buffer.dc, damage, || {
                        if let Some(clip) = clip {
                            if let Some((_, _, direction, edge, progress)) = turn
                                && let Some(animation) = &reader.page_animation
                            {
                                with_clip(buffer.dc, clip, || animation.draw_edge(buffer.dc, edge, direction, progress));
                            } else if reader.page_drag.is_none() {
                                reader.ink.draw_live(buffer.dc, layout.ink_area(bounds), clip, reader.tool);
                            }
                        }
                        reader.draw_chrome(hwnd, buffer.dc, layout, damage);
                    });
                    BitBlt(dc, damage.left, damage.top, width, height, Some(buffer.dc), damage.left, damage.top, SRCCOPY)
                });
            let released = self.interop.ReleaseDC(Some(&damage));
            copied.and(released)
        }
    }

    fn bitmap(&mut self, surface: &Surface, bounds: RECT, clip: RECT) -> Result<()> {
        if intersect(bounds, clip).is_none() {
            return Ok(());
        }
        let bitmap = if let Some(index) = self.textures.iter().position(|(id, revision, _)| {
            *id == surface.id && *revision == surface.revision
        }) {
            self.textures.remove(index).unwrap().2
        } else {
            self.textures.retain(|(id, _, _)| *id != surface.id);
            let pixels = read_pixels(surface)?;
            unsafe {
                self.target.CreateBitmap(
                    D2D_SIZE_U { width: surface.width as u32, height: surface.height as u32 },
                    Some(pixels.as_ptr().cast()),
                    surface.width as u32 * 4,
                    &D2D1_BITMAP_PROPERTIES { pixelFormat: PIXEL_FORMAT, dpiX: 96.0, dpiY: 96.0 },
                )?
            }
        };
        unsafe {
            self.target.DrawBitmap(&bitmap, Some(&rectangle(bounds)), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
        }
        self.textures.push_back((surface.id, surface.revision, bitmap));
        if self.textures.len() > 4 {
            self.textures.pop_front();
        }
        Ok(())
    }
}

impl Reader {
    pub(super) fn paint_direct2d(&mut self, hwnd: HWND, window_dc: HDC, layout: &Layout, damage: RECT) -> bool {
        if self.page_grid_open() || self.visible.is_none() {
            self.direct2d = None;
            return false;
        }
        if self.direct2d_unavailable
            || self.page_surface.as_ref().is_none_or(|page| page.ink_version.is_none())
            || layout.client.right <= 0 || layout.client.bottom <= 0
        {
            return false;
        }
        let (mut renderer, damage) = if let Some(renderer) = self.direct2d.take() {
            (renderer, damage)
        } else {
            match Renderer::new(hwnd, client_size(layout)) {
                Ok(renderer) => (renderer, layout.client),
                Err(error) => {
                    trace(format_args!("Direct2D initialization failed, using GDI: {error}"));
                    self.direct2d_unavailable = true;
                    return false;
                }
            }
        };
        if let Err(error) = renderer.paint(self, hwnd, window_dc, layout, damage) {
            self.direct2d_unavailable = error.code() != windows::Win32::Foundation::D2DERR_RECREATE_TARGET;
            trace(format_args!("Direct2D presentation failed: {error}"));
            repaint(hwnd);
            return false;
        }
        if !renderer.presented {
            trace(format_args!("Direct2D presented ({})", if renderer.hardware { "hardware" } else { "software" }));
            renderer.presented = true;
        }
        self.direct2d = Some(renderer);
        true
    }
}

pub(super) fn trace(message: std::fmt::Arguments<'_>) {
    if let Some(path) = std::env::var_os("SCOREDRAW_RENDER_TRACE")
        && let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path)
    {
        let _ = writeln!(file, "{message}");
    }
}

fn client_size(layout: &Layout) -> D2D_SIZE_U {
    D2D_SIZE_U { width: layout.client.right as u32, height: layout.client.bottom as u32 }
}

fn rectangle(rect: RECT) -> D2D_RECT_F {
    D2D_RECT_F {
        left: rect.left as f32,
        top: rect.top as f32,
        right: rect.right as f32,
        bottom: rect.bottom as f32,
    }
}

fn background() -> D2D1_COLOR_F {
    let color = BACKGROUND.0;
    D2D1_COLOR_F {
        r: (color & 0xff) as f32 / 255.0,
        g: ((color >> 8) & 0xff) as f32 / 255.0,
        b: ((color >> 16) & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

fn read_pixels(surface: &Surface) -> Result<Vec<u8>> {
    let mut pixels = vec![0; surface.width as usize * surface.height as usize * 4];
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: surface.width,
            biHeight: -surface.height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    unsafe {
        SelectObject(surface.dc, surface.previous);
        let rows = GetDIBits(
            surface.dc, surface.bitmap, 0, surface.height as u32,
            Some(pixels.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS,
        );
        SelectObject(surface.dc, surface.bitmap.into());
        if rows != surface.height {
            return Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL));
        }
    }
    Ok(pixels)
}
