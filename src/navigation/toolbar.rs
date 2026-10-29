use std::rc::Rc;

use objc2::rc::{Retained, Weak};
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::*;
use objc2_foundation::{NSArray, NSObject, NSObjectProtocol, NSString};

use crate::controls::Popup;
use crate::image::Symbol;
use crate::input::SearchField;
use crate::layout::HStack;
use crate::menu::{Menu, MenuItem};
use crate::text::Label;
use crate::view::{GlassEffectView, NativeView};
use crate::{CGRect, Insets, Ui};

/// Page-owned native header controls. Toolbar presentation and glass remain in CGS.
pub struct HeaderControls {
    menu: Rc<Popup>,
    search: SearchField,
    icon_only: bool,
    actions: Option<Popup>,
    label: String,
}
impl HeaderControls {
    pub fn new(ui: &Ui, label: &str, symbol: &str, menu: Menu, search: SearchField) -> Self {
        let menu = Popup::toolbar_menu(ui, label, menu).toolbar_style();
        let button = menu.ns_popup_button();
        let image = Symbol::header(symbol);
        button.setImage(image.as_deref());
        button.setImagePosition(NSCellImagePosition::ImageOnly);
        let display = MenuItem::new(ui, "");
        display.ns_menu_item().setImage(image.as_deref());
        if let Some(cell) = button.cell().and_then(|cell| cell.downcast::<NSPopUpButtonCell>().ok())
        {
            cell.setUsesItemFromMenu(false);
            cell.setMenuItem(Some(display.ns_menu_item()));
            cell.setAltersStateOfSelectedItem(false);
        }
        button.setBordered(false);
        menu.width(36.0);
        menu.height(36.0);
        search.width(216.0);
        Self {
            menu: Rc::new(menu),
            search,
            icon_only: true,
            actions: None,
            label: label.into(),
        }
    }

    /// A native selection popup beside the same toolbar search field.
    pub fn selector(label: &str, menu: Rc<Popup>, search: SearchField) -> Self {
        crate::controls::header_menu_style(menu.ns_popup_button());
        menu.accessibility_label(label);
        menu.ns_popup_button().setImageHugsTitle(false);
        menu.min_width(100.0);
        search.width(216.0);
        Self {
            menu,
            search,
            icon_only: false,
            actions: None,
            label: label.into(),
        }
    }

    /// Secondary actions adjacent to the header selector and search.
    pub fn actions(mut self, actions: Popup) -> Self {
        let actions = actions.toolbar_style();
        actions.width(36.0);
        actions.height(36.0);
        self.actions = Some(actions);
        self
    }

    /// Use the semantic blue tint while a filter is active.
    pub fn set_filtered(&self, active: bool) {
        let blue = NSColor::systemBlueColor();
        self.menu
            .ns_popup_button()
            .setContentTintColor(if active { Some(&blue) } else { None });
    }
}

