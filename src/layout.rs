use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::*;
use objc2_foundation::NSArray;

use crate::view::prepare;
use crate::{Insets, Metrics, NativeView, Ui, View};

pub struct Stack {
    native: Retained<NSStackView>,
    children: RefCell<Vec<Box<dyn NativeView>>>,
    owners: Vec<Box<dyn std::any::Any>>,
}
impl Stack {
    pub fn new(ui: &Ui, orientation: NSUserInterfaceLayoutOrientation) -> Self {
        let native = NSStackView::new(ui.mtm());
        prepare(&native);
        native.setOrientation(orientation);
        native.setSpacing(Metrics::CONTROL_SPACING);
        native.setAlignment(if orientation == NSUserInterfaceLayoutOrientation::Vertical {
            NSLayoutAttribute::Leading
        } else {
            NSLayoutAttribute::CenterY
        });
        Self {
            native,
            children: RefCell::new(Vec::new()),
            owners: Vec::new(),
        }
    }

    pub fn ns_stack_view(&self) -> &NSStackView { &self.native }

    /// Keep a non-view owner, such as a control wrapper whose view sits in a composite,
    /// alive for as long as the stack.
    pub fn keep(mut self, owner: impl std::any::Any) -> Self {
        self.owners.push(Box::new(owner));
        self
    }

    pub fn spacing(self, value: f64) -> Self {
        self.native.setSpacing(value);
        self
    }

    pub fn alignment(self, value: NSLayoutAttribute) -> Self {
        self.native.setAlignment(value);
        self
    }

    pub fn distribution(self, value: NSStackViewDistribution) -> Self {
        self.native.setDistribution(value);
        self
    }

    pub fn insets(self, value: Insets) -> Self {
        self.native.setEdgeInsets(value);
        self
    }

    pub fn push(self, view: impl NativeView) -> Self {
        self.add(view);
        self
    }

    pub fn add(&self, view: impl NativeView) {
        prepare(view.ns_view());
        self.native.addArrangedSubview(view.ns_view());
        if self.native.orientation() == NSUserInterfaceLayoutOrientation::Vertical {
            let insets = self.native.edgeInsets();
            view.ns_view()
                .widthAnchor()
                .constraintEqualToAnchor_constant(
                    &self.native.widthAnchor(),
                    -insets.left - insets.right,
                )
                .setActive(true);
        }
        self.children.borrow_mut().push(Box::new(view));
    }

    pub fn clear(&self) {
        for child in self.children.borrow_mut().drain(..) {
            self.native.removeArrangedSubview(child.ns_view());
            child.ns_view().removeFromSuperview();
        }
    }

    pub fn spacer(self, ui: &Ui) -> Self { self.push(Spacer::new(ui)) }
}
impl NativeView for Stack {
    fn ns_view(&self) -> &NSView { &self.native }
}
macro_rules! stack {
    ($name:ident, $axis:ident) => {
        pub struct $name(Stack);
        impl $name {
            pub fn new(ui: &Ui) -> Self {
                Self(Stack::new(ui, NSUserInterfaceLayoutOrientation::$axis))
            }

            pub fn spacing(self, value: f64) -> Self { Self(self.0.spacing(value)) }

            pub fn alignment(self, value: NSLayoutAttribute) -> Self {
                Self(self.0.alignment(value))
            }

            pub fn distribution(self, value: NSStackViewDistribution) -> Self {
                Self(self.0.distribution(value))
            }

            pub fn insets(self, value: Insets) -> Self { Self(self.0.insets(value)) }

            pub fn push(self, view: impl NativeView) -> Self { Self(self.0.push(view)) }

            pub fn add(&self, view: impl NativeView) { self.0.add(view); }

            pub fn clear(&self) { self.0.clear(); }

            pub fn spacer(self, ui: &Ui) -> Self { Self(self.0.spacer(ui)) }

            pub fn keep(self, owner: impl std::any::Any) -> Self { Self(self.0.keep(owner)) }

            pub fn ns_stack_view(&self) -> &NSStackView { self.0.ns_stack_view() }
        }
        impl NativeView for $name {
            fn ns_view(&self) -> &NSView { self.0.ns_view() }
        }
    };
}
stack!(VStack, Vertical);
stack!(HStack, Horizontal);

