use std::cell::RefCell;

use block2::RcBlock;
use objc2::rc::{Retained, Weak};
use objc2::{MainThreadOnly, Message, msg_send};
use objc2_app_kit::*;

use crate::Ui;
use crate::view::NativeView;
use crate::window::{Window, WindowRef};

pub struct Sheet {
    window: Window,
    monitor: RefCell<Option<Retained<objc2::runtime::AnyObject>>>,
}
impl Sheet {
    pub fn new(ui: &Ui, title: &str, content: impl NativeView) -> Self {
        Self {
            window: Window::new(ui).title(title).content(content),
            monitor: RefCell::new(None),
        }
    }

    /// Size a small editor from its Auto Layout content, rather than Window's default frame.
    pub fn fit_content(&self) {
        if let Some(content) = self.window.ns_window().contentView() {
            content.layoutSubtreeIfNeeded();
            self.window.ns_window().setContentSize(content.fittingSize());
        }
    }

    /// Attach to `parent`; returns false when the parent window has already closed.
    pub fn show(&self, parent: &WindowRef) -> bool {
        let Some(parent) = parent.0.load() else {
            return false;
        };
        if self.window.ns_window().sheetParent().is_some() {
            return false;
        }
        parent.beginSheet_completionHandler(self.window.ns_window(), None);
        self.install_dismissal_monitor();
        true
    }

    /// Commit the active field editor; false when a field rejects its current text.
    pub fn end_editing(&self) -> bool { self.window.ns_window().makeFirstResponder(None) }

    fn install_dismissal_monitor(&self) {
        self.remove_dismissal_monitor();
        let window = Weak::new(self.window.ns_window());
        let handler = RcBlock::new(move |event: std::ptr::NonNull<NSEvent>| {
            let event_ref = unsafe { event.as_ref() };
            let Some(sheet) = window.load() else {
                return event.as_ptr();
            };
            let Some(parent) = sheet.sheetParent() else {
                return event.as_ptr();
            };
            let event_window = event_ref.window(sheet.mtm());
            let inside = event_window
                .as_deref()
                .is_some_and(|w| w.windowNumber() == sheet.windowNumber());
            let outside = event_window
                .as_deref()
                .is_some_and(|w| w.windowNumber() == parent.windowNumber());
            let escape = event_ref.r#type() == NSEventType::KeyDown
                && event_ref.keyCode() == 53
                && !event_ref.modifierFlags().intersects(
                    NSEventModifierFlags::Command
                        | NSEventModifierFlags::Control
                        | NSEventModifierFlags::Option,
                );
            if !((escape && (inside || outside))
                || (outside && event_ref.r#type() == NSEventType::LeftMouseDown))
            {
                return event.as_ptr();
            }
            let cancel =
                sheet.contentView().and_then(|view| sheet_button(&view, "cgs.sheet-cancel"));
            if !escape && cancel.as_ref().is_some_and(|button| !button.isHidden()) {
                if let Some(button) =
                    sheet.contentView().and_then(|view| sheet_button(&view, "cgs.sheet-primary"))
                {
                    pulse_sheet_action(&button);
                }
            } else if let Some(cancel) = cancel {
                // Dispatch the action directly: a hidden Cancel must still respond to Escape.
                if let Some(action) = cancel.action() {
                    unsafe {
                        NSApplication::sharedApplication(sheet.mtm()).sendAction_to_from(
                            action,
                            cancel.target().as_deref(),
                            Some(&*cancel),
                        );
                    }
                }
            } else {
                parent.endSheet(&sheet);
            }
            std::ptr::null_mut()
        });
        *self.monitor.borrow_mut() = unsafe {
            NSEvent::addLocalMonitorForEventsMatchingMask_handler(
                NSEventMask::KeyDown | NSEventMask::LeftMouseDown,
                &handler,
            )
        };
    }

    fn remove_dismissal_monitor(&self) {
        if let Some(monitor) = self.monitor.borrow_mut().take() {
            unsafe {
                NSEvent::removeMonitor(&monitor);
            }
        }
    }

    pub fn end(&self) {
        self.remove_dismissal_monitor();
        if let Some(parent) = self.window.ns_window().sheetParent() {
            parent.endSheet(self.window.ns_window());
        }
        self.window.hide();
    }

    pub fn ns_window(&self) -> &NSWindow { self.window.ns_window() }
}
impl Drop for Sheet {
    fn drop(&mut self) { self.end(); }
}

fn sheet_button(view: &NSView, identifier: &str) -> Option<Retained<NSButton>> {
    if view.identifier().is_some_and(|value| value.to_string() == identifier) {
        return view.downcast_ref::<NSButton>().map(|button| button.retain());
    }
    view.subviews().iter().find_map(|child| sheet_button(&child, identifier))
}

fn pulse_sheet_action(button: &NSButton) {
    if NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion() {
        return;
    }
    // A brief native opacity pulse draws attention without activating or saving.
    let weak = Weak::new(button);
    let fade = RcBlock::new(move |context: std::ptr::NonNull<NSAnimationContext>| {
        unsafe { context.as_ref() }.setDuration(0.12);
        let animator: Retained<NSButton> = unsafe { msg_send![button, animator] };
        animator.setAlphaValue(0.55);
    });
    let restore = RcBlock::new(move || {
        if let Some(button) = weak.load() {
            let changes = RcBlock::new(move |context: std::ptr::NonNull<NSAnimationContext>| {
                unsafe { context.as_ref() }.setDuration(0.12);
                let animator: Retained<NSButton> = unsafe { msg_send![&*button, animator] };
                animator.setAlphaValue(1.0);
            });
            NSAnimationContext::runAnimationGroup(&changes);
        }
    });
    NSAnimationContext::runAnimationGroup_completionHandler(&fade, Some(&restore));
}
