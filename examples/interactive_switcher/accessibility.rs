//! Exact-window activation through macOS Accessibility.
//!
//! `_AXUIElementGetWindow` is a private ApplicationServices API; applications
//! which do not expose their windows through AX cannot be activated by this module.

use std::{ffi::c_void, ptr::NonNull};

use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
use objc2_core_foundation::{
    CFDictionary, CFRetained, CFString, CFType, kCFBooleanFalse, kCFBooleanTrue,
};

type AXElement = *const c_void;
type AXError = i32;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
    static kAXTrustedCheckOptionPrompt: *const CFString;
    fn AXUIElementCreateApplication(pid: i32) -> *const CFType;
    fn AXUIElementGetTypeID() -> usize;
    fn AXUIElementCopyAttributeValue(
        element: AXElement,
        attribute: *const CFString,
        value: *mut *const CFType,
    ) -> AXError;
    fn AXUIElementSetAttributeValue(
        element: AXElement,
        attribute: *const CFString,
        value: *const c_void,
    ) -> AXError;
    fn AXUIElementIsAttributeSettable(
        element: AXElement,
        attribute: *const CFString,
        settable: *mut bool,
    ) -> AXError;
    fn AXUIElementPerformAction(element: AXElement, action: *const CFString) -> AXError;
    fn _AXUIElementGetWindow(element: AXElement, window_id: *mut u32) -> AXError;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFGetTypeID(value: *const c_void) -> usize;
    fn CFArrayGetTypeID() -> usize;
    fn CFArrayGetCount(array: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
}

/// Check Accessibility permission, optionally asking macOS to show its prompt.
/// Grant permission to the host executable in System Settings > Privacy &
/// Security > Accessibility. Prompting is asynchronous; a false result is normal.
pub fn trusted(prompt: bool) -> bool {
    unsafe {
        if !prompt {
            return AXIsProcessTrusted();
        }
        let Some(value) = kCFBooleanTrue else {
            return AXIsProcessTrusted();
        };
        let Some(key) = kAXTrustedCheckOptionPrompt.as_ref() else {
            return AXIsProcessTrusted();
        };
        let options = CFDictionary::from_slices(&[key], &[value]);
        AXIsProcessTrustedWithOptions(CFRetained::as_ptr(&options).as_ptr().cast())
    }
}

fn ax_result(code: AXError, operation: &str) -> Result<(), String> {
    if code == 0 {
        return Ok(());
    }
    let hint = match code {
        -25201 => "invalid AX argument",
        -25202 => "invalid AX element; the window or application may have closed",
        -25204 => "the application is not responding to Accessibility requests; retry",
        -25205 => "the application does not expose this AX attribute",
        -25206 => "the application does not support this AX action",
        -25208 => "AX operation is not implemented by this application",
        -25211 => "enable this executable in System Settings > Privacy & Security > Accessibility",
        -25212 => "the application returned no AX value; refresh the window list",
        _ => "refresh the window list and retry; the application may not support Accessibility",
    };
    Err(format!("{operation}: AX error {code}: {hint}"))
}

fn pointer(value: &CFRetained<CFType>) -> AXElement {
    CFRetained::as_ptr(value).as_ptr().cast()
}

// Only call for Create/Copy results, transferring their +1 ownership to RAII.
unsafe fn owned(value: *const CFType, operation: &str) -> Result<CFRetained<CFType>, String> {
    let value = NonNull::new(value.cast_mut())
        .ok_or_else(|| format!("{operation}: macOS returned a null AX value"))?;
    Ok(unsafe { CFRetained::from_raw(value) })
}

fn set(element: AXElement, attribute: &str, value: *const c_void) -> Result<(), String> {
    let name = CFString::from_str(attribute);
    ax_result(
        unsafe { AXUIElementSetAttributeValue(element, CFRetained::as_ptr(&name).as_ptr(), value) },
        &format!("set {attribute}"),
    )
}