pub struct Spacer(View);
impl Spacer {
    pub fn new(ui: &Ui) -> Self {
        let view = View::new(ui);
        prepare(view.ns_view());
        for axis in [
            NSLayoutConstraintOrientation::Horizontal,
            NSLayoutConstraintOrientation::Vertical,
        ] {
            view.ns_view().setContentHuggingPriority_forOrientation(1.0, axis);
            view.ns_view().setContentCompressionResistancePriority_forOrientation(1.0, axis);
        }
        Self(view)
    }
}
impl NativeView for Spacer {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }
}

pub struct Grid {
    native: Retained<NSGridView>,
    children: Vec<Box<dyn NativeView>>,
}
impl Grid {
    pub fn new(ui: &Ui) -> Self {
        let native = NSGridView::new(ui.mtm());
        prepare(&native);
        native.setXPlacement(NSGridCellPlacement::Leading);
        native.setYPlacement(NSGridCellPlacement::Center);
        Self { native, children: Vec::new() }
    }

    pub fn row(mut self, cells: Vec<Box<dyn NativeView>>) -> Self {
        let views: Vec<&NSView> = cells.iter().map(|v| v.ns_view()).collect();
        self.native.addRowWithViews(&NSArray::from_slice(&views));
        self.children.extend(cells);
        self
    }

    pub fn spacing(self, rows: f64, columns: f64) -> Self {
        self.native.setRowSpacing(rows);
        self.native.setColumnSpacing(columns);
        self
    }

    /// Fixed column widths, leading columns first.
    pub fn column_widths(self, widths: &[f64]) -> Self {
        for (index, width) in widths.iter().enumerate() {
            self.native.columnAtIndex(index as isize).setWidth(*width);
        }
        self
    }

    /// Align each row's cells on their first text baseline.
    pub fn first_baseline(self) -> Self {
        self.native.setRowAlignment(objc2_app_kit::NSGridRowAlignment::FirstBaseline);
        self
    }

    /// Share the available width evenly, without fixing or limiting the grid’s size.
    pub fn equal_columns(self) -> Self {
        self.native.setXPlacement(NSGridCellPlacement::Fill);
        if let Some(first) = self.children.first() {
            for cell in
                self.children.iter().skip(1).take(self.native.numberOfColumns() as usize - 1)
            {
                cell.ns_view()
                    .widthAnchor()
                    .constraintEqualToAnchor(&first.ns_view().widthAnchor())
                    .setActive(true);
            }
        }
        self
    }

    pub fn ns_grid_view(&self) -> &NSGridView { &self.native }
}
impl NativeView for Grid {
    fn ns_view(&self) -> &NSView { &self.native }
}

pub struct Form {
    ui: Ui,
    grid: Grid,
}
impl Form {
    pub fn new(ui: &Ui) -> Self {
        Self {
            ui: *ui,
            grid: Grid::new(ui).spacing(Metrics::ROW_SPACING, Metrics::CONTROL_SPACING),
        }
    }

    pub fn row(mut self, title: &str, control: impl NativeView) -> Self {
        self.grid = self.grid.row(vec![
            Box::new(crate::Label::new(&self.ui, title)),
            Box::new(control),
        ]);
        self
    }

    pub fn help(mut self, text: &str) -> Self {
        self.grid = self.grid.row(vec![
            Box::new(crate::Label::new(&self.ui, "")),
            Box::new(crate::SecondaryLabel::new(&self.ui, text)),
        ]);
        self
    }

    pub fn ns_grid_view(&self) -> &NSGridView { self.grid.ns_grid_view() }
}
impl NativeView for Form {
    fn ns_view(&self) -> &NSView { self.grid.ns_view() }
}

pub struct Divider(Retained<NSBox>);
impl Divider {
    pub fn new(ui: &Ui) -> Self {
        let native = NSBox::new(ui.mtm());
        native.setBoxType(NSBoxType::Separator);
        Self(native)
    }

