use std::cell::{Cell, OnceCell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, DefinedClass, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::*;
use objc2_foundation::NSString;

use crate::Ui;
use crate::bridge::callback;
use crate::text::Label;
use crate::view::{NativeControl, NativeView};

pub type Modifiers = NSEventModifierFlags;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub code: u16,
    pub display: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyShortcut {
    pub modifiers: Modifiers,
    pub key: Key,
}
fn shortcut_modifiers(flags: Modifiers) -> Modifiers {
    flags & (Modifiers::Control | Modifiers::Option | Modifiers::Shift | Modifiers::Command)
}
pub fn modifier_glyphs(flags: Modifiers) -> String {
    let mut out = String::new();
    for (flag, glyph) in [
        (Modifiers::Control, '⌃'),
        (Modifiers::Option, '⌥'),
        (Modifiers::Shift, '⇧'),
        (Modifiers::Command, '⌘'),
    ] {
        if flags.contains(flag) {
            out.push(glyph);
        }
    }
    out
}
impl std::fmt::Display for KeyShortcut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", modifier_glyphs(self.modifiers), self.key.display)
    }
}
fn key_display(event: &NSEvent) -> String {
    match event.keyCode() {
        122 => "F1".into(),
        120 => "F2".into(),
        99 => "F3".into(),
        118 => "F4".into(),
        96 => "F5".into(),
        97 => "F6".into(),
        98 => "F7".into(),
        100 => "F8".into(),
        101 => "F9".into(),
        109 => "F10".into(),
        103 => "F11".into(),
        111 => "F12".into(),
        105 => "F13".into(),
        107 => "F14".into(),
        113 => "F15".into(),
        106 => "F16".into(),
        64 => "F17".into(),
        79 => "F18".into(),
        80 => "F19".into(),
        90 => "F20".into(),
        115 => "↖".into(),
        119 => "↘".into(),
        116 => "⇞".into(),
        121 => "⇟".into(),
        36 => "↩".into(),
        48 => "⇥".into(),
        49 => "Space".into(),
        51 => "⌫".into(),
        53 => "⎋".into(),
        76 => "⌤".into(),
        117 => "⌦".into(),
        123 => "←".into(),
        124 => "→".into(),
        125 => "↓".into(),
        126 => "↑".into(),
        _ => event
            .charactersIgnoringModifiers()
            .map(|v| v.to_string().to_uppercase())
            .unwrap_or_default(),
    }
}
type ShortcutCallback = Box<dyn FnMut(Option<KeyShortcut>)>;
type ConflictCallback = Box<dyn FnMut(&KeyShortcut) -> Option<String>>;
struct RecorderState {
    recording: Cell<bool>,
    hovered: Cell<bool>,
    surface: RefCell<Option<objc2::rc::Weak<NSBox>>>,
    tracking: OnceCell<Retained<NSTrackingArea>>,
    actions: RefCell<Option<objc2::rc::Weak<NSView>>>,
    modifiers_only: bool,
    modifiers: Cell<Modifiers>,
    value: RefCell<Option<KeyShortcut>>,
    changed: RefCell<Option<ShortcutCallback>>,
    conflict: RefCell<Option<ConflictCallback>>,
    cancelled: RefCell<Option<Box<dyn FnMut()>>>,
    capture: RefCell<Option<Box<dyn FnMut(bool) -> Result<(), String>>>>,
}
impl Drop for RecorderState {
    fn drop(&mut self) {
        if self.recording.get() {
            if let Some(capture) = self.capture.get_mut().as_mut() {
                let _ = capture(false);
            }
        }
    }
}
define_class!(
    #[unsafe(super(NSButton))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgUiKeyRecorder"]
    #[ivars = RecorderState]
    struct RecorderButton;
    impl RecorderButton {
        #[unsafe(method(viewDidMoveToWindow))]
        fn moved_to_window(&self) {
            unsafe { let _: () = msg_send![super(self), viewDidMoveToWindow]; }
            if self.ivars().recording.get() || self.window().is_none() { self.cancel(); }
        }
        #[unsafe(method(cancelRecording:))]
        fn cancel_action(&self, _: &AnyObject) { self.abort(); }
        #[unsafe(method(clearRecordedShortcut:))]
        fn clear_action(&self, _: &AnyObject) { self.accept(None); }
        #[unsafe(method(recordingWindowResigned:))]
        fn window_resigned(&self, _: &objc2_foundation::NSNotification) {
            self.ivars().hovered.set(false);
            self.abort();
        }
        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking(&self) {
            unsafe { let _: () = msg_send![super(self), updateTrackingAreas]; }
            self.ivars().tracking.get_or_init(|| {
                let area = unsafe { NSTrackingArea::initWithRect_options_owner_userInfo(
                    NSTrackingArea::alloc(), crate::CGRect::ZERO,
                    NSTrackingAreaOptions::MouseEnteredAndExited | NSTrackingAreaOptions::ActiveInKeyWindow | NSTrackingAreaOptions::InVisibleRect,
                    Some(self), None,
                ) };
                self.addTrackingArea(&area);
                area
            });
        }
        #[unsafe(method(mouseEntered:))]
        fn entered(&self, _: &NSEvent) {
            self.ivars().hovered.set(true);
            self.refresh_surface();
        }
        #[unsafe(method(mouseExited:))]
        fn exited(&self, _: &NSEvent) {
            self.ivars().hovered.set(false);
            self.refresh_surface();
        }
        #[unsafe(method(viewDidHide))]
        fn hidden(&self) {
            unsafe { let _: () = msg_send![super(self), viewDidHide]; }
            self.ivars().hovered.set(false);
            self.abort();
        }
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self)->bool{true}
        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self)->bool{self.cancel();unsafe{msg_send![super(self),resignFirstResponder]}}
        #[unsafe(method(beginRecording:))]
        fn begin_action(&self,_sender:&AnyObject){self.begin();}
        #[unsafe(method(performKeyEquivalent:))]
        fn key_equivalent(&self,event:&NSEvent)->bool{
            if self.ivars().recording.get(){self.record(event);true}else{unsafe{msg_send![super(self),performKeyEquivalent:event]}}
        }
        #[unsafe(method(keyDown:))]
        fn key_down(&self,event:&NSEvent){
            if self.ivars().recording.get(){self.record(event);}else{unsafe{let _:()=msg_send![super(self),keyDown:event];}}
        }
        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self,event:&NSEvent){
            if self.ivars().recording.get() {
                let flags = shortcut_modifiers(event.modifierFlags());
                if !self.ivars().modifiers_only || !flags.is_empty() { self.ivars().modifiers.set(flags); }
                let flags = self.ivars().modifiers.get();
                let text = if flags.is_empty() { "Press keys…".into() } else { modifier_glyphs(flags) };
                self.setTitle(&NSString::from_str(&text));
            }else{unsafe{let _:()=msg_send![super(self),flagsChanged:event];}}
        }
    }
);
impl RecorderButton {
    fn record(&self, event: &NSEvent) {
        if event.isARepeat() {
            return;
        }
        match event.keyCode() {
            53 => self.abort(),
            51 | 117 if shortcut_modifiers(event.modifierFlags()).is_empty() => self.accept(None),
            _ => {
                let modifiers = if self.ivars().modifiers_only {
                    self.ivars().modifiers.get()
                } else {
                    shortcut_modifiers(event.modifierFlags())
                };
                let key = if self.ivars().modifiers_only {
                    if event.keyCode() != 36 {
                        return;
                    }
                    Key {
                        code: 0,
                        display: String::new(),
                    }
                } else {
                    Key {
                        code: event.keyCode(),
                        display: key_display(event),
                    }
                };
                self.accept(Some(KeyShortcut { modifiers, key }));
            }
        }
    }

