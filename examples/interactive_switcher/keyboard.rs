use std::{
    collections::HashSet,
    error::Error,
    ffi::c_void,
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
};

use super::Input;
use cgs::compositor::switcher::SwitcherCommand;

type Ref = *mut c_void;
type Callback = unsafe extern "C" fn(Ref, u32, Ref, Ref) -> Ref;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapCreate(
        location: u32,
        placement: u32,
        options: u32,
        mask: u64,
        callback: Callback,
        user_info: Ref,
    ) -> Ref;
    fn CGEventTapEnable(tap: Ref, enable: bool);
    fn CGEventGetIntegerValueField(event: Ref, field: u32) -> i64;
    fn CGEventGetFlags(event: Ref) -> u64;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopCommonModes: Ref;
    fn CFMachPortCreateRunLoopSource(allocator: Ref, port: Ref, order: isize) -> Ref;
    fn CFMachPortInvalidate(port: Ref);
    fn CFRunLoopGetMain() -> Ref;
    fn CFRunLoopAddSource(run_loop: Ref, source: Ref, mode: Ref);
    fn CFRunLoopRemoveSource(run_loop: Ref, source: Ref, mode: Ref);
    fn CFRunLoopSourceInvalidate(source: Ref);
    fn CFRelease(value: Ref);
}

const KEY_DOWN: u32 = 10;
const KEY_UP: u32 = 11;
const SHIFT: u64 = 1 << 17;
const ALTERNATE: u64 = 1 << 19;

struct Context {
    tx: Sender<Input>,
    active: Arc<AtomicBool>,
    tap: Ref,
    suppressed: HashSet<i64>,
    opening_space: bool,
}

pub struct Keyboard {
    source: Ref,
    context: Box<Context>,
    // Registration and teardown must both happen on the main thread.
    _main_thread: objc2::MainThreadMarker,
}

impl Keyboard {
    pub fn new(tx: Sender<Input>, active: Arc<AtomicBool>) -> Result<Self, Box<dyn Error>> {
        let main_thread = objc2::MainThreadMarker::new()
            .ok_or("Keyboard must be installed on the main thread")?;
        let mut context = Box::new(Context {
            tx,
            active,
            tap: ptr::null_mut(),
            suppressed: HashSet::new(),
            opening_space: false,
        });
        unsafe {
            // Session tap, head insertion, default (suppressing, not listen-only) options.
            let tap = CGEventTapCreate(
                1,
                0,
                0,
                (1 << KEY_DOWN) | (1 << KEY_UP),
                callback,
                (&mut *context as *mut Context).cast(),
            );
            if tap.is_null() {
                return Err("Cannot create keyboard event tap; grant Accessibility and Input Monitoring permission in System Settings, then restart".into());
            }
            context.tap = tap;
            let source = CFMachPortCreateRunLoopSource(ptr::null_mut(), tap, 0);
            if source.is_null() {
                CFMachPortInvalidate(tap);
                CFRelease(tap);
                return Err("Cannot create keyboard event tap run-loop source".into());
            }
            CFRunLoopAddSource(CFRunLoopGetMain(), source, kCFRunLoopCommonModes);
            CGEventTapEnable(tap, true);
            Ok(Self {
                source,
                context,
                _main_thread: main_thread,
            })
        }
    }
}

impl Drop for Keyboard {
    fn drop(&mut self) {
        unsafe {
            CGEventTapEnable(self.context.tap, false);
            CFRunLoopRemoveSource(CFRunLoopGetMain(), self.source, kCFRunLoopCommonModes);
            CFRunLoopSourceInvalidate(self.source);
            CFMachPortInvalidate(self.context.tap);
            CFRelease(self.source);
            CFRelease(self.context.tap);
        }
    }
}

unsafe extern "C" fn callback(_proxy: Ref, kind: u32, event: Ref, user_info: Ref) -> Ref {
    // The boxed context outlives the source and is only accessed on the main run loop.
    let context = unsafe { &mut *user_info.cast::<Context>() };
    if kind == u32::MAX || kind == u32::MAX - 1 {
        unsafe { CGEventTapEnable(context.tap, true) };
        return event;
    }
    if event.is_null() || (kind != KEY_DOWN && kind != KEY_UP) {
        return event;
    }
    let key = unsafe { CGEventGetIntegerValueField(event, 9) }; // kCGKeyboardEventKeycode
    if kind == KEY_UP {
        let suppressed = context.suppressed.remove(&key);
        if key == 49 {
            context.opening_space = false;
        }
        return if suppressed { ptr::null_mut() } else { event };
    }
    if key == 49 && context.opening_space {
        return ptr::null_mut();
    }
    let flags = unsafe { CGEventGetFlags(event) };
    let input = if !context.active.load(Ordering::Acquire) {
        if key == 49 && flags & ALTERNATE != 0 {
            context.opening_space = true;
            Some(Input::Open)
        } else {
            None
        }
    } else {
        let command = match key {
            123 | 126 => Some(SwitcherCommand::Step(-1)),
            124 | 125 => Some(SwitcherCommand::Step(1)),
            48 => Some(SwitcherCommand::Step(if flags & SHIFT != 0 {
                -1
            } else {
                1
            })),
            37 => Some(SwitcherCommand::Layout(flags & SHIFT == 0)),
            36 | 76 | 49 => Some(SwitcherCommand::Commit { release: false }),
            53 => Some(SwitcherCommand::Dismiss),
            _ => None,
        };
        command.map(Input::Command)
    };
    if let Some(input) = input {
        if context.tx.send(input).is_ok() {
            context.suppressed.insert(key);
            return ptr::null_mut();
        }
        if key == 49 {
            context.opening_space = false;
        }
    }
    // A held key can repeat after a commit closes the switcher. Its entire
    // remaining press must stay hidden, including the eventual key-up.
    if context.suppressed.contains(&key) {
        ptr::null_mut()
    } else {
        event
    }
}
