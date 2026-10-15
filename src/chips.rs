//! A short, editable list of numbers shown as chips.
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSBezelStyle, NSButton, NSCellImagePosition, NSColor, NSEvent, NSEventMask, NSEventType,
    NSImageSymbolConfiguration, NSLayoutConstraintOrientation, NSTrackingArea,
    NSTrackingAreaOptions, NSView, NSWorkspace,
};
use objc2_foundation::NSString;

use crate::{
    Application, Button, CGPoint, CGRect, CGSize, HStack, NativeView, NumberField, Popover, Slider,
    Spacer, Symbol, Ui, VStack, View,
};

/// A change the user made to a [`ValueChips`] list, in display units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChipEdit {
    Add(f64),
    Change(usize, f64),
    Remove(usize),
    /// Move a value to its final index in the list.
    Move(usize, usize),
}

// Chip positions are independent of Auto Layout, so their fixed widths cannot
// force the preferences window wider. The row contributes only its flowing height.
define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsChipFlow"]
    #[ivars = FlowIvars]
    struct ChipFlow;
    impl ChipFlow {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool { true }

        #[unsafe(method(intrinsicContentSize))]
        fn intrinsic_size(&self) -> CGSize {
            let children = self.subviews();
            let width = children.iter().map(|view| view.fittingSize().width).sum::<f64>()
                + 6.0 * children.len().saturating_sub(1) as f64;
            let widest = children.iter().map(|view| view.fittingSize().width).fold(0.0, f64::max);
            // A long list grows downward instead of inflating NSGridView's
            // minimum fitting width. Keep a single chip fully readable.
            let ivars = self.ivars();
            CGSize::new(width.min(ivars.preferred_width.get()).max(widest), ivars.height.get())
        }

        #[unsafe(method(layout))]
        fn layout_chips(&self) {
            unsafe { let _: () = msg_send![super(self), layout]; }
            let available = self.bounds().size.width.max(1.0);
            // Fill rows left to right, then shift each to the trailing edge if asked.
            let mut rows: Vec<Vec<(Retained<NSView>, CGSize)>> = vec![Vec::new()];
            let mut x = 0.0;
            for child in self.subviews() {
                let size = child.fittingSize();
                if x > 0.0 && x + size.width > available + 0.5 {
                    rows.push(Vec::new());
                    x = 0.0;
                }
                x += size.width + 6.0;
                rows.last_mut().unwrap().push((child, size));
            }
            let mut y = 0.0;
            for row in &rows {
                let width = row.iter().map(|(_, size)| size.width + 6.0).sum::<f64>() - 6.0;
                let mut x = if self.ivars().trailing.get() { (available - width).max(0.0) } else { 0.0 };
                let height = row.iter().map(|(_, size)| size.height).fold(0.0, f64::max);
                for (child, size) in row {
                    child.setFrame(CGRect::new(CGPoint::new(x, y), *size));
                    x += size.width + 6.0;
                }
                y += height + 6.0;
            }
            let height = (y - 6.0).max(0.0);
            if self.ivars().height.replace(height) != height {
                self.invalidateIntrinsicContentSize();
            }
        }
    }
);

struct FlowIvars {
    height: Cell<f64>,
    /// Align each line with the trailing edge, as a trailing settings control.
    trailing: Cell<bool>,
    /// The widest the row asks to be before it wraps.
    preferred_width: Cell<f64>,
}

impl ChipFlow {
    fn new(ui: &Ui) -> Retained<Self> {
        let row: Retained<Self> = unsafe {
            msg_send![
                super(Self::alloc(ui.mtm()).set_ivars(FlowIvars {
                    height: Cell::new(24.0),
                    trailing: Cell::new(false),
                    preferred_width: Cell::new(220.0),
                })),
                init
            ]
        };
        crate::view::prepare(&row);
        row.setContentCompressionResistancePriority_forOrientation(
            250.0,
            NSLayoutConstraintOrientation::Horizontal,
        );
        // A compact two-column flow remains readable even when the form would
        // otherwise shrink this control to the width of its largest chip.
        row.widthAnchor().constraintGreaterThanOrEqualToConstant(180.0).setActive(true);
        row
    }

    fn clear(&self) {
        for view in self.subviews() {
            view.removeFromSuperview();
        }
    }

    fn add(&self, view: &NSView) {
        view.setTranslatesAutoresizingMaskIntoConstraints(true);
        self.addSubview(view);
        self.setNeedsLayout(true);
        self.invalidateIntrinsicContentSize();
    }
}

