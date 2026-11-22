use std::cell::RefCell;
use std::ffi::c_void;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::UI::Controls::{POINTER_TYPE_INFO, POINTER_TYPE_INFO_0};
use windows::Win32::UI::Input::Pointer::{
    POINTER_FLAG_INCONTACT, POINTER_TOUCH_INFO, SkipPointerFrameMessages,
};
use windows::Win32::UI::InteractionContext::*;
use windows::Win32::UI::WindowsAndMessaging::PT_TOUCHPAD;
use windows::core::{BOOL, HRESULT, PCSTR, w};

type RegisterWindow = unsafe extern "system" fn(HWND, BOOL) -> BOOL;
type UntypedProc = unsafe extern "system" fn() -> isize;
type ReadFrames =
    unsafe extern "system" fn(u32, *mut u32, *mut u32, *mut POINTER_TOUCH_INFO) -> BOOL;
type ProcessFrames =
    unsafe extern "system" fn(HINTERACTIONCONTEXT, u32, u32, *const POINTER_TYPE_INFO) -> HRESULT;

pub(super) struct Trackpad {
    context: HINTERACTIONCONTEXT,
    read_frames: ReadFrames,
    process_frames: ProcessFrames,
    output: Box<RefCell<Vec<INTERACTION_CONTEXT_OUTPUT>>>,
    contacts: Vec<POINTER_TOUCH_INFO>,
    packets: Vec<POINTER_TYPE_INFO>,
    active: bool,
    gesture: Gesture,
}

pub(super) struct Movement {
    pub anchor: POINT,
    pub pages: i32,
    pub pan: f32,
    pub zoom: f32,
}

#[derive(Default, PartialEq, Eq)]
enum Gesture {
    #[default]
    Waiting,
    Turned,
    Pan,
    Zoom,
}

impl Trackpad {
    pub fn register(hwnd: HWND) -> Option<Self> {
        // These exports are optional on older Windows builds. The ordinals are documented
        // by Microsoft and cover systems where the functions have no named export.
        let (register, read_frames, process_frames) = unsafe {
            let user = GetModuleHandleW(w!("user32.dll")).ok()?;
            let input = GetModuleHandleW(w!("ninput.dll")).ok()?;
            (
                std::mem::transmute::<UntypedProc, RegisterWindow>(GetProcAddress(
                    user,
                    PCSTR(2689 as _),
                )?),
                std::mem::transmute::<UntypedProc, ReadFrames>(GetProcAddress(
                    user,
                    PCSTR(2694 as _),
                )?),
                std::mem::transmute::<UntypedProc, ProcessFrames>(GetProcAddress(
                    input,
                    PCSTR(2507 as _),
                )?),
            )
        };
        let trackpad = Self {
            context: unsafe { CreateInteractionContext().ok()? },
            read_frames,
            process_frames,
            output: Box::default(),
            contacts: Vec::new(),
            packets: Vec::new(),
            active: false,
            gesture: Gesture::default(),
        };
        let configuration = INTERACTION_CONTEXT_CONFIGURATION {
            interactionId: INTERACTION_ID_MANIPULATION,
            enable: INTERACTION_CONFIGURATION_FLAG_MANIPULATION
                | INTERACTION_CONFIGURATION_FLAG_MANIPULATION_TRANSLATION_X
                | INTERACTION_CONFIGURATION_FLAG_MANIPULATION_TRANSLATION_Y
                | INTERACTION_CONFIGURATION_FLAG_MANIPULATION_RAILS_X
                | INTERACTION_CONFIGURATION_FLAG_MANIPULATION_RAILS_Y
                | INTERACTION_CONFIGURATION_FLAG_MANIPULATION_SCALING,
        };
        unsafe {
            SetInteractionConfigurationInteractionContext(trackpad.context, &[configuration])
                .ok()?;
            SetPropertyInteractionContext(
                trackpad.context,
                INTERACTION_CONTEXT_PROPERTY_FILTER_POINTERS,
                0,
            )
            .ok()?;
            RegisterOutputCallbackInteractionContext(
                trackpad.context,
                Some(on_gesture),
                Some(std::ptr::from_ref(trackpad.output.as_ref()).cast()),
            )
            .ok()?;
            register(hwnd, BOOL(1)).as_bool().then_some(trackpad)
        }
    }

    pub fn input(
        &mut self,
        id: u32,
        down: bool,
        available: bool,
        zoomed: bool,
        dpi: f32,
    ) -> Option<Movement> {
        if !(self.active || down && available) {
            return None;
        }
        let Some((anchor, contacts_down)) = self.feed(id) else {
            self.cancel();
            return None;
        };
        self.active = contacts_down;
        let mut movement = Movement {
            anchor,
            pages: 0,
            pan: 0.0,
            zoom: 1.0,
        };
        for output in self.output.get_mut().drain(..) {
            self.gesture.accept(output, zoomed, dpi, &mut movement);
        }
        if !contacts_down {
            self.gesture = Gesture::Waiting;
        }
        Some(movement)
    }