fn set_if_writable(
    element: AXElement,
    attribute: &str,
    value: *const c_void,
) -> Result<(), String> {
    let name = CFString::from_str(attribute);
    let mut writable = false;
    let code = unsafe {
        AXUIElementIsAttributeSettable(element, CFRetained::as_ptr(&name).as_ptr(), &mut writable)
    };
    // Many applications omit these optional focus attributes entirely.
    if code == -25205 || code == -25208 {
        return Ok(());
    }
    ax_result(code, &format!("check whether {attribute} is writable"))?;
    if writable {
        set(element, attribute, value)?;
    }
    Ok(())
}

/// Unminimize, raise and focus precisely `window_id` belonging to `pid`.
/// Never falls back to activating the application when no exact AX window exists.
/// Success means macOS accepted the requests, not that asynchronous focus has finished.
pub fn activate(window_id: u32, pid: i32) -> Result<(), String> {
    if window_id == 0 || pid <= 0 {
        return Err(format!(
            "Invalid window ID {window_id} or PID {pid}; refresh the window list"
        ));
    }
    if !trusted(false) {
        return Err("Accessibility permission is required: enable this executable in System Settings > Privacy & Security > Accessibility, then retry".into());
    }
    let running = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
        .ok_or_else(|| format!("Application PID {pid} has exited; refresh the window list"))?;
    let app = unsafe { owned(AXUIElementCreateApplication(pid), "create AX application")? };
    let attribute = CFString::from_str("AXWindows");
    let mut raw = std::ptr::null();
    let code = unsafe {
        AXUIElementCopyAttributeValue(
            pointer(&app),
            CFRetained::as_ptr(&attribute).as_ptr(),
            &mut raw,
        )
    };
    // Adopt even an unexpected non-null error result so it cannot leak.
    let windows = NonNull::new(raw.cast_mut()).map(|p| unsafe { CFRetained::from_raw(p) });
    ax_result(code, &format!("read AXWindows for PID {pid}"))?;
    let windows = windows
        .ok_or_else(|| format!("PID {pid} returned no AXWindows; refresh the window list"))?;
    if unsafe { CFGetTypeID(pointer(&windows)) != CFArrayGetTypeID() } {
        return Err(format!("PID {pid} returned a non-array AXWindows value"));
    }
    let mut matched = None;
    let mut last_error = None;
    // Array members are borrowed and remain alive while `windows` is retained.
    for index in 0..unsafe { CFArrayGetCount(pointer(&windows)) } {
        let window = unsafe { CFArrayGetValueAtIndex(pointer(&windows), index) };
        if window.is_null() || unsafe { CFGetTypeID(window) != AXUIElementGetTypeID() } {
            continue;
        }
        let mut candidate = 0;
        let code = unsafe { _AXUIElementGetWindow(window, &mut candidate) };
        if code == 0 && candidate == window_id {
            matched = Some(window);
            break;
        }
        if code != 0 {
            last_error = Some(code);
        }
    }
    let window = matched.ok_or_else(|| {
        let detail = last_error.map(|code| format!(" (window-ID lookup AX error {code})")).unwrap_or_default();
        format!("No exact AX window for ID {window_id} in PID {pid}{detail}; it may have closed or may not be exposed through Accessibility. Refresh the window list; application-only activation was not attempted")
    })?;
    let (yes, no) = unsafe { (kCFBooleanTrue, kCFBooleanFalse) };
    let yes =
        yes.ok_or("CoreFoundation did not provide kCFBooleanTrue")? as *const _ as *const c_void;
    let no =
        no.ok_or("CoreFoundation did not provide kCFBooleanFalse")? as *const _ as *const c_void;
    set(window, "AXMinimized", no)?;
    set_if_writable(window, "AXMain", yes)?;
    set_if_writable(window, "AXFocused", yes)?;
    set_if_writable(pointer(&app), "AXFocusedWindow", window)?;
    let raise = CFString::from_str("AXRaise");
    ax_result(
        unsafe { AXUIElementPerformAction(window, CFRetained::as_ptr(&raise).as_ptr()) },
        &format!("raise window {window_id} in PID {pid}"),
    )?;
    // Do not use ActivateAllWindows: only the matched window should be raised.
    if !running.activateWithOptions(NSApplicationActivationOptions::empty()) {
        return Err(format!(
            "macOS rejected activation of PID {pid} after raising window {window_id}; check that the application is still running and retry"
        ));
    }
    Ok(())
}
