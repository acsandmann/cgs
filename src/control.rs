use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::*;
use objc2_foundation::{
    NSArray, NSNumber, NSNumberFormatter, NSNumberFormatterStyle, NSObjectProtocol, NSString,
};

use crate::bridge::ActionTarget;
use crate::{HStack, NativeControl, NativeView, Ui};

macro_rules! native_control {
    ($wrapper:ident, $native:ident, $accessor:ident) => {
        impl $wrapper {
            pub fn $accessor(&self) -> &$native { &self.native }
        }
        impl NativeView for $wrapper {
            fn ns_view(&self) -> &NSView { &self.native }
        }
        impl NativeControl for $wrapper {
            fn ns_control(&self) -> &NSControl { &self.native }
        }
    };
}
pub(crate) use native_control;

// The glass bezel and NSGlassEffectView were introduced together in macOS 26.
// Check runtime availability so older systems retain their standard push buttons.
pub(crate) fn action_button_bezel() -> NSBezelStyle {
    if objc2::runtime::AnyClass::get(c"NSGlassEffectView").is_some() {
        NSBezelStyle::Glass
    } else {
        NSBezelStyle::Push
    }
}

pub struct Button {
    native: Retained<NSButton>,
    target: Retained<ActionTarget>,
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
        if let Some(image) = crate::Symbol::named(name) {
            self.native.setImage(Some(&image));
            self.native.setImagePosition(NSCellImagePosition::ImageOnly);
        }
        self
    }
}
native_control!(Button, NSButton, ns_button);

/// One choice in a gallery of layouts: a live illustration above its name, and an
/// optional quiet marker, such as “Default”, on a second line. Selection rings the
/// illustration with the accent color, as thumbnails do in System Settings, and
/// assistive apps hear each tile as a radio button with its marker in its name.
/// Whether a tile is selected, and its marker.
type TileState = std::rc::Rc<std::cell::RefCell<Option<(bool, Option<String>)>>>;

pub struct LayoutTile {
    button: Button,
    preview: crate::LayoutPreview,
    title: String,
    state: TileState,
}
impl LayoutTile {
    pub fn new(ui: &Ui, title: &str, canvas: crate::CGSize) -> Self {
        let preview = crate::LayoutPreview::selectable(ui, canvas);
        let native = NSButton::new(ui.mtm());
        native.setButtonType(NSButtonType::PushOnPushOff);
        #[allow(deprecated)] // AppKit’s flexible native image-and-title button bezel.
        native.setBezelStyle(NSBezelStyle::RegularSquare);
        native.setBordered(false);
        native.setContentTintColor(Some(&NSColor::labelColor()));
        // An empty image reserves the illustration’s room, with its ring, above the title.
        let outset = crate::LayoutPreview::RING_OUTSET;
        let room = crate::CGSize::new(canvas.width, canvas.height + 2.0 * outset);
        native.setImage(Some(&NSImage::initWithSize(NSImage::alloc(), room)));
        native.setImagePosition(NSCellImagePosition::ImageAbove);
        native.addSubview(preview.ns_view());
        native.setAccessibilityRole(Some(unsafe { NSAccessibilityRadioButtonRole }));
        let view = preview.ns_view();
        view.setAccessibilityElement(false);
        view.centerXAnchor()
            .constraintEqualToAnchor(&native.centerXAnchor())
            .setActive(true);
        view.topAnchor()
            .constraintEqualToAnchor_constant(&native.topAnchor(), outset + 2.0)
            .setActive(true);
        let button = Button::from_native(ui, native);
        // The illustration, then two caption lines: the name and its marker.
        button.height(room.height + 2.0 + 2.0 * NSFont::smallSystemFontSize() + 16.0);
        let tile = Self {
            button,
            preview,
            title: title.into(),
            state: Default::default(),
        };
        tile.set_state(false, None);
        tile
    }

    pub fn on_click(mut self, mut f: impl FnMut() + 'static) -> Self {
        let native = objc2::rc::Weak::new(self.button.ns_button());
        let state = self.state.clone();
        self.button = self.button.on_click(move || {
            f();
            // A click toggles the native state; selection is only ever set by `set_state`.
            if let (Some(native), Some((selected, _))) = (native.load(), &*state.borrow()) {
                native.setState(if *selected {
                    NSControlStateValueOn
                } else {
                    NSControlStateValueOff
                });
            }
        });
        self
    }

