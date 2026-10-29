use std::cell::RefCell;
use std::rc::Rc;

use objc2::rc::{Retained, Weak};
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadOnly};
use objc2_app_kit::*;
use objc2_foundation::{NSArray, NSIndexSet, NSString};

use super::bridge::{CollectionBridge, Selection, drag_type};
use crate::Ui;
use crate::bridge::ActionTarget;
use crate::collection::SettingsList;
use crate::controls::AddRemoveControl;
use crate::layout::{ScrollView, VStack};
use crate::text::Label;
use crate::view::NativeView;

pub struct Table<T: 'static> {
    pub(super) native: Retained<NSTableView>,
    pub(super) scroll: ScrollView,
    pub(super) bridge: Retained<CollectionBridge>,
    pub(super) rows: Rc<RefCell<Vec<T>>>,
    pub(super) double: Retained<ActionTarget>,
    pub(super) menu: RefCell<Option<crate::menu::Menu>>,
}
impl<T: 'static> Table<T> {
    pub fn new(ui: &Ui) -> Self {
        let native = NSTableView::new(ui.mtm());
        native.setAllowsEmptySelection(true);
        native.setAllowsMultipleSelection(false);
        native.setUsesAutomaticRowHeights(false);
        native.setStyle(NSTableViewStyle::Inset);
        native.setRowSizeStyle(NSTableViewRowSizeStyle::Custom);
        native.setColumnAutoresizingStyle(
            NSTableViewColumnAutoresizingStyle::LastColumnOnlyAutoresizingStyle,
        );
        let rows = Rc::new(RefCell::new(Vec::new()));
        let r = rows.clone();
        let ui_copy = *ui;
        let bridge = CollectionBridge::new(
            ui,
            move || r.borrow().len(),
            move |_, _| Box::new(Label::new(&ui_copy, "")),
        );
        unsafe {
            native.setDataSource(Some(ProtocolObject::from_ref(&*bridge)));
            native.setDelegate(Some(ProtocolObject::from_ref(&*bridge)));
        }
        let double = ActionTarget::new(ui);
        unsafe {
            native.setTarget(Some(&double));
            native.setDoubleAction(Some(objc2::sel!(invoke:)));
        }
        let view: Retained<NSView> = native.clone().into_super().into_super();
        let scroll = ScrollView::new(ui, view);
        scroll.ns_scroll_view().setDrawsBackground(true);
        scroll.ns_scroll_view().setBorderType(NSBorderType::NoBorder);
        {
            let r = rows.clone();
            *bridge.ivars().reorder.borrow_mut() = Some(Box::new(move |from, to| {
                let value = r.borrow_mut().remove(from);
                r.borrow_mut().insert(to, value);
            }));
            Self {
                native,
                scroll,
                bridge,
                rows,
                double,
                menu: RefCell::new(None),
            }
        }
    }

    pub fn column(self, id: &str, title: &str, width: f64) -> Self {
        let column = NSTableColumn::initWithIdentifier(
            NSTableColumn::alloc(self.native.mtm()),
            &NSString::from_str(id),
        );
        column.setTitle(&NSString::from_str(title));
        if width > 0.0 {
            column.setWidth(width);
        }
        self.native.addTableColumn(&column);
        self
    }

    pub fn cells(self, mut f: impl FnMut(&T, usize) -> Box<dyn NativeView> + 'static) -> Self {
        self.cells_with_index(move |item, column, _| f(item, column))
    }

    pub fn cells_with_index(
        self,
        mut f: impl FnMut(&T, usize, usize) -> Box<dyn NativeView> + 'static,
    ) -> Self {
        let rows = self.rows.clone();
        *self.bridge.ivars().native_cell.borrow_mut() = None;
        *self.bridge.ivars().cell.borrow_mut() =
            Box::new(move |row, column| f(&rows.borrow()[row], column, row));
        self.bridge.reload(&self.native);
        self
    }

    pub fn set_rows_if_changed(&self, rows: Vec<T>)
    where T: PartialEq {
        if *self.rows.borrow() != rows {
            self.set_rows(rows);
        }
    }

    pub fn rows(self, rows: Vec<T>) -> Self {
        self.set_rows(rows);
        self
    }

    pub fn set_rows(&self, rows: Vec<T>) {
        *self.rows.borrow_mut() = rows;
        self.bridge.reload(&self.native);
    }

    pub fn selection(&self) -> Option<usize> { usize::try_from(self.native.selectedRow()).ok() }

    /// A copy of the row at `index`, as last set.
    pub fn row(&self, index: usize) -> Option<T>
    where T: Clone {
        self.rows.borrow().get(index).cloned()
    }

    /// A copy of the selected row.
    pub fn selected_row(&self) -> Option<T>
    where T: Clone {
        self.row(self.selection()?)
    }

    /// Headerless source-list styling for navigation and search results.
    pub fn source_list(self) -> Self {
        self.native.setHeaderView(None);
        self.native.setFloatsGroupRows(false);
        self.native.setStyle(NSTableViewStyle::SourceList);
        self.ns_scroll_view().setDrawsBackground(false);
        self
    }

    pub fn scroll_to(&self, row: usize) { self.native.scrollRowToVisible(row as isize); }

    /// Redraw rows after external data their cells read has changed.
    pub fn reload(&self) { self.native.reloadData(); }

    pub fn set_selected(&self, index: Option<usize>) {
        if self.selection() == index {
            return;
        }
        if let Some(index) = index.filter(|i| *i < self.rows.borrow().len()) {
            self.native.selectRowIndexes_byExtendingSelection(
                &NSIndexSet::indexSetWithIndex(index),
                false,
            );
        } else {
            unsafe {
                self.native.deselectAll(None);
            }
        }
    }

    pub fn on_select_item(self, mut f: impl FnMut(Option<T>) + 'static) -> Self
    where T: Clone {
        let rows = self.rows.clone();
        *self.bridge.ivars().selection.borrow_mut() = Some(Box::new(move |index| {
            let item = index.and_then(|i| rows.borrow().get(i).cloned());
            f(item);
        }));
        self
    }

    pub fn on_select(self, f: impl FnMut(Option<usize>) + 'static) -> Self {
        self.set_on_select(f);
        self
    }

    pub fn set_on_select(&self, f: impl FnMut(Option<usize>) + 'static) {
        *self.bridge.ivars().selection.borrow_mut() = Some(Box::new(f));
    }

    /// Use AppKit source-list section rows and intrinsic per-item heights.
    pub fn group_rows(self, group: impl Fn(&T) -> bool + 'static) -> Self {
        let rows = self.rows.clone();
        *self.bridge.ivars().table_groups.borrow_mut() =
            Box::new(move |row| group(&rows.borrow()[row]));
        self
    }

    pub fn row_heights(self, height: impl Fn(&T) -> f64 + 'static) -> Self {
        let rows = self.rows.clone();
        *self.bridge.ivars().row_height.borrow_mut() =
            Some(Box::new(move |row| height(&rows.borrow()[row])));
        self
    }

    pub fn selectable(self, f: impl Fn(&T) -> bool + 'static) -> Self {
        let rows = self.rows.clone();
        *self.bridge.ivars().selectable.borrow_mut() =
            Box::new(move |row| rows.borrow().get(row).is_some_and(&f));
        self
    }

    pub fn on_double_click(self, mut f: impl FnMut(usize) + 'static) -> Self {
        self.double.set(move |sender| {
            if let Some(table) = sender.downcast_ref::<NSTableView>() {
                if let Ok(row) = usize::try_from(if table.clickedRow() >= 0 {
                    table.clickedRow()
                } else {
                    table.selectedRow()
                }) {
                    f(row);
                }
            }
        });
        self
    }

    pub fn context_menu(self, menu: crate::menu::Menu) -> Self {
        self.set_context_menu(menu);
        self
    }

    pub fn set_context_menu(&self, menu: crate::menu::Menu) {
        let table = objc2::rc::Weak::new(&*self.native);
        let native_menu = objc2::rc::Weak::new(menu.ns_menu());
        let menu = menu.on_tracking(move |opening| {
            if !opening {
                return;
            }
            if let (Some(table), Some(menu)) = (table.load(), native_menu.load()) {
                let row = table.clickedRow();
                if row >= 0 {
                    table.selectRowIndexes_byExtendingSelection(
                        &NSIndexSet::indexSetWithIndex(row as usize),
                        false,
                    );
                }
                for item in menu.itemArray() {
                    item.setEnabled(row >= 0);
                }
            }
        });
        unsafe {
            self.native.setMenu(Some(menu.ns_menu()));
        }
        *self.menu.borrow_mut() = Some(menu);
    }

    pub fn reorderable(self, value: bool) -> Self {
        self.bridge.ivars().reorderable.set(value);
        self.native.registerForDraggedTypes(&NSArray::from_slice(&[&*drag_type()]));
        self.native.setDraggingSourceOperationMask_forLocal(NSDragOperation::Move, true);
        self
    }

    pub fn on_reorder(self, mut f: impl FnMut(usize, usize) + 'static) -> Self {
        let rows = self.rows.clone();
        *self.bridge.ivars().reorder.borrow_mut() = Some(Box::new(move |from, to| {
            let value = rows.borrow_mut().remove(from);
            rows.borrow_mut().insert(to, value);
            f(from, to);
        }));
        self
    }

    pub fn ns_table_view(&self) -> &NSTableView { &self.native }

    pub fn ns_scroll_view(&self) -> &NSScrollView { self.scroll.ns_scroll_view() }
}
impl<T: 'static> NativeView for Table<T> {
    fn ns_view(&self) -> &NSView { self.scroll.ns_view() }
}
impl<T: 'static> Drop for Table<T> {
    fn drop(&mut self) {
        unsafe {
            self.native.setDataSource(None);
            self.native.setDelegate(None);
            self.native.setTarget(None);
            self.native.setDoubleAction(None);
        }
    }
}

