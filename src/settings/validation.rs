use std::cell::RefCell;

use objc2_app_kit::{NSTextField, NSView};

use crate::Ui;
use crate::layout::VStack;
use crate::style::Color;
use crate::text::SecondaryLabel;
use crate::view::{NativeControl, NativeView};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Validation {
    #[default]
    None,
    Info(String),
    Warning(String),
    Error(String),
}
impl Validation {
    pub fn message(&self) -> Option<&str> {
        match self {
            Self::None => None,
            Self::Info(s) | Self::Warning(s) | Self::Error(s) => Some(s),
        }
    }
}
pub struct ValidationMessage {
    label: SecondaryLabel,
    value: RefCell<Validation>,
}
impl ValidationMessage {
    pub fn new(ui: &Ui) -> Self {
        let label = SecondaryLabel::new(ui, "");
        let this = Self {
            label,
            value: RefCell::new(Validation::None),
        };
        this.set_hidden(true);
        this
    }

    pub fn set_validation(&self, value: &Validation) {
        if *self.value.borrow() == *value {
            return;
        }
        *self.value.borrow_mut() = value.clone();
        self.label.set_text(value.message().unwrap_or(""));
        let color = match value {
            Validation::Error(_) => objc2_app_kit::NSColor::systemRedColor(),
            Validation::Warning(_) => objc2_app_kit::NSColor::systemOrangeColor(),
            _ => Color::secondary_label(),
        };
        self.label.ns_text_field().setTextColor(Some(&color));
        self.set_hidden(matches!(value, Validation::None));
        self.accessibility_label(value.message().unwrap_or(""));
    }

    pub fn ns_text_field(&self) -> &NSTextField { self.label.ns_text_field() }
}
impl NativeView for ValidationMessage {
    fn ns_view(&self) -> &NSView { self.label.ns_view() }
}

pub struct Validated<T: NativeControl> {
    stack: VStack,
    input: T,
    message: ValidationMessage,
}
impl<T: NativeControl> Validated<T> {
    pub fn new(ui: &Ui, input: T) -> Self {
        let stack = VStack::new(ui).spacing(4.0);
        let message = ValidationMessage::new(ui);
        stack.ns_stack_view().addArrangedSubview(input.ns_view());
        stack.ns_stack_view().addArrangedSubview(message.ns_view());
        Self { stack, input, message }
    }

    pub fn input(&self) -> &T { &self.input }

    pub fn set_validation(&self, value: &Validation) { self.message.set_validation(value); }
}
impl<T: NativeControl> NativeView for Validated<T> {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}
impl<T: NativeControl> NativeControl for Validated<T> {
    fn ns_control(&self) -> &objc2_app_kit::NSControl { self.input.ns_control() }
}
