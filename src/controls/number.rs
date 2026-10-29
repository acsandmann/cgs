use objc2::rc::Retained;
use objc2_app_kit::*;
use objc2_foundation::{NSNumber, NSNumberFormatter, NSNumberFormatterStyle, NSString};

use crate::Ui;
use crate::bridge::ActionTarget;
use crate::layout::HStack;
use crate::view::{NativeControl, NativeView};

macro_rules! numeric_control {
    ($name:ident, $native:ident, $accessor:ident) => {
        pub struct $name {
            native: Retained<$native>,
            target: Retained<ActionTarget>,
        }
        impl $name {
            pub fn new(ui: &Ui) -> Self {
                let native = $native::new(ui.mtm());
                let target = ActionTarget::new(ui);
                target.attach(&native);
                Self { native, target }
            }

            pub fn range(self, min: f64, max: f64) -> Self {
                self.set_range(min, max);
                self
            }

            pub fn set_range(&self, min: f64, max: f64) {
                if self.native.minValue() != min {
                    self.native.setMinValue(min);
                }
                if self.native.maxValue() != max {
                    self.native.setMaxValue(max);
                }
            }

            /// Report every change while tracking, rather than only on release.
            pub fn continuous(self, value: bool) -> Self {
                self.native.setContinuous(value);
                self
            }

            pub fn value(self, value: f64) -> Self {
                self.set_value(value);
                self
            }

            pub fn set_value(&self, value: f64) {
                if self.native.doubleValue() != value {
                    self.native.setDoubleValue(value);
                }
            }

            pub fn get_value(&self) -> f64 { self.native.doubleValue() }

            pub fn on_change(self, mut f: impl FnMut(f64) + 'static) -> Self {
                self.target.set(move |sender| {
                    if let Some(control) = sender.downcast_ref::<$native>() {
                        f(control.doubleValue());
                    }
                });
                self
            }
        }
        native_control!($name, $native, $accessor);
    };
}
numeric_control!(Stepper, NSStepper, ns_stepper);
impl Stepper {
    pub fn increment(self, value: f64) -> Self {
        self.native.setIncrement(value);
        self
    }
}

fn formatted_number(formatter: &NSNumberFormatter, text: &NSString) -> Option<f64> {
    let value = formatter.numberFromString(text)?.doubleValue();
    if !value.is_finite()
        || (!formatter.allowsFloats() && value.fract() != 0.0)
        || formatter.minimum().is_some_and(|min| value < min.doubleValue())
        || formatter.maximum().is_some_and(|max| value > max.doubleValue())
    {
        None
    } else {
        Some(value)
    }
}

pub struct NumberField {
    field: crate::input::TextField,
    formatter: Retained<NSNumberFormatter>,
}
impl NumberField {
    pub fn with_validation(self, ui: &Ui) -> crate::settings::Validated<Self> {
        crate::settings::Validated::new(ui, self)
    }

    /// A leading-aligned value inside one bezel with a trailing unit, such as "pt".
    pub fn unit_field(&self, ui: &Ui, unit: &str) -> crate::input::UnitField {
        self.ns_text_field().setAlignment(NSTextAlignment::Left);
        crate::input::UnitField::new(ui, self.ns_text_field(), unit)
    }

    pub fn new(ui: &Ui) -> Self {
        let field = crate::input::TextField::new(ui);
        let formatter = NSNumberFormatter::new();
        formatter.setNumberStyle(NSNumberFormatterStyle::DecimalStyle);
        formatter.setUsesGroupingSeparator(false);
        formatter.setMaximumFractionDigits(6);
        field.ns_text_field().setFormatter(Some(&formatter));
        Self { field, formatter }
    }

    pub fn integer(self) -> Self {
        self.formatter.setAllowsFloats(false);
        self.formatter.setMaximumFractionDigits(0);
        self
    }

    pub fn range(self, min: f64, max: f64) -> Self {
        assert!(min.is_finite() && max.is_finite() && min <= max);
        self.formatter.setMinimum(Some(&NSNumber::new_f64(min)));
        self.formatter.setMaximum(Some(&NSNumber::new_f64(max)));
        self
    }

    pub fn value(self, value: f64) -> Self {
        self.set_value(value);
        self
    }

    pub fn set_value(&self, value: f64) {
        if self.get_value() != Some(value) {
            self.field.ns_text_field().setDoubleValue(value);
        }
    }

    pub fn get_value(&self) -> Option<f64> {
        formatted_number(&self.formatter, &self.field.ns_text_field().stringValue())
    }

