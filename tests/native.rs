use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cgs::*;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSApplicationActivationPolicy, NSControl, NSControlStateValueOn, NSControlTextEditingDelegate,
    NSDragOperation, NSDraggingInfo, NSEvent, NSEventType, NSPasteboard, NSPasteboardWriting,
    NSTableViewDataSource, NSTableViewDropOperation, NSTextField,
};
use objc2_foundation::{NSArray, NSNotification, NSObject, NSObjectProtocol, NSString};

fn action(control: &NSControl) {
    assert!(unsafe { control.sendAction_to(control.action(), control.target().as_deref()) });
}

fn callbacks_survive_composition_and_release_with_the_page(ui: &Ui) {
    let changes = Rc::new(RefCell::new(Vec::new()));
    let host = PageHost::new(ui);
    let native =
        autoreleasepool(|_| {
            let received = changes.clone();
            let switch = Switch::new(ui).on_change(move |value| received.borrow_mut().push(value));
            let native = Weak::new(switch.ns_switch());
            host.set_page(SettingsPage::new(ui, "General").section(
                Section::new(ui, "Behavior").row(SwitchRow::new(ui, "Animations", switch)),
            ));
            native
        });
    autoreleasepool(|_| {
        let switch = native.load().unwrap();
        switch.setState(NSControlStateValueOn);
        action(&switch);
        assert_eq!(*changes.borrow(), [true]);
    });
    autoreleasepool(|_| host.set_page(Label::new(ui, "Another page")));
    assert!(
        native.load().is_none(),
        "replacing a page must release its controls"
    );
    assert_eq!(Rc::strong_count(&changes), 1, "action closure must be released");

    let clicks = Rc::new(Cell::new(0));
    let c = clicks.clone();
    let menu = Menu::new(ui).item(MenuItem::new(ui, "Reset").on_click(move || c.set(c.get() + 1)));
    autoreleasepool(|_| {
        menu.ns_menu().performActionForItemAtIndex(0);
        assert_eq!(clicks.get(), 1);
        menu.clear();
    });
    assert_eq!(Rc::strong_count(&clicks), 1);
}

fn callbacks_can_remove_their_own_controls(ui: &Ui) {
    let slot = Rc::new(RefCell::new(None::<Button>));
    let owner = Rc::downgrade(&slot);
    let button = Button::new(ui, "Remove").on_click(move || {
        owner.upgrade().unwrap().borrow_mut().take();
    });
    let native = button.ns_button().retain();
    let target = autoreleasepool(|_| Weak::new(&*native.target().unwrap()));
    *slot.borrow_mut() = Some(button);
    autoreleasepool(|_| action(&native));
    assert!(slot.borrow().is_none());
    assert!(
        target.load().is_none(),
        "target must finish dispatch before deallocating"
    );
}

fn numeric_fields_reject_invalid_commits(ui: &Ui) {
    let values = Rc::new(RefCell::new(Vec::new()));
    let copy = values.clone();
    let number = Rc::new(
        NumberField::new(ui)
            .integer()
            .range(1.0, 10.0)
            .on_change(move |value| copy.borrow_mut().push(value)),
    );
    let _row = SettingsRow::new(ui, "Offset", number.clone()).suffix("pt");
    number.set_value(4.0);
    assert_eq!(number.ns_text_field().stringValue().to_string(), "4");
    let unit = unsafe { number.ns_text_field().superview() }
        .unwrap()
        .subviews()
        .iter()
        .filter_map(|view| view.downcast_ref::<NSTextField>().map(|field| field.retain()))
        .find(|field| field.stringValue().to_string() == "pt")
        .unwrap();
    assert!(!unit.isEditable());
    assert!(!unit.isSelectable());
    let field = number.ns_text_field();
    for text in ["4", "11", "not a number", "2.5", "-1"] {
        field.setStringValue(&NSString::from_str(text));
        let note = unsafe {
            NSNotification::notificationWithName_object(
                &NSString::from_str("NSControlTextDidEndEditingNotification"),
                Some(field),
            )
        };
        field.delegate().unwrap().controlTextDidEndEditing(&note);
    }
    assert_eq!(*values.borrow(), [4.0]);
    let number = NumberField::new(ui).range(0.0, 1.0).value(0.25);
    assert_eq!(number.get_value(), Some(0.25));
}

fn delegates_and_selection_use_current_data(ui: &Ui) {
    let selected = Rc::new(RefCell::new(Vec::new()));
    let s = selected.clone();
    let ui_copy = *ui;
    let table = Table::new(ui)
        .column("name", "Name", 240.0)
        .cells(move |value: &String, _| Box::new(Label::new(&ui_copy, value)))
        .rows(vec!["one".into(), "two".into()])
        .on_select_item(move |value| s.borrow_mut().push(value));
    table.set_selected(Some(1));
    assert_eq!(selected.borrow().last(), Some(&Some("two".to_string())));
    table.set_rows(vec!["new".into()]);
    table.set_selected(Some(0));
    assert_eq!(selected.borrow().last(), Some(&Some("new".to_string())));

    let edits = Rc::new(RefCell::new(Vec::new()));
    let e = edits.clone();
    let commits = Rc::new(Cell::new(0));
    let committed = commits.clone();
    let field = TextField::new(ui)
        .on_change(move |text| e.borrow_mut().push(text))
        .on_commit(move |_| committed.set(committed.get() + 1));
    field.set_value("edited");
    let note = unsafe {
        NSNotification::notificationWithName_object(
            &NSString::from_str("NSControlTextDidChangeNotification"),
            Some(field.ns_text_field()),
        )
    };
    field.ns_text_field().delegate().unwrap().controlTextDidChange(&note);
    assert_eq!(*edits.borrow(), ["edited"]);
    field.ns_text_field().delegate().unwrap().controlTextDidEndEditing(&note);
    assert_eq!(commits.get(), 1);

    let outline = Outline::new(ui).items(vec![OutlineItem {
        id: 1,
        title: "Group".into(),
        children: vec![OutlineItem {
            id: 2,
            title: "Child".into(),
            children: vec![],
        }],
    }]);
    outline.expand_all();
    assert_eq!(outline.ns_outline_view().numberOfRows(), 2);
}

