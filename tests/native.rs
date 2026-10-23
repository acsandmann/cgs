use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cgs::*;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSApplicationActivationPolicy, NSControl, NSControlStateValueOn,
    NSControlTextEditingDelegate, NSDragOperation, NSDraggingInfo, NSEvent, NSEventType,
    NSPasteboard, NSPasteboardWriting, NSTableViewDataSource, NSTableViewDropOperation,
    NSTextField,
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
        assert!(native.subviews().is_empty(), "host drop must unmount all pages");
        assert!(controller.childViewControllers().is_empty());
        assert!(child.load().is_none_or(|child| child.parentViewController().is_none()));
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
                StatusItem::new(ui).content(old).content(Label::new(ui, "New status")),
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
        objc2_foundation::NSRunLoop::currentRunLoop()
            .runUntilDate(&objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(0.15));
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
            window.ns_window().contentView().unwrap().layoutSubtreeIfNeeded();
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
        probe(&mut probes, "popover controller", popover.ns_view_controller());
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
        let window = Window::new(ui)
            .content(VStack::new(ui).push(host).push(canvas).push(preview).push(effect));
        window.show();
        window.ns_window().contentView().unwrap().layoutSubtreeIfNeeded();
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
            ("footprint", vec![
                "-p".to_string(),
                std::process::id().to_string(),
            ]),
            ("vmmap", vec![
                "-summary".to_string(),
                std::process::id().to_string(),
            ]),
        ] {
            let output = std::process::Command::new(tool).args(args).output().unwrap();
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
        let survivors = probes.iter().filter(|(_, weak)| weak.load().is_some()).count();
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

fn tab_bar_updates_and_releases_native_owners(ui: &Ui) {
    use objc2::ClassType;
    let actions = Rc::new(RefCell::new(Vec::new()));
    let received = actions.clone();
    let weak = autoreleasepool(|_| {
        let bar = TabBar::new(ui).on_action(move |a| received.borrow_mut().push(a));
        let panel = Weak::new(bar.ns_panel());
        let number = bar.ns_panel().windowNumber();
        assert!(!bar.ns_panel().canBecomeKeyWindow());
        assert!(!bar.ns_panel().canBecomeMainWindow());
        let icon = objc2_app_kit::NSImage::initWithSize(
            objc2_app_kit::NSImage::alloc(),
            CGSize::new(14.0, 14.0),
        );
        let icon_owner = Weak::new(&*icon);
        bar.set_tabs(
            vec![TabBarItem {
                title: "Icon".into(),
                icon: Some(icon),
            }],
            usize::MAX,
        );
        for title in ["First", "Updated 窗口 title"] {
            bar.set_tabs(
                vec![TabBarItem {
                    title: title.into(),
                    icon: None,
                }],
                0,
            );
            assert_eq!(bar.ns_panel().windowNumber(), number);
        }
        fn controls(view: &objc2_app_kit::NSView, out: &mut Vec<Retained<NSControl>>) {
            if let Some(control) = view.downcast_ref::<NSControl>() {
                out.push(control.retain());
            }
            for child in view.subviews() {
                controls(&child, out);
            }
        }
        let anchor = Window::new(ui).size(CGSize::new(250.0, 100.0));
        anchor.show();
        bar.show_above(anchor.ns_window().windowNumber());
        let app = objc2_app_kit::NSApplication::sharedApplication(ui.mtm());
        let key_before = app.keyWindow();
        let frontmost_before = objc2_app_kit::NSWorkspace::sharedWorkspace().frontmostApplication();
        bar.ns_panel().makeKeyWindow();
        assert!(
            !bar.ns_panel().isKeyWindow(),
            "appearance must never fake key status"
        );
        assert!(!bar.ns_panel().isMainWindow());
        assert_eq!(
            app.keyWindow(),
            key_before,
            "a nonactivating bar cannot take keyboard focus"
        );
        bar.ns_panel().contentView().unwrap().layoutSubtreeIfNeeded();
        let mut buttons = Vec::new();
        controls(&bar.ns_panel().contentView().unwrap(), &mut buttons);
        let select = buttons
            .iter()
            .find(|button| {
                button
                    .accessibilityLabel()
                    .is_some_and(|label| label.to_string() == "Updated 窗口 title")
            })
            .unwrap();
        let close = buttons
            .iter()
            .find(|button| {
                button
                    .accessibilityLabel()
                    .is_some_and(|label| label.to_string() == "Close Updated 窗口 title")
            })
            .unwrap();
        assert!(
            close.isHidden(),
            "selected tabs show their close control only on hover"
        );
        fn tab_group(view: &objc2_app_kit::NSView) -> Option<Retained<objc2_app_kit::NSView>> {
            if view
                .accessibilityLabel()
                .is_some_and(|label| label.to_string() == "Window tabs")
            {
                return Some(view.retain());
            }
            view.subviews().iter().find_map(|child| tab_group(&child))
        }
        let content = tab_group(&bar.ns_panel().contentView().unwrap()).unwrap();
        let location = content.convertPoint_toView(CGPoint::new(17.0, 14.0), None);
        let movement = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            NSEventType::MouseMoved, location, objc2_app_kit::NSEventModifierFlags::empty(), 0.0, number, None, 0, 0, 0.0,
        ).unwrap();
        content.mouseMoved(&movement);
        assert!(!close.isHidden(), "hover reveals the native close control");
        assert!(
            actions.borrow().is_empty(),
            "hover never selects or closes a tab"
        );
        let hit_point = content.convertPoint_toView(
            CGPoint::new(17.0, 14.0),
            unsafe { content.superview() }.as_deref(),
        );
        assert_eq!(
            content.hitTest(hit_point).as_deref(),
            Some(close.as_super()),
            "close gets its own hit target"
        );
        action(select);
        action(close);
        assert!(matches!(actions.borrow()[0], TabBarAction::Select(0)));
        assert!(matches!(actions.borrow()[1], TabBarAction::Close(0)));
        assert!(!bar.ns_panel().isKeyWindow());
        assert_eq!(app.keyWindow(), key_before);
        assert_eq!(
            objc2_app_kit::NSWorkspace::sharedWorkspace().frontmostApplication(),
            frontmost_before,
            "native tab actions must not activate the overlay application"
        );
        content.mouseExited(&movement);
        assert!(close.isHidden(), "leaving the strip hides close controls");
        bar.set_frame(CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(218.0, 28.0)));
        bar.set_tabs(
            vec![
                TabBarItem {
                    title: "First".into(),
                    icon: None,
                },
                TabBarItem {
                    title: "Second".into(),
                    icon: None,
                },
            ],
            1,
        );
        let mut overflow = Vec::new();
        controls(&bar.ns_panel().contentView().unwrap(), &mut overflow);
        let second = overflow
            .iter()
            .find(|control| {
                control.accessibilityLabel().is_some_and(|label| label.to_string() == "Second")
            })
            .unwrap()
            .frame();
        assert!(second.size.width >= 120.0);
        assert!(second.origin.x >= 0.0 && second.origin.x + second.size.width <= 218.0);
        // Native controls must follow their window when count changes/reordering
        // retarget the visuals. Hit testing uses the new logical frames immediately.
        let second_control = overflow
            .iter()
            .find(|control| {
                control.accessibilityLabel().is_some_and(|label| label.to_string() == "Second")
            })
            .unwrap()
            .retain();
        let items = |names: &[&str]| {
            names
                .iter()
                .map(|name| TabBarItem {
                    title: (*name).into(),
                    icon: None,
                })
                .collect()
        };
        let first_control = overflow
            .iter()
            .find(|control| {
                control.accessibilityLabel().is_some_and(|label| label.to_string() == "First")
            })
            .unwrap();
        for selected in [0, 1, usize::MAX, 1] {
            bar.set_selected_index(selected);
            assert_eq!(
                first_control.downcast_ref::<objc2_app_kit::NSButton>().unwrap().state()
                    == NSControlStateValueOn,
                selected == 0
            );
            assert_eq!(
                second_control.downcast_ref::<objc2_app_kit::NSButton>().unwrap().state()
                    == NSControlStateValueOn,
                selected == 1
            );
            assert_eq!(second_control.isAccessibilitySelected(), selected == 1);
            if selected == 1 {
                let frame = second_control.frame();
                assert!(frame.origin.x >= 0.0 && frame.origin.x + frame.size.width <= 218.0);
            }
        }
        bar.set_tabs(items(&["First", "Second", "Third"]), 2);
        bar.set_tabs(items(&["Second", "Third"]), 0);
        assert!(unsafe { second_control.superview() }.is_some());
        let frame = second_control.frame();
        let point = content.convertPoint_toView(
            CGPoint::new(frame.origin.x + frame.size.width / 2.0, 14.0),
            unsafe { content.superview() }.as_deref(),
        );
        assert_eq!(
            content.hitTest(point).as_deref(),
            Some(&*content),
            "tabs, not their transitioning visuals, take presses in the new logical region"
        );
        actions.borrow_mut().clear();
        action(&second_control);
        assert!(
            matches!(actions.borrow().as_slice(), [TabBarAction::Select(0)]),
            "reused controls must dispatch the current index after removal"
        );
        anchor.ns_window().orderWindow_relativeTo(
            objc2_app_kit::NSWindowOrderingMode::Above,
            bar.ns_panel().windowNumber(),
        );
        let order = || {
            objc2_app_kit::NSWindow::windowNumbersWithOptions(
                objc2_app_kit::NSWindowNumberListOptions::empty(),
                ui.mtm(),
            )
            .unwrap()
            .iter()
            .map(|number| number.integerValue())
            .collect::<Vec<_>>()
        };
        let windows = order();
        let anchor_number = anchor.ns_window().windowNumber();
        let panel_number = bar.ns_panel().windowNumber();
        assert!(
            windows.iter().position(|n| *n == anchor_number).unwrap()
                < windows.iter().position(|n| *n == panel_number).unwrap()
        );
        bar.show_above(anchor_number);
        let windows = order();
        assert!(
            windows.iter().position(|n| *n == panel_number).unwrap()
                < windows.iter().position(|n| *n == anchor_number).unwrap(),
            "reasserting the same anchor must repair native stacking after focus raises it"
        );
        bar.set_tabs(Vec::new(), usize::MAX);
        assert!(icon_owner.load().is_none(), "replacing tabs must release icons");
        for _ in 0..3 {
            bar.show_above(anchor.ns_window().windowNumber());
            assert!(bar.is_visible());
            bar.hide();
            bar.hide();
            assert!(!bar.is_visible());
        }
        anchor.close();
        panel
    });
    assert!(weak.load().is_none(), "tab panel must release on teardown");
    assert_eq!(Rc::strong_count(&actions), 1, "callback must release");
}

