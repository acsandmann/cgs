use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::{MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSBezierPath, NSColor, NSControl, NSControlSize, NSGraphicsContext,
    NSLayoutAttribute, NSLayoutConstraint, NSLayoutRelation, NSUserInterfaceItemIdentification,
    NSView, NSViewController,
};
use objc2_foundation::NSString;

use crate::{Insets, Ui};

pub trait NativeView: 'static {
    fn ns_view(&self) -> &NSView;
    fn view_controller(&self) -> Option<&objc2_app_kit::NSViewController> { None }
    fn set_hidden(&self, value: bool) {
        if self.ns_view().isHidden() != value {
            self.ns_view().setHidden(value);
        }
    }
    fn tooltip(&self, text: &str) { self.ns_view().setToolTip(Some(&NSString::from_str(text))); }
    fn identifier(&self, text: &str) {
        self.ns_view().setIdentifier(Some(&NSString::from_str(text)));
    }
    fn accessibility_label(&self, text: &str) {
        self.ns_view().setAccessibilityLabel(Some(&NSString::from_str(text)));
    }
    fn accessibility_help(&self, text: &str) {
        self.ns_view().setAccessibilityHelp(Some(&NSString::from_str(text)));
    }
    fn accessibility_identifier(&self, text: &str) {
        self.ns_view().setAccessibilityIdentifier(Some(&NSString::from_str(text)));
    }
    fn width(&self, value: f64) {
        dimension(
            self.ns_view(),
            NSLayoutAttribute::Width,
            NSLayoutRelation::Equal,
            value,
        );
    }
    fn height(&self, value: f64) {
        dimension(
            self.ns_view(),
            NSLayoutAttribute::Height,
            NSLayoutRelation::Equal,
            value,
        );
    }
    fn min_height(&self, value: f64) {
        dimension(
            self.ns_view(),
            NSLayoutAttribute::Height,
            NSLayoutRelation::GreaterThanOrEqual,
            value,
        );
    }
    fn min_width(&self, value: f64) {
        dimension(
            self.ns_view(),
            NSLayoutAttribute::Width,
            NSLayoutRelation::GreaterThanOrEqual,
            value,
        );
    }
    /// Fill spare vertical space, preferring at least `preferred` points but yielding it first
    /// when the window is short.
    fn flexible_height(&self, preferred: f64) {
        let view = self.ns_view();
        let constraint = view.heightAnchor().constraintGreaterThanOrEqualToConstant(preferred);
        constraint.setPriority(750.0);
        constraint.setActive(true);
        let vertical = objc2_app_kit::NSLayoutConstraintOrientation::Vertical;
        view.setContentCompressionResistancePriority_forOrientation(1.0, vertical);
        view.setContentHuggingPriority_forOrientation(1.0, vertical);
    }
    fn hugging_priority(&self, priority: f32, axis: objc2_app_kit::NSLayoutConstraintOrientation) {
        self.ns_view().setContentHuggingPriority_forOrientation(priority, axis);
    }
    fn compression_resistance(
        &self,
        priority: f32,
        axis: objc2_app_kit::NSLayoutConstraintOrientation,
    ) {
        self.ns_view()
            .setContentCompressionResistancePriority_forOrientation(priority, axis);
    }
    fn max_width(&self, value: f64) {
        dimension(
            self.ns_view(),
            NSLayoutAttribute::Width,
            NSLayoutRelation::LessThanOrEqual,
            value,
        );
    }
}
/// A weak handle for updating a view from callbacks without retaining it.
#[derive(Clone, Default)]
pub struct WeakView(pub(crate) objc2::rc::Weak<NSView>);
impl WeakView {
    pub fn new(view: &impl NativeView) -> Self { Self(objc2::rc::Weak::new(view.ns_view())) }

    /// The live view, usable anywhere a `NativeView` is expected (for example as an anchor).
    pub fn load(&self) -> Option<Retained<NSView>> { self.0.load() }

    /// Enable or disable every control in the view, including the view itself.
    pub fn set_enabled(&self, value: bool) {
        fn apply(view: &NSView, value: bool) {
            if let Some(control) = view.downcast_ref::<NSControl>() {
                control.setEnabled(value);
            }
            for child in view.subviews() {
                apply(&child, value);
            }
        }
        if let Some(view) = self.0.load() {
            apply(&view, value);
        }
    }
}