fn event(code: u16, chars: &str, modifiers: Modifiers) -> Retained<NSEvent> {
    NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
        NSEventType::KeyDown, CGPoint::ZERO, modifiers, 0.0, 0, None,
        &NSString::from_str(chars), &NSString::from_str(chars), false, code,
    ).unwrap()
}

fn recording_is_scoped_and_cancellable(ui: &Ui) {
    let values = Rc::new(RefCell::new(Vec::new()));
    let v = values.clone();
    let recorder = KeyRecorder::new(ui).on_change(move |value| v.borrow_mut().push(value));
    recorder.begin_recording();
    assert!(recorder.ns_button().performKeyEquivalent(&event(
        4,
        "h",
        Modifiers::Control | Modifiers::Option | Modifiers::Shift | Modifiers::Command,
    )));
    assert!(!recorder.ns_button().performKeyEquivalent(&event(4, "h", Modifiers::Command)));
    let saved = recorder.get_value().unwrap();
    assert_eq!(saved.to_string(), "⌃⌥⇧⌘H");
    assert_eq!(values.borrow().len(), 1);
    recorder.begin_recording();
    recorder.ns_button().keyDown(&event(53, "\u{1b}", Modifiers::empty()));
    assert_eq!(recorder.get_value(), Some(saved));
    assert_eq!(
        values.borrow().len(),
        1,
        "Escape cancels without publishing a change"
    );
    recorder.begin_recording();
    recorder.ns_button().keyDown(&event(51, "\u{7f}", Modifiers::empty()));
    assert!(recorder.get_value().is_none());
    assert_eq!(values.borrow().last(), Some(&None));
}

struct DragState {
    source: Retained<AnyObject>,
    pasteboard: Retained<NSPasteboard>,
}
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgUiTestDragInfo"]
    #[ivars = DragState]
    struct DragInfo;
    unsafe impl NSObjectProtocol for DragInfo {}
    unsafe impl NSDraggingInfo for DragInfo {
        #[unsafe(method(animatesToDestination))]
        fn animates(&self) -> bool { false }

        #[unsafe(method(setAnimatesToDestination:))]
        fn set_animates(&self, _value: bool) {}

        #[unsafe(method_id(draggingDestinationWindow))]
        fn destination(&self) -> Option<Retained<objc2_app_kit::NSWindow>> { None }

        #[unsafe(method(draggingSourceOperationMask))]
        fn operations(&self) -> NSDragOperation { NSDragOperation::Move }

        #[unsafe(method(draggingLocation))]
        fn location(&self) -> CGPoint { CGPoint::ZERO }

        #[unsafe(method(draggedImageLocation))]
        fn image_location(&self) -> CGPoint { CGPoint::ZERO }

        #[unsafe(method_id(draggedImage))]
        fn image(&self) -> Option<Retained<objc2_app_kit::NSImage>> { None }

        #[unsafe(method(draggingSequenceNumber))]
        fn sequence(&self) -> isize { 0 }

        #[unsafe(method(slideDraggedImageTo:))]
        fn slide(&self, _point: CGPoint) {}

        #[unsafe(method_id(namesOfPromisedFilesDroppedAtDestination:))]
        fn promised_files(
            &self,
            _url: &objc2_foundation::NSURL,
        ) -> Option<Retained<NSArray<NSString>>> {
            None
        }

        #[unsafe(method(draggingFormation))]
        fn formation(&self) -> objc2_app_kit::NSDraggingFormation {
            objc2_app_kit::NSDraggingFormation::None
        }

        #[unsafe(method(setDraggingFormation:))]
        fn set_formation(&self, _value: objc2_app_kit::NSDraggingFormation) {}

        #[unsafe(method(numberOfValidItemsForDrop))]
        fn valid_items(&self) -> isize { 1 }

        #[unsafe(method(setNumberOfValidItemsForDrop:))]
        fn set_valid_items(&self, _value: isize) {}

        #[unsafe(method(springLoadingHighlight))]
        fn highlight(&self) -> objc2_app_kit::NSSpringLoadingHighlight {
            objc2_app_kit::NSSpringLoadingHighlight::None
        }

        #[unsafe(method(resetSpringLoading))]
        fn reset_spring_loading(&self) {}

        #[unsafe(method(enumerateDraggingItemsWithOptions:forView:classes:searchOptions:usingBlock:))]
        unsafe fn enumerate(
            &self,
            _opts: objc2_app_kit::NSDraggingItemEnumerationOptions,
            _view: Option<&objc2_app_kit::NSView>,
            _classes: &NSArray<objc2::runtime::AnyClass>,
            _options: &objc2_foundation::NSDictionary<NSString, AnyObject>,
            _block: &block2::DynBlock<
                dyn Fn(
                    std::ptr::NonNull<objc2_app_kit::NSDraggingItem>,
                    isize,
                    std::ptr::NonNull<objc2::runtime::Bool>,
                ),
            >,
        ) {
        }

        #[unsafe(method_id(draggingSource))]
        fn source(&self) -> Option<Retained<AnyObject>> { Some(self.ivars().source.clone()) }

        #[unsafe(method_id(draggingPasteboard))]
        fn pasteboard(&self) -> Retained<NSPasteboard> { self.ivars().pasteboard.clone() }
    }
);