fn tab_bar_drag_reorders_tabs(ui: &Ui, edge: TabBarEdge) {
    let vertical = matches!(edge, TabBarEdge::Left | TabBarEdge::Right);
    let actions = Rc::new(RefCell::new(Vec::new()));
    let received = actions.clone();
    let bar = TabBar::new(ui).on_action(move |a| received.borrow_mut().push(a));
    let items = |names: &[&str]| {
        names
            .iter()
            .map(|name| TabBarItem {
                title: (*name).into(),
                icon: None,
            })
            .collect()
    };
    bar.set_edge(edge);
    let size = if vertical {
        CGSize::new(28.0, 460.0)
    } else {
        CGSize::new(460.0, 28.0)
    };
    bar.set_frame(CGRect::new(CGPoint::new(0.0, 0.0), size));
    bar.set_tabs(items(&["First", "Second", "Third"]), 0);
    let surface = bar.ns_panel().contentView().unwrap();
    surface.layoutSubtreeIfNeeded();
    fn find(view: &objc2_app_kit::NSView, label: &str) -> Option<Retained<objc2_app_kit::NSView>> {
        if view.accessibilityLabel().is_some_and(|l| l.to_string() == label) {
            return Some(view.retain());
        }
        view.subviews().iter().find_map(|child| find(&child, label))
    }
    let content = find(&surface, "Window tabs").unwrap();
    let number = bar.ns_panel().windowNumber();
    let mouse = |kind: NSEventType, x: f64| {
        let point = if vertical {
            CGPoint::new(14.0, x)
        } else {
            CGPoint::new(x, 14.0)
        };
        let location = content.convertPoint_toView(point, None);
        let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            kind, location, objc2_app_kit::NSEventModifierFlags::empty(), 0.0, number, None, 0, 1, 1.0,
        ).unwrap();
        match kind {
            NSEventType::LeftMouseDown => content.mouseDown(&event),
            NSEventType::LeftMouseDragged => content.mouseDragged(&event),
            _ => content.mouseUp(&event),
        }
    };
    let center = |label: &str| {
        let frame = find(&surface, label).unwrap().frame();
        if vertical {
            frame.origin.y + frame.size.height / 2.0
        } else {
            frame.origin.x + frame.size.width / 2.0
        }
    };
    // A press that wobbles less than the drag threshold is still a click.
    let first = center("First");
    mouse(NSEventType::LeftMouseDown, first);
    mouse(NSEventType::LeftMouseDragged, first + 2.0);
    mouse(NSEventType::LeftMouseUp, first + 2.0);
    assert!(matches!(actions.borrow().as_slice(), [TabBarAction::Select(0)]));
    actions.borrow_mut().clear();

    // Carry the first tab past the others, in steps as a real drag arrives.
    let third = center("Third");
    mouse(NSEventType::LeftMouseDown, first);
    for step in 1..=8 {
        mouse(
            NSEventType::LeftMouseDragged,
            first + (third - first) * step as f64 / 8.0,
        );
    }
    assert!(actions.borrow().is_empty(), "nothing is reported until the drop");
    mouse(NSEventType::LeftMouseUp, third);
    assert!(
        matches!(actions.borrow().as_slice(), [TabBarAction::Move {
            from: 0,
            to: 2
        }]),
        "{:?}",
        actions.borrow()
    );
    assert!(
        center("Second") < center("Third") && center("Third") < center("First"),
        "the strip shows the new order without waiting for its owner"
    );
    actions.borrow_mut().clear();
    let close = find(&surface, "Close First").unwrap().downcast::<NSControl>().unwrap();
    action(&close);
    assert!(
        matches!(actions.borrow().as_slice(), [TabBarAction::Close(2)]),
        "moved controls dispatch their new index"
    );
    actions.borrow_mut().clear();

    // A title animating mid-drag, like a terminal's spinner, must not drop the tab.
    let (second, first) = (center("Second"), center("First"));
    mouse(NSEventType::LeftMouseDown, second);
    mouse(NSEventType::LeftMouseDragged, second + (first - second) / 2.0);
    for spinner in ["⠋ Third", "⠙ Third"] {
        bar.set_tabs(items(&["Second", spinner, "First"]), 2);
        mouse(NSEventType::LeftMouseDragged, first);
    }
    mouse(NSEventType::LeftMouseUp, first);
    assert!(
        matches!(actions.borrow().as_slice(), [TabBarAction::Move {
            from: 0,
            to: 2
        }]),
        "{edge:?}: {:?}",
        actions.borrow()
    );
    assert!(center("⠙ Third") < center("First") && center("First") < center("Second"));
    actions.borrow_mut().clear();
    bar.set_tabs(items(&["Third", "First", "Second"]), 1);
    // Adding a tab mid-drag invalidates the slots, so the drag ends without a move.
    let third = center("Third");
    mouse(NSEventType::LeftMouseDown, third);
    mouse(NSEventType::LeftMouseDragged, third + 60.0);
    bar.set_tabs(items(&["Third", "First", "Second", "Fourth"]), 1);
    mouse(NSEventType::LeftMouseDragged, third + 200.0);
    mouse(NSEventType::LeftMouseUp, third + 200.0);
    assert!(actions.borrow().is_empty(), "{edge:?}: {:?}", actions.borrow());
    bar.set_tabs(items(&["Second", "Third", "First"]), 2);

    // Releasing in the starting slot reorders nothing.
    let second = center("Second");
    mouse(NSEventType::LeftMouseDown, second);
    mouse(NSEventType::LeftMouseDragged, second + 20.0);
    mouse(NSEventType::LeftMouseDragged, second);
    mouse(NSEventType::LeftMouseUp, second);
    assert!(actions.borrow().is_empty(), "{:?}", actions.borrow());

    // The owner confirming the order it was told about changes nothing.
    bar.set_tabs(items(&["Second", "Third", "First"]), 2);
    assert!(center("Second") < center("Third") && center("Third") < center("First"));
}

