use objc2::rc::Retained;
use objc2_app_kit::*;
use objc2_foundation::NSString;

use crate::Ui;
use crate::bridge::ActionTarget;
use crate::view::{NativeControl, NativeView};

pub struct Switch {
    native: Retained<NSSwitch>,
    target: Retained<ActionTarget>,
}
impl Switch {
    pub fn new(ui: &Ui) -> Self {
        let native = NSSwitch::new(ui.mtm());
        let target = ActionTarget::new(ui);
        target.attach(&native);
        Self { native, target }
    }

    pub fn value(self, value: bool) -> Self {
        self.set_value(value);
        self
    }

    pub fn set_value(&self, value: bool) {
        if (self.native.state() == NSControlStateValueOn) == value {
            return;
        }
        self.native.setState(if value {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
    }

    pub fn get_value(&self) -> bool { self.native.state() == NSControlStateValueOn }

    /// The smaller switch System Settings uses in its grouped rows.
    pub fn compact(self) -> Self {
        self.native.setControlSize(NSControlSize::Mini);
        self
    }

    pub fn on_change(self, mut f: impl FnMut(bool) + 'static) -> Self {
        self.target.set(move |sender| {
            if let Some(control) = sender.downcast_ref::<NSSwitch>() {
                f(control.state() == NSControlStateValueOn);
            }
        });
        self
    }
}
native_control!(Switch, NSSwitch, ns_switch);

pub struct Checkbox {
    native: Retained<NSButton>,
    target: Retained<ActionTarget>,
}
impl Checkbox {
    pub fn new(ui: &Ui, title: &str) -> Self {
        let native = NSButton::new(ui.mtm());
        native.setTitle(&NSString::from_str(title));
        native.setButtonType(NSButtonType::Switch);
        let target = ActionTarget::new(ui);
        target.attach(&native);
        Self { native, target }
    }

    pub fn set_value(&self, value: bool) {
        if (self.native.state() == NSControlStateValueOn) == value {
            return;
        }
        self.native.setState(if value {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
    }

    pub fn value(self, value: bool) -> Self {
        self.set_value(value);
        self
    }

    pub fn on_change(self, mut f: impl FnMut(bool) + 'static) -> Self {
        self.target.set(move |sender| {
            if let Some(control) = sender.downcast_ref::<NSButton>() {
                f(control.state() == NSControlStateValueOn);
            }
        });
        self
    }
}
native_control!(Checkbox, NSButton, ns_button);

pub struct RadioGroup {
    stack: crate::layout::VStack,
    buttons: Vec<Retained<NSButton>>,
    targets: Vec<Retained<ActionTarget>>,
}
impl RadioGroup {
    pub fn new(ui: &Ui, labels: &[&str]) -> Self {
        let stack = crate::layout::VStack::new(ui);
        let mut buttons = Vec::new();
        let mut targets = Vec::new();
        for title in labels {
            let native = NSButton::new(ui.mtm());
            native.setTitle(&NSString::from_str(title));
            native.setButtonType(NSButtonType::Radio);
            stack.ns_stack_view().addArrangedSubview(&native);
            let target = ActionTarget::new(ui);
            target.attach(&native);
            buttons.push(native);
            targets.push(target);
        }
        Self { stack, buttons, targets }
    }

    pub fn set_selected(&self, index: usize) {
        for (i, b) in self.buttons.iter().enumerate() {
            if (b.state() == NSControlStateValueOn) == (i == index) {
                continue;
            }
            b.setState(if i == index {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
    }

    pub fn on_change(self, f: impl FnMut(usize) + 'static) -> Self {
        use std::cell::RefCell;
        use std::rc::Rc;

        use objc2::rc::Weak;
        let f = Rc::new(RefCell::new(f));
        let buttons: Vec<_> = self.buttons.iter().map(|b| Weak::new(&**b)).collect();
        let buttons = Rc::new(buttons);
        for (index, target) in self.targets.iter().enumerate() {
            let f = f.clone();
            let buttons = buttons.clone();
            target.set(move |_| {
                for (i, b) in buttons.iter().enumerate() {
                    if let Some(b) = b.load() {
                        b.setState(if i == index {
                            NSControlStateValueOn
                        } else {
                            NSControlStateValueOff
                        });
                    }
                }
                (f.borrow_mut())(index);
            });
        }
        self
    }
}
impl NativeView for RadioGroup {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}