    pub fn on_change(mut self, mut f: impl FnMut(f64) + 'static) -> Self {
        let formatter = self.formatter.clone();
        self.field = self.field.on_commit(move |text| {
            if let Some(value) = formatted_number(&formatter, &NSString::from_str(&text)) {
                f(value);
            }
        });
        self
    }

    /// Update a local draft as valid numeric input is entered.
    pub fn on_edit(mut self, mut f: impl FnMut(f64) + 'static) -> Self {
        let formatter = self.formatter.clone();
        self.field = self.field.on_change(move |text| {
            if let Some(value) = formatted_number(&formatter, &NSString::from_str(&text)) {
                f(value);
            }
        });
        self
    }

    /// Leave the field empty, showing `placeholder`, such as an inherited value.
    pub fn set_inherited(&self, placeholder: &str) {
        let field = self.field.ns_text_field();
        field.setPlaceholderString(Some(&NSString::from_str(placeholder)));
        if field.currentEditor().is_none() && !field.stringValue().is_empty() {
            field.setStringValue(&NSString::from_str(""));
        }
    }

    /// Commit valid values, or `None` when the field is left empty.
    pub fn on_change_optional(mut self, mut f: impl FnMut(Option<f64>) + 'static) -> Self {
        let formatter = self.formatter.clone();
        self.field = self.field.on_commit(move |text| {
            if text.trim().is_empty() {
                f(None);
            } else if let Some(value) = formatted_number(&formatter, &NSString::from_str(&text)) {
                f(Some(value));
            }
        });
        self
    }

    pub fn ns_text_field(&self) -> &NSTextField { self.field.ns_text_field() }

    pub fn ns_number_formatter(&self) -> &NSNumberFormatter { &self.formatter }

    pub fn set_validation(&self, validation: &crate::settings::Validation) {
        self.field.set_validation(validation);
    }
}
impl NativeView for NumberField {
    fn ns_view(&self) -> &NSView { self.field.ns_view() }
}
impl NativeControl for NumberField {
    fn ns_control(&self) -> &NSControl { self.field.ns_control() }
}

pub struct NumberStepper {
    stack: HStack,
    field: NumberField,
    stepper: Stepper,
}
impl NumberStepper {
    pub fn new(ui: &Ui, min: f64, max: f64, increment: f64) -> Self {
        use objc2::rc::Weak;
        let field = NumberField::new(ui).range(min, max);
        let weak = Weak::new(field.ns_text_field());
        let stepper =
            Stepper::new(ui).range(min, max).increment(increment).on_change(move |value| {
                if let Some(field) = weak.load() {
                    field.setDoubleValue(value);
                }
            });
        let weak = Weak::new(stepper.ns_stepper());
        let field = field.on_change(move |value| {
            if let Some(stepper) = weak.load() {
                stepper.setDoubleValue(value);
            }
        });
        let stack = HStack::new(ui);
        stack.ns_stack_view().addArrangedSubview(field.ns_view());
        stack.ns_stack_view().addArrangedSubview(stepper.ns_view());
        Self { stack, field, stepper }
    }

    pub fn integer(mut self) -> Self {
        self.field = self.field.integer();
        self
    }

    pub fn accessibility_label(&self, label: &str) {
        self.field.accessibility_label(label);
        self.stepper.accessibility_label(label);
    }

    pub fn set_value(&self, value: f64) {
        self.field.set_value(value);
        self.stepper.set_value(value);
    }

    pub fn on_change(mut self, mut f: impl FnMut(f64) + 'static) -> Self {
        use std::cell::RefCell;
        use std::rc::Rc;

        use objc2::rc::Weak;
        let cb = Rc::new(RefCell::new(move |v| f(v)));
        let weak = Weak::new(self.stepper.ns_stepper());
        let c = cb.clone();
        self.field = self.field.on_change(move |v| {
            if let Some(s) = weak.load() {
                s.setDoubleValue(v);
            }
            (c.borrow_mut())(v);
        });
        let weak = Weak::new(self.field.ns_text_field());
        self.stepper = self.stepper.on_change(move |v| {
            if let Some(s) = weak.load() {
                s.setDoubleValue(v);
            }
            (cb.borrow_mut())(v);
        });
        self
    }

    pub fn ns_text_field(&self) -> &NSTextField { self.field.ns_text_field() }

    pub fn ns_stepper(&self) -> &NSStepper { self.stepper.ns_stepper() }
}
impl NativeView for NumberStepper {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}