fn tab_bar_callback_can_destroy_its_owner(ui: &Ui) {
    let slot = Rc::new(RefCell::new(None::<TabBar>));
    let owner = Rc::downgrade(&slot);
    let calls = Rc::new(Cell::new(0));
    let received = calls.clone();
    let bar = TabBar::new(ui).on_action(move |_| {
        received.set(received.get() + 1);
        owner.upgrade().unwrap().borrow_mut().take();
    });
    bar.set_tabs(
        vec![TabBarItem {
            title: "Remove owner".into(),
            icon: None,
        }],
        0,
    );
    fn select_control(view: &objc2_app_kit::NSView) -> Option<Retained<NSControl>> {
        if let Some(control) = view.downcast_ref::<NSControl>() {
            if control
                .accessibilityLabel()
                .is_some_and(|label| label.to_string() == "Remove owner")
            {
                return Some(control.retain());
            }
        }
        view.subviews().iter().find_map(|v| select_control(&v))
    }
    let button = select_control(&bar.ns_panel().contentView().unwrap()).unwrap();
    *slot.borrow_mut() = Some(bar);
    action(&button);
    assert!(slot.borrow().is_none());
    assert_eq!(calls.get(), 1);
    action(&button);
    assert_eq!(
        calls.get(),
        1,
        "borrowed controls cannot dispatch after owner teardown"
    );
}