struct NavigationToolbarItems {
    title: Option<Rc<Label>>,
    back: Retained<NSToolbarItem>,
    navigation: std::cell::RefCell<Vec<Retained<NSToolbarItem>>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsNavigationToolbarDelegate"]
    #[ivars = NavigationToolbarItems]
    struct NavigationToolbarDelegate;
    unsafe impl NSObjectProtocol for NavigationToolbarDelegate {}
    unsafe impl NSToolbarDelegate for NavigationToolbarDelegate {
        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn item(
            &self,
            _toolbar: &NSToolbar,
            identifier: &NSToolbarItemIdentifier,
            _insert: bool,
        ) -> Option<Retained<NSToolbarItem>> {
            if let Some(item) = self
                .ivars()
                .navigation
                .borrow()
                .iter()
                .find(|item| &*item.itemIdentifier() == identifier)
            {
                Some(item.clone())
            } else if identifier.to_string() == "cgs.back" {
                Some(self.ivars().back.clone())
            } else {
                self.ivars()
                    .title
                    .as_ref()
                    .filter(|_| identifier.to_string() == "cgs.page-title")
                    .map(|title| {
                        let item = NSToolbarItem::initWithItemIdentifier(
                            NSToolbarItem::alloc(self.mtm()),
                            identifier,
                        );
                        let label = HStack::new(&Ui::new(self.mtm()))
                            .insets(Insets {
                                top: 0.0,
                                left: 3.0,
                                bottom: 0.0,
                                right: 0.0,
                            })
                            .push(title.clone());
                        item.setView(Some(label.ns_view()));
                        item.setBordered(false);
                        item
                    })
            }
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn allowed(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            let mut ids: Vec<_> = self.item_identifiers().iter().collect();
            ids.push(NSString::from_str("cgs.back"));
            ids.extend(self.ivars().navigation.borrow().iter().map(|item| item.itemIdentifier()));
            NSArray::from_retained_slice(&ids)
        }

        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn defaults(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            self.item_identifiers()
        }
    }
);

impl NavigationToolbarDelegate {
    fn item_identifiers(&self) -> Retained<NSArray<NSToolbarItemIdentifier>> {
        if self.ivars().title.is_some() {
            NSArray::from_retained_slice(&[
                unsafe { NSToolbarFlexibleSpaceItemIdentifier }.retain(),
                unsafe { NSToolbarToggleSidebarItemIdentifier }.retain(),
                unsafe { NSToolbarSidebarTrackingSeparatorItemIdentifier }.retain(),
                NSString::from_str("cgs.page-title"),
                unsafe { NSToolbarFlexibleSpaceItemIdentifier }.retain(),
            ])
        } else {
            NSArray::from_slice(&[unsafe { NSToolbarToggleSidebarItemIdentifier }, unsafe {
                NSToolbarFlexibleSpaceItemIdentifier
            }])
        }
    }
}

pub struct Toolbar {
    native: Retained<NSToolbar>,
    delegate: Option<Retained<NavigationToolbarDelegate>>,
    back_target: Retained<crate::bridge::ActionTarget>,
    forward_target: Retained<crate::bridge::ActionTarget>,
    menu: std::cell::RefCell<
        Option<(
            Rc<crate::menu::Menu>,
            std::cell::OnceCell<Retained<NSPopUpButton>>,
        )>,
    >,
    page_controls: std::cell::RefCell<[Option<Weak<NSView>>; 3]>,
}
impl Toolbar {
    pub fn new(ui: &Ui, id: &str) -> Self {
        Self {
            native: NSToolbar::initWithIdentifier(
                NSToolbar::alloc(ui.mtm()),
                &NSString::from_str(id),
            ),
            delegate: None,
            back_target: crate::bridge::ActionTarget::new(ui),
            forward_target: crate::bridge::ActionTarget::new(ui),
            menu: std::cell::RefCell::new(None),
            page_controls: std::cell::RefCell::new([None, None, None]),
        }
    }

    pub fn navigation(ui: &Ui, id: &str) -> Self { Self::navigation_title(ui, id, None) }

    pub(crate) fn navigation_title(ui: &Ui, id: &str, title: Option<Rc<Label>>) -> Self {
        let mut toolbar = Self::new(ui, id);
        let back = NSToolbarItem::initWithItemIdentifier(
            NSToolbarItem::alloc(ui.mtm()),
            &NSString::from_str("cgs.back"),
        );
        back.setLabel(&NSString::from_str("Back"));
        back.setPaletteLabel(&NSString::from_str("Back"));
        back.setNavigational(true);
        back.setImage(crate::image::Symbol::named("chevron.backward").as_deref());
        back.setEnabled(false);
        back.setAutovalidates(false);
        unsafe {
            back.setTarget(Some(&toolbar.back_target));
            back.setAction(Some(objc2::sel!(invoke:)));
        }
        let delegate: Retained<NavigationToolbarDelegate> = unsafe {
            msg_send![
                super(NavigationToolbarDelegate::alloc(ui.mtm()).set_ivars(
                    NavigationToolbarItems {
                        title,
                        back,
                        navigation: std::cell::RefCell::new(Vec::new())
                    }
                )),
                init
            ]
        };
        toolbar.native.setDisplayMode(NSToolbarDisplayMode::IconOnly);
        toolbar.native.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        toolbar.delegate = Some(delegate);
        toolbar
    }

    pub fn attach(&self, window: &NSWindow) {
        window.setToolbar(Some(&self.native));
        if self.native.items().is_empty() {
            self.native.insertItemWithItemIdentifier_atIndex(
                unsafe { NSToolbarToggleSidebarItemIdentifier },
                0,
            );
            self.native.insertItemWithItemIdentifier_atIndex(
                unsafe { NSToolbarFlexibleSpaceItemIdentifier },
                1,
            );
        }
        self.native.setVisible(true);
    }