pub struct ChipIvars {
    owner: Weak<Inner>,
    index: usize,
    remove: RefCell<Option<Retained<NSButton>>>,
    area: RefCell<Option<Retained<NSTrackingArea>>>,
}

define_class!(
    // A push button that reveals its remove control while the pointer is over it.
    #[unsafe(super(NSButton))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsValueChip"]
    #[ivars = ChipIvars]
    struct ChipButton;
    impl ChipButton {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            if let Some(owner) = self.ivars().owner.upgrade() {
                owner.track(self, self.ivars().index, event);
            }
        }
        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking_areas(&self) {
            unsafe { let _: () = msg_send![super(self), updateTrackingAreas]; }
            if self.ivars().area.borrow().is_some() { return; }
            let area = unsafe {
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    NSTrackingArea::alloc(),
                    CGRect::ZERO,
                    NSTrackingAreaOptions::MouseEnteredAndExited
                        | NSTrackingAreaOptions::ActiveInKeyWindow
                        | NSTrackingAreaOptions::InVisibleRect,
                    Some(self),
                    None,
                )
            };
            self.addTrackingArea(&area);
            *self.ivars().area.borrow_mut() = Some(area);
        }
        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) { self.reveal(true); }
        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) { self.reveal(false); }
    }
);

impl ChipButton {
    fn reveal(&self, visible: bool) {
        let Some(remove) = self.ivars().remove.borrow().clone() else {
            return;
        };
        let alpha = if visible && remove.isEnabled() {
            1.0
        } else {
            0.0
        };
        if NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion() {
            remove.setAlphaValue(alpha);
        } else {
            unsafe {
                let animator: Retained<AnyObject> = msg_send![&*remove, animator];
                let _: () = msg_send![&*animator, setAlphaValue: alpha];
            }
        }
    }
}

/// One value: the chip and the remove control inside its leading edge.
struct Chip {
    button: Button,
    remove: Button,
    width: Retained<objc2_app_kit::NSLayoutConstraint>,
}
impl NativeView for Chip {
    fn ns_view(&self) -> &NSView { self.button.ns_view() }
}

type Format = RefCell<Box<dyn Fn(f64) -> String>>;
type OnEdit = RefCell<Option<Rc<dyn Fn(ChipEdit)>>>;

struct Inner {
    ui: Ui,
    label: String,
    unit: String,
    range: (f64, f64),
    min_count: Cell<usize>,
    compact: Cell<bool>,
    row: Retained<ChipFlow>,
    add: Rc<Button>,
    chips: RefCell<Vec<Rc<Chip>>>,
    values: RefCell<Vec<f64>>,
    rendered: Cell<bool>,
    format: Format,
    on_edit: OnEdit,
    popover: RefCell<Option<Popover>>,
}

/// A short list of numbers as native chips. Click a chip to adjust it with a slider or
/// an exact value, hover it to reveal its remove control, or add a value with the
/// trailing button, choosing it before it's added. Drag chips to reorder them.
/// Edits are reported, not applied:
/// the owner updates its model and calls [`ValueChips::set_values`].
pub struct ValueChips(Rc<Inner>);
impl ValueChips {
    /// `label` names one value for accessibility, e.g. “Width preset”.
    pub fn new(ui: &Ui, label: &str, unit: &str, min: f64, max: f64) -> Self {
        assert!(min.is_finite() && max.is_finite() && min < max);
        Self(Rc::new_cyclic(|weak: &Weak<Inner>| {
            let owner = weak.clone();
            let add = Button::new(ui, "").on_click(move || {
                if let Some(inner) = owner.upgrade() {
                    inner.open(None);
                }
            });
            let native = add.ns_button();
            native.setBezelStyle(NSBezelStyle::Push);
            native.setImage(Symbol::named("plus").as_deref());
            native.setImagePosition(NSCellImagePosition::ImageOnly);
            add.accessibility_label(&format!("Add {}", label.to_lowercase()));
            add.tooltip(&format!("Add {}", label.to_lowercase()));
            Inner {
                ui: *ui,
                label: label.into(),
                unit: unit.into(),
                range: (min, max),
                min_count: Cell::new(1),
                compact: Cell::new(false),
                row: ChipFlow::new(ui),
                add: Rc::new(add),
                chips: RefCell::new(Vec::new()),
                values: RefCell::new(Vec::new()),
                rendered: Cell::new(false),
                format: RefCell::new(Box::new(|value| format!("{value}"))),
                on_edit: RefCell::new(None),
                popover: RefCell::new(None),
            }
        }))
    }