    pub fn preview(&self) -> &crate::LayoutPreview { &self.preview }

    /// Whether this tile is the chosen one, and its marker, such as “Default”.
    pub fn set_state(&self, selected: bool, marker: Option<&str>) {
        let state = Some((selected, marker.map(str::to_owned)));
        if *self.state.borrow() == state {
            return;
        }
        *self.state.borrow_mut() = state;
        let native = self.button.ns_button();
        native.setState(if selected {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        self.preview.set_selected(selected);
        native.setAttributedTitle(&two_line_title(&self.title, marker, selected));
        let name = match marker {
            Some(marker) => format!("{}, {}", self.title, marker.to_lowercase()),
            None => self.title.clone(),
        };
        native.setAccessibilityLabel(Some(&NSString::from_str(&name)));
    }

    pub fn ns_button(&self) -> &NSButton { self.button.ns_button() }
}
impl NativeView for LayoutTile {
    fn ns_view(&self) -> &NSView { self.button.ns_view() }
}

/// A centered caption, emphasized while selected, over a quieter marker line. Every tile
/// reserves the second line, so names stay aligned across the gallery.
fn two_line_title(
    title: &str,
    marker: Option<&str>,
    selected: bool,
) -> Retained<objc2_foundation::NSAttributedString> {
    use objc2_foundation::{NSAttributedString, NSDictionary, NSMutableAttributedString};
    let paragraph = NSMutableParagraphStyle::new();
    paragraph.setAlignment(NSTextAlignment::Center);
    paragraph.setParagraphSpacing(1.0);
    let line = |text: &str, font: &NSFont, color: &NSColor| unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(text),
            Some(&NSDictionary::from_slices(
                &[
                    NSFontAttributeName,
                    NSForegroundColorAttributeName,
                    NSParagraphStyleAttributeName,
                ],
                &[
                    font as &AnyObject,
                    color as &AnyObject,
                    &*paragraph as &AnyObject,
                ],
            )),
        )
    };
    let text = NSMutableAttributedString::new();
    let name = if selected {
        crate::Font::caption_emphasized()
    } else {
        crate::Font::caption()
    };
    text.appendAttributedString(&line(&format!("{title}\n"), &name, &NSColor::labelColor()));
    text.appendAttributedString(&line(
        marker.unwrap_or(" "),
        &NSFont::systemFontOfSize_weight(NSFont::labelFontSize() - 1.0, unsafe {
            NSFontWeightMedium
        }),
        &NSColor::secondaryLabelColor(),
    ));
    Retained::into_super(text)
}

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

pub(crate) fn header_menu_style(button: &NSPopUpButton) {
    button.setControlSize(NSControlSize::Regular);
    button.setBezelStyle(action_button_bezel());
    button.setFont(Some(&crate::Font::body()));
    if let Some(menu) = button.menu() {
        unsafe {
            menu.setFont(Some(&crate::Font::body()));
        }
    }
}

pub struct Popup {
    native: Retained<NSPopUpButton>,
    target: Retained<ActionTarget>,
    menu: Option<crate::Menu>,
    /// Menu index of each choice when separators divide them; empty when they don't.
    positions: std::rc::Rc<std::cell::RefCell<Vec<isize>>>,
}
impl Popup {
    pub fn new(ui: &Ui) -> Self {
        let native = NSPopUpButton::new(ui.mtm());
        let target = ActionTarget::new(ui);
        target.attach(&native);
        Self {
            native,
            target,
            menu: None,
            positions: Default::default(),
        }
    }

    fn position(&self, index: usize) -> isize {
        self.positions.borrow().get(index).copied().unwrap_or(index as isize)
    }