    /// Show one native navigation item without replacing the toolbar.
    pub fn set_back(&self, back: Option<(&str, Box<dyn FnMut()>)>) {
        let Some(delegate) = &self.delegate else {
            return;
        };
        let item = &delegate.ivars().back;
        let existing = self
            .native
            .items()
            .iter()
            .position(|item| item.itemIdentifier().to_string() == "cgs.back");
        if let Some((label, mut action)) = back {
            self.back_target.set(move |_| action());
            item.setToolTip(Some(&NSString::from_str(label)));
            item.setEnabled(true);
            if existing.is_none() {
                let index = self
                    .native
                    .items()
                    .iter()
                    .position(|item| {
                        item.itemIdentifier().to_string()
                            == unsafe { NSToolbarSidebarTrackingSeparatorItemIdentifier }
                                .to_string()
                    })
                    .map_or(1, |index| index + 1);
                self.native.insertItemWithItemIdentifier_atIndex(
                    &NSString::from_str("cgs.back"),
                    index as isize,
                );
            }
        } else {
            self.back_target.clear();
            item.setEnabled(false);
            if let Some(index) = existing {
                self.native.removeItemAtIndex(index as isize);
            }
        }
    }

    /// Standard toolbar arrows and a native title menu; AppKit owns their appearance.
    pub fn set_navigation(
        &self,
        ui: &Ui,
        mut back: impl FnMut() + 'static,
        mut forward: impl FnMut() + 'static,
        menu: Rc<crate::menu::Menu>,
    ) {
        let Some(delegate) = &self.delegate else {
            return;
        };
        // Reconfiguration replaces only history/menu items, preserving page controls.
        for id in ["cgs.history", "cgs.page-menu", "cgs.forward"] {
            let id = NSString::from_str(id);
            if let Some(index) =
                self.native.items().iter().position(|item| item.itemIdentifier() == id)
            {
                if id.to_string() == "cgs.page-menu" {
                    self.native.items().objectAtIndex(index).setView(None);
                }
                self.native.removeItemAtIndex(index as isize);
            }
            delegate
                .ivars()
                .navigation
                .borrow_mut()
                .retain(|item| item.itemIdentifier() != id);
        }
        self.back_target.set(move |_| back());
        self.forward_target.set(move |_| forward());
        let next = NSToolbarItem::initWithItemIdentifier(
            NSToolbarItem::alloc(ui.mtm()),
            &NSString::from_str("cgs.forward"),
        );
        next.setLabel(&NSString::from_str("Forward"));
        next.setToolTip(Some(&NSString::from_str("Forward")));
        next.setImage(Symbol::named("chevron.forward").as_deref());
        next.setNavigational(true);
        next.setAutovalidates(false);
        next.setBordered(false);
        unsafe {
            next.setTarget(Some(&self.forward_target));
            next.setAction(Some(objc2::sel!(invoke:)));
        }
        let arrows = NSToolbarItemGroup::initWithItemIdentifier(
            NSToolbarItemGroup::alloc(ui.mtm()),
            &NSString::from_str("cgs.history"),
        );
        arrows.setLabel(&NSString::from_str("Back and Forward"));
        arrows.setNavigational(true);
        arrows.setBordered(true);
        arrows.setSelectionMode(NSToolbarItemGroupSelectionMode::Momentary);
        arrows.setControlRepresentation(NSToolbarItemGroupControlRepresentation::Expanded);
        arrows.setSubitems(&NSArray::from_retained_slice(&[
            delegate.ivars().back.clone(),
            next.clone(),
        ]));
        let title = NSToolbarItem::initWithItemIdentifier(
            NSToolbarItem::alloc(ui.mtm()),
            &NSString::from_str("cgs.page-menu"),
        );
        title.setBordered(false);
        title.setAutovalidates(false);
        delegate.ivars().back.setBordered(false);
        delegate.ivars().back.setToolTip(Some(&NSString::from_str("Back")));
        delegate
            .ivars()
            .navigation
            .borrow_mut()
            .splice(0..0, [next, title, arrows.into_super()]);
        *self.menu.borrow_mut() = Some((menu, std::cell::OnceCell::new()));
        let index = self
            .native
            .items()
            .iter()
            .position(|item| item.itemIdentifier().to_string() == "cgs.page-title")
            .unwrap_or(0);
        self.native.insertItemWithItemIdentifier_atIndex(
            &NSString::from_str("cgs.history"),
            index as isize,
        );
    }

