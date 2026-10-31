use std::cell::RefCell;
use std::rc::Rc;

use objc2::Message;
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

type NumberCallback = Box<dyn FnMut(Option<f64>)>;
type NumberCallbacks = Rc<RefCell<[Option<NumberCallback>; 2]>>;

pub struct NumberField {
    field: crate::input::TextField,
    formatter: Retained<NSNumberFormatter>,
    bezel: Retained<NSTextField>,
    content: HStack,
    stepper: Stepper,
    changed: NumberCallbacks,
}
impl NumberField {
    pub fn with_validation(self, ui: &Ui) -> crate::settings::Validated<Self> {
        crate::settings::Validated::new(ui, self)
    }

    /// A numeric input with a trailing unit outside its bezel.
    pub fn unit_field(&self, ui: &Ui, unit: &str) -> crate::input::UnitField {
        crate::input::UnitField::with_content(ui, self.ns_view().retain(), unit)
    }

    pub fn new(ui: &Ui) -> Self {
        let field = crate::input::TextField::new(ui);
        let formatter = NSNumberFormatter::new();
        formatter.setNumberStyle(NSNumberFormatterStyle::DecimalStyle);
        formatter.setUsesGroupingSeparator(false);
        formatter.setMaximumFractionDigits(6);
        field.ns_text_field().setFormatter(Some(&formatter));
        let changed: NumberCallbacks = Rc::new(RefCell::new([None, None]));
        let (value, format, notify) = (
            objc2::rc::Weak::new(field.ns_text_field()),
            formatter.clone(),
            changed.clone(),
        );
        let stepper = Stepper::new(ui).range(-1.0, 1.0).increment(1.0);
        stepper.ns_stepper().setControlSize(NSControlSize::Mini);
        stepper.height(22.0);
        let reset = objc2::rc::Weak::new(stepper.ns_stepper());
        let stepper = stepper.on_change(move |delta| {
            if let Some(stepper) = reset.load() {
                stepper.setDoubleValue(0.0);
            }
            let Some(field) = value.load() else { return };
            if !field.isEnabled() {
                return;
            }
            let current = formatted_number(&format, &field.stringValue())
                .or_else(|| {
                    field
                        .placeholderString()
                        .and_then(|text| format.numberFromString(&text))
                        .map(|v| v.doubleValue())
                })
                .unwrap_or(0.0);
            let min = format.minimum().map_or(-f64::MAX, |v| v.doubleValue());
            let max = format.maximum().map_or(f64::MAX, |v| v.doubleValue());
            let step = if format.allowsFloats() && max - min <= 1.0 {
                0.01
            } else {
                1.0
            };
            let next = (current + delta * step).clamp(min, max);
            if next == current && !field.stringValue().is_empty() {
                return;
            }
            field.setDoubleValue(next);
            if let Some(editor) = field.currentEditor() {
                editor.setString(&field.stringValue());
            }
            for notify in notify.borrow_mut().iter_mut().flatten() {
                notify(Some(next));
            }
        });
        let bezel = NSTextField::new(ui.mtm());
        bezel.heightAnchor().constraintEqualToConstant(22.0).setActive(true);
        bezel.setEditable(false);
        bezel.setSelectable(false);
        bezel.setAccessibilityElement(true);
        bezel.setAccessibilityRole(Some(unsafe { NSAccessibilityGroupRole }));
        let value_object: &objc2::runtime::AnyObject = field.ns_text_field();
        let step_object: &objc2::runtime::AnyObject = stepper.ns_stepper();
        unsafe {
            bezel.setAccessibilityChildren(Some(&objc2_foundation::NSArray::from_slice(&[
                value_object,
                step_object,
            ])));
        }
        field.ns_text_field().setBezeled(false);
        field.ns_text_field().setDrawsBackground(false);
        field.ns_text_field().setAlignment(NSTextAlignment::Right);
        let content = HStack::new(ui)
            .spacing(5.0)
            .insets(crate::Insets {
                left: 6.0,
                right: 0.0,
                top: 0.0,
                bottom: 0.0,
            })
            .push(field.ns_view().retain())
            .push(stepper.ns_view().retain());
        bezel.addSubview(content.ns_view());
        crate::view::pin(&bezel, content.ns_view(), crate::layout::uniform_insets(0.0));
        Self {
            field,
            formatter,
            bezel,
            content,
            stepper,
            changed,
        }
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
        self.changed.borrow_mut()[1] = Some(Box::new(move |value| {
            if let Some(value) = value {
                f(value);
            }
        }));
        let (formatter, changed) = (self.formatter.clone(), self.changed.clone());
        self.field = self.field.on_commit(move |text| {
            if let Some(value) = formatted_number(&formatter, &NSString::from_str(&text)) {
                if let Some(f) = changed.borrow_mut()[1].as_mut() {
                    f(Some(value));
                }
            }
        });
        self
    }