    /// How a value appears on its chip, without the unit.
    pub fn format(self, format: impl Fn(f64) -> String + 'static) -> Self {
        *self.0.format.borrow_mut() = Box::new(format);
        self.0.rendered.set(false);
        self
    }

    /// Small chips that stay on one line while it fits, for a trailing settings control.
    pub fn compact(self) -> Self {
        self.0.compact.set(true);
        self.0.row.ivars().preferred_width.set(f64::MAX);
        self.0.row.ivars().trailing.set(true);
        // Stay on one line while there's room, but wrap rather than set a minimum width:
        // below the fitting priority, so the row never widens its window.
        self.0.row.setContentCompressionResistancePriority_forOrientation(
            49.0,
            NSLayoutConstraintOrientation::Horizontal,
        );
        self.0.add.ns_button().setControlSize(objc2_app_kit::NSControlSize::Small);
        self.0.rendered.set(false);
        self
    }

    /// The fewest values allowed; their remove controls stay hidden at this count.
    pub fn min_count(self, count: usize) -> Self {
        self.0.min_count.set(count);
        self.0.rendered.set(false);
        self
    }

    pub fn on_edit(self, f: impl Fn(ChipEdit) + 'static) -> Self {
        *self.0.on_edit.borrow_mut() = Some(Rc::new(f));
        self
    }

    /// Show `values`, keeping existing chips (and any open editor) when the count is unchanged.
    pub fn set_values(&self, values: &[f64]) {
        let inner = &self.0;
        if inner.rendered.get() && inner.values.borrow().as_slice() == values {
            return;
        }
        if inner.chips.borrow().len() != values.len() || inner.row.subviews().is_empty() {
            inner.rebuild(values.len());
        }
        let removable = values.len() > inner.min_count.get();
        for (chip, &value) in inner.chips.borrow().iter().zip(values) {
            let title = format!("{}{}", (inner.format.borrow())(value), inner.unit);
            chip.button.set_title(&title);
            chip.button.accessibility_label(&format!("{} {title}", inner.label));
            chip.remove.accessibility_label(&format!("Remove {title}"));
            chip.button.tooltip("Click to adjust; drag to reorder");
            chip.remove.ns_button().setEnabled(removable);
            if !removable {
                chip.remove.ns_view().setAlphaValue(0.0);
            }
        }
        // One width for every chip, so a list reads as an even row of tokens, with room
        // inside the leading edge for the remove control, keeping titles centered.
        let room = if inner.compact.get() { 16.0 } else { 18.0 };
        let chips = inner.chips.borrow();
        let width = chips
            .iter()
            .map(|chip| chip.button.ns_view().intrinsicContentSize().width)
            .fold(0.0, f64::max);
        for chip in chips.iter() {
            chip.width.setConstant(width + room);
        }
        drop(chips);
        *inner.values.borrow_mut() = values.to_vec();
        inner.rendered.set(true);
        inner.row.setNeedsLayout(true);
        inner.row.invalidateIntrinsicContentSize();
    }
}
impl NativeView for ValueChips {
    fn ns_view(&self) -> &NSView { &self.0.row }
}

impl Inner {
    fn emit(&self, edit: ChipEdit) {
        let callback = self.on_edit.borrow().clone();
        if let Some(callback) = callback {
            callback(edit);
        }
    }

    fn rebuild(self: &Rc<Self>, count: usize) {
        self.row.clear();
        let mut chips = self.chips.borrow_mut();
        chips.clear();
        for index in 0..count {
            let native: Retained<ChipButton> = unsafe {
                msg_send![
                    super(ChipButton::alloc(self.ui.mtm()).set_ivars(ChipIvars {
                        owner: Rc::downgrade(self),
                        index,
                        remove: RefCell::new(None),
                        area: RefCell::new(None),
                    })),
                    init
                ]
            };
            native.setBezelStyle(NSBezelStyle::Push);
            if self.compact.get() {
                native.setControlSize(objc2_app_kit::NSControlSize::Small);
                native.setFont(Some(&crate::Font::caption()));
            }
            let owner = Rc::downgrade(self);
            let button =
                Button::from_native(&self.ui, native.clone().into_super()).on_click(move || {
                    if let Some(inner) = owner.upgrade() {
                        inner.open(Some(index));
                    }
                });
            let owner = Rc::downgrade(self);
            let remove = Button::new(&self.ui, "").on_click(move || {
                if let Some(inner) = owner.upgrade() {
                    inner.emit(ChipEdit::Remove(index));
                }
            });
            let icon = remove.ns_button();
            let configuration =
                NSImageSymbolConfiguration::configurationWithPointSize_weight(10.0, unsafe {
                    objc2_app_kit::NSFontWeightSemibold
                });
            icon.setImage(
                Symbol::named("xmark.circle.fill")
                    .and_then(|image| image.imageWithSymbolConfiguration(&configuration))
                    .as_deref(),
            );
            icon.setImagePosition(NSCellImagePosition::ImageOnly);
            icon.setBordered(false);
            icon.setRefusesFirstResponder(true);
            icon.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
            icon.setAlphaValue(0.0);
            crate::view::prepare(icon);
            native.addSubview(icon);
            icon.leadingAnchor()
                .constraintEqualToAnchor_constant(&native.leadingAnchor(), 7.0)
                .setActive(true);
            icon.centerYAnchor()
                .constraintEqualToAnchor(&native.centerYAnchor())
                .setActive(true);
            *native.ivars().remove.borrow_mut() = Some(icon.retain());
            let width = native.widthAnchor().constraintEqualToConstant(0.0);
            width.setActive(true);
            let chip = Rc::new(Chip { button, remove, width });
            self.row.add(chip.ns_view());
            chips.push(chip);
        }
        self.row.add(self.add.ns_view());
    }