    pub fn update_navigation(&self, back: bool, forward: bool, title: &str, has_menu: bool) {
        let Some(delegate) = &self.delegate else {
            return;
        };
        delegate.ivars().back.setEnabled(back);
        let items = delegate.ivars().navigation.borrow();
        if let Some(item) = items.first() {
            item.setEnabled(forward);
        }
        if let (Some(item), Some((menu, popup))) = (items.get(1), self.menu.borrow().as_ref())
            && has_menu
        {
            let popup = popup.get_or_init(|| {
                let ui = Ui::new(self.native.mtm());
                let popup = NSPopUpButton::initWithFrame_pullsDown(
                    NSPopUpButton::alloc(ui.mtm()),
                    CGRect::ZERO,
                    false,
                );
                crate::controls::header_menu_style(&popup);
                popup.setImagePosition(NSCellImagePosition::ImageLeading);
                popup.setImageHugsTitle(false);
                popup.setMenu(Some(menu.ns_menu()));
                let display = MenuItem::new(&ui, "All");
                display
                    .ns_menu_item()
                    .setImage(Symbol::header("line.3.horizontal.decrease").as_deref());
                if let Some(cell) =
                    popup.cell().and_then(|cell| cell.downcast::<NSPopUpButtonCell>().ok())
                {
                    cell.setUsesItemFromMenu(false);
                    cell.setMenuItem(Some(display.ns_menu_item()));
                    cell.setAltersStateOfSelectedItem(false);
                }

                item.setView(Some(&popup));
                popup
            });
            item.setLabel(&NSString::from_str(title));
            for entry in menu.ns_menu().itemArray() {
                if entry.tag() == 1 || entry.tag() >= 9 {
                    entry.setState(if entry.title().to_string() == title {
                        NSControlStateValueOn
                    } else {
                        NSControlStateValueOff
                    });
                }
            }
            if let Some(cell) =
                popup.cell().and_then(|cell| cell.downcast::<NSPopUpButtonCell>().ok())
            {
                if let Some(display) = cell.menuItem() {
                    display.setTitle(&NSString::from_str(title));
                }
            }
            popup.sizeToFit();
            let mut frame = popup.frame();
            frame.size.width = (frame.size.width + 12.0).max(80.0);
            popup.setFrame(frame);
        }
        let index = self
            .native
            .items()
            .iter()
            .position(|item| item.itemIdentifier().to_string() == "cgs.page-menu");
        if has_menu && index.is_none() {
            self.native.insertItemWithItemIdentifier_atIndex(
                &NSString::from_str("cgs.page-menu"),
                self.native.items().len() as isize,
            );
        } else if !has_menu {
            if let Some(index) = index {
                self.native.removeItemAtIndex(index as isize);
            }
        }
    }