    pub fn ns_box(&self) -> &NSBox { &self.0 }
}
impl NativeView for Divider {
    fn ns_view(&self) -> &NSView { &self.0 }
}

/// A quiet native group surface; semantic fill follows the window appearance.
pub struct GroupBox {
    native: Retained<NSBox>,
    _content: Box<dyn NativeView>,
}
impl GroupBox {
    pub fn new(ui: &Ui, content: impl NativeView) -> Self {
        Self::with_insets(ui, content, Insets {
            top: 14.0,
            left: 16.0,
            bottom: 14.0,
            right: 16.0,
        })
    }

    pub fn with_insets(ui: &Ui, content: impl NativeView, insets: Insets) -> Self {
        let native = NSBox::new(ui.mtm());
        native.setBoxType(NSBoxType::Custom);
        native.setTitlePosition(NSTitlePosition::NoTitle);
        native.setBorderWidth(0.0);
        native.setCornerRadius(10.0);
        native.setFillColor(&NSColor::quaternarySystemFillColor());
        // Animate the group boundary with an explicitly layer-backed child subtree.
        native.setWantsLayer(content.ns_view().wantsLayer());
        native.setContentViewMargins(crate::CGSize::new(0.0, 0.0));
        let host = native.contentView().unwrap();
        host.addSubview(content.ns_view());
        crate::view::pin(&host, content.ns_view(), insets);
        Self {
            native,
            _content: Box::new(content),
        }
    }
}
impl NativeView for GroupBox {
    fn ns_view(&self) -> &NSView { &self.native }
}

define_class!(
    #[unsafe(super(NSScrollView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgUiScrollView"]
    #[ivars = Cell<bool>]
    struct NativeScrollView;

    impl NativeScrollView {
        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            if self.ivars().get() {
                if let Some(parent) = unsafe { self.superview() } {
                    parent.scrollWheel(event);
                }
            } else {
                unsafe { let _: () = msg_send![super(self), scrollWheel: event]; }
            }
        }
    }
);

pub struct ScrollView {
    native: Retained<NativeScrollView>,
    content: Box<dyn NativeView>,
}
impl ScrollView {
    pub fn new(ui: &Ui, content: impl NativeView) -> Self {
        let native: Retained<NativeScrollView> = unsafe {
            msg_send![
                super(NativeScrollView::alloc(ui.mtm()).set_ivars(Cell::new(false))),
                init
            ]
        };
        native.setHasVerticalScroller(true);
        native.setAutohidesScrollers(true);
        native.setDrawsBackground(false);
        native.setDocumentView(Some(content.ns_view()));
        Self {
            native,
            content: Box::new(content),
        }
    }

    pub fn ns_scroll_view(&self) -> &NSScrollView { &self.native }

    /// Forward wheel input to the enclosing scroll view when content is fully expanded.
    pub fn defer_scrolling(&self) {
        self.native.ivars().set(true);
        self.native.setHasVerticalScroller(false);
        self.native.setHasHorizontalScroller(false);
        self.native.setVerticalScrollElasticity(NSScrollElasticity::None);
        self.native.setHorizontalScrollElasticity(NSScrollElasticity::None);
    }

    pub fn content_view(&self) -> &NSView { self.content.ns_view() }

    /// Pin the document to the viewport width while allowing vertical scrolling.
    pub fn fit_width(&self) {
        let clip = self.native.contentView();
        let child = self.content.ns_view();
        prepare(child);
        child
            .leadingAnchor()
            .constraintEqualToAnchor(&clip.leadingAnchor())
            .setActive(true);
        child.topAnchor().constraintEqualToAnchor(&clip.topAnchor()).setActive(true);
        child.widthAnchor().constraintEqualToAnchor(&clip.widthAnchor()).setActive(true);
    }
}
impl NativeView for ScrollView {
    fn ns_view(&self) -> &NSView { &self.native }
}