fn inline_hosts_size_only_the_visible_inspector(ui: &Ui) {
    let host = Rc::new(PageHost::content_sized(ui));
    let short = Rc::new(VStack::new(ui).push(Label::new(ui, "Short")));
    short.height(48.0);
    let long = Rc::new(VStack::new(ui).push(Label::new(ui, "Long")));
    long.height(140.0);
    let window = Window::new(ui).content(SettingsPage::new(ui, "").section(host.clone()));
    for (page, height) in [
        (short.clone(), 48.0),
        (long.clone(), 140.0),
        (short.clone(), 48.0),
    ] {
        host.set_cached_page(page);
        window.ns_window().contentView().unwrap().layoutSubtreeIfNeeded();
        assert_eq!(
            host.ns_view().frame().size.height,
            height,
            "parked inspectors do not constrain the inline host"
        );
    }
    host.clear();
    assert!(host.ns_view().subviews().is_empty());
}

fn find<T: objc2::DowncastTarget + 'static>(view: &objc2_app_kit::NSView) -> Option<Retained<T>> {
    if let Some(value) = view.downcast_ref::<T>() {
        return Some(value.retain());
    }
    view.subviews().iter().find_map(|child| find(&child))
}

fn sectioned_popups_report_choices_not_separators(ui: &Ui) {
    let chosen = Rc::new(Cell::new(None));
    let seen = chosen.clone();
    let popup = Popup::new(ui)
        .sections([vec!["Default (Next to Selection)"], vec![
            "Next to Selection",
            "End of Layout",
        ]])
        .on_change(move |index| seen.set(Some(index)));
    let native = popup.ns_popup_button();
    assert_eq!(native.numberOfItems(), 4);
    assert!(native.itemAtIndex(1).unwrap().isSeparatorItem());
    popup.set_selected(2);
    assert_eq!(
        native.titleOfSelectedItem().unwrap().to_string(),
        "End of Layout"
    );
    assert_eq!(popup.selected(), Some(2));
    native.selectItemAtIndex(2);
    action(native);
    assert_eq!(chosen.get(), Some(1), "the separator is not a choice");
    // Without separators, indices are menu indices again.
    popup.set_items(["One", "Two"]);
    popup.set_selected(1);
    assert_eq!(native.titleOfSelectedItem().unwrap().to_string(), "Two");
}