    /// Native pull-down menu for secondary collection actions.
    pub fn actions(ui: &Ui, label: &str, menu: crate::Menu) -> Self {
        let header = crate::MenuItem::new(ui, "");
        header.ns_menu_item().setImage(crate::Symbol::named("ellipsis").as_deref());
        menu.ns_menu().insertItem_atIndex(header.ns_menu_item(), 0);
        let mut popup = Self::new(ui);
        popup.native.setPullsDown(true);
        if let Some(cell) =
            popup.native.cell().and_then(|cell| cell.downcast::<NSPopUpButtonCell>().ok())
        {
            cell.setArrowPosition(NSPopUpArrowPosition::NoArrow);
        }
        popup.native.setMenu(Some(menu.ns_menu()));
        popup.native.setControlSize(NSControlSize::Small);
        popup.accessibility_label(label);
        popup.menu = Some(menu);
        popup
    }

    /// Native pull-down menu for toolbar filters that have no placeholder row.
    pub fn toolbar_menu(ui: &Ui, label: &str, menu: crate::Menu) -> Self {
        let mut popup = Self::new(ui);
        popup.native.setPullsDown(true);
        if let Some(cell) =
            popup.native.cell().and_then(|cell| cell.downcast::<NSPopUpButtonCell>().ok())
        {
            cell.setArrowPosition(NSPopUpArrowPosition::NoArrow);
        }
        popup.native.setMenu(Some(menu.ns_menu()));
        popup.native.setControlSize(NSControlSize::Small);
        popup.accessibility_label(label);
        popup.menu = Some(menu);
        popup
    }

    pub fn toolbar_style(self) -> Self {
        header_menu_style(&self.native);
        self
    }

    pub fn items(self, values: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        self.set_items(values);
        self
    }

    pub fn set_items(&self, values: impl IntoIterator<Item = impl AsRef<str>>) {
        self.set_sections([values]);
    }

    /// Choices in groups divided by native separators, such as a default above explicit
    /// values. Indices passed to and reported by the popup count choices only.
    pub fn sections(
        self,
        sections: impl IntoIterator<Item = impl IntoIterator<Item = impl AsRef<str>>>,
    ) -> Self {
        self.set_sections(sections);
        self
    }

    pub fn set_sections(
        &self,
        sections: impl IntoIterator<Item = impl IntoIterator<Item = impl AsRef<str>>>,
    ) {
        let mut titles = Vec::new();
        let mut positions = Vec::new();
        for section in sections {
            let start = titles.len();
            for value in section {
                if start > 0 && titles.len() == start {
                    titles.push(None);
                }
                positions.push(titles.len() as isize);
                titles.push(Some(value.as_ref().to_owned()));
            }
        }
        let separated = titles.len() != positions.len();
        let menu = self.native.menu().unwrap();
        let current = menu.itemArray();
        if current.len() == titles.len()
            && current.iter().zip(&titles).all(|(item, title)| match title {
                Some(title) => !item.isSeparatorItem() && item.title().to_string() == *title,
                None => item.isSeparatorItem(),
            })
        {
            return;
        }
        self.native.removeAllItems();
        for title in titles {
            let item = match title {
                // addItemWithTitle merges duplicate titles, which breaks index-based choices.
                Some(title) => unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(self.native.mtm()),
                        &NSString::from_str(&title),
                        None,
                        &NSString::from_str(""),
                    )
                },
                None => NSMenuItem::separatorItem(self.native.mtm()),
            };
            menu.addItem(&item);
        }
        *self.positions.borrow_mut() = if separated { positions } else { Vec::new() };
    }

    /// Let AppKit render a native menu badge beside a choice.
    pub fn set_item_badge(&self, index: usize, label: Option<&str>) {
        use objc2::AnyThread;
        use objc2_foundation::NSObjectProtocol;
        let Some(item) = self.native.itemAtIndex(self.position(index)) else {
            return;
        };
        if !item.respondsToSelector(objc2::sel!(setBadge:)) {
            return;
        }
        if item
            .badge()
            .and_then(|badge| badge.stringValue())
            .map(|s| s.to_string())
            .as_deref()
            == label
        {
            return;
        }
        let badge = label.map(|label| {
            objc2_app_kit::NSMenuItemBadge::initWithString(
                objc2_app_kit::NSMenuItemBadge::alloc(),
                &NSString::from_str(label),
            )
        });
        item.setBadge(badge.as_deref());
    }

    pub fn set_selected(&self, index: usize) {
        let position = self.position(index);
        if self.native.indexOfSelectedItem() != position {
            self.native.selectItemAtIndex(position);
        }
    }

    pub fn selected(&self) -> Option<usize> {
        choice(&self.positions.borrow(), self.native.indexOfSelectedItem())
    }

    pub fn on_change(self, mut f: impl FnMut(usize) + 'static) -> Self {
        let positions = self.positions.clone();
        self.target.set(move |sender| {
            if let Some(control) = sender.downcast_ref::<NSPopUpButton>() {
                let index = {
                    let positions = positions.borrow();
                    choice(&positions, control.indexOfSelectedItem())
                };
                if let Some(index) = index {
                    f(index);
                }
            }
        });
        self
    }
}
/// The choice at a menu index, skipping separators.
fn choice(positions: &[isize], position: isize) -> Option<usize> {
    if positions.is_empty() {
        usize::try_from(position).ok()
    } else {
        positions.iter().position(|&p| p == position)
    }
}
native_control!(Popup, NSPopUpButton, ns_popup_button);

