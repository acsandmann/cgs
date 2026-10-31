use std::cell::RefCell;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::*;
use objc2_foundation::NSString;

use crate::Ui;
use crate::bridge::{DelegateBridge, Event};
use crate::settings::Validation;
use crate::view::{NativeControl, NativeView};

type TextCallback = Box<dyn FnMut(String)>;
type TextCallbacks = Rc<RefCell<[Option<TextCallback>; 2]>>;

define_class!(
    #[unsafe(super(NSTextField))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsTextField"]
    struct NaturalTextField;
    impl NaturalTextField {
        #[unsafe(method(intrinsicContentSize))]
        fn intrinsic_size(&self) -> crate::CGSize {
            let mut size: crate::CGSize = unsafe { msg_send![super(self), intrinsicContentSize] };
            let font = self.font().unwrap_or_else(crate::style::Font::body);
            // Fit the value (live while editing) or the placeholder, whichever is wider.
            // Leave room to edit empty fields and numbers without imposing a fixed
            // width. A unit bezel supplies its own surrounding padding.
            let value = self
                .currentEditor()
                .map(|editor| editor.string())
                .unwrap_or_else(|| self.stringValue());
            let text = text_width(&value, &font).max(
                self.placeholderString().map_or(0.0, |placeholder| text_width(&placeholder, &font)),
            );
            let padding = if self.isBezeled() { 12.0 } else { 4.0 };
            let minimum = font.pointSize() * if self.isBezeled() { 6.0 } else { 3.0 };
            size.width = (text + padding).ceil().max(minimum);
            size
        }

        #[unsafe(method(setStringValue:))]
        fn set_string_value(&self, value: &NSString) {
            let _: () = unsafe { msg_send![super(self), setStringValue: value] };
            self.invalidateIntrinsicContentSize();
        }

        #[unsafe(method(setPlaceholderString:))]
        fn set_placeholder_string(&self, value: Option<&NSString>) {
            let _: () = unsafe { msg_send![super(self), setPlaceholderString: value] };
            self.invalidateIntrinsicContentSize();
        }

        #[unsafe(method(textDidChange:))]
        fn text_did_change(&self, notification: &objc2_foundation::NSNotification) {
            let _: () = unsafe { msg_send![super(self), textDidChange: notification] };
            self.invalidateIntrinsicContentSize();
        }
    }
);
fn text_width(text: &NSString, font: &NSFont) -> f64 {
    let attributes =
        objc2_foundation::NSDictionary::from_slices(&[unsafe { NSFontAttributeName }], &[
            font as &AnyObject
        ]);
    unsafe {
        objc2_foundation::NSAttributedString::initWithString_attributes(
            objc2_foundation::NSAttributedString::alloc(),
            text,
            Some(&attributes),
        )
    }
    .size()
    .width
}
fn natural_text_field(ui: &Ui) -> Retained<NSTextField> {
    let field: Retained<NaturalTextField> =
        unsafe { msg_send![NaturalTextField::alloc(ui.mtm()), init] };
    // A bare `init` cell wraps; keep long values such as paths on one scrolling line.
    if let Some(cell) = field.cell() {
        cell.setWraps(false);
        cell.setScrollable(true);
    }
    field.setUsesSingleLineMode(true);
    // Grow with the content, but scroll it rather than widen the window or wrap labels.
    field.setContentCompressionResistancePriority_forOrientation(
        crate::text::FIELD_COMPRESSION_RESISTANCE,
        NSLayoutConstraintOrientation::Horizontal,
    );
    field.into_super()
}