fn local_reordering_uses_final_indices_and_rejects_other_tables(ui: &Ui) {
    let moved = Rc::new(RefCell::new(Vec::new()));
    let m = moved.clone();
    let selected = Rc::new(RefCell::new(None));
    let selected_copy = selected.clone();
    let table = Table::new(ui)
        .column("item", "", 200.0)
        .rows(vec!["a", "b", "c"])
        .reorderable(true)
        .on_reorder(move |from, to| m.borrow_mut().push((from, to)))
        .on_select_item(move |value| *selected_copy.borrow_mut() = value);
    let data_source = unsafe { table.ns_table_view().dataSource() }.unwrap();
    let item = data_source.tableView_pasteboardWriterForRow(table.ns_table_view(), 0).unwrap();
    let pasteboard = NSPasteboard::pasteboardWithUniqueName();
    assert!(pasteboard.writeObjects(
        &NSArray::<ProtocolObject<dyn NSPasteboardWriting>>::from_slice(&[&*item])
    ));
    let drag = DragInfo::alloc(ui.mtm()).set_ivars(DragState {
        source: table.ns_table_view().retain().into(),
        pasteboard,
    });
    let drag: Retained<DragInfo> = unsafe { msg_send![super(drag), init] };
    let info = ProtocolObject::from_ref(&*drag);
    assert_eq!(
        data_source.tableView_validateDrop_proposedRow_proposedDropOperation(
            table.ns_table_view(),
            info,
            3,
            NSTableViewDropOperation::Above
        ),
        NSDragOperation::Move
    );
    assert!(data_source.tableView_acceptDrop_row_dropOperation(
        table.ns_table_view(),
        info,
        3,
        NSTableViewDropOperation::Above
    ));
    assert_eq!(*moved.borrow(), [(0, 2)]);
    assert_eq!(*selected.borrow(), Some("a"));
    table.set_selected(Some(0));
    assert_eq!(*selected.borrow(), Some("b"));
    let other = Table::<&str>::new(ui).reorderable(true);
    let other_source = unsafe { other.ns_table_view().dataSource() }.unwrap();
    assert_eq!(
        other_source.tableView_validateDrop_proposedRow_proposedDropOperation(
            other.ns_table_view(),
            info,
            0,
            NSTableViewDropOperation::Above
        ),
        NSDragOperation::None
    );
}

fn pages_start_at_top_and_controllers_leave_the_host(ui: &Ui) {
    let mut section = Section::new(ui, "Long page");
    for _ in 0..30 {
        section = section.row(SwitchRow::new(ui, "Option", Switch::new(ui)));
    }
    let controls = Rc::new(AddRemoveControl::new(ui));
    let page = SettingsPage::new(ui, "Settings").section(section).bottom_bar(controls.clone());
    let scroll = Weak::new(page.ns_scroll_view());
    let window = Window::new(ui).size(CGSize::new(640.0, 400.0)).content(page);
    window.ns_window().contentView().unwrap().layoutSubtreeIfNeeded();
    let scroll = scroll.load().unwrap();
    let document = scroll.documentView().unwrap();
    assert!(document.isFlipped());
    assert!(document.frame().size.height > scroll.contentView().bounds().size.height);
    assert!(
        scroll.documentVisibleRect().origin.y.abs() < 1.0,
        "new pages must begin at the top"
    );
    let position = controls.ns_view().convertRect_toView(controls.ns_view().bounds(), None);
    scroll.contentView().scrollToPoint(CGPoint::new(0.0, 400.0));
    scroll.reflectScrolledClipView(&scroll.contentView());
    window.ns_window().contentView().unwrap().layoutSubtreeIfNeeded();
    assert_eq!(
        controls.ns_view().convertRect_toView(controls.ns_view().bounds(), None),
        position,
        "collection actions must remain fixed while the page scrolls"
    );
    let host = Rc::new(PageHost::new(ui));
    let navigation = NavigationSplitView::new(
        ui,
        Sidebar::new(ui, vec![SidebarItem {
            id: 0,
            title: "General".into(),
            symbol: "gearshape".into(),
        }]),
        host.clone(),
    );
    let _window = SettingsWindow::new(ui, "cgs tests").content(navigation);
    let controller = ViewController::new(ui, Label::new(ui, "Child"));
    let child = Weak::new(controller.ns_view_controller());
    host.set_page(controller);
    autoreleasepool(|_| {
        assert!(child.load().unwrap().parentViewController().is_some());
        host.set_page(Label::new(ui, "Replacement"));
        assert!(host.ns_view_controller().childViewControllers().is_empty());
    });
}

fn settings_lists_do_not_materialize_offscreen_rows(ui: &Ui) {
    let configured = Rc::new(Cell::new(0));
    let count = configured.clone();
    let list = SettingsList::new(
        ui,
        move |row: &usize| {
            count.set(count.get() + 1);
            format!("Layout {row}")
        },
        |_| "Description".into(),
    )
    .navigation()
    .fit_content(120.0);
    list.set_rows((0..200).collect());
    assert_eq!(list.ns_table_view().rectOfRow(0).size.height, 56.0);
    assert!(
        configured.get() < 32,
        "sizing a capped list created {} row hierarchies",
        configured.get()
    );
    let configured = Rc::new(Cell::new(0));
    let count = configured.clone();
    let list = Rc::new(
        SettingsList::new(
            ui,
            move |row: &usize| {
                count.set(count.get() + 1);
                format!("Rule {row}")
            },
            |_| "Description".into(),
        )
        .navigation()
        .full_length(),
    );
    list.min_height(100.0);
    list.set_rows((0..10_000).collect());
    let controls = Rc::new(AddRemoveControl::new(ui));
    let page = SettingsPage::new(ui, "").section(list.clone()).bottom_bar(controls.clone());
    let scroll = Weak::new(page.ns_scroll_view());
    let window = Window::new(ui).size(CGSize::new(600.0, 300.0)).content(page);
    window.ns_window().contentView().unwrap().layoutSubtreeIfNeeded();
    assert!(
        !list.ns_scroll_view().hasVerticalScroller(),
        "the enclosing page must own scrolling"
    );
    let scroll = scroll.load().unwrap();
    let position = controls.ns_view().convertRect_toView(controls.ns_view().bounds(), None);
    scroll.contentView().scrollToPoint(CGPoint::new(0.0, 20_000.0));
    scroll.reflectScrolledClipView(&scroll.contentView());
    window.ns_window().contentView().unwrap().layoutSubtreeIfNeeded();
    objc2_foundation::NSRunLoop::currentRunLoop()
        .runUntilDate(&objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(0.03));
    assert_eq!(
        controls.ns_view().convertRect_toView(controls.ns_view().bounds(), None),
        position,
        "list actions must remain fixed while the page scrolls"
    );
    assert!(
        configured.get() < 64,
        "page-scrolled collection materialized {} rows",
        configured.get()
    );
    assert!(
        controls.ns_view().window().is_some(),
        "editor must retain its action footer"
    );
}