    fn new(ui: &Ui, modifiers_only: bool) -> Retained<Self> {
        let this = Self::alloc(ui.mtm()).set_ivars(RecorderState {
            recording: Cell::new(false),
            hovered: Cell::new(false),
            surface: RefCell::new(None),
            tracking: OnceCell::new(),
            actions: RefCell::new(None),
            modifiers_only,
            modifiers: Cell::new(Modifiers::empty()),
            value: RefCell::new(None),
            changed: RefCell::new(None),
            conflict: RefCell::new(None),
            cancelled: RefCell::new(None),
            capture: RefCell::new(None),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        this.setBezelStyle(crate::controls::action_button_bezel());
        unsafe {
            this.setTarget(Some(&this));
            this.setAction(Some(sel!(beginRecording:)));
        }
        this.setAccessibilityLabel(Some(&NSString::from_str(if modifiers_only {
            "Record modifiers"
        } else {
            "Record shortcut"
        })));
        this.refresh();
        this
    }

    // Recording actions are not part of the idle field's allocation cost.
    fn ensure_actions(&self) {
        let Some(actions) = self.ivars().actions.borrow().as_ref().and_then(|view| view.load())
        else {
            return;
        };
        let Some(stack) = actions.downcast_ref::<NSStackView>() else {
            return;
        };
        if !stack.arrangedSubviews().is_empty() {
            return;
        }
        for (symbol, label, help, action, color) in [
            (
                "arrow.uturn.forward.circle.fill",
                "Cancel recording and restore previous shortcut",
                "Restore previous shortcut (Escape)",
                sel!(cancelRecording:),
                NSColor::systemGreenColor(),
            ),
            (
                "xmark.circle.fill",
                "Clear shortcut",
                "Remove this binding (Delete)",
                sel!(clearRecordedShortcut:),
                NSColor::secondaryLabelColor(),
            ),
        ] {
            let button = NSButton::new(self.mtm());
            button.setBordered(false);
            button.setImage(crate::image::Symbol::named(symbol).as_deref());
            button.setImagePosition(NSCellImagePosition::ImageOnly);
            button.setContentTintColor(Some(&color));
            button.setAccessibilityLabel(Some(&NSString::from_str(label)));
            button.setToolTip(Some(&NSString::from_str(help)));
            button.setTranslatesAutoresizingMaskIntoConstraints(false);
            button.widthAnchor().constraintEqualToConstant(16.0).setActive(true);
            unsafe {
                button.setTarget(Some(self));
                button.setAction(Some(action));
            }
            stack.addArrangedSubview(&button);
        }
    }

    fn begin(&self) {
        if self.ivars().recording.get() {
            return;
        }
        // Resign the previous recorder before acquiring this recorder's lease.
        if let Some(window) = self.window() {
            window.makeFirstResponder(Some(self));
        }
        if let Some(capture) = self.ivars().capture.borrow_mut().as_mut() {
            if let Err(message) = capture(true) {
                self.setToolTip(Some(&NSString::from_str(&message)));
                self.setAccessibilityHelp(Some(&NSString::from_str(&message)));
                return;
            }
        }
        if let Some(window) = self.window() {
            let center = objc2_foundation::NSNotificationCenter::defaultCenter();
            unsafe {
                center.removeObserver(self);
                center.addObserver_selector_name_object(
                    self,
                    sel!(recordingWindowResigned:),
                    Some(NSWindowDidResignKeyNotification),
                    Some(&window),
                );
            }
        }
        self.ensure_actions();
        self.ivars().recording.set(true);
        self.ivars().modifiers.set(Modifiers::empty());
        self.setTitle(&NSString::from_str(if self.ivars().modifiers_only {
            "Press modifiers, then Return"
        } else {
            "Press keys…"
        }));
        self.setToolTip(Some(&NSString::from_str(
            "Press the new keys. Escape cancels; Delete removes the shortcut.",
        )));
        self.setAccessibilityHelp(Some(&NSString::from_str(
            "Recording shortcut. Escape cancels; Delete removes the shortcut.",
        )));
        self.refresh_surface();
        if let Some(w) = self.window() {
            w.makeFirstResponder(Some(self));
        }
    }

    fn abort(&self) {
        if !self.ivars().recording.get() {
            return;
        }
        let _keep_alive = self.retain();
        self.cancel();
        if let Ok(mut cancelled) = self.ivars().cancelled.try_borrow_mut() {
            if let Some(cancelled) = cancelled.as_mut() {
                callback(cancelled);
            }
        }
    }

    fn cancel(&self) {
        if self.ivars().recording.replace(false) {
            unsafe {
                objc2_foundation::NSNotificationCenter::defaultCenter().removeObserver(self);
            }
            if let Some(capture) = self.ivars().capture.borrow_mut().as_mut() {
                let _ = capture(false);
            }
        }
        self.setToolTip(None);
        self.setAccessibilityHelp(None);
        self.refresh();
    }

    fn refresh_surface(&self) {
        let Some(surface) =
            self.ivars().surface.borrow().as_ref().and_then(|surface| surface.load())
        else {
            return;
        };
        let bound = self.ivars().value.borrow().is_some();
        let recording = self.ivars().recording.get();
        let hovered = self.ivars().hovered.get();
        if let Some(actions) = self.ivars().actions.borrow().as_ref().and_then(|view| view.load()) {
            actions.setHidden(!recording);
        }
        self.setAlignment(if recording {
            NSTextAlignment::Left
        } else {
            NSTextAlignment::Center
        });
        let base = if bound {
            NSColor::quaternaryLabelColor()
        } else {
            NSColor::tertiarySystemFillColor()
        };
        let fill = if recording {
            NSColor::controlBackgroundColor()
        } else if hovered {
            base.blendedColorWithFraction_ofColor(0.10, &NSColor::labelColor())
                .unwrap_or(base)
        } else {
            base
        };
        surface.setFillColor(&fill);
        surface.setBorderColor(&NSColor::controlAccentColor().colorWithAlphaComponent(0.65));
        surface.setBorderWidth(if recording { 3.0 } else { 0.0 });
        self.setContentTintColor(Some(&*if bound || hovered || recording {
            NSColor::labelColor()
        } else {
            NSColor::secondaryLabelColor()
        }));
    }

    fn refresh(&self) {
        let value = self
            .ivars()
            .value
            .borrow()
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "Record Shortcut".into());
        self.setTitle(&NSString::from_str(&value));
        let help = if self.ivars().modifiers_only {
            "Click and press modifiers, then Return. Escape cancels."
        } else if self.ivars().value.borrow().is_some() {
            "Click to change this shortcut. Escape cancels; Delete while recording removes it."
        } else {
            "Click to record the keys for this command. Escape cancels."
        };
        self.setToolTip(Some(&NSString::from_str(help)));
        self.setAccessibilityHelp(Some(&NSString::from_str(help)));
        self.refresh_surface();
        unsafe {
            self.setAccessibilityValue(Some(&NSString::from_str(&value)));
        }
    }

    fn accept(&self, value: Option<KeyShortcut>) {
        let _keep_alive = self.retain();
        if let Some(shortcut) = &value {
            let mut conflict = None;
            if let Ok(mut f) = self.ivars().conflict.try_borrow_mut() {
                if let Some(f) = f.as_mut() {
                    callback(|| conflict = f(shortcut));
                }
            }
            if let Some(message) = conflict {
                self.setToolTip(Some(&NSString::from_str(&message)));
                self.setAccessibilityHelp(Some(&NSString::from_str(&message)));
                return;
            }
        }
        *self.ivars().value.borrow_mut() = value.clone();
        self.cancel();
        if let Ok(mut f) = self.ivars().changed.try_borrow_mut() {
            if let Some(f) = f.as_mut() {
                callback(|| f(value));
            }
        }
    }
}
pub struct KeyRecorder {
    native: Retained<RecorderButton>,
}
impl KeyRecorder {
    pub fn new(ui: &Ui) -> Self {
        Self {
            native: RecorderButton::new(ui, false),
        }
    }