macro_rules! text_input {
    ($name:ident,$native:ident,$accessor:ident,$constructor:expr) => {
        pub struct $name {
            native: Retained<$native>,
            delegate: Option<Retained<DelegateBridge>>,
            callbacks: TextCallbacks,
        }
        impl $name {
            pub fn new(ui: &Ui) -> Self {
                let native = ($constructor)(ui);
                native.setFont(Some(&crate::style::Font::body()));
                Self {
                    native,
                    delegate: None,
                    callbacks: Rc::new(RefCell::new([None, None])),
                }
            }

            pub fn value(self, value: &str) -> Self {
                self.set_value(value);
                self
            }

            pub fn set_value(&self, value: &str) {
                if self.native.stringValue().to_string() != value {
                    self.native.setStringValue(&NSString::from_str(value));
                }
            }

            pub fn get_value(&self) -> String { self.native.stringValue().to_string() }

            pub fn placeholder(self, text: &str) -> Self {
                self.native.setPlaceholderString(Some(&NSString::from_str(text)));
                self
            }

            pub fn focus(&self) {
                if let Some(window) = self.native.window() {
                    window.makeFirstResponder(Some(&*self.native));
                }
            }

            fn listen(mut self, event: Event, f: impl FnMut(String) + 'static) -> Self {
                let index = usize::from(event == Event::TextCommitted);
                self.callbacks.borrow_mut()[index] = Some(Box::new(f));
                if self.delegate.is_none() {
                    let callbacks = self.callbacks.clone();
                    let bridge =
                        DelegateBridge::new(&Ui::new(self.native.mtm()), move |event, sender| {
                            let index = match event {
                                Event::TextChanged => 0,
                                Event::TextCommitted => 1,
                                _ => return,
                            };
                            if let Some(control) = sender.downcast_ref::<$native>() {
                                if let Ok(mut callbacks) = callbacks.try_borrow_mut() {
                                    if let Some(f) = callbacks[index].as_mut() {
                                        f(control.stringValue().to_string());
                                    }
                                }
                            }
                        });
                    unsafe {
                        self.native.setDelegate(Some(ProtocolObject::from_ref(&*bridge)));
                    }
                    self.delegate = Some(bridge);
                }
                self
            }

            pub fn on_change(self, f: impl FnMut(String) + 'static) -> Self {
                self.listen(Event::TextChanged, f)
            }

            pub fn on_commit(self, f: impl FnMut(String) + 'static) -> Self {
                self.listen(Event::TextCommitted, f)
            }

            pub fn set_validation(&self, validation: &Validation) {
                self.native
                    .setToolTip(validation.message().map(|s| NSString::from_str(s)).as_deref());
            }
        }
        crate::controls::native_control!($name, $native, $accessor);
        impl Drop for $name {
            fn drop(&mut self) {
                unsafe {
                    self.native.setDelegate(None);
                }
            }
        }
    };
}
text_input!(TextField, NSTextField, ns_text_field, natural_text_field);
impl TextField {
    pub fn with_validation(self, ui: &Ui) -> crate::settings::Validated<Self> {
        crate::settings::Validated::new(ui, self)
    }
}
text_input!(SearchField, NSSearchField, ns_search_field, |ui: &Ui| {
    NSSearchField::new(ui.mtm())
});
text_input!(TokenField, NSTokenField, ns_token_field, |ui: &Ui| {
    NSTokenField::new(ui.mtm())
});

pub struct TextArea {
    scroll: crate::layout::ScrollView,
    native: Retained<NSTextView>,
    delegate: Option<Retained<DelegateBridge>>,
}
impl TextArea {
    pub fn new(ui: &Ui) -> Self {
        let native = NSTextView::new(ui.mtm());
        native.setRichText(false);
        native.setFont(Some(&crate::style::Font::body()));
        native.setVerticallyResizable(true);
        let view: Retained<NSView> = native.clone().into_super().into_super();
        let scroll = crate::layout::ScrollView::new(ui, view);
        Self { scroll, native, delegate: None }
    }

    pub fn set_value(&self, text: &str) { self.native.setString(&NSString::from_str(text)); }

    pub fn get_value(&self) -> String { self.native.string().to_string() }

    pub fn focus(&self) {
        if let Some(w) = self.native.window() {
            w.makeFirstResponder(Some(&*self.native));
        }
    }

    pub fn on_change(mut self, mut f: impl FnMut(String) + 'static) -> Self {
        let bridge = DelegateBridge::new(&Ui::new(self.native.mtm()), move |event, sender| {
            if event == Event::TextChanged {
                if let Some(view) = sender.downcast_ref::<NSTextView>() {
                    f(view.string().to_string());
                }
            }
        });
        self.native.setDelegate(Some(ProtocolObject::from_ref(&*bridge)));
        self.delegate = Some(bridge);
        self
    }

    pub fn ns_text_view(&self) -> &NSTextView { &self.native }
}
impl NativeView for TextArea {
    fn ns_view(&self) -> &NSView { self.scroll.ns_view() }
}

impl Drop for TextArea {
    fn drop(&mut self) { self.native.setDelegate(None); }
}

/// Native text-field bezel with an editable value and a separate, read-only unit.
/// A numeric input with its unit outside the editable bezel.
pub struct UnitField {
    content: crate::layout::HStack,
}
impl UnitField {
    pub fn new(ui: &Ui, field: &NSTextField, unit: &str) -> Self {
        Self::with_content(ui, field.retain().into_super().into_super(), unit)
    }

    pub(crate) fn with_content(ui: &Ui, field: impl NativeView, unit: &str) -> Self {
        let label =
            crate::text::Label::new(ui, unit).color(&crate::style::Color::secondary_label());
        label.compression_resistance(1000.0, NSLayoutConstraintOrientation::Horizontal);
        label.hugging_priority(1000.0, NSLayoutConstraintOrientation::Horizontal);
        Self {
            content: crate::layout::HStack::new(ui).spacing(6.0).push(field).push(label),
        }
    }
}
impl NativeView for UnitField {
    fn ns_view(&self) -> &NSView { self.content.ns_view() }
}