fn reused_cells_clear_missing_images(ui: &Ui) {
    fn image(view: &objc2_app_kit::NSView) -> Option<Retained<objc2_app_kit::NSImageView>> {
        if let Some(icon) = view.downcast_ref::<objc2_app_kit::NSImageView>() {
            return Some(icon.retain());
        }
        view.subviews().iter().find_map(|child| image(&child))
    }
    let list = Rc::new(
        SettingsList::new(ui, |row: &bool| format!("Icon {row}"), |_| String::new())
            .fit_content(120.0)
            .images(move |present| {
                present.then(|| {
                    objc2_app_kit::NSWorkspace::sharedWorkspace()
                        .iconForFile(&NSString::from_str("/System/Applications/Music.app"))
                })
            }),
    );
    list.set_rows((0..200).map(|row| row < 4).collect());
    let window = Window::new(ui).size(CGSize::new(600.0, 140.0)).content(list.clone());
    window.show();
    window.ns_window().contentView().unwrap().layoutSubtreeIfNeeded();
    let first = list.ns_table_view().viewAtColumn_row_makeIfNecessary(0, 0, true).unwrap();
    assert!(image(&first).unwrap().image().is_some());
    let mut pointers = std::collections::HashSet::new();
    pointers.insert(&*first as *const _ as usize);
    drop(first);
    let mut reused = false;
    for row in [20, 40, 60, 80, 100, 120, 140, 160, 180, 20] {
        list.ns_table_view().scrollRowToVisible(row);
        objc2_foundation::NSRunLoop::currentRunLoop()
            .runUntilDate(&objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(0.03));
        window.ns_window().contentView().unwrap().layoutSubtreeIfNeeded();
        let cell = list.ns_table_view().viewAtColumn_row_makeIfNecessary(0, row, true).unwrap();
        reused |= !pointers.insert(&*cell as *const _ as usize);
        assert!(
            image(&cell).unwrap().image().is_none(),
            "reused cells must clear stale application icons"
        );
    }
    assert!(reused, "scrolling must recycle cells");
    window.close();
}

fn unchanged_popup_items_preserve_selection_and_native_items(ui: &Ui) {
    let duplicates = Popup::new(ui).items(["Same app", "Same app", "Another app"]);
    assert_eq!(
        duplicates.ns_popup_button().numberOfItems(),
        3,
        "duplicate names must not shift selection indices"
    );
    duplicates.set_selected(2);
    assert_eq!(
        duplicates.ns_popup_button().titleOfSelectedItem().unwrap().to_string(),
        "Another app"
    );
    let popup = Popup::new(ui).items(["One", "Two"]);
    popup.set_selected(1);
    let first = popup.ns_popup_button().itemAtIndex(0).unwrap();
    popup.set_items(["One", "Two"]);
    assert_eq!(popup.selected(), Some(1));
    assert!(std::ptr::eq(
        &*first,
        &*popup.ns_popup_button().itemAtIndex(0).unwrap()
    ));
}

fn page_headings_preserve_window_identity(ui: &Ui) {
    let title = Rc::new(Label::new(ui, "General"));
    let window = SettingsWindow::new(ui, "Rift Settings").page_title(ui, title.clone());
    let items = window.ns_window().toolbar().unwrap().items();
    let item = items
        .iter()
        .find(|item| item.itemIdentifier().to_string() == "cgs.page-title")
        .expect("the page heading must be installed in the native toolbar");
    let heading = item.view().unwrap();
    title.set_text("Keyboard");
    assert!(title.ns_text_field().isDescendantOf(&heading));
    assert_eq!(title.ns_text_field().stringValue().to_string(), "Keyboard");
    assert_eq!(window.ns_window().title().to_string(), "Rift Settings");
}