    /// Track the gesture before activating the button so dragging never opens the editor.
    fn track(&self, button: &ChipButton, from: usize, down: &NSEvent) {
        if !button.isEnabled() {
            return;
        }
        let Some(window) = button.window() else { return };
        let row: &NSView = &self.row;
        row.layoutSubtreeIfNeeded();
        // Keep the original slots stable while the chips preview their new order.
        let chips = self.chips.borrow().clone();
        if from >= chips.len() {
            return;
        }
        let centers: Vec<CGPoint> = chips
            .iter()
            .map(|chip| {
                let frame = chip.ns_view().frame();
                CGPoint::new(
                    frame.origin.x + frame.size.width / 2.0,
                    frame.origin.y + frame.size.height / 2.0,
                )
            })
            .collect();
        let start = down.locationInWindow();
        let mut dragging = false;
        let mut to = from;
        let mut released = false;
        let mut inside = false;
        button.setHighlighted(true);
        while let Some(event) =
            window.nextEventMatchingMask(NSEventMask::LeftMouseDragged | NSEventMask::LeftMouseUp)
        {
            let location = event.locationInWindow();
            let point = row.convertPoint_fromView(location, None);
            let bounds = row.bounds();
            inside = point.x >= bounds.origin.x
                && point.x <= bounds.origin.x + bounds.size.width
                && point.y >= bounds.origin.y
                && point.y <= bounds.origin.y + bounds.size.height;
            if !dragging && (location.x - start.x).hypot(location.y - start.y) >= 5.0 {
                dragging = true;
                button.setHighlighted(false);
                button.reveal(false);
                button.setAlphaValue(0.6);
                haptic(objc2_app_kit::NSHapticFeedbackPattern::Generic);
                if let Some(popover) = self.popover.borrow_mut().take() {
                    popover.close();
                }
            }
            if dragging {
                let destination = centers
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| {
                        (a.x - point.x)
                            .hypot(a.y - point.y)
                            .total_cmp(&(b.x - point.x).hypot(b.y - point.y))
                    })
                    .map_or(from, |(index, _)| index);
                if destination != to {
                    to = destination;
                    let mut order: Vec<usize> = (0..chips.len()).collect();
                    order.remove(from);
                    order.insert(to, from);
                    self.arrange(&chips, &order);
                    // Each new slot clicks into place, as a stepped slider does.
                    haptic(objc2_app_kit::NSHapticFeedbackPattern::Alignment);
                }
            } else {
                let point = button.convertPoint_fromView(location, None);
                let bounds = button.bounds();
                button.setHighlighted(
                    point.x >= 0.0
                        && point.x <= bounds.size.width
                        && point.y >= 0.0
                        && point.y <= bounds.size.height,
                );
            }
            if event.r#type() == NSEventType::LeftMouseUp {
                released = true;
                break;
            }
        }
        let clicked = button.isHighlighted();
        button.setHighlighted(false);
        button.setAlphaValue(1.0);
        if dragging {
            // Restore slot identities before the owner synchronizes the saved values.
            self.arrange(&chips, &(0..chips.len()).collect::<Vec<_>>());
            if released && inside && from != to {
                haptic(objc2_app_kit::NSHapticFeedbackPattern::LevelChange);
                self.emit(ChipEdit::Move(from, to));
            }
        } else if released && clicked {
            unsafe {
                button.performClick(None);
            }
        }
    }

    fn arrange(&self, chips: &[Rc<Chip>], order: &[usize]) {
        self.row.clear();
        for &index in order {
            self.row.add(chips[index].ns_view());
        }
        self.row.add(self.add.ns_view());
        self.row.layoutSubtreeIfNeeded();
    }

    /// The middle of the widest gap between existing values and the range ends.
    fn suggestion(&self) -> f64 {
        let (min, max) = self.range;
        let mut points: Vec<f64> = self.values.borrow().clone();
        points.extend([min, max]);
        points.sort_by(f64::total_cmp);
        points
            .windows(2)
            .max_by(|a, b| (a[1] - a[0]).total_cmp(&(b[1] - b[0])))
            .map_or((min + max) / 2.0, |gap| ((gap[0] + gap[1]) / 2.0).round())
    }

    /// Edit a value (`Some(index)`) or choose one to add (`None`) in a compact popover.
    fn open(self: &Rc<Self>, index: Option<usize>) {
        let anchor: Retained<NSView> = match index {
            Some(index) => match self.chips.borrow().get(index) {
                Some(chip) => chip.button.ns_view().retain(),
                None => return,
            },
            None => self.add.ns_view().retain(),
        };
        let (min, max) = self.range;
        let original = index.and_then(|index| self.values.borrow().get(index).copied());
        // Show at most two decimals; an unchanged rounded value is never written back.
        let start = (original.unwrap_or_else(|| self.suggestion()) * 100.0).round() / 100.0;
        let owner = Rc::downgrade(self);
        let unchanged = move |value: f64| {
            original.is_some_and(|original| {
                owner.upgrade().is_some_and(|inner| {
                    let format = inner.format.borrow();
                    format(original) == format(value)
                })
            })
        };
        let owner = Rc::downgrade(self);
        let committed = Rc::new(Cell::new(false));
        let done = committed.clone();
        // Edits apply as they're made; a new value applies once, when it's added.
        let commit = Rc::new(move |value: f64| {
            let Some(inner) = owner.upgrade() else { return };
            match index {
                Some(index) if !unchanged(value) => inner.emit(ChipEdit::Change(index, value)),
                None if !done.replace(true) => {
                    inner.emit(ChipEdit::Add(value));
                    if let Some(popover) = inner.popover.borrow().as_ref() {
                        popover.close();
                    }
                }
                _ => {}
            }
        });
        let field_commit = commit.clone();
        let field = Rc::new(
            NumberField::new(&self.ui)
                .range(min, max)
                .value(start)
                .on_change(move |value| field_commit(value)),
        );
        field.accessibility_label(&self.label);
        field.width(56.0);
        let weak_field = Rc::downgrade(&field);
        let app = Application::shared(&self.ui);
        let slide = commit.clone();
        let editing = index.is_some();
        let slider = Slider::new(&self.ui).range(min, max).value(start).continuous(true).on_change(
            move |value| {
                let value = value.round();
                if let Some(field) = weak_field.upgrade() {
                    field.set_value(value);
                }
                // Follow the drag in the field; save an existing value once on release.
                if editing && !app.is_mouse_dragging() {
                    slide(value);
                }
            },
        );
        slider.accessibility_label(&self.label);
        slider.width(180.0);
        let controls = HStack::new(&self.ui)
            .spacing(10.0)
            .push(slider)
            .push(field.unit_field(&self.ui, &self.unit));
        let mut content = VStack::new(&self.ui).spacing(12.0).push(controls);
        if index.is_none() {
            let (add_commit, add_field) = (commit.clone(), Rc::downgrade(&field));
            let add = Button::new(&self.ui, "Add").on_click(move || {
                if let Some(field) = add_field.upgrade() {
                    add_commit(field.ns_text_field().doubleValue());
                }
            });
            add.ns_button().setKeyEquivalent(&NSString::from_str("\r"));
            content = content.push(HStack::new(&self.ui).push(Spacer::new(&self.ui)).push(add));
        }
        let popover = Popover::new(
            &self.ui,
            View::new(&self.ui).content(content.keep(field), crate::uniform_insets(14.0)),
        );
        popover.show(&anchor);
        if let Some(old) = self.popover.borrow_mut().replace(popover) {
            old.close();
        }
    }
}

/// Trackpad feedback for a reordering gesture, on hardware that supports it.
fn haptic(pattern: objc2_app_kit::NSHapticFeedbackPattern) {
    use objc2_app_kit::NSHapticFeedbackPerformer;
    objc2_app_kit::NSHapticFeedbackManager::defaultPerformer()
        .performFeedbackPattern_performanceTime(
            pattern,
            objc2_app_kit::NSHapticFeedbackPerformanceTime::Now,
        );
}