fn popup_callback_can_rebuild_sections(ui: &Ui) {
    let owner = Rc::new(std::cell::OnceCell::<Popup>::new());
    let weak = Rc::downgrade(&owner);
    let received = Rc::new(RefCell::new(Vec::new()));
    let seen = received.clone();
    let popup = Popup::new(ui)
        .sections([vec!["Default"], vec!["Same", "Same", "Old"]])
        .on_change(move |index| {
            seen.borrow_mut().push(index);
            let owner = weak.upgrade().unwrap();
            let popup = owner.get().unwrap();
            popup.set_sections([vec!["Default"], vec!["Same", "Same"]]);
            popup.set_selected(index.min(2));
        });
    assert!(owner.set(popup).is_ok());
    let popup = owner.get().unwrap();
    let native = popup.ns_popup_button();
    for (position, index) in [(3, 2), (2, 1), (3, 2)] {
        native.selectItemAtIndex(position);
        action(native);
        assert_eq!(received.borrow().last(), Some(&index));
        assert_eq!(popup.selected(), Some(index));
        assert_eq!(native.numberOfItems(), 4);
        assert!(native.itemAtIndex(1).unwrap().isSeparatorItem());
        assert_eq!(native.itemAtIndex(2).unwrap().title().to_string(), "Same");
        assert_eq!(native.itemAtIndex(3).unwrap().title().to_string(), "Same");
    }
    native.selectItemAtIndex(1);
    action(native);
    assert_eq!(
        *received.borrow(),
        [2, 1, 2],
        "separators do not dispatch choices"
    );
    popup.set_items(["Flat", "Other"]);
    native.selectItemAtIndex(1);
    action(native);
    assert_eq!(*received.borrow(), [2, 1, 2, 1], "rebuilt indices stay current");
}

fn row_visibility_hides_a_row_with_its_divider(ui: &Ui) {
    let hidden = SettingsRow::new(ui, "Custom ratio", TextField::new(ui));
    let visibility = hidden.visibility();
    let section = Section::new(ui, "Single Window")
        .row(SettingsRow::new(ui, "Shape", Popup::new(ui)))
        .row(hidden);
    let grid = find::<objc2_app_kit::NSGridView>(section.ns_view()).unwrap();
    assert_eq!(grid.numberOfRows(), 3, "two rows and their divider");
    visibility.set_hidden(true);
    assert!(grid.rowAtIndex(1).isHidden() && grid.rowAtIndex(2).isHidden());
    assert!(!grid.rowAtIndex(0).isHidden());
    visibility.set_hidden(false);
    assert!(!grid.rowAtIndex(1).isHidden() && !grid.rowAtIndex(2).isHidden());
}

fn symbol_segments_name_each_choice(ui: &Ui) {
    let control = SegmentedControl::symbols(ui, &[
        ("rectangle.lefthalf.inset.filled", "Left"),
        ("cgs.no-such-symbol", "Right"),
    ]);
    let native = control.ns_segmented_control();
    assert_eq!(native.segmentCount(), 2);
    let image = native.imageForSegment(0).expect("a symbol segment");
    assert_eq!(image.accessibilityDescription().unwrap().to_string(), "Left");
    assert_eq!(
        native.labelForSegment(1).unwrap().to_string(),
        "Right",
        "a missing symbol falls back to its label"
    );
    assert_eq!(
        Switch::new(ui).compact().ns_switch().controlSize(),
        objc2_app_kit::NSControlSize::Mini
    );
}