fn navigation_uses_native_toolbar_items_and_responder_chain(ui: &Ui) {
    let title = Rc::new(Label::new(ui, "Workspaces"));
    let window = SettingsWindow::new(ui, "Navigation test").page_title(ui, title);
    let toolbar = window.toolbar();
    let original = toolbar.ns_toolbar() as *const _;
    let calls = Rc::new(Cell::new(0));
    let count = calls.clone();
    toolbar.set_back(Some((
        "Back to Workspaces",
        Box::new(move || count.set(count.get() + 1)),
    )));
    let back = toolbar
        .ns_toolbar()
        .items()
        .iter()
        .find(|item| item.itemIdentifier().to_string() == "cgs.back")
        .unwrap();
    assert!(back.isNavigational());
    assert!(
        back.view().is_none(),
        "AppKit must render the navigation control"
    );
    assert_eq!(back.toolTip().unwrap().to_string(), "Back to Workspaces");
    let command = unsafe {
        objc2_app_kit::NSMenuItem::initWithTitle_action_keyEquivalent(
            objc2_app_kit::NSMenuItem::alloc(ui.mtm()),
            &NSString::from_str("Back"),
            Some(objc2::sel!(cgsGoBack:)),
            &NSString::from_str("["),
        )
    };
    let validated =
        ProtocolObject::<dyn objc2_app_kit::NSValidatedUserInterfaceItem>::from_ref(&*command);
    assert!(
        objc2_app_kit::NSUserInterfaceValidations::validateUserInterfaceItem(
            window.ns_window(),
            validated
        )
    );
    window.show();
    assert!(unsafe {
        objc2_app_kit::NSApplication::sharedApplication(ui.mtm()).sendAction_to_from(
            objc2::sel!(cgsGoBack:),
            Some(window.ns_window()),
            None,
        )
    });
    assert_eq!(calls.get(), 1);
    toolbar.set_back(None);
    assert!(
        !objc2_app_kit::NSUserInterfaceValidations::validateUserInterfaceItem(
            window.ns_window(),
            validated
        )
    );
    assert_eq!(toolbar.ns_toolbar() as *const _, original);
    assert!(
        !toolbar
            .ns_toolbar()
            .items()
            .iter()
            .any(|item| item.itemIdentifier().to_string() == "cgs.back")
    );
    window.close();
}

fn cached_pages_keep_their_mount_and_release_on_clear(ui: &Ui) {
    let host = Rc::new(PageHost::new(ui));
    let (first, second) = autoreleasepool(|_| {
        let field = Rc::new(TextField::new(ui));
        field.set_value("Retained edit");
        let mut rows =
            Section::new(ui, "Cached page").row(SettingsRow::new(ui, "Name", field.clone()));
        for _ in 0..30 {
            rows = rows.row(SwitchRow::new(ui, "Option", Switch::new(ui)));
        }
        let first = Rc::new(SettingsPage::new(ui, "").section(rows));
        let window = Window::new(ui).size(CGSize::new(600.0, 300.0)).content(host.clone());
        let second = Rc::new(ViewController::new(ui, Label::new(ui, "Second")));
        host.set_cached_page(first.clone());
        window.ns_window().contentView().unwrap().layoutSubtreeIfNeeded();
        first.ns_scroll_view().contentView().scrollToPoint(CGPoint::new(0.0, 300.0));
        first
            .ns_scroll_view()
            .reflectScrolledClipView(&first.ns_scroll_view().contentView());
        let position = first.ns_scroll_view().documentVisibleRect().origin;
        host.set_cached_page(second.clone());
        assert!(
            first.ns_view().isHidden(),
            "leaving a cached page parks it hidden"
        );
        assert!(unsafe { first.ns_view().superview() }.is_some());
        assert!(second.ns_view_controller().parentViewController().is_some());
        host.set_cached_page(first.clone());
        assert!(second.ns_view().isHidden());
        assert!(!first.ns_view().isHidden());
        assert_eq!(field.get_value(), "Retained edit");
        window.ns_window().contentView().unwrap().layoutSubtreeIfNeeded();
        assert_eq!(
            first.ns_scroll_view().documentVisibleRect().origin,
            position,
            "returning to a cached page must preserve its scroll position"
        );
        assert_eq!(
            first.ns_view().frame().size,
            host.ns_view().bounds().size,
            "a returning page fills the host"
        );
        // A dropped cache entry is unmounted at the next switch rather than kept alive.
        let third = Rc::new(Label::new(ui, "Third"));
        let dropped = Weak::new(third.ns_view());
        host.set_cached_page(third);
        host.set_cached_page(first.clone());
        assert!(dropped.load().is_none_or(|view| unsafe { view.superview() }.is_none()));
        assert_eq!(host.ns_view().subviews().len(), 2);
        (
            Weak::new(first.ns_view()),
            Weak::new(second.ns_view_controller()),
        )
    });
    autoreleasepool(|_| host.clear());
    assert!(first.load().is_none(), "clear must release the active page");
    assert!(
        second.load().is_none(),
        "detached controller must be released with its caller cache"
    );
    assert!(host.ns_view().subviews().is_empty());
}

fn released_components_detach_borrowed_native_content(ui: &Ui) {
    autoreleasepool(|_| {
        let host = PageHost::new(ui);
        // A legitimate native owner must not keep pages attached after the Rust host dies.
        let native = host.ns_view().retain();
        let controller = host.ns_view_controller().retain();
        let first = Rc::new(ViewController::new(ui, Label::new(ui, "Cached")));
        let child = Weak::new(first.ns_view_controller());
        host.set_cached_page(first);
        host.set_page(Label::new(ui, "Current"));
        drop(host);
        assert!(
            native.subviews().is_empty(),
            "host drop must unmount all pages"
        );
        assert!(controller.childViewControllers().is_empty());
        assert!(
            child
                .load()
                .is_none_or(|child| child.parentViewController().is_none())
        );
    });
}

fn replaced_footer_releases_native_content(ui: &Ui) {
    autoreleasepool(|_| {
        let (page, weak) = autoreleasepool(|_| {
            let old = Label::new(ui, "Old footer");
            let weak = Weak::new(old.ns_view());
            (
                SettingsPage::new(ui, "Footer")
                    .bottom_bar(old)
                    .bottom_bar(Label::new(ui, "New footer")),
                weak,
            )
        });
        assert!(
            weak.load().is_none(),
            "replacing a footer must release the old view"
        );
        drop(page);
    });
}