pub trait NativeControl: NativeView {
    fn ns_control(&self) -> &NSControl;
    fn set_enabled(&self, value: bool) {
        if self.ns_control().isEnabled() != value {
            self.ns_control().setEnabled(value);
        }
    }
    fn control_size(&self, size: NSControlSize) { self.ns_control().setControlSize(size); }
}
impl NativeView for Retained<NSView> {
    fn ns_view(&self) -> &NSView { self }
}

impl<T: NativeView + ?Sized> NativeView for Box<T> {
    fn ns_view(&self) -> &NSView { (**self).ns_view() }

    fn view_controller(&self) -> Option<&NSViewController> { (**self).view_controller() }

    fn accessibility_label(&self, label: &str) { (**self).accessibility_label(label); }
}

impl<T: NativeView + ?Sized> NativeView for std::rc::Rc<T> {
    fn ns_view(&self) -> &NSView { (**self).ns_view() }

    fn view_controller(&self) -> Option<&NSViewController> { (**self).view_controller() }

    fn accessibility_label(&self, label: &str) { (**self).accessibility_label(label); }
}
impl<T: NativeControl> NativeControl for std::rc::Rc<T> {
    fn ns_control(&self) -> &NSControl { (**self).ns_control() }

    fn set_enabled(&self, value: bool) { (**self).set_enabled(value); }
}

pub(crate) fn dimension(
    view: &NSView,
    attribute: NSLayoutAttribute,
    relation: NSLayoutRelation,
    value: f64,
) {
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    unsafe {
        NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
            view,
            attribute,
            relation,
            None,
            NSLayoutAttribute::NotAnAttribute,
            1.0,
            value,
        )
    }
    .setActive(true);
}
pub(crate) fn pin(parent: &NSView, child: &NSView, insets: Insets) {
    child.setTranslatesAutoresizingMaskIntoConstraints(false);
    for (attribute, constant) in [
        (NSLayoutAttribute::Leading, insets.left),
        (NSLayoutAttribute::Trailing, -insets.right),
        (NSLayoutAttribute::Top, insets.top),
        (NSLayoutAttribute::Bottom, -insets.bottom),
    ] {
        unsafe {
            NSLayoutConstraint::constraintWithItem_attribute_relatedBy_toItem_attribute_multiplier_constant(
                child,
                attribute,
                NSLayoutRelation::Equal,
                Some(parent),
                attribute,
                1.0,
                constant,
            )
        }
        .setActive(true);
    }
}
pub(crate) fn prepare(view: &NSView) { view.setTranslatesAutoresizingMaskIntoConstraints(false); }

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgUiDocumentView"]
    struct DocumentView;
    impl DocumentView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self)->bool{true}
    }
);

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsSettingsCardView"]
    #[ivars = Cell<u8>]
    struct SettingsCardView;
    impl SettingsCardView {
        #[unsafe(method(drawRect:))]
        fn draw(&self, _: crate::CGRect) {
            use objc2::DefinedClass;
            let bounds = self.bounds();
            let mut shape = bounds;
            if self.ivars().get() & 1 == 0 { shape.size.height += 10.0; }
            if self.ivars().get() & 2 == 0 { shape.origin.y -= 10.0; shape.size.height += 10.0; }
            NSGraphicsContext::saveGraphicsState_class();
            NSBezierPath::bezierPathWithRect(bounds).addClip();
            NSColor::quaternarySystemFillColor().setFill();
            NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(shape, 10.0, 10.0).fill();
            NSGraphicsContext::restoreGraphicsState_class();
        }
        #[unsafe(method(setFrameSize:))]
        fn resize(&self, size: crate::CGSize) {
            unsafe { let _: () = msg_send![super(self), setFrameSize: size]; }
            self.setNeedsDisplay(true);
        }
        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn appearance(&self) {
            unsafe { let _: () = msg_send![super(self), viewDidChangeEffectiveAppearance]; }
            self.setNeedsDisplay(true);
        }
    }
);

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsSettingsColorView"]
    #[ivars = u8]
    struct SettingsColorView;
    impl SettingsColorView {
        #[unsafe(method(wantsUpdateLayer))]
        fn wants_update_layer(&self) -> bool { true }
        #[unsafe(method(updateLayer))]
        fn update_layer(&self) {
            use objc2::DefinedClass;
            let color = match *self.ivars() {
                0 => crate::style::Color::settings_background(),
                1 => objc2_app_kit::NSColor::controlBackgroundColor(),
                _ => objc2_app_kit::NSColor::labelColor().colorWithAlphaComponent(0.05),
            };
            if let Some(layer) = self.layer() { layer.setBackgroundColor(Some(&color.CGColor())); }
        }
    }
);