// A disclosure header is one native button target, including its labels and whitespace.
define_class!(
    #[unsafe(super(NSButton))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsDisclosureHeader"]
    #[ivars = Retained<NSStackView>]
    struct DisclosureHeader;
    impl DisclosureHeader {
        #[unsafe(method(intrinsicContentSize))]
        fn intrinsic_size(&self) -> crate::CGSize { self.ivars().fittingSize() }
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: crate::CGPoint) -> *mut NSView {
            let hit: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
            if hit.is_some() { (self as *const Self).cast_mut().cast() } else { std::ptr::null_mut() }
        }
    }
);

pub struct Disclosure {
    stack: VStack,
    button: crate::Button,
    body: Box<dyn NativeView>,
    header: Rc<VStack>,
    heading: Rc<HStack>,
    arrow: crate::Button,
    symbol: Option<crate::ImageView>,
    summary: Option<crate::SecondaryLabel>,
    toggle: Rc<RefCell<Box<dyn FnMut(bool) -> bool>>>,
}
impl Disclosure {
    pub fn new(ui: &Ui, title: &str, content: impl NativeView) -> Self {
        use objc2::rc::Weak;
        let body = Box::new(content);
        body.set_hidden(true);
        let weak = Weak::new(body.ns_view());
        let arrow = crate::Button::new(ui, "").disclosure();
        arrow.ns_view().setAccessibilityElement(false);
        let header =
            Rc::new(VStack::new(ui).spacing(3.0).push(crate::SectionTitle::new(ui, title)));
        let heading = Rc::new(HStack::new(ui).push(arrow.ns_view().retain()).push(header.clone()));
        heading.ns_view().setAccessibilityElement(false);
        let native: Retained<DisclosureHeader> = unsafe {
            msg_send![
                super(
                    DisclosureHeader::alloc(ui.mtm()).set_ivars(heading.ns_stack_view().retain())
                ),
                init
            ]
        };
        prepare(&native);
        native.setButtonType(NSButtonType::PushOnPushOff);
        native.setBordered(false);
        native.setTitle(&objc2_foundation::NSString::from_str(""));
        native.addSubview(heading.ns_view());
        crate::view::pin(&native, heading.ns_view(), crate::uniform_insets(0.0));
        let button = crate::Button::from_native(ui, native.into_super());
        let weak_button = Weak::new(button.ns_button());
        let weak_arrow = Weak::new(arrow.ns_button());
        let toggle: Rc<RefCell<Box<dyn FnMut(bool) -> bool>>> =
            Rc::new(RefCell::new(Box::new(|_| true)));
        let callback = toggle.clone();
        let button = button.on_click(move || {
            if let Some(view) = weak.load() {
                let expanded = view.isHidden();
                let root = view.window().and_then(|window| window.contentView());
                if let Some(root) = &root {
                    root.layoutSubtreeIfNeeded();
                }
                NSAnimationContext::beginGrouping();
                let context = NSAnimationContext::currentContext();
                let animate = view.window().is_some_and(|window| window.isVisible())
                    && !NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion();
                context.setDuration(if animate { 0.18 } else { 0.0 });
                context.setAllowsImplicitAnimation(animate);
                if callback.borrow_mut()(expanded) {
                    view.setHidden(!expanded);
                    if let Some(arrow) = weak_arrow.load() {
                        arrow.setState(if expanded { 1 } else { 0 });
                    }
                } else if let Some(button) = weak_button.load() {
                    button.setState(if expanded { 0 } else { 1 });
                }
                if let Some(root) = &root {
                    root.layoutSubtreeIfNeeded();
                }
                NSAnimationContext::endGrouping();
            }
        });
        button.set_title("");
        button.accessibility_label(title);
        let stack = VStack::new(ui);
        // One backing subtree lets AppKit interpolate the header, body, and group geometry
        // in the same Auto Layout transaction, without separate fades or rendering timers.
        stack.ns_view().setWantsLayer(true);
        stack.add(button.ns_view().retain());
        stack.add(body.ns_view().retain());
        Self {
            stack,
            button,
            body,
            header,
            heading,
            arrow,
            symbol: None,
            summary: None,
            toggle,
        }
    }

