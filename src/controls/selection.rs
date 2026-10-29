use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::*;
use objc2_foundation::{NSArray, NSString};

use super::button::action_button_bezel;
use crate::Ui;
use crate::bridge::ActionTarget;
use crate::view::{NativeControl, NativeView};

pub(crate) fn header_menu_style(button: &NSPopUpButton) {
    button.setControlSize(NSControlSize::Regular);
    button.setBezelStyle(action_button_bezel());
    button.setFont(Some(&crate::style::Font::body()));
    if let Some(menu) = button.menu() {
        unsafe {
            menu.setFont(Some(&crate::style::Font::body()));
        }
    }
}

define_class!(
    #[unsafe(super(NSPopUpButton))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsPopup"]
    #[ivars = std::cell::Cell<bool>]
    pub(crate) struct SettingsPopup;
    impl SettingsPopup {
        #[unsafe(method(intrinsicContentSize))]
        fn intrinsic_size(&self) -> crate::CGSize {
            let mut size: crate::CGSize = unsafe { msg_send![super(self), intrinsicContentSize] };
            if self.ivars().get() { size.width += 12.0; }
            size
        }
        #[unsafe(method(drawRect:))]
        fn draw(&self, dirty: crate::CGRect) {
            if self.ivars().get() {
                let bounds = self.bounds();
                let size = 20.0;
                let circle = crate::CGRect::new(
                    crate::CGPoint::new(bounds.size.width - size, (bounds.size.height - size) / 2.0),
                    crate::CGSize::new(size, size),
                );
                NSColor::labelColor().colorWithAlphaComponent(0.09).setFill();
                NSBezierPath::bezierPathWithOvalInRect(circle).fill();
            }
            unsafe { let _: () = msg_send![super(self), drawRect: dirty]; }
        }
    }
);
impl SettingsPopup {
    pub(crate) fn settings_style(&self) {
        self.ivars().set(true);
        self.setBordered(false);
        self.setControlSize(NSControlSize::Regular);
        self.invalidateIntrinsicContentSize();
        NSView::setNeedsDisplay(self, true);
    }
}

pub struct Popup {
    native: Retained<NSPopUpButton>,
    target: Retained<ActionTarget>,
    menu: Option<crate::menu::Menu>,
    /// Menu index of each choice when separators divide them; empty when they don't.
    positions: std::rc::Rc<std::cell::RefCell<Vec<isize>>>,
}
impl Popup {
    pub fn new(ui: &Ui) -> Self {
        let native: Retained<SettingsPopup> = unsafe {
            msg_send![
                super(SettingsPopup::alloc(ui.mtm()).set_ivars(std::cell::Cell::new(false))),
                init
            ]
        };
        let native = native.into_super();
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
    pub fn actions(ui: &Ui, label: &str, menu: crate::menu::Menu) -> Self {
        let header = crate::menu::MenuItem::new(ui, "");
        header
            .ns_menu_item()
            .setImage(crate::image::Symbol::named("ellipsis").as_deref());
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
    pub fn toolbar_menu(ui: &Ui, label: &str, menu: crate::menu::Menu) -> Self {
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

    /// Text choices with optional SF Symbols alongside their labels.
    pub fn choices(ui: &Ui, segments: &[(&str, Option<&str>)]) -> Self {
        let labels: Vec<_> = segments.iter().map(|(label, _)| *label).collect();
        let control = Self::new(ui, &labels);
        control.native.setTrackingMode(NSSegmentSwitchTracking::SelectOne);
        // Capsule uses AppKit's rounded, neutral selection treatment.
        control.native.setSegmentStyle(NSSegmentStyle::Rounded);
        control.native.setControlSize(NSControlSize::Large);
        for (index, (label, symbol)) in segments.iter().enumerate() {
            if let Some(symbol) = symbol {
                let description = NSString::from_str(label);
                if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str(symbol),
                    Some(&description),
                ) {
                    control.native.setImage_forSegment(Some(&image), index as isize);
                }
            }
        }
        control
    }

    pub fn selected(&self) -> Option<usize> { usize::try_from(self.native.selectedSegment()).ok() }

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
