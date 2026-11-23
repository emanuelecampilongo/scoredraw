use super::{HWND, STORE_UPDATE_AVAILABLE, SW_SHOWNORMAL, post};
use windows::Services::Store::{StoreContext, StorePackageUpdate};
use windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
use windows::Win32::System::WinRT::{RO_INIT_SINGLETHREADED, RoInitialize, RoUninitialize};
use windows::Win32::UI::Shell::{IInitializeWithWindow, ShellExecuteW};
use windows::core::{Interface, PCWSTR, Result, w};
use windows_collections::IVectorView;
use windows_future::IAsyncOperation;

pub(super) struct Check {
    // Keep the operation and its apartment alive for the lifetime of the window.
    _operation: IAsyncOperation<IVectorView<StorePackageUpdate>>,
    _context: StoreContext,
    _apartment: Apartment,
}

impl Check {
    pub(super) fn start(hwnd: HWND) -> Result<Self> {
        let mut length = 0;
        let identity = unsafe { GetCurrentPackageFullName(&mut length, None) };
        if identity != ERROR_INSUFFICIENT_BUFFER {
            return Err(identity.into());
        }

        unsafe { RoInitialize(RO_INIT_SINGLETHREADED) }?;
        let apartment = Apartment;
        // StoreContext expects the UI thread and an owner window for desktop callers.
        let context = StoreContext::GetDefault()?;
        unsafe { context.cast::<IInitializeWithWindow>()?.Initialize(hwnd) }?;
        let operation = context.GetAppAndOptionalStorePackageUpdatesAsync()?;
        let window = hwnd.0 as isize;
        operation.when(move |updates| {
            report(HWND(window as _), updates.and_then(|items| items.Size()));
        })?;

        Ok(Self {
            _operation: operation,
            _context: context,
            _apartment: apartment,
        })
    }
}

fn report(hwnd: HWND, count: Result<u32>) {
    if count.is_ok_and(|count| count > 0) {
        post(hwnd, STORE_UPDATE_AVAILABLE);
    }
}

struct Apartment;

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

pub(super) fn open_listing(hwnd: HWND) {
    unsafe {
        ShellExecuteW(
            Some(hwnd),
            w!("open"),
            w!("ms-windows-store://pdp/?ProductId=9N5Q5Q5N916X"),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::{Check, report};
    use windows::Win32::Foundation::{APPMODEL_ERROR_NO_PACKAGE, ERROR_NETWORK_UNREACHABLE};

    #[test]
    fn skips_the_store_without_package_identity() {
        assert!(matches!(
            Check::start(HWND::default()),
            Err(error) if error.code() == APPMODEL_ERROR_NO_PACKAGE.to_hresult()
        ));
    }

    #[test]
    fn offers_the_update_after_a_concert_and_dismisses_it_for_the_session() {
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!(""),
                WINDOW_STYLE::default(),
                0,
                0,
                410,
                430,
                None,
                None,
                None,
                None,
            )
        }
        .unwrap();
        let take_result = || {
            let mut message = MSG::default();
            unsafe {
                PeekMessageW(
                    &mut message,
                    Some(hwnd),
                    STORE_UPDATE_AVAILABLE,
                    STORE_UPDATE_AVAILABLE,
                    PM_REMOVE,
                )
                .as_bool()
            }
        };

        report(hwnd, Err(ERROR_NETWORK_UNREACHABLE.into()));
        report(hwnd, Ok(0));
        assert!(!take_result());

        let mut reader = Reader {
            concert: true,
            fullscreen: Some(WINDOWPLACEMENT::default()),
            ..Reader::default()
        };
        report(hwnd, Ok(1));
        assert!(take_result());
        reader.handle(hwnd, STORE_UPDATE_AVAILABLE, WPARAM(0), LPARAM(0));
        assert!(reader.cards(&Layout::new(hwnd, true)).is_empty());

        reader.fullscreen = None;
        let layout = Layout::new(hwnd, false);
        let cards = reader.cards(&layout);
        let [banner] = cards.as_slice() else {
            panic!("expected the update banner after leaving the concert");
        };
        assert_eq!(banner.buttons[0].event, Event::OpenStore);
        let outside = POINT { x: 200, y: 300 };
        assert!(!reader.overlay_input(hwnd, WM_POINTERDOWN, MOUSE, outside));

        let later = banner.buttons[1].rect;
        let point = POINT {
            x: (later.left + later.right) / 2,
            y: (later.top + later.bottom) / 2,
        };
        assert!(reader.overlay_input(hwnd, WM_POINTERDOWN, MOUSE, point));
        assert!(reader.overlay_input(hwnd, WM_POINTERUP, MOUSE, point));
        assert!(reader.cards(&layout).is_empty());
        unsafe { DestroyWindow(hwnd) }.unwrap();
    }
}