pub struct SegmentedControl {
    native: Retained<NSSegmentedControl>,
    target: Retained<ActionTarget>,
}
impl SegmentedControl {
    pub fn new(ui: &Ui, labels: &[&str]) -> Self {
        let native = NSSegmentedControl::new(ui.mtm());
        native.setSegmentCount(labels.len() as isize);
        for (index, label) in labels.iter().enumerate() {
            native.setLabel_forSegment(&NSString::from_str(label), index as isize);
        }
        let target = ActionTarget::new(ui);
        target.attach(&native);
        Self { native, target }
    }

    /// Segments drawn as SF Symbols, each named by `label` for VoiceOver and help tags.
    /// A symbol missing on this system falls back to its label.
    pub fn symbols(ui: &Ui, segments: &[(&str, &str)]) -> Self {
        let control = Self::new(ui, &[]);
        let native = &control.native;
        native.setSegmentCount(segments.len() as isize);
        for (index, (symbol, label)) in segments.iter().enumerate() {
            let segment = index as isize;
            let label = NSString::from_str(label);
            match NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(symbol),
                Some(&label),
            ) {
                Some(image) => {
                    native.setImage_forSegment(Some(&image), segment);
                    native.setWidth_forSegment(32.0, segment);
                }
                None => native.setLabel_forSegment(&label, segment),
            }
            native.setToolTip_forSegment(Some(&label), segment);
        }
        control
    }

    pub fn set_selected(&self, index: usize) {
        if self.native.selectedSegment() != index as isize {
            self.native.setSelectedSegment(index as isize);
        }
    }

    pub fn on_change(self, mut f: impl FnMut(usize) + 'static) -> Self {
        self.target.set(move |sender| {
            if let Some(control) = sender.downcast_ref::<NSSegmentedControl>() {
                if let Ok(index) = usize::try_from(control.selectedSegment()) {
                    f(index);
                }
            }
        });
        self
    }
}
native_control!(SegmentedControl, NSSegmentedControl, ns_segmented_control);

pub struct ComboBox {
    native: Retained<NSComboBox>,
    target: Retained<ActionTarget>,
}
impl ComboBox {
    pub fn new(ui: &Ui) -> Self {
        let native = NSComboBox::new(ui.mtm());
        let target = ActionTarget::new(ui);
        target.attach(&native);
        Self { native, target }
    }

    pub fn items(self, values: &[&str]) -> Self {
        let strings: Vec<_> = values.iter().map(|v| NSString::from_str(v)).collect();
        let objects: Vec<&AnyObject> = strings.iter().map(|v| &**v as &AnyObject).collect();
        unsafe {
            self.native.addItemsWithObjectValues(&NSArray::from_slice(&objects));
        }
        self
    }

    pub fn set_value(&self, value: &str) {
        if self.native.stringValue().to_string() != value {
            self.native.setStringValue(&NSString::from_str(value));
        }
    }