    pub fn set_value(&self, value: Option<KeyShortcut>) {
        *self.native.ivars().value.borrow_mut() = value;
        self.native.refresh();
    }

    /// A quiet shortcut field, with hover feedback and an accent outline while recording.
    pub fn flat_field(self: &std::rc::Rc<Self>, ui: &Ui) -> crate::layout::GroupBox {
        self.native.setBordered(false);
        let actions = crate::layout::HStack::new(ui).spacing(2.0);
        *self.native.ivars().actions.borrow_mut() = Some(objc2::rc::Weak::new(actions.ns_view()));
        actions.set_hidden(true);
        self.hugging_priority(1.0, NSLayoutConstraintOrientation::Horizontal);
        let content = crate::layout::HStack::new(ui)
            .spacing(4.0)
            .insets(crate::Insets {
                left: 8.0,
                right: 8.0,
                top: 0.0,
                bottom: 0.0,
            })
            .push(self.clone())
            .push(actions);
        let field =
            crate::layout::GroupBox::with_insets(ui, content, crate::layout::uniform_insets(0.0));
        if let Some(surface) = field.ns_view().downcast_ref::<NSBox>() {
            surface.setCornerRadius(6.0);
            *self.native.ivars().surface.borrow_mut() = Some(objc2::rc::Weak::new(surface));
        }
        self.native.refresh_surface();
        field
    }