fn replaced_status_releases_native_content(ui: &Ui) {
    autoreleasepool(|_| {
        let (status, weak) = autoreleasepool(|_| {
            let old = Label::new(ui, "Old status");
            let weak = Weak::new(old.ns_view());
            (
                StatusItem::new(ui)
                    .content(old)
                    .content(Label::new(ui, "New status")),
                weak,
            )
        });
        assert!(
            weak.load().is_none(),
            "replacing status content must release the old view"
        );
        drop(status);
    });
}

fn toolbar_releases_disabled_actions_and_preserves_unchanged_controls(ui: &Ui) {
    let window = SettingsWindow::new(ui, "Toolbar lifecycle");
    let toolbar = window.toolbar();
    let captured = Rc::new(());
    let copy = captured.clone();
    toolbar.set_back(Some((
        "Back",
        Box::new(move || {
            let _ = &copy;
        }),
    )));
    toolbar.set_back(None);
    assert_eq!(
        Rc::strong_count(&captured),
        1,
        "disabled action must release its captures"
    );
    let weak_toolbar = Rc::downgrade(toolbar);
    let copy = captured.clone();
    toolbar.set_back(Some((
        "Back",
        Box::new(move || {
            let _ = &copy;
            weak_toolbar.upgrade().unwrap().set_back(None);
        }),
    )));
    let item = toolbar
        .ns_toolbar()
        .items()
        .iter()
        .find(|item| item.itemIdentifier().to_string() == "cgs.back")
        .unwrap();
    let target = item.target().unwrap();
    unsafe {
        let _: () = msg_send![&*target, invoke: &*item];
    }
    assert_eq!(
        Rc::strong_count(&captured),
        1,
        "an action can clear itself during dispatch"
    );
    let controls = HeaderControls::new(
        ui,
        "Filter",
        "line.3.horizontal.decrease",
        Menu::new(ui).item(MenuItem::new(ui, "All")),
        SearchField::new(ui),
    );
    toolbar.set_page_controls(ui, Some(&controls));
    let items = toolbar.ns_toolbar().items();
    toolbar.set_page_controls(ui, Some(&controls));
    assert_eq!(items.len(), toolbar.ns_toolbar().items().len());
    for (old, new) in items.iter().zip(toolbar.ns_toolbar().items()) {
        assert!(
            std::ptr::eq(&*old, &*new),
            "unchanged controls must reuse toolbar items"
        );
    }
    let old = autoreleasepool(|_| {
        toolbar.set_navigation(ui, || {}, || {}, Rc::new(Menu::new(ui)));
        let group = toolbar
            .ns_toolbar()
            .items()
            .iter()
            .find(|item| item.itemIdentifier().to_string() == "cgs.history")
            .unwrap();
        let weak = Weak::new(&*group);
        toolbar.set_navigation(ui, || {}, || {}, Rc::new(Menu::new(ui)));
        weak
    });
    assert!(
        old.load().is_none(),
        "reconfiguring history must release its previous group"
    );
    assert_eq!(
        toolbar
            .ns_toolbar()
            .items()
            .iter()
            .filter(|item| item.itemIdentifier().to_string() == "cgs.history")
            .count(),
        1
    );
    assert!(
        toolbar
            .ns_toolbar()
            .items()
            .iter()
            .any(|item| item.itemIdentifier().to_string() == "cgs.page-search")
    );
}

fn window_close_is_idempotent_and_drop_does_not_dispatch(ui: &Ui) {
    let calls = Rc::new(Cell::new(0));
    let copy = calls.clone();
    let window = Window::new(ui).on_close(move || copy.set(copy.get() + 1));
    window.show();
    window.ns_window().close();
    window.close();
    assert_eq!(calls.get(), 1);
    window.show();
    window.close();
    assert_eq!(calls.get(), 2, "a reopened window must close normally");
    drop(window);
    assert_eq!(
        calls.get(),
        2,
        "destruction must detach callbacks before closing"
    );
    let copy = calls.clone();
    let window = Window::new(ui).on_close(move || copy.set(copy.get() + 1));
    window.show();
    drop(window);
    assert_eq!(calls.get(), 2);
}

