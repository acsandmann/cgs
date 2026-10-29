use objc2::rc::Retained;
use objc2_app_kit::*;
use objc2_foundation::NSString;

use crate::Ui;
use crate::bridge::ActionTarget;
use crate::view::{NativeControl, NativeView};

// The glass bezel and NSGlassEffectView were introduced together in macOS 26.
// Check runtime availability so older systems retain their standard push buttons.
pub(crate) fn action_button_bezel() -> objc2_app_kit::NSBezelStyle {
    if objc2::runtime::AnyClass::get(c"NSGlassEffectView").is_some() {
        objc2_app_kit::NSBezelStyle::Glass
    } else {
        objc2_app_kit::NSBezelStyle::Push
    }
}

pub struct Button {
    native: Retained<NSButton>,
    pub(super) target: Retained<ActionTarget>,
}
impl Button {
    pub fn new(ui: &Ui, title: &str) -> Self {
        let native = NSButton::new(ui.mtm());
        native.setTitle(&NSString::from_str(title));
        native.setBezelStyle(action_button_bezel());
        Self::from_native(ui, native)
    }

    pub(crate) fn from_native(ui: &Ui, native: Retained<NSButton>) -> Self {
        let target = ActionTarget::new(ui);
        target.attach(&native);
        Self { native, target }
    }

    pub fn on_click(self, mut f: impl FnMut() + 'static) -> Self {
        self.target.set(move |_| f());
        self
    }

    pub fn key_equivalent(self, value: &str) -> Self {
        self.native.setKeyEquivalent(&NSString::from_str(value));
        self.native.setKeyEquivalentModifierMask(NSEventModifierFlags::empty());
        self
    }

    pub fn set_title(&self, title: &str) { self.native.setTitle(&NSString::from_str(title)); }

    /// A standard push button for page content, which keeps glass off the content layer.
    pub fn push(self) -> Self {
        self.native.setBezelStyle(NSBezelStyle::Push);
        self
    }

    pub fn borderless(self) -> Self {
        self.native.setBordered(false);
        self
    }

    pub fn disclosure(self) -> Self {
        self.native.setBezelStyle(NSBezelStyle::Disclosure);
        self.native.setButtonType(NSButtonType::PushOnPushOff);
        self
    }

    pub fn symbol_with_title(self, name: &str) -> Self {
        let button = self.symbol(name);
        button.native.setImagePosition(NSCellImagePosition::ImageLeading);
        button
    }

    pub fn symbol(self, name: &str) -> Self {
        if let Some(image) = crate::image::Symbol::named(name) {
            self.native.setImage(Some(&image));
            self.native.setImagePosition(NSCellImagePosition::ImageOnly);
        }
        self
    }
}
native_control!(Button, NSButton, ns_button);

pub struct IconButton(Button);
impl IconButton {
    pub fn new(ui: &Ui, symbol: &str, label: &str) -> Self {
        let button = Button::new(ui, label).symbol(symbol);
        button.ns_button().setBezelStyle(NSBezelStyle::AccessoryBar);
        button.ns_button().setControlSize(NSControlSize::Small);
        button.accessibility_label(label);
        Self(button)
    }

    pub fn on_click(self, f: impl FnMut() + 'static) -> Self { Self(self.0.on_click(f)) }

    pub fn ns_button(&self) -> &NSButton { self.0.ns_button() }
}
impl NativeView for IconButton {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }
}
impl NativeControl for IconButton {
    fn ns_control(&self) -> &NSControl { self.0.ns_control() }
}
pub type SymbolButton = IconButton;
pub struct HelpButton(Button);
impl HelpButton {
    pub fn new(ui: &Ui) -> Self {
        let b = Button::new(ui, "Help");
        b.native.setBezelStyle(NSBezelStyle::HelpButton);
        Self(b)
    }

    pub fn on_click(self, f: impl FnMut() + 'static) -> Self { Self(self.0.on_click(f)) }

    pub fn ns_button(&self) -> &NSButton { self.0.ns_button() }
}
impl NativeView for HelpButton {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }
}