    pub fn on_change(self, mut f: impl FnMut(String) + 'static) -> Self {
        self.target.set(move |sender| {
            if let Some(control) = sender.downcast_ref::<NSComboBox>() {
                f(control.stringValue().to_string());
            }
        });
        self
    }
}
native_control!(ComboBox, NSComboBox, ns_combo_box);

pub struct RadioGroup {
    stack: crate::VStack,
    buttons: Vec<Retained<NSButton>>,
    targets: Vec<Retained<ActionTarget>>,
}
impl RadioGroup {
    pub fn new(ui: &Ui, labels: &[&str]) -> Self {
        let stack = crate::VStack::new(ui);
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
define_class!(
    #[unsafe(super(NSSlider))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsSlider"]
    struct NaturalSlider;
    impl NaturalSlider {
        #[unsafe(method(intrinsicContentSize))]
        fn intrinsic_size(&self) -> crate::CGSize {
            let mut size: crate::CGSize = unsafe { msg_send![super(self), intrinsicContentSize] };
            // AppKit sliders otherwise have no intrinsic width. This is a
            // compressible preference, not a constraint on the containing window.
            size.width = size.width.max(NSFont::systemFontSize() * 6.0);
            size
        }
    }
);

/// An accent-colored native slider, with optional snapping and tactile detents.
/// Native tracking preserves keyboard, accessibility, and appearance support.
pub struct Slider {
    native: Retained<NSSlider>,
    target: Retained<ActionTarget>,
    last_value: std::rc::Rc<std::cell::Cell<f64>>,
    step: std::rc::Rc<std::cell::Cell<Option<f64>>>,
}
impl Slider {
    pub fn new(ui: &Ui) -> Self {
        let native: Retained<NaturalSlider> =
            unsafe { msg_send![NaturalSlider::alloc(ui.mtm()), init] };
        let native = native.into_super();
        native.setTrackFillColor(Some(&NSColor::controlAccentColor()));
        let target = ActionTarget::new(ui);
        target.attach(&native);
        Self {
            native,
            target,
            last_value: std::rc::Rc::new(std::cell::Cell::new(0.0)),
            step: Default::default(),
        }
    }

    pub fn range(self, min: f64, max: f64) -> Self {
        self.set_range(min, max);
        self
    }

    pub fn set_range(&self, min: f64, max: f64) {
        assert!(min.is_finite() && max.is_finite() && min <= max);
        if self.native.minValue() != min {
            self.native.setMinValue(min);
        }
        if self.native.maxValue() != max {
            self.native.setMaxValue(max);
        }
        self.update_ticks();
        self.last_value.set(self.native.doubleValue());
    }

    /// Move in multiples of `step`, such as 5 for percentages, with an alignment haptic
    /// at each one while dragging. Range endpoints stay reachable when they fall between
    /// steps. Tick marks appear when the range spans whole steps and a few of them.
    pub fn step(self, step: f64) -> Self {
        self.set_step(step);
        self
    }

    pub fn set_step(&self, step: f64) {
        assert!(step.is_finite() && step > 0.0);
        self.step.set(Some(step));
        self.update_ticks();
    }

    fn update_ticks(&self) {
        let Some(step) = self.step.get() else { return };
        let (min, max) = (self.native.minValue(), self.native.maxValue());
        let whole = |value: f64| ((value / step).round() * step - value).abs() < 1e-6;
        let count = ((max - min) / step).round() as isize + 1;
        let ticks = if whole(min) && whole(max) && (2..=21).contains(&count) {
            count
        } else {
            0
        };
        if self.native.numberOfTickMarks() != ticks {
            self.native.setNumberOfTickMarks(ticks);
            self.native.setTickMarkPosition(NSTickMarkPosition::Below);
        }
    }

    /// Equally spaced notches, including both endpoints. Each changed detent
    /// produces a subtle alignment haptic on supported hardware during tracking.
    /// Programmatic updates never produce feedback.
    pub fn notches(self, count: usize) -> Self {
        assert!(count >= 2 && count <= isize::MAX as usize);
        self.native.setNumberOfTickMarks(count as isize);
        self.native.setAllowsTickMarkValuesOnly(true);
        self.native.setTickMarkPosition(NSTickMarkPosition::Below);
        self.native.setContinuous(true);
        self
    }

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
        self.last_value.set(self.native.doubleValue());
    }

    pub fn get_value(&self) -> f64 { self.native.doubleValue() }

    pub fn on_change(self, mut f: impl FnMut(f64) + 'static) -> Self {
        let last = self.last_value.clone();
        let step = self.step.clone();
        self.target.set(move |sender| {
            let Some(control) = sender.downcast_ref::<NSSlider>() else {
                return;
            };
            let mut value = control.doubleValue();
            let snapped = if control.allowsTickMarkValuesOnly() {
                Some(control.closestTickMarkValueToValue(value))
            } else {
                step.get().map(|step| {
                    let (min, max) = (control.minValue(), control.maxValue());
                    let nearest = ((value / step).round() * step).clamp(min, max);
                    // An endpoint between steps wins when it’s closer than the step.
                    [min, max]
                        .into_iter()
                        .find(|bound| (value - bound).abs() < (value - nearest).abs())
                        .unwrap_or(nearest)
                })
            };
            if let Some(snapped) = snapped {
                value = snapped;
                control.setDoubleValue(value);
                let changed = last.replace(value) != value;
                let tracking = NSApplication::sharedApplication(control.mtm())
                    .currentEvent()
                    .is_some_and(|event| {
                        matches!(
                            event.r#type(),
                            NSEventType::LeftMouseDown | NSEventType::LeftMouseDragged
                        )
                    });
                if changed && tracking {
                    NSHapticFeedbackManager::defaultPerformer()
                        .performFeedbackPattern_performanceTime(
                            NSHapticFeedbackPattern::Alignment,
                            NSHapticFeedbackPerformanceTime::Now,
                        );
                }
            } else {
                last.set(value);
            }
            f(value);
        });
        self
    }
}
native_control!(Slider, NSSlider, ns_slider);

/// A native slider paired with a field for exact values.
/// The caller binds both controls to its model; the field can accept values
/// outside the slider's convenient range without losing precision.
pub struct NumberSlider {
    stack: HStack,
}
impl NumberSlider {
    pub fn new(ui: &Ui, slider: std::rc::Rc<Slider>, field: std::rc::Rc<NumberField>) -> Self {
        Self {
            stack: HStack::new(ui).spacing(12.0).push(slider).push(field),
        }
    }

