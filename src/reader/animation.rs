use super::{HWND, LPARAM, WPARAM};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::time::{Duration, Instant};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const ANIMATION_FRAME: u32 = WM_APP + 11;
const FRAME_PERIOD: Duration = Duration::from_micros(16_667);

pub struct Animation {
    wake: SyncSender<()>,
    active: Arc<AtomicBool>,
    pending: Arc<AtomicBool>,
}

impl Animation {
    pub fn new(hwnd: HWND) -> std::io::Result<Self> {
        let (wake, idle) = sync_channel(1);
        let active = Arc::new(AtomicBool::new(false));
        let pending = Arc::new(AtomicBool::new(false));
        let running = active.clone();
        let queued = pending.clone();
        let window = hwnd.0 as isize;
        std::thread::Builder::new()
            .name("animazioni".into())
            .spawn(move || {
                while idle.recv().is_ok() {
                    let mut deadline = Instant::now();
                    while running.load(Ordering::Acquire) {
                        deadline += FRAME_PERIOD;
                        std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                        if running.load(Ordering::Acquire)
                            && !queued.swap(true, Ordering::AcqRel)
                            && unsafe {
                                PostMessageW(
                                    Some(HWND(window as _)),
                                    ANIMATION_FRAME,
                                    WPARAM(0),
                                    LPARAM(0),
                                )
                            }
                            .is_err()
                        {
                            running.store(false, Ordering::Release);
                            queued.store(false, Ordering::Release);
                        }
                        let now = Instant::now();
                        if now > deadline + FRAME_PERIOD {
                            deadline = now;
                        }
                    }
                }
            })?;
        Ok(Self {
            wake,
            active,
            pending,
        })
    }

    pub fn start(&self) {
        if !self.active.swap(true, Ordering::AcqRel) {
            let _ = self.wake.try_send(());
        }
    }

    pub fn read(&self) {
        self.pending.store(false, Ordering::Release);
    }

    pub fn stop(&self) {
        self.active.store(false, Ordering::Release);
    }
}

impl Drop for Animation {
    fn drop(&mut self) {
        self.stop();
    }
}