fn thumbnail_pickers_are_one_radio_group(ui: &Ui) {
    let selections = Rc::new(RefCell::new(Vec::new()));
    let activations = Rc::new(RefCell::new(Vec::new()));
    let (s, a) = (selections.clone(), activations.clone());
    let previews: Vec<_> = (0..3)
        .map(|_| Rc::new(LayoutPreview::new(ui, CGSize::new(64.0, 40.0))))
        .collect();
    let picker = ThumbnailPicker::new(
        ui,
        "Layouts",
        4.0,
        ["One", "Two", "Three"]
            .into_iter()
            .zip(&previews)
            .map(|(name, preview)| {
                (name.to_owned(), Box::new(preview.clone()) as Box<dyn NativeView>)
            })
            .collect(),
    )
    .on_select(move |index| s.borrow_mut().push(index))
    .on_double_click(move |index| a.borrow_mut().push(index))
    .menu(1, Menu::new(ui).item(MenuItem::new(ui, "Set as Default")));
    let group = picker.ns_view();
    assert!(group.acceptsFirstResponder());
    assert_eq!(
        group.accessibilityRole().unwrap().to_string(),
        unsafe { objc2_app_kit::NSAccessibilityRadioGroupRole }.to_string()
    );
    fn tiles(view: &objc2_app_kit::NSView, out: &mut Vec<Retained<objc2_app_kit::NSView>>) {
        if view.accessibilityRole().is_some_and(|role| {
            role.to_string() == unsafe { objc2_app_kit::NSAccessibilityRadioButtonRole }.to_string()
        }) {
            out.push(view.retain());
        }
        for child in view.subviews() {
            tiles(&child, out);
        }
    }
    let mut found = Vec::new();
    tiles(group, &mut found);
    assert_eq!(found.len(), 3, "each thumbnail is a radio button");
    let value = |tile: &objc2_app_kit::NSView| {
        tile.accessibilityValue()
            .and_then(|value| value.downcast::<objc2_foundation::NSNumber>().ok())
            .is_some_and(|value| value.boolValue())
    };
    assert!(value(&found[0]) && !value(&found[1]));
    // Arrow keys move the selection and report it; the ends don't wrap.
    group.keyDown(&event(124, "", Modifiers::empty()));
    group.keyDown(&event(124, "", Modifiers::empty()));
    group.keyDown(&event(124, "", Modifiers::empty()));
    assert_eq!(*selections.borrow(), [1, 2]);
    assert_eq!(picker.selected(), 2);
    assert!(value(&found[2]) && !value(&found[0]));
    // VoiceOver presses select too; programmatic selection doesn't report.
    let _: bool = unsafe { msg_send![&*found[0], accessibilityPerformPress] };
    assert_eq!(*selections.borrow(), [1, 2, 0]);
    picker.set_selected(1);
    assert_eq!(selections.borrow().len(), 3);
    picker.set_marker(Some(2), "Default", "Default layout");
    assert_eq!(
        found[2].accessibilityLabel().unwrap().to_string(),
        "Three, Default layout"
    );
    assert_eq!(found[1].accessibilityLabel().unwrap().to_string(), "Two");
    let menu: Option<Retained<objc2_app_kit::NSMenu>> =
        unsafe { msg_send![&*found[1], menuForEvent: &*event(0, "", Modifiers::empty())] };
    assert_eq!(
        menu.unwrap().numberOfItems(),
        1,
        "each thumbnail has its own menu"
    );
    let none: Option<Retained<objc2_app_kit::NSMenu>> =
        unsafe { msg_send![&*found[0], menuForEvent: &*event(0, "", Modifiers::empty())] };
    assert!(none.is_none());
    assert!(activations.borrow().is_empty());
}