    fn feed(&mut self, id: u32) -> Option<(POINT, bool)> {
        let mut frames = 0;
        let mut contacts = 0;
        unsafe { (self.read_frames)(id, &mut frames, &mut contacts, std::ptr::null_mut()) }
            .ok()
            .ok()?;
        let count = (frames as usize).checked_mul(contacts as usize)?;
        if count == 0 {
            return None;
        }
        self.contacts.resize(count, POINTER_TOUCH_INFO::default());
        unsafe { (self.read_frames)(id, &mut frames, &mut contacts, self.contacts.as_mut_ptr()) }
            .ok()
            .ok()?;
        let contacts_in_history = self.contacts.get(..frames as usize * contacts as usize)?;
        let anchor = contacts_in_history.first()?.pointerInfo.ptPixelLocation;
        self.packets.clear();
        self.packets.extend(
            contacts_in_history
                .iter()
                .copied()
                .map(|touch| POINTER_TYPE_INFO {
                    r#type: PT_TOUCHPAD,
                    Anonymous: POINTER_TYPE_INFO_0 { touchInfo: touch },
                }),
        );
        // The first frame is the most recent. Keep ownership until the last finger leaves.
        let contacts_down = contacts_in_history[..contacts as usize]
            .iter()
            .any(|contact| {
                contact
                    .pointerInfo
                    .pointerFlags
                    .contains(POINTER_FLAG_INCONTACT)
            });
        unsafe {
            (self.process_frames)(self.context, frames, contacts, self.packets.as_ptr())
                .ok()
                .ok()?;
            let _ = SkipPointerFrameMessages(id);
        }
        Some((anchor, contacts_down))
    }

    pub fn cancel(&mut self) {
        self.active = false;
        self.gesture = Gesture::Waiting;
        unsafe {
            let _ = StopInteractionContext(self.context);
        }
        self.output.get_mut().clear();
    }
}

impl Drop for Trackpad {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyInteractionContext(self.context);
        }
    }
}

unsafe extern "system" fn on_gesture(
    client: *const c_void,
    output: *const INTERACTION_CONTEXT_OUTPUT,
) {
    let output = unsafe { &*output };
    if output.inputType == PT_TOUCHPAD && output.interactionId == INTERACTION_ID_MANIPULATION {
        unsafe { &*client.cast::<RefCell<Vec<INTERACTION_CONTEXT_OUTPUT>>>() }
            .borrow_mut()
            .push(*output);
    }
}

impl Gesture {
    fn accept(
        &mut self,
        output: INTERACTION_CONTEXT_OUTPUT,
        zoomed: bool,
        dpi: f32,
        movement: &mut Movement,
    ) {
        let flags = output.interactionFlags;
        if flags.contains(INTERACTION_FLAG_BEGIN) {
            *self = Self::Waiting;
        }
        if !flags.contains(INTERACTION_FLAG_INERTIA) && !flags.contains(INTERACTION_FLAG_CANCEL) {
            // The callback only retains manipulation events, which use this union member.
            self.update(
                unsafe { output.arguments.manipulation },
                zoomed,
                dpi,
                movement,
            );
        }
        if flags.contains(INTERACTION_FLAG_END) || flags.contains(INTERACTION_FLAG_CANCEL) {
            *self = Self::Waiting;
        }
    }

    fn update(
        &mut self,
        sample: INTERACTION_ARGUMENTS_MANIPULATION,
        zoomed: bool,
        dpi: f32,
        movement: &mut Movement,
    ) {
        let total = sample.cumulative;
        let delta = sample.delta;
        let mut next = delta.scale;
        if *self == Self::Waiting {
            let (x, y) = (total.translationX, total.translationY);
            if total.scale > 0.0 && (total.scale - 1.0).abs() >= 0.05 {
                *self = Self::Zoom;
                next = total.scale;
            } else if x.abs() >= 26.0 * dpi && x.abs() > y.abs() * 1.15 {
                *self = Self::Turned;
                movement.pages += if x < 0.0 { 1 } else { -1 };
            } else if y.abs() > x.abs() * 1.15 && zoomed && y.abs() >= 10.0 * dpi {
                *self = Self::Pan;
                movement.pan += y - delta.translationY;
            } else if y.abs() > x.abs() * 1.15 && y.abs() >= 26.0 * dpi {
                *self = Self::Turned;
                movement.pages += if y < 0.0 { 1 } else { -1 };
            }
        }
        match self {
            Self::Pan => movement.pan += delta.translationY,
            Self::Zoom if next.is_finite() && next > 0.0 => movement.zoom *= next,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successive_native_swipes_turn_once_each_without_an_idle_period() {
        let mut gesture = Gesture::default();
        let mut result = Movement {
            anchor: POINT::default(),
            pages: 0,
            pan: 0.0,
            zoom: 1.0,
        };
        let mut send = |flags, x| {
            let output = INTERACTION_CONTEXT_OUTPUT {
                interactionFlags: flags,
                arguments: INTERACTION_CONTEXT_OUTPUT_0 {
                    manipulation: INTERACTION_ARGUMENTS_MANIPULATION {
                        cumulative: MANIPULATION_TRANSFORM {
                            translationX: x,
                            scale: 1.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                },
                ..Default::default()
            };
            gesture.accept(output, false, 1.0, &mut result);
        };
        for _ in 0..3 {
            send(INTERACTION_FLAG_BEGIN, -15.0);
            send(INTERACTION_FLAG_NONE, -31.0);
            send(INTERACTION_FLAG_NONE, -110.0);
            send(INTERACTION_FLAG_END, -112.0);
            send(INTERACTION_FLAG_INERTIA, -250.0);
        }
        assert_eq!(result.pages, 3);
    }
}