    /// Prepare lazy content before expansion, or decline collapsing an invalid editor.
    pub fn on_toggle(self, callback: impl FnMut(bool) -> bool + 'static) -> Self {
        *self.toggle.borrow_mut() = Box::new(callback);
        self
    }

    /// Keep the header’s padding inside its native button hit target.
    pub fn header_insets(self, insets: Insets) -> Self {
        self.heading.ns_stack_view().setEdgeInsets(insets);
        self.button.ns_view().invalidateIntrinsicContentSize();
        self
    }

    pub fn description(self, ui: &Ui, text: &str) -> Self {
        self.header.add(crate::Caption::new(ui, text));
        self
    }

    /// Show the disclosure indicator at the trailing edge, so the title shares the
    /// leading edge of neighbouring settings rows.
    pub fn trailing_indicator(self, ui: &Ui) -> Self {
        let stack = self.heading.ns_stack_view();
        stack.removeArrangedSubview(self.arrow.ns_view());
        stack.addArrangedSubview(crate::Spacer::new(ui).ns_view());
        stack.addArrangedSubview(self.arrow.ns_view());
        self
    }

    /// A one-line summary of the hidden settings at the header’s trailing edge, such as
    /// the values they currently hold, so collapsed content stays informative.
    pub fn summary(mut self, ui: &Ui) -> Self {
        let label = crate::SecondaryLabel::single_line(ui, "");
        let stack = self.heading.ns_stack_view();
        let arrow = self.arrow.ns_view();
        let trailing = stack.arrangedSubviews().lastObject().is_some_and(|last| &*last == arrow);
        if trailing {
            stack.insertArrangedSubview_atIndex(
                label.ns_view(),
                stack.arrangedSubviews().len() as isize - 1,
            );
        } else {
            stack.addArrangedSubview(crate::Spacer::new(ui).ns_view());
            stack.addArrangedSubview(label.ns_view());
        }
        label.ns_view().setAccessibilityElement(false);
        self.summary = Some(label);
        self
    }

    pub fn set_summary(&self, text: &str) {
        if let Some(label) = &self.summary {
            label.set_text(text);
            self.button.ns_button().setAccessibilityValueDescription(Some(
                &objc2_foundation::NSString::from_str(text),
            ));
        }
    }

    /// A row-weight title for disclosures listed among ordinary settings rows.
    pub fn compact(self) -> Self {
        if let Some(title) = self.header.ns_stack_view().arrangedSubviews().firstObject()
            && let Some(title) = title.downcast_ref::<objc2_app_kit::NSTextField>()
        {
            title.setFont(Some(&crate::Font::body()));
        }
        self
    }

    pub fn symbol(mut self, ui: &Ui, name: &str) -> Self {
        if let Some(image) = crate::ImageView::symbol(ui, name) {
            image.width(22.0);
            image.height(22.0);
            image.ns_image_view().setContentTintColor(Some(&NSColor::labelColor()));
            self.heading.ns_stack_view().insertArrangedSubview_atIndex(image.ns_view(), 1);
            self.symbol = Some(image);
        }
        self
    }