    /// Update a local draft as valid numeric input is entered.
    pub fn on_edit(mut self, mut f: impl FnMut(f64) + 'static) -> Self {
        self.changed.borrow_mut()[0] = Some(Box::new(move |value| {
            if let Some(value) = value {
                f(value);
            }
        }));
        let (formatter, changed) = (self.formatter.clone(), self.changed.clone());
        self.field = self.field.on_change(move |text| {
            if let Some(value) = formatted_number(&formatter, &NSString::from_str(&text)) {
                if let Some(f) = changed.borrow_mut()[0].as_mut() {
                    f(Some(value));
                }
            }
        });
        self
    }

    /// Slider-backed inputs use the same bezel without redundant stepper arrows.
    pub fn hide_stepper(&self) {
        self.stepper.set_hidden(true);
        self.content.ns_stack_view().setDetachesHiddenViews(true);
        self.content.ns_stack_view().setSpacing(0.0);
        self.content.ns_stack_view().setDistribution(NSStackViewDistribution::Fill);
        self.content.ns_stack_view().setEdgeInsets(crate::Insets {
            left: 6.0,
            right: 6.0,
            top: 0.0,
            bottom: 0.0,
        });
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
    pub fn on_change_optional(mut self, f: impl FnMut(Option<f64>) + 'static) -> Self {
        self.changed.borrow_mut()[1] = Some(Box::new(f));
        let (formatter, changed) = (self.formatter.clone(), self.changed.clone());
        self.field = self.field.on_commit(move |text| {
            let value = if text.trim().is_empty() {
                None
            } else {
                let Some(value) = formatted_number(&formatter, &NSString::from_str(&text)) else {
                    return;
                };
                Some(value)
            };
            if let Some(f) = changed.borrow_mut()[1].as_mut() {
                f(value);
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
    fn ns_view(&self) -> &NSView { &self.bezel }

    fn accessibility_label(&self, label: &str) {
        self.field.accessibility_label(label);
        self.stepper.accessibility_label(label);
    }
}
impl NativeControl for NumberField {
    fn ns_control(&self) -> &NSControl { self.field.ns_control() }

    fn set_enabled(&self, value: bool) {
        self.field.set_enabled(value);
        self.stepper.set_enabled(value);
        self.bezel.setEnabled(value);
    }
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
        field.stepper.set_hidden(true);
        let weak = Weak::new(field.ns_text_field());
        let stepper =
            Stepper::new(ui).range(min, max).increment(increment).on_change(move |value| {
                if let Some(field) = weak.load() {
                    field.setDoubleValue(value);
                }
            });
        stepper.ns_stepper().setControlSize(NSControlSize::Mini);
        stepper.height(22.0);
        let weak = Weak::new(stepper.ns_stepper());
        let field = field.on_change(move |value| {
            if let Some(stepper) = weak.load() {
                stepper.setDoubleValue(value);
            }
        });
        let stack = HStack::new(ui);
        stack.ns_stack_view().addArrangedSubview(field.ns_view());
        field.content.add(stepper.ns_view().retain());
        let value_object: &objc2::runtime::AnyObject = field.ns_text_field();
        let step_object: &objc2::runtime::AnyObject = stepper.ns_stepper();
        unsafe {
            field
                .bezel
                .setAccessibilityChildren(Some(&objc2_foundation::NSArray::from_slice(&[
                    value_object,
                    step_object,
                ])));
        }
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