pub struct List<T: 'static>(Table<T>);
impl<T: 'static> List<T> {
    pub fn new(ui: &Ui, label: impl Fn(&T) -> String + 'static) -> Self {
        let ui_copy = *ui;
        let table = Table::new(ui)
            .column("item", "", 240.0)
            .cells(move |item, _| Box::new(Label::new(&ui_copy, &label(item))));
        table.native.setHeaderView(None);
        Self(table)
    }

    pub fn items(self, items: Vec<T>) -> Self { Self(self.0.rows(items)) }

    pub fn set_items(&self, items: Vec<T>) { self.0.set_rows(items); }

    pub fn on_select(self, f: impl FnMut(Option<usize>) + 'static) -> Self {
        Self(self.0.on_select(f))
    }

    pub fn selection(&self) -> Option<usize> { self.0.selection() }

    pub fn set_selected(&self, index: Option<usize>) { self.0.set_selected(index); }

    pub fn ns_table_view(&self) -> &NSTableView { self.0.ns_table_view() }

    pub fn defer_scrolling(&self) { self.0.scroll.defer_scrolling(); }
}
impl<T: 'static> NativeView for List<T> {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }
}

pub struct EditableList<T: 'static> {
    stack: VStack,
    list: SettingsList<T>,
    controls: AddRemoveControl,
    selection: Rc<RefCell<Option<Selection>>>,
}
impl<T: 'static> EditableList<T> {
    pub fn new(ui: &Ui, label: impl Fn(&T) -> String + 'static) -> Self {
        let controls = AddRemoveControl::new(ui);
        controls.show_add(false);
        controls.set_remove_enabled(false);
        let selection: Rc<RefCell<Option<Selection>>> = Rc::new(RefCell::new(None));
        let cb = selection.clone();
        let remove = Weak::new(controls.remove_button());
        let list = SettingsList::new(ui, label, |_| String::new()).fit_content(220.0).on_select(
            move |index| {
                if let Some(remove) = remove.load() {
                    remove.setEnabled(index.is_some());
                }
                if let Ok(mut f) = cb.try_borrow_mut() {
                    if let Some(f) = f.as_mut() {
                        f(index);
                    }
                }
            },
        );
        let stack = VStack::new(ui);
        stack.ns_stack_view().addArrangedSubview(list.ns_view());
        crate::view::prepare(list.ns_view());
        list.ns_view()
            .widthAnchor()
            .constraintEqualToAnchor(&stack.ns_view().widthAnchor())
            .setActive(true);

        stack.ns_stack_view().addArrangedSubview(controls.ns_view());
        Self {
            stack,
            list,
            controls,
            selection,
        }
    }

    /// Let the enclosing settings page scroll the entire collection.
    pub fn full_length(mut self) -> Self {
        self.list = self.list.full_length();
        self
    }

    pub fn items(self, items: Vec<T>) -> Self {
        self.list.set_rows(items);
        self
    }

    pub fn set_items(&self, items: Vec<T>) {
        self.list.set_rows(items);
        self.controls.set_remove_enabled(self.list.selection().is_some());
    }

    pub fn reorderable(mut self, value: bool) -> Self {
        self.list = self.list.reorderable(value);
        self
    }

    pub fn on_reorder(mut self, f: impl FnMut(usize, usize) + 'static) -> Self {
        self.list = self.list.on_reorder(f);
        self
    }

    pub fn on_add(mut self, f: impl FnMut() + 'static) -> Self {
        self.controls.show_add(true);
        self.controls = self.controls.on_add(f);
        self
    }

    pub fn on_remove(mut self, mut f: impl FnMut(usize) + 'static) -> Self {
        let table = Weak::new(self.list.ns_table_view());
        self.controls = self.controls.on_remove(move || {
            if let Some(t) = table.load() {
                if let Ok(i) = usize::try_from(t.selectedRow()) {
                    f(i);
                }
            }
        });
        self
    }

    pub fn on_select(self, f: impl FnMut(Option<usize>) + 'static) -> Self {
        *self.selection.borrow_mut() = Some(Box::new(f));
        self
    }

    pub fn set_selected(&self, index: Option<usize>) {
        self.list.set_selected(index);
        self.controls.set_remove_enabled(self.list.selection().is_some());
    }

    pub fn selection(&self) -> Option<usize> { self.list.selection() }

    pub fn ns_table_view(&self) -> &NSTableView { self.list.ns_table_view() }
}
impl<T: 'static> NativeView for EditableList<T> {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}