    pub fn get_value(&self) -> Option<KeyShortcut> { self.native.ivars().value.borrow().clone() }

    /// Show text in place of the recorded value, such as a shortcut it cannot represent.
    pub fn set_title(&self, title: &str) { self.native.setTitle(&NSString::from_str(title)); }

    pub fn begin_recording(&self) { self.native.begin(); }

    pub fn cancel(&self) { self.native.cancel(); }

    pub fn on_change(self, f: impl FnMut(Option<KeyShortcut>) + 'static) -> Self {
        *self.native.ivars().changed.borrow_mut() = Some(Box::new(f));
        self
    }

    /// Acquire/release a backend recording lease. A failed acquisition prevents recording.
    pub fn capture(self, f: impl FnMut(bool) -> Result<(), String> + 'static) -> Self {
        *self.native.ivars().capture.borrow_mut() = Some(Box::new(f));
        self
    }

    /// Called when Escape, hiding, or window deactivation abandons recording.
    pub fn on_cancel(self, f: impl FnMut() + 'static) -> Self {
        *self.native.ivars().cancelled.borrow_mut() = Some(Box::new(f));
        self
    }

    pub fn conflict(self, f: impl FnMut(&KeyShortcut) -> Option<String> + 'static) -> Self {
        *self.native.ivars().conflict.borrow_mut() = Some(Box::new(f));
        self
    }