pub struct View {
    native: Retained<NSView>,
    children: RefCell<Vec<Box<dyn NativeView>>>,
}
impl View {
    pub fn new(ui: &Ui) -> Self {
        Self {
            native: NSView::new(ui.mtm()),
            children: RefCell::new(Vec::new()),
        }
    }

    /// The adaptive detail-pane surface for custom settings layouts.
    pub fn settings_background(ui: &Ui) -> Self { Self::settings_color(ui, 0) }

    /// Adaptive card fill for view-based collection cells.
    pub fn settings_card(ui: &Ui) -> Self {
        let native: Retained<SettingsCardView> = unsafe {
            msg_send![
                super(SettingsCardView::alloc(ui.mtm()).set_ivars(Cell::new(0))),
                init
            ]
        };
        Self {
            native: native.into_super(),
            children: RefCell::new(Vec::new()),
        }
    }

    pub fn set_card_edges(&self, first: bool, last: bool) {
        use objc2::DefinedClass;
        if let Some(view) = self.native.downcast_ref::<SettingsCardView>() {
            let edges = u8::from(first) | (u8::from(last) << 1);
            if view.ivars().replace(edges) != edges {
                view.setNeedsDisplay(true);
            }
        }
    }

    pub(crate) fn settings_color(ui: &Ui, kind: u8) -> Self {
        let native: Retained<SettingsColorView> = unsafe {
            msg_send![
                super(SettingsColorView::alloc(ui.mtm()).set_ivars(kind)),
                init
            ]
        };
        native.setWantsLayer(true);
        Self {
            native: native.into_super(),
            children: RefCell::new(Vec::new()),
        }
    }

    pub fn flipped(ui: &Ui) -> Self {
        let native: Retained<DocumentView> =
            unsafe { msg_send![super(DocumentView::alloc(ui.mtm()).set_ivars(())), init] };
        Self {
            native: native.into_super(),
            children: RefCell::new(Vec::new()),
        }
    }

    pub fn ns_view(&self) -> &NSView { &self.native }

    pub(crate) fn retain_child(&self, child: impl NativeView) {
        self.children.borrow_mut().push(Box::new(child));
    }

    /// Keep controls below full-size window chrome using AppKit's safe area.
    pub fn safe_area_content(self, child: impl NativeView) -> Self {
        self.native.addSubview(child.ns_view());
        prepare(child.ns_view());
        let guide = self.native.safeAreaLayoutGuide();
        child
            .ns_view()
            .topAnchor()
            .constraintEqualToAnchor(&guide.topAnchor())
            .setActive(true);
        child
            .ns_view()
            .leadingAnchor()
            .constraintEqualToAnchor(&guide.leadingAnchor())
            .setActive(true);
        child
            .ns_view()
            .trailingAnchor()
            .constraintEqualToAnchor(&guide.trailingAnchor())
            .setActive(true);
        child
            .ns_view()
            .bottomAnchor()
            .constraintEqualToAnchor(&guide.bottomAnchor())
            .setActive(true);
        self.children.borrow_mut().push(Box::new(child));
        self
    }

    pub fn content(self, child: impl NativeView, insets: Insets) -> Self {
        self.native.addSubview(child.ns_view());
        pin(&self.native, child.ns_view(), insets);
        self.children.borrow_mut().push(Box::new(child));
        self
    }
}
impl NativeView for View {
    fn ns_view(&self) -> &NSView { &self.native }
}