    /// Native trailing filter and search controls owned by the selected page.
    pub fn set_page_controls(&self, ui: &Ui, controls: Option<&HeaderControls>) {
        let Some(delegate) = &self.delegate else {
            return;
        };
        let views = controls.map_or([None, None, None], |controls| {
            [
                Some(controls.menu.ns_view()),
                Some(controls.search.ns_view()),
                controls.actions.as_ref().map(|actions| actions.ns_view()),
            ]
        });
        if self
            .page_controls
            .borrow()
            .iter()
            .zip(views)
            .all(|(weak, view)| match (weak, view) {
                (None, None) => true,
                (Some(weak), Some(view)) => {
                    weak.load().is_some_and(|old| std::ptr::eq(&*old, view))
                }
                _ => false,
            })
        {
            return;
        }
        *self.page_controls.borrow_mut() = views.map(|view| view.map(Weak::new));
        for id in ["cgs.page-filter", "cgs.page-actions", "cgs.page-search"] {
            let id = NSString::from_str(id);
            if let Some(index) =
                self.native.items().iter().position(|item| item.itemIdentifier() == id)
            {
                if let Some(search) =
                    self.native.items().objectAtIndex(index).downcast_ref::<NSSearchToolbarItem>()
                {
                    // Removing the item releases its field; it needs no replacement field.
                    search.searchField().removeFromSuperview();
                }
                let item = self.native.items().objectAtIndex(index);
                if item.downcast_ref::<NSSearchToolbarItem>().is_none() {
                    crate::view::detach_glass_content(item.view());
                    item.setView(None);
                }
                self.native.removeItemAtIndex(index as isize);
            }
            delegate
                .ivars()
                .navigation
                .borrow_mut()
                .retain(|item| item.itemIdentifier() != id);
        }
        if let Some(controls) = controls {
            let filter = controls.menu.ns_popup_button();
            let search = controls.search.ns_search_field();
            let item: Retained<NSToolbarItem> = if controls.icon_only {
                let item = NSMenuToolbarItem::initWithItemIdentifier(
                    NSMenuToolbarItem::alloc(ui.mtm()),
                    &NSString::from_str("cgs.page-filter"),
                );
                item.setLabel(&NSString::from_str(&controls.label));
                let content: Retained<NSView> = unsafe {
                    Retained::retain(filter as *const NSPopUpButton as *mut NSView).unwrap()
                };
                let glass = GlassEffectView::new(ui, content).corner_radius(18.0);
                glass.width(36.0);
                glass.height(36.0);
                item.setView(Some(glass.ns_view()));
                if let Some(menu) = filter.menu() {
                    item.setMenu(&menu);
                }
                item.setShowsIndicator(false);
                item.setBordered(false);
                item.into_super()
            } else {
                filter.sizeToFit();
                let mut frame = filter.frame();
                frame.size.width = (frame.size.width + 12.0).max(100.0);
                filter.setFrame(frame);
                let item = NSToolbarItem::initWithItemIdentifier(
                    NSToolbarItem::alloc(ui.mtm()),
                    &NSString::from_str("cgs.page-filter"),
                );
                item.setLabel(&NSString::from_str(&controls.label));
                item.setView(Some(filter));
                item.setBordered(false);
                item
            };
            let search_item = NSSearchToolbarItem::initWithItemIdentifier(
                NSSearchToolbarItem::alloc(ui.mtm()),
                &NSString::from_str("cgs.page-search"),
            );
            search_item.setLabel(
                &search.placeholderString().unwrap_or_else(|| NSString::from_str("Search")),
            );
            search_item.setSearchField(search);
            search_item.setPreferredWidthForSearchField(216.0);
            item.setVisibilityPriority(1000);
            search_item.setVisibilityPriority(-1000);
            let mut items = vec![item];
            if let Some(actions) = &controls.actions {
                let item = NSToolbarItem::initWithItemIdentifier(
                    NSToolbarItem::alloc(ui.mtm()),
                    &NSString::from_str("cgs.page-actions"),
                );
                item.setLabel(&NSString::from_str("Actions"));
                item.setVisibilityPriority(1000);
                item.setView(Some(actions.ns_view()));
                item.setBordered(false);
                items.push(item);
            }
            items.push(search_item.into_super());
            let ids: Vec<_> = items.iter().map(|item| item.itemIdentifier()).collect();
            delegate.ivars().navigation.borrow_mut().extend(items);
            for id in ids {
                self.native
                    .insertItemWithItemIdentifier_atIndex(&id, self.native.items().len() as isize);
            }
        }
    }

    /// A contextual trailing action, independent of navigation, search, and page filters.
    /// The caller keeps the control and its callback owner alive while it is attached.
    pub fn set_trailing_control(&self, ui: &Ui, control: Option<&dyn NativeView>) {
        let Some(delegate) = &self.delegate else { return };
        let id = NSString::from_str("cgs.page-trailing");
        if let Some(index) = self.native.items().iter().position(|item| item.itemIdentifier() == id)
        {
            let item = self.native.items().objectAtIndex(index);
            if control.is_some_and(|control| {
                item.view().is_some_and(|view| std::ptr::eq(&*view, control.ns_view()))
            }) {
                return;
            }
            item.setView(None);
            self.native.removeItemAtIndex(index as isize);
        }
        delegate
            .ivars()
            .navigation
            .borrow_mut()
            .retain(|item| item.itemIdentifier() != id);
        if let Some(control) = control {
            let item = NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(ui.mtm()), &id);
            item.setLabel(
                &control
                    .ns_view()
                    .accessibilityLabel()
                    .unwrap_or_else(|| NSString::from_str("Help")),
            );
            item.setView(Some(control.ns_view()));
            item.setBordered(false);
            delegate.ivars().navigation.borrow_mut().push(item);
            self.native
                .insertItemWithItemIdentifier_atIndex(&id, self.native.items().len() as isize);
        }
    }

    pub fn ns_toolbar(&self) -> &NSToolbar { &self.native }
}

impl Drop for Toolbar {
    fn drop(&mut self) {
        self.native.setDelegate(None);
        if let Some(delegate) = &self.delegate {
            unsafe {
                delegate.ivars().back.setTarget(None);
                delegate.ivars().back.setAction(None);
                for item in delegate.ivars().navigation.borrow().iter() {
                    if let Some(search) = item.downcast_ref::<NSSearchToolbarItem>() {
                        search.searchField().removeFromSuperview();
                    } else {
                        crate::view::detach_glass_content(item.view());
                        item.setView(None);
                    }
                    item.setTarget(None);
                    item.setAction(None);
                }
            }
        }
    }
}