    pub fn ns_button(&self) -> &NSButton { &self.native }
}
impl NativeView for KeyRecorder {
    fn ns_view(&self) -> &NSView { &self.native }
}
impl NativeControl for KeyRecorder {
    fn ns_control(&self) -> &NSControl { &self.native }
}
pub struct ModifierRecorder(KeyRecorder);
impl ModifierRecorder {
    pub fn new(ui: &Ui) -> Self {
        Self(KeyRecorder {
            native: RecorderButton::new(ui, true),
        })
    }

    pub fn set_value(&self, value: Modifiers) {
        self.0.set_value(Some(KeyShortcut {
            modifiers: shortcut_modifiers(value),
            key: Key {
                code: 0,
                display: String::new(),
            },
        }));
    }

    pub fn on_change(self, mut f: impl FnMut(Modifiers) + 'static) -> Self {
        Self(
            self.0
                .on_change(move |s| f(s.map(|s| s.modifiers).unwrap_or_else(Modifiers::empty))),
        )
    }

    pub fn capture(self, f: impl FnMut(bool) -> Result<(), String> + 'static) -> Self {
        Self(self.0.capture(f))
    }

    pub fn ns_button(&self) -> &NSButton { self.0.ns_button() }
}
impl NativeView for ModifierRecorder {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }
}
impl NativeControl for ModifierRecorder {
    fn ns_control(&self) -> &NSControl { self.0.ns_control() }
}

pub struct ShortcutLabel(Label);
impl ShortcutLabel {
    pub fn new(ui: &Ui, value: &KeyShortcut) -> Self { Self(Label::new(ui, &value.to_string())) }

    pub fn set_value(&self, value: &KeyShortcut) { self.0.set_text(&value.to_string()); }

    pub fn ns_text_field(&self) -> &NSTextField { self.0.ns_text_field() }
}
impl NativeView for ShortcutLabel {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }
}