// Probe the public native boundary without extending any object's lifetime.
fn probe(
    probes: &mut Vec<(&'static str, Weak<AnyObject>)>,
    name: &'static str,
    object: &AnyObject,
) {
    probes.push((name, Weak::new(object)));
}

fn settle() {
    autoreleasepool(|_| {
        objc2_foundation::NSRunLoop::currentRunLoop().runUntilDate(
            &objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(0.15),
        );
    });
}

fn settings_lifecycle(
    ui: &Ui,
    explicit_close: bool,
    open: impl FnOnce(),
) -> Vec<(&'static str, Weak<AnyObject>)> {
    autoreleasepool(|_| {
        let mut probes = Vec::new();
        let host = Rc::new(PageHost::new(ui));
        probe(&mut probes, "page host", host.ns_view());
        probe(&mut probes, "host controller", host.ns_view_controller());
        let sidebar = Sidebar::new(
            ui,
            (0..3)
                .map(|id| SidebarItem {
                    id,
                    title: format!("Page {id}"),
                    symbol: "gearshape".into(),
                })
                .collect(),
        );
        probe(&mut probes, "sidebar", sidebar.ns_view());
        let window = SettingsWindow::new(ui, "cgs lifecycle")
            .page_title(ui, Rc::new(Label::new(ui, "General")))
            .content(NavigationSplitView::new(ui, sidebar, host.clone()))
            .on_close(|| {});
        probe(&mut probes, "window", window.ns_window());
        probe(
            &mut probes,
            "window delegate",
            (&*window.ns_window().delegate().unwrap()).as_ref(),
        );
        let toolbar = window.toolbar();
        probe(&mut probes, "toolbar", toolbar.ns_toolbar());
        probe(
            &mut probes,
            "toolbar delegate",
            (&*toolbar.ns_toolbar().delegate().unwrap()).as_ref(),
        );
        let pages: Vec<Rc<dyn NativeView>> = (0..3)
            .map(|id| {
                let field = TextField::new(ui).on_change(|_| {});
                probe(&mut probes, "text field", field.ns_view());
                probe(
                    &mut probes,
                    "field delegate",
                    (&*field.ns_text_field().delegate().unwrap()).as_ref(),
                );
                let switch = Switch::new(ui).on_change(|_| {});
                probe(&mut probes, "switch", switch.ns_view());
                probe(
                    &mut probes,
                    "action target",
                    &*switch.ns_switch().target().unwrap(),
                );
                let popup = Popup::new(ui).items(["One", "Two"]).on_change(|_| {});
                probe(&mut probes, "popup", popup.ns_view());
                let list =
                    SettingsList::new(ui, |v: &usize| format!("Item {v}"), |_| "Summary".into())
                        .navigation()
                        .fit_content(160.0)
                        .on_open(|_| {});
                list.set_rows((0..100).collect());
                probe(&mut probes, "table", list.ns_table_view());
                probe(
                    &mut probes,
                    "collection delegate",
                    (&*unsafe { list.ns_table_view().delegate() }.unwrap()).as_ref(),
                );
                let info = InfoButton::new(ui, "Option", "Details");
                probe(&mut probes, "info button", info.ns_view());
                let glass = GlassEffectView::new(ui, Label::new(ui, "Glass"));
                probe(&mut probes, "glass content", glass.ns_view());
                let page = ViewController::new(
                    ui,
                    SettingsPage::new(ui, &format!("Page {id}"))
                        .section(
                            Section::new(ui, "Options")
                                .row(SettingsRow::new(ui, "Name", field))
                                .row(SwitchRow::new(ui, "Enabled", switch))
                                .row(SettingsRow::new(ui, "Mode", popup))
                                .row(SettingsRow::new(ui, "Help", info)),
                        )
                        .section(list)
                        .section(glass),
                );
                probe(&mut probes, "page controller", page.ns_view_controller());
                Rc::new(page) as Rc<dyn NativeView>
            })
            .collect();
        window.show();
        for page in &pages {
            host.set_cached_page(page.clone());
            let search = SearchField::new(ui).placeholder("Search").on_change(|_| {});
            probe(&mut probes, "search field", search.ns_view());
            let controls = HeaderControls::new(
                ui,
                "Filter",
                "line.3.horizontal.decrease",
                Menu::new(ui).item(MenuItem::new(ui, "All").on_click(|| {})),
                search,
            );
            toolbar.set_page_controls(ui, Some(&controls));
            window
                .ns_window()
                .contentView()
                .unwrap()
                .layoutSubtreeIfNeeded();
            for item in toolbar.ns_toolbar().items() {
                probe(&mut probes, "toolbar item", &item);
            }
            // Replace controls while the previous native items still have weak probes.
            toolbar.set_page_controls(ui, None);
        }
        let sheet = Sheet::new(ui, "Editor", TextField::new(ui));
        probe(&mut probes, "sheet", sheet.ns_window());
        assert!(sheet.show(&window.handle()));
        sheet.end();
        let popover = Popover::new(ui, Label::new(ui, "Details"));
        probe(&mut probes, "popover", popover.ns_popover());
        probe(
            &mut probes,
            "popover controller",
            popover.ns_view_controller(),
        );
        popover.show(&host);
        popover.close();
        popover.show(&host);
        popover.close();
        open();
        if explicit_close {
            window.close();
        }
        probes
    })
}

fn active_presentations_teardown_with_their_owners(ui: &Ui) {
    let (window_probe, sheet_probe, popover_probe, controller_probe) = autoreleasepool(|_| {
        let anchor = Rc::new(Label::new(ui, "Anchor"));
        let window = Window::new(ui).content(anchor.clone());
        window.show();
        let sheet = Sheet::new(ui, "Sheet", Label::new(ui, "Content"));
        assert!(sheet.show(&WindowRef::new(window.ns_window())));
        let sheet_probe = Weak::new(sheet.ns_window());
        drop(sheet);
        assert!(
            window.ns_window().sheets().is_empty(),
            "dropping a sheet must detach it"
        );
        let popover = Popover::new(ui, Label::new(ui, "Popover"));
        let popover_probe = Weak::new(popover.ns_popover());
        let controller_probe = Weak::new(popover.ns_view_controller());
        popover.show(&anchor);
        let native = popover.ns_popover().retain();
        drop(popover);
        assert!(!native.isShown());
        assert!(
            native.contentViewController().is_none(),
            "popover drop must release its controller even with a native owner"
        );
        (
            Weak::new(window.ns_window()),
            sheet_probe,
            popover_probe,
            controller_probe,
        )
    });
    settle();
    assert!(window_probe.load().is_none());
    assert!(sheet_probe.load().is_none());
    assert!(popover_probe.load().is_none());
    assert!(controller_probe.load().is_none());
}

fn graphics_resources_follow_their_native_views(ui: &Ui) {
    let probes = autoreleasepool(|_| {
        let mut probes = Vec::new();
        let layer = objc2_quartz_core::CALayer::layer();
        probe(&mut probes, "hosted layer", &layer);
        let host = LayerHost::new(ui, &layer);
        probe(&mut probes, "layer host", host.ns_view());
        let canvas = Canvas::new(ui, |_, _| {});
        probe(&mut probes, "canvas", canvas.ns_view());
        let preview = LayoutPreview::new(ui, CGSize::new(240.0, 120.0));
        preview.set_panes(&[CGRect::new(CGPoint::ZERO, CGSize::new(120.0, 120.0))]);
        probe(&mut probes, "preview", preview.ns_view());
        let effect = VisualEffect::sidebar(ui, Label::new(ui, "Sidebar"));
        probe(&mut probes, "visual effect", effect.ns_view());
        let window = Window::new(ui).content(
            VStack::new(ui)
                .push(host)
                .push(canvas)
                .push(preview)
                .push(effect),
        );
        window.show();
        window
            .ns_window()
            .contentView()
            .unwrap()
            .layoutSubtreeIfNeeded();
        probes
    });
    settle();
    for (name, weak) in probes {
        assert!(weak.load().is_none(), "{name} survived destruction");
    }
}

fn settings_windows_release_native_owners(ui: &Ui) {
    for cycle in 0..20 {
        let probes = settings_lifecycle(ui, cycle % 2 == 0, || {});
        settle();
        for (name, weak) in probes {
            assert!(
                weak.load().is_none(),
                "cycle {cycle}: {name} survived destruction"
            );
        }
    }
}

fn memory_profile(ui: &Ui) {
    let directory = std::path::PathBuf::from(
        std::env::var("CGS_MEMORY_REPORT").expect("set CGS_MEMORY_REPORT"),
    );
    std::fs::create_dir_all(&directory).unwrap();
    let sample = |stage: &str| {
        for (tool, args) in [
            (
                "footprint",
                vec!["-p".to_string(), std::process::id().to_string()],
            ),
            (
                "vmmap",
                vec!["-summary".to_string(), std::process::id().to_string()],
            ),
        ] {
            let output = std::process::Command::new(tool)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{tool} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let mut bytes = output.stdout;
            bytes.extend(output.stderr);
            std::fs::write(directory.join(format!("{stage}-{tool}.txt")), bytes).unwrap();
        }
    };
    settle();
    sample("cold");
    for cycle in 0..20 {
        let start = std::time::Instant::now();
        let sampling = Cell::new(std::time::Duration::ZERO);
        let probes = settings_lifecycle(ui, cycle % 2 == 0, || {
            let start = std::time::Instant::now();
            settle();
            if cycle == 0 || cycle == 19 {
                sample(&format!("open-{cycle}"));
            }
            sampling.set(start.elapsed());
        });
        let navigation = start.elapsed() - sampling.get();
        settle();
        let survivors = probes
            .iter()
            .filter(|(_, weak)| weak.load().is_some())
            .count();
        println!(
            "cycle={cycle} lifecycle_ms={} survivors={survivors}",
            navigation.as_millis()
        );
        if cycle == 0 || cycle == 19 {
            sample(&format!("settled-{cycle}"));
        }
    }
    for stage in ["idle-start", "idle-end"] {
        let output = std::process::Command::new("ps")
            .args(["-p", &std::process::id().to_string(), "-o", "time=,%cpu="])
            .output()
            .unwrap();
        std::fs::write(directory.join(format!("{stage}.txt")), output.stdout).unwrap();
        if stage == "idle-start" {
            autoreleasepool(|_| {
                objc2_foundation::NSRunLoop::currentRunLoop()
                    .runUntilDate(&objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(2.0))
            });
        }
    }
}

fn main() {
    // A single libtest-compatible case lets nextest list this harness without running AppKit.
    if std::env::args().any(|arg| arg == "--list") {
        if !std::env::args().any(|arg| arg == "--ignored") {
            println!("native: test");
        }
        return;
    }
    let ui =
        Ui::new(MainThreadMarker::new().expect("native tests must run on the macOS main thread"));
    let app = Application::shared(&ui);
    app.ns_application()
        .setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
    if std::env::args().any(|arg| arg == "--memory-profile") {
        memory_profile(&ui);
        return;
    }
    if std::env::args().any(|arg| arg == "--regressions") {
        let mut failures = 0;
        for (name, test) in [
            (
                "host",
                released_components_detach_borrowed_native_content as fn(&Ui),
            ),
            ("footer", replaced_footer_releases_native_content),
            ("status", replaced_status_releases_native_content),
            (
                "toolbar",
                toolbar_releases_disabled_actions_and_preserves_unchanged_controls,
            ),
            (
                "window",
                window_close_is_idempotent_and_drop_does_not_dispatch,
            ),
        ] {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| test(&ui)));
            println!(
                "{name}: {}",
                if result.is_ok() { "passed" } else { "failed" }
            );
            failures += usize::from(result.is_err());
        }
        assert_eq!(failures, 0);
        return;
    }
    released_components_detach_borrowed_native_content(&ui);
    replaced_footer_releases_native_content(&ui);
    replaced_status_releases_native_content(&ui);
    toolbar_releases_disabled_actions_and_preserves_unchanged_controls(&ui);
    window_close_is_idempotent_and_drop_does_not_dispatch(&ui);
    active_presentations_teardown_with_their_owners(&ui);
    graphics_resources_follow_their_native_views(&ui);
    settings_windows_release_native_owners(&ui);
    autoreleasepool(|_| {
        page_headings_preserve_window_identity(&ui);
        navigation_uses_native_toolbar_items_and_responder_chain(&ui);
        cached_pages_keep_their_mount_and_release_on_clear(&ui);
        callbacks_survive_composition_and_release_with_the_page(&ui);
        callbacks_can_remove_their_own_controls(&ui);
        settings_lists_do_not_materialize_offscreen_rows(&ui);
        reused_cells_clear_missing_images(&ui);
        unchanged_popup_items_preserve_selection_and_native_items(&ui);
        numeric_fields_reject_invalid_commits(&ui);
        delegates_and_selection_use_current_data(&ui);
        recording_is_scoped_and_cancellable(&ui);
        local_reordering_uses_final_indices_and_rejects_other_tables(&ui);
        pages_start_at_top_and_controllers_leave_the_host(&ui);
    });
    println!("native ownership, callbacks, delegates, selection and shortcut recording passed");
}