    pub fn set_expanded(&self, value: bool) {
        self.body.set_hidden(!value);
        self.arrow.ns_button().setState(if value {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        self.button.ns_button().setState(if value {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
    }

    pub fn ns_button(&self) -> &NSButton { self.button.ns_button() }
}
impl NativeView for Disclosure {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}

/// Open only disclosures containing the requested setting. Native clicks run
/// lazy-content callbacks and keep the header's accessibility state in sync.
pub(crate) fn reveal_disclosures(view: &NSView, title: &str) {
    fn contains(view: &NSView, title: &str) -> bool {
        view.accessibilityLabel().is_some_and(|name| {
            let name = name.to_string();
            name == title || name.starts_with(&format!("{title} ("))
        }) || view
            .downcast_ref::<NSTextField>()
            .is_some_and(|field| field.stringValue().to_string() == title)
            || view.subviews().iter().any(|child| contains(&child, title))
    }
    if view.isHidden() {
        return;
    }
    let children = view.subviews();
    if let Some(header) =
        children.iter().find_map(|child| child.downcast::<DisclosureHeader>().ok())
    {
        if children.iter().any(|child| child.isHidden() && contains(&child, title)) {
            // The mounted disclosure owns its target and content for this call.
            unsafe { header.performClick(None) };
        }
    }
    for child in view.subviews() {
        reveal_disclosures(&child, title);
    }
}

define_class!(
    #[unsafe(super(NSStackView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsResponsiveStack"]
    #[ivars = ResponsiveLayout]
    struct ResponsiveStackView;
    impl ResponsiveStackView {
        #[unsafe(method(layout))]
        fn layout_stack(&self) {
            // A horizontal stack can inflate its ancestor's fitting width before
            // the window shrinks. Check the actual window edge too, so an
            // oversized layout cannot keep this stack stuck in its wide mode.
            let mut width = self.bounds().size.width;
            if let Some(window) = self.window() {
                let origin = self.convertPoint_toView(crate::CGPoint::new(0.0, 0.0), None);
                width = width.min(window.contentRectForFrameRect(window.frame()).size.width - origin.x);
            }
            let ivars = self.ivars();
            let orientation = if width >= ivars.breakpoint { NSUserInterfaceLayoutOrientation::Horizontal } else { NSUserInterfaceLayoutOrientation::Vertical };
            let compact = orientation == NSUserInterfaceLayoutOrientation::Vertical;
            if ivars.hide_trailing.get()
                && let Some(last) = self.arrangedSubviews().lastObject()
                && last.isHidden() != compact
            {
                last.setHidden(compact);
            }
            if self.orientation() != orientation {
                self.setOrientation(orientation);
                let side_by_side = if ivars.centered.get() { NSLayoutAttribute::CenterY } else { NSLayoutAttribute::Top };
                self.setAlignment(if orientation == NSUserInterfaceLayoutOrientation::Horizontal { side_by_side } else { NSLayoutAttribute::Leading });
                self.invalidateIntrinsicContentSize();
            }
            unsafe { let _: () = msg_send![super(self), layout]; }
        }
    }
);
struct ResponsiveLayout {
    breakpoint: f64,
    /// Center side-by-side content vertically, rather than aligning its tops.
    centered: Cell<bool>,
    /// Hide the last item, such as an optional preview, instead of stacking it.
    hide_trailing: Cell<bool>,
}

/// Places content side by side when it fits, and vertically at smaller widths.
pub struct ResponsiveStack {
    native: Retained<ResponsiveStackView>,
    children: Vec<Box<dyn NativeView>>,
}
impl ResponsiveStack {
    pub fn new(ui: &Ui, wide_min_width: f64) -> Self {
        let native: Retained<ResponsiveStackView> = unsafe {
            msg_send![
                super(ResponsiveStackView::alloc(ui.mtm()).set_ivars(ResponsiveLayout {
                    breakpoint: wide_min_width,
                    centered: Cell::new(false),
                    hide_trailing: Cell::new(false),
                })),
                init
            ]
        };
        prepare(&native);
        native.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        native.setAlignment(NSLayoutAttribute::Leading);
        native.setSpacing(16.0);
        Self { native, children: Vec::new() }
    }

    /// Center content vertically while it sits side by side.
    pub fn centered(self) -> Self {
        self.native.ivars().centered.set(true);
        self
    }

    /// At narrow widths, hide the last item rather than placing it below the others,
    /// for supplementary content such as a preview.
    pub fn hiding_trailing(self) -> Self {
        self.native.ivars().hide_trailing.set(true);
        self
    }

    pub fn push(mut self, content: impl NativeView) -> Self {
        prepare(content.ns_view());
        self.native.addArrangedSubview(content.ns_view());
        self.children.push(Box::new(content));
        self
    }
}
impl NativeView for ResponsiveStack {
    fn ns_view(&self) -> &NSView { &self.native }
}