// Opt-in because this posts real mouse events and opens a foreign helper window.
fn tab_bar_cross_process_input(ui: &Ui) {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    use objc2_core_foundation::{CFDictionary, CFNumber, CFString};
    use objc2_core_graphics::*;

    struct Anchor(std::process::Child);
    impl Drop for Anchor {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn order() -> Vec<u32> {
        let list = CGWindowListCopyWindowInfo(CGWindowListOption::OptionOnScreenOnly, 0).unwrap();
        let list = unsafe {
            objc2_core_foundation::CFRetained::cast_unchecked::<
                objc2_core_foundation::CFArray<objc2_core_foundation::CFType>,
            >(list)
        };
        list.iter()
            .filter_map(|value| {
                let dict = value.downcast::<CFDictionary>().ok()?;
                let dict = unsafe {
                    objc2_core_foundation::CFRetained::cast_unchecked::<
                        CFDictionary<CFString, objc2_core_foundation::CFType>,
                    >(dict)
                };
                dict.get(&CFString::from_static_str("kCGWindowNumber"))?
                    .downcast_ref::<CFNumber>()?
                    .as_i32()
                    .map(|id| id as u32)
            })
            .collect()
    }
    fn pump(ui: &Ui) {
        let app = objc2_app_kit::NSApplication::sharedApplication(ui.mtm());
        let until = objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(0.15);
        while let Some(event) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
            objc2_app_kit::NSEventMask::Any,
            Some(&until),
            unsafe { objc2_foundation::NSDefaultRunLoopMode },
            true,
        ) {
            app.sendEvent(&event);
        }
        app.updateWindows();
    }
    let mut child = Anchor(
        Command::new(std::env::current_exe().unwrap())
            .arg("--stack-anchor")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut line = String::new();
    BufReader::new(child.0.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let anchor: u32 = line.trim().parse().unwrap();
    assert!(
        CGPreflightPostEventAccess(),
        "real input test requires Accessibility/event-posting permission"
    );
    let actions = Rc::new(RefCell::new(Vec::new()));
    let received = actions.clone();
    let bar = TabBar::new(ui).on_action(move |a| received.borrow_mut().push(a));
    bar.set_frame(CGRect::new(CGPoint::new(120.0, 210.0), CGSize::new(460.0, 40.0)));
    bar.set_tabs(
        vec![TabBarItem {
            title: "Click me".into(),
            icon: None,
        }],
        0,
    );
    bar.show_above(anchor as isize);
    pump(ui);
    let number = bar.ns_panel().windowNumber() as u32;
    let ids = order();
    println!("foreign anchor {anchor}, AppKit/WindowServer panel {number}");
    assert!(
        ids.contains(&number),
        "AppKit number must identify the real WindowServer panel"
    );
    assert!(
        ids.iter().position(|&id| id == number).unwrap()
            < ids.iter().position(|&id| id == anchor).unwrap(),
        "bar must be above the foreign anchor: {ids:?}"
    );
    let front = objc2_app_kit::NSWorkspace::sharedWorkspace().frontmostApplication().unwrap();
    assert_ne!(
        front.processIdentifier(),
        std::process::id() as i32,
        "another application must remain active"
    );
    fn post(kind: CGEventType, cocoa: CGPoint) {
        let p = CGPoint::new(cocoa.x, CGDisplayBounds(CGMainDisplayID()).size.height - cocoa.y);
        let event = CGEvent::new_mouse_event(None, kind, p, CGMouseButton::Left).unwrap();
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
    }
    fn control(view: &objc2_app_kit::NSView, label: &str) -> Option<Retained<NSControl>> {
        if view.accessibilityLabel().is_some_and(|s| s.to_string() == label) {
            return view.downcast_ref::<NSControl>().map(|v| v.retain());
        }
        view.subviews().iter().find_map(|v| control(&v, label))
    }
    // Post through WindowServer, rather than sendAction or NSWindow.sendEvent.
    for edge in [
        TabBarEdge::Top,
        TabBarEdge::Bottom,
        TabBarEdge::Left,
        TabBarEdge::Right,
    ] {
        let size = if matches!(edge, TabBarEdge::Top | TabBarEdge::Bottom) {
            CGSize::new(460.0, 40.0)
        } else {
            CGSize::new(40.0, 460.0)
        };
        bar.set_edge(edge);
        bar.set_frame(CGRect::new(CGPoint::new(120.0, 210.0), size));
        pump(ui);
        let frame = bar.ns_panel().frame();
        let center = CGPoint::new(
            frame.origin.x + size.width / 2.0,
            frame.origin.y + size.height / 2.0,
        );
        post(CGEventType::LeftMouseDown, center);
        post(CGEventType::LeftMouseUp, center);
        pump(ui);
        assert!(
            matches!(actions.borrow().as_slice(), [TabBarAction::Select(0)]),
            "real {edge:?} click must reach the native button: {:?}",
            actions.borrow()
        );
        actions.borrow_mut().clear();
        post(CGEventType::MouseMoved, center);
        pump(ui);
        let close = control(&bar.ns_panel().contentView().unwrap(), "Close Click me").unwrap();
        assert!(!close.isHidden());
        let point = close.convertPoint_toView(
            CGPoint::new(close.bounds().size.width / 2.0, close.bounds().size.height / 2.0),
            None,
        );
        let point = CGPoint::new(frame.origin.x + point.x, frame.origin.y + point.y);
        post(CGEventType::LeftMouseDown, point);
        post(CGEventType::LeftMouseUp, point);
        pump(ui);
        assert!(
            matches!(actions.borrow().as_slice(), [TabBarAction::Close(0)]),
            "real {edge:?} close click must reach its own control: {:?}",
            actions.borrow()
        );
        actions.borrow_mut().clear();
        assert!(!bar.ns_panel().isKeyWindow());
        assert_eq!(
            objc2_app_kit::NSWorkspace::sharedWorkspace()
                .frontmostApplication()
                .unwrap()
                .processIdentifier(),
            front.processIdentifier()
        );
    }
    let unrelated = Window::new(ui).size(CGSize::new(400.0, 300.0));
    unrelated.ns_window().orderFrontRegardless();
    pump(ui);
    bar.show_above(anchor as isize);
    pump(ui);
    let ids = order();
    let position = |id| ids.iter().position(|&wid| wid == id).unwrap();
    assert!(position(unrelated.ns_window().windowNumber() as u32) < position(number));
    assert!(
        position(number) < position(anchor),
        "relative ordering must leave unrelated windows above the bar"
    );
    bar.hide();
    pump(ui);
    assert!(!order().contains(&number));
    bar.show_above(anchor as isize);
    pump(ui);
    assert!(bar.is_visible());
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
    if std::env::args().any(|arg| arg == "--stack-anchor") {
        use std::io::Write;
        let app = objc2_app_kit::NSApplication::sharedApplication(ui.mtm());
        app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
        app.finishLaunching();
        let window = Window::new(&ui).size(CGSize::new(500.0, 200.0));
        window.ns_window().setFrame_display(
            CGRect::new(CGPoint::new(100.0, 200.0), CGSize::new(500.0, 200.0)),
            true,
        );
        // A real foreign WindowServer anchor above existing windows, while the
        // user's application stays active. No helper activation is required.
        window.ns_window().orderFrontRegardless();
        app.updateWindows();
        objc2_foundation::NSRunLoop::mainRunLoop()
            .runUntilDate(&objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(0.15));
        println!("{}", window.ns_window().windowNumber());
        std::io::stdout().flush().unwrap();
        app.run();
        return;
    }
    if std::env::args().any(|arg| arg == "--stack-line-input") {
        let app = objc2_app_kit::NSApplication::sharedApplication(ui.mtm());
        app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
        app.finishLaunching();
        tab_bar_cross_process_input(&ui);
        return;
    }
    if std::env::args().any(|arg| arg == "--layouts") {
        autoreleasepool(|_| {
            inline_hosts_size_only_the_visible_inspector(&ui);
            cached_pages_keep_their_mount_and_release_on_clear(&ui);
        });
        println!("native inline host, search focus and teardown checks passed");
        return;
    }
    tab_bar_updates_and_releases_native_owners(&ui);
    for edge in [TabBarEdge::Top, TabBarEdge::Left, TabBarEdge::Right] {
        tab_bar_drag_reorders_tabs(&ui, edge);
    }
    tab_bar_callback_can_destroy_its_owner(&ui);
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
            ("window", window_close_is_idempotent_and_drop_does_not_dispatch),
        ] {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| test(&ui)));
            println!("{name}: {}", if result.is_ok() { "passed" } else { "failed" });
            failures += usize::from(result.is_err());
        }
        assert_eq!(failures, 0);
        return;
    }
    if std::env::args().any(|arg| arg == "--lifecycle") {
        window_close_is_idempotent_and_drop_does_not_dispatch(&ui);
        active_presentations_teardown_with_their_owners(&ui);
        graphics_resources_follow_their_native_views(&ui);
        settings_windows_release_native_owners(&ui);
        println!("native window lifecycle checks passed");
        return;
    }
    released_components_detach_borrowed_native_content(&ui);
    replaced_footer_releases_native_content(&ui);
    replaced_status_releases_native_content(&ui);
    toolbar_releases_disabled_actions_and_preserves_unchanged_controls(&ui);
    autoreleasepool(|_| {
        page_headings_preserve_window_identity(&ui);
        navigation_uses_native_toolbar_items_and_responder_chain(&ui);
        cached_pages_keep_their_mount_and_release_on_clear(&ui);
        callbacks_survive_composition_and_release_with_the_page(&ui);
        callbacks_can_remove_their_own_controls(&ui);
        settings_lists_do_not_materialize_offscreen_rows(&ui);
        reused_cells_clear_missing_images(&ui);
        unchanged_popup_items_preserve_selection_and_native_items(&ui);
        sectioned_popups_report_choices_not_separators(&ui);
        popup_callback_can_rebuild_sections(&ui);
        row_visibility_hides_a_row_with_its_divider(&ui);
        symbol_segments_name_each_choice(&ui);
        thumbnail_pickers_are_one_radio_group(&ui);
        numeric_fields_reject_invalid_commits(&ui);
        delegates_and_selection_use_current_data(&ui);
        recording_is_scoped_and_cancellable(&ui);
        local_reordering_uses_final_indices_and_rejects_other_tables(&ui);
        pages_start_at_top_and_controllers_leave_the_host(&ui);
    });
    println!("native ownership, callbacks, delegates, selection and shortcut recording passed");
}