    /// A standard-length slider and a compact field with its unit inside the bezel.
    pub fn with_unit(
        ui: &Ui,
        slider: std::rc::Rc<Slider>,
        field: std::rc::Rc<NumberField>,
        unit: &str,
    ) -> Self {
        slider.width(crate::Metrics::SLIDER_WIDTH);
        let value = field.unit_field(ui, unit);
        value.width(crate::Metrics::VALUE_FIELD_WIDTH);
        Self {
            // The bezel hosts the field's view; keep its Rust owner with the row.
            stack: HStack::new(ui).spacing(8.0).push(slider).push(value).keep(field),
        }
    }
}
impl NativeView for NumberSlider {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
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
    field: crate::TextField,
    formatter: Retained<NSNumberFormatter>,
}
impl NumberField {
    pub fn with_validation(self, ui: &Ui) -> crate::Validated<Self> {
        crate::Validated::new(ui, self)
    }

    /// A leading-aligned value inside one bezel with a trailing unit, such as "pt".
    pub fn unit_field(&self, ui: &Ui, unit: &str) -> crate::UnitField {
        self.ns_text_field().setAlignment(NSTextAlignment::Left);
        crate::UnitField::new(ui, self.ns_text_field(), unit)
    }

    pub fn new(ui: &Ui) -> Self {
        let field = crate::TextField::new(ui);
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

    pub fn set_validation(&self, validation: &crate::Validation) {
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

pub struct ColorWell {
    native: Retained<NSColorWell>,
    target: Retained<ActionTarget>,
}
impl ColorWell {
    pub fn new(ui: &Ui) -> Self {
        let native = NSColorWell::new(ui.mtm());
        native.setContinuous(false);
        let target = ActionTarget::new(ui);
        target.attach(&native);
        Self { native, target }
    }

    /// Show an sRGB `[red, green, blue, alpha]` color.
    pub fn set_value(&self, [r, g, b, a]: [f64; 4]) {
        let color = NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a);
        if !self.native.color().isEqual(Some(&color)) {
            self.native.setColor(&color);
        }
    }

    /// Receive the chosen color as sRGB `[red, green, blue, alpha]`.
    pub fn on_change(self, mut f: impl FnMut([f64; 4]) + 'static) -> Self {
        self.target.set(move |sender| {
            let color = sender.downcast_ref::<NSColorWell>().and_then(|control| {
                control
                    .color()
                    .colorUsingColorSpace(&objc2_app_kit::NSColorSpace::sRGBColorSpace())
            });
            if let Some(c) = color {
                f([
                    c.redComponent(),
                    c.greenComponent(),
                    c.blueComponent(),
                    c.alphaComponent(),
                ]);
            }
        });
        self
    }
}
native_control!(ColorWell, NSColorWell, ns_color_well);

pub struct AddRemoveControl {
    stack: HStack,
    add: IconButton,
    remove: IconButton,
}
impl AddRemoveControl {
    pub fn new(ui: &Ui) -> Self {
        let add = IconButton::new(ui, "plus", "Add");
        let remove = IconButton::new(ui, "minus", "Remove");
        for button in [&add, &remove] {
            button.ns_button().setBordered(false);
            button.width(28.0);
            button.height(24.0);
        }
        let divider = NSBox::new(ui.mtm());
        divider.setBoxType(NSBoxType::Separator);
        divider.widthAnchor().constraintEqualToConstant(1.0).setActive(true);
        divider.heightAnchor().constraintEqualToConstant(16.0).setActive(true);
        let stack = HStack::new(ui)
            .spacing(0.0)
            .push(add.ns_view().retain())
            .push(divider.into_super())
            .push(remove.ns_view().retain());
        Self { stack, add, remove }
    }

    pub fn on_add(mut self, f: impl FnMut() + 'static) -> Self {
        self.add = self.add.on_click(f);
        self
    }

    pub fn on_remove(mut self, f: impl FnMut() + 'static) -> Self {
        self.remove = self.remove.on_click(f);
        self
    }

    pub fn set_remove_enabled(&self, value: bool) { self.remove.set_enabled(value); }

    pub(crate) fn show_add(&self, visible: bool) {
        self.add.set_hidden(!visible);
        self.stack
            .ns_stack_view()
            .arrangedSubviews()
            .objectAtIndex(1)
            .setHidden(!visible);
    }

    pub(crate) fn remove_button(&self) -> &NSButton { self.remove.ns_button() }
}
impl NativeView for AddRemoveControl {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}

/// Native trailing sheet actions. Cancel is visible only while the draft differs
/// from its initial value; clients report draft changes without managing buttons.
pub struct SheetActions {
    row: HStack,
    done: std::rc::Rc<Button>,
    cancel: std::rc::Rc<Button>,
}
impl SheetActions {
    pub fn new(ui: &Ui) -> Self {
        let done = std::rc::Rc::new(Button::new(ui, "Done").key_equivalent("\r"));
        let cancel = std::rc::Rc::new(Button::new(ui, "Cancel").key_equivalent("\u{1b}"));
        cancel.set_hidden(true);
        cancel.identifier("cgs.sheet-cancel");
        done.identifier("cgs.sheet-primary");
        let row = HStack::new(ui).spacing(8.0).spacer(ui).push(cancel.clone()).push(done.clone());
        Self { row, done, cancel }
    }

    pub fn set_changed(&self, changed: bool) { self.cancel.set_hidden(!changed); }

    pub fn set_primary_title(&self, title: &str) { self.done.set_title(title); }

    pub fn set_on_done(&self, mut f: impl FnMut() + 'static) { self.done.target.set(move |_| f()); }

    pub fn set_on_cancel(&self, mut f: impl FnMut() + 'static) {
        self.cancel.target.set(move |_| f());
    }
}
impl NativeView for SheetActions {
    fn ns_view(&self) -> &NSView { self.row.ns_view() }
}
