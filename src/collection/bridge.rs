use std::cell::{Cell, Ref, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::*;
use objc2_foundation::{
    NSIndexSet, NSNotification, NSNumber, NSObject, NSObjectProtocol, NSString,
};

use crate::Ui;
use crate::bridge::callback;
use crate::view::NativeView;

// The native reuse pool owns the Rust content and its action targets, not the datasource.
// A legacy arbitrary cell factory can replace content; reusable factories configure it in place.
define_class!(
    #[unsafe(super(NSTableCellView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgUiCollectionCell"]
    #[ivars = RefCell<Option<Box<dyn NativeView>>>]
    struct CollectionCell;
);
impl CollectionCell {
    fn new(ui: &Ui) -> Retained<Self> {
        unsafe {
            msg_send![
                super(Self::alloc(ui.mtm()).set_ivars(RefCell::new(None))),
                init
            ]
        }
    }

    fn set_content(&self, content: Box<dyn NativeView>) {
        unsafe {
            self.setTextField(None);
            self.setImageView(None);
        }
        if let Some(old) = self.ivars().borrow_mut().take() {
            if unsafe { old.ns_view().superview() }
                .is_some_and(|parent| std::ptr::eq(&*parent, &***self))
            {
                old.ns_view().removeFromSuperview();
            }
        }
        self.addSubview(content.ns_view());
        crate::view::pin(self, content.ns_view(), crate::Insets {
            top: 0.0,
            left: 0.0,
            bottom: 0.0,
            right: 0.0,
        });
        // Expose outlets to AppKit's native selected-cell appearance.
        let cell = content.ns_view().downcast_ref::<NSTableCellView>();
        unsafe {
            self.setTextField(cell.and_then(|cell| cell.textField()).as_deref());
            self.setImageView(cell.and_then(|cell| cell.imageView()).as_deref());
        }
        *self.ivars().borrow_mut() = Some(content);
    }
}

pub(super) fn drag_type() -> Retained<NSString> { NSString::from_str("org.cgs.local-row") }
type CellFactory = Box<dyn FnMut(usize, usize) -> Box<dyn NativeView>>;
type NativeCellFactory = Box<dyn FnMut(&NSTableView, usize, usize) -> Retained<NSView>>;
pub(super) type Selection = Box<dyn FnMut(Option<usize>)>;
type Reorder = Box<dyn FnMut(usize, usize)>;
pub(super) struct Node {
    pub(super) object: Retained<NSNumber>,
    pub(super) children: Vec<usize>,
}
pub(super) struct CollectionState {
    pub(super) count: Box<dyn Fn() -> usize>,
    pub(super) cell: RefCell<CellFactory>,
    pub(super) native_cell: RefCell<Option<NativeCellFactory>>,
    pub(super) selection: RefCell<Option<Selection>>,
    pub(super) selectable: RefCell<Box<dyn Fn(usize) -> bool>>,
    pub(super) reorder: RefCell<Option<Reorder>>,
    pub(super) reorderable: Cell<bool>,
    pub(super) group_parents: Cell<bool>,
    pub(super) table_groups: RefCell<Box<dyn Fn(usize) -> bool>>,
    pub(super) row_height: RefCell<Option<Box<dyn Fn(usize) -> f64>>>,
    pub(super) roots: RefCell<Vec<usize>>,
    pub(super) nodes: RefCell<Vec<Node>>,
}
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgUiCollectionBridge"]
    #[ivars = CollectionState]
    pub(super) struct CollectionBridge;
    unsafe impl NSObjectProtocol for CollectionBridge {}
    unsafe impl NSControlTextEditingDelegate for CollectionBridge {}
    unsafe impl NSTableViewDataSource for CollectionBridge {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn count(&self, _table: &NSTableView) -> isize { (self.ivars().count)() as isize }

        #[unsafe(method_id(tableView:pasteboardWriterForRow:))]
        fn writer(
            &self,
            _table: &NSTableView,
            row: isize,
        ) -> Option<Retained<ProtocolObject<dyn NSPasteboardWriting>>> {
            if !self.ivars().reorderable.get() {
                None
            } else {
                let item = NSPasteboardItem::new();
                item.setString_forType(&NSString::from_str(&row.to_string()), &drag_type());
                Some(ProtocolObject::from_retained(item))
            }
        }

        #[unsafe(method(tableView:validateDrop:proposedRow:proposedDropOperation:))]
        fn validate_drop(
            &self,
            table: &NSTableView,
            info: &ProtocolObject<dyn NSDraggingInfo>,
            row: isize,
            operation: NSTableViewDropOperation,
        ) -> NSDragOperation {
            if operation == NSTableViewDropOperation::Above
                && row >= 0
                && row as usize <= (self.ivars().count)()
                && self.local_drag(table, info)
            {
                NSDragOperation::Move
            } else {
                NSDragOperation::None
            }
        }

        #[unsafe(method(tableView:acceptDrop:row:dropOperation:))]
        fn accept_drop(
            &self,
            table: &NSTableView,
            info: &ProtocolObject<dyn NSDraggingInfo>,
            row: isize,
            operation: NSTableViewDropOperation,
        ) -> bool {
            self.do_accept_drop(table, info, row, operation)
        }
    }
    unsafe impl NSTableViewDelegate for CollectionBridge {
        #[unsafe(method(tableView:isGroupRow:))]
        fn table_group(&self, _table: &NSTableView, row: isize) -> bool {
            (self.ivars().table_groups.borrow())(row as usize)
        }

        #[unsafe(method(tableView:heightOfRow:))]
        fn table_height(&self, table: &NSTableView, row: isize) -> f64 {
            self.ivars()
                .row_height
                .borrow()
                .as_ref()
                .map_or(table.rowHeight(), |height| height(row as usize))
        }

        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn table_cell(
            &self,
            table: &NSTableView,
            column: Option<&NSTableColumn>,
            row: isize,
        ) -> Option<Retained<NSView>> {
            usize::try_from(row).ok().and_then(|row| {
                self.make_cell(
                    table,
                    row,
                    column
                        .map(|c| {
                            table
                                .tableColumns()
                                .iter()
                                .position(|v| std::ptr::eq(&*v, c))
                                .unwrap_or(0)
                        })
                        .unwrap_or(0),
                )
            })
        }

        #[unsafe(method(tableViewSelectionDidChange:))]
        fn selection_changed(&self, note: &NSNotification) {
            if let Some(table) = note.object().and_then(|o| o.downcast::<NSTableView>().ok()) {
                self.select(usize::try_from(table.selectedRow()).ok());
            }
        }

        #[unsafe(method(tableView:shouldSelectRow:))]
        fn should_select_row(&self, _table: &NSTableView, row: isize) -> bool {
            let _keep_alive = self.retain();
            let mut result = false;
            callback(|| {
                result = usize::try_from(row)
                    .ok()
                    .is_some_and(|row| (self.ivars().selectable.borrow())(row))
            });
            result
        }
    }
    unsafe impl NSOutlineViewDataSource for CollectionBridge {
        #[unsafe(method(outlineView:numberOfChildrenOfItem:))]
        unsafe fn children_count(&self, _view: &NSOutlineView, item: Option<&AnyObject>) -> isize {
            self.children(item).len() as isize
        }

        #[unsafe(method_id(outlineView:child:ofItem:))]
        unsafe fn child(
            &self,
            _view: &NSOutlineView,
            index: isize,
            item: Option<&AnyObject>,
        ) -> Retained<AnyObject> {
            let child = self.children(item)[index as usize];
            self.ivars().nodes.borrow()[child].object.clone().into()
        }

        #[unsafe(method(outlineView:isItemExpandable:))]
        unsafe fn expandable(&self, _view: &NSOutlineView, item: &AnyObject) -> bool {
            !self.children(Some(item)).is_empty()
        }
    }
    unsafe impl NSOutlineViewDelegate for CollectionBridge {
        #[unsafe(method(outlineView:isGroupItem:))]
        fn is_group(&self, _view: &NSOutlineView, item: &AnyObject) -> bool {
            self.ivars().group_parents.get()
                && node_index(item).is_some_and(|index| {
                    self.ivars()
                        .nodes
                        .borrow()
                        .get(index)
                        .is_some_and(|node| !node.children.is_empty())
                })
        }

        #[unsafe(method(outlineView:shouldSelectItem:))]
        fn should_select_item(&self, _view: &NSOutlineView, item: &AnyObject) -> bool {
            !self.ivars().group_parents.get()
                || node_index(item).is_some_and(|index| {
                    self.ivars()
                        .nodes
                        .borrow()
                        .get(index)
                        .is_some_and(|node| node.children.is_empty())
                })
        }

        #[unsafe(method_id(outlineView:viewForTableColumn:item:))]
        unsafe fn outline_cell(
            &self,
            view: &NSOutlineView,
            _column: Option<&NSTableColumn>,
            item: &AnyObject,
        ) -> Option<Retained<NSView>> {
            node_index(item).and_then(|row| self.make_cell(view, row, 0))
        }

        #[unsafe(method(outlineViewSelectionDidChange:))]
        fn outline_selection(&self, note: &NSNotification) {
            if let Some(view) = note.object().and_then(|o| o.downcast::<NSOutlineView>().ok()) {
                let row = view.selectedRow();
                let index = if row < 0 {
                    None
                } else {
                    view.itemAtRow(row).as_deref().and_then(node_index)
                };
                self.select(index);
            }
        }
    }
);
fn node_index(item: &AnyObject) -> Option<usize> {
    item.downcast_ref::<NSNumber>()
        .and_then(|n| usize::try_from(n.unsignedLongLongValue()).ok())
}
impl CollectionBridge {
    pub(super) fn do_accept_drop(
        &self,
        table: &NSTableView,
        info: &ProtocolObject<dyn NSDraggingInfo>,
        row: isize,
        operation: NSTableViewDropOperation,
    ) -> bool {
        let _keep_alive = self.retain();
        if operation != NSTableViewDropOperation::Above || !self.local_drag(table, info) {
            return false;
        }
        let Some(from) = info
            .draggingPasteboard()
            .stringForType(&drag_type())
            .and_then(|s| s.to_string().parse::<usize>().ok())
        else {
            return false;
        };
        let count = (self.ivars().count)();
        if from >= count || row < 0 || row as usize > count {
            return false;
        }
        let to = row as usize - usize::from(from < row as usize);
        if from == to {
            return true;
        }
        if let Ok(mut cb) = self.ivars().reorder.try_borrow_mut() {
            if let Some(cb) = cb.as_mut() {
                callback(|| cb(from, to));
            }
        }
        self.reload(table);
        table.selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(to), false);
        true
    }

    pub(super) fn new(
        ui: &Ui,
        count: impl Fn() -> usize + 'static,
        cell: impl FnMut(usize, usize) -> Box<dyn NativeView> + 'static,
    ) -> Retained<Self> {
        let this = Self::alloc(ui.mtm()).set_ivars(CollectionState {
            count: Box::new(count),
            cell: RefCell::new(Box::new(cell)),
            native_cell: RefCell::new(None),
            selection: RefCell::new(None),
            selectable: RefCell::new(Box::new(|_| true)),
            reorder: RefCell::new(None),
            reorderable: Cell::new(false),
            group_parents: Cell::new(false),
            table_groups: RefCell::new(Box::new(|_| false)),
            row_height: RefCell::new(None),
            roots: RefCell::new(Vec::new()),
            nodes: RefCell::new(Vec::new()),
        });
        unsafe { msg_send![super(this), init] }
    }

    pub(super) fn make_cell(
        &self,
        table: &NSTableView,
        row: usize,
        column: usize,
    ) -> Option<Retained<NSView>> {
        let _keep_alive = self.retain();
        if row >= (self.ivars().count)() {
            return None;
        }
        let mut result = None;
        callback(|| {
            if let Some(make_cell) = self.ivars().native_cell.borrow_mut().as_mut() {
                result = Some(make_cell(table, row, column));
            } else {
                let identifier = NSString::from_str(&format!("cgs.cell.{column}"));
                let native = unsafe { table.makeViewWithIdentifier_owner(&identifier, None) }
                    .and_then(|view| view.downcast::<CollectionCell>().ok())
                    .unwrap_or_else(|| {
                        let cell = CollectionCell::new(&Ui::new(table.mtm()));
                        cell.setIdentifier(Some(&identifier));
                        cell
                    });
                native.set_content((self.ivars().cell.borrow_mut())(row, column));
                result = Some(native.into_super().into_super());
            }
        });
        result
    }

    pub(super) fn reload(&self, table: &NSTableView) { table.reloadData(); }

    pub(super) fn select(&self, index: Option<usize>) {
        let _keep_alive = self.retain();
        if let Ok(mut f) = self.ivars().selection.try_borrow_mut() {
            if let Some(f) = f.as_mut() {
                callback(|| f(index));
            }
        }
    }

    pub(super) fn local_drag(
        &self,
        table: &NSTableView,
        info: &ProtocolObject<dyn NSDraggingInfo>,
    ) -> bool {
        self.ivars().reorderable.get()
            && info
                .draggingSource()
                .is_some_and(|source| std::ptr::eq(&*source, table as &AnyObject))
    }

    pub(super) fn children(&self, item: Option<&AnyObject>) -> Ref<'_, [usize]> {
        match item {
            None => Ref::map(self.ivars().roots.borrow(), |roots| roots.as_slice()),
            Some(item) => Ref::map(self.ivars().nodes.borrow(), |nodes| {
                node_index(item)
                    .and_then(|i| nodes.get(i))
                    .map_or(&[][..], |node| node.children.as_slice())
            }),
        }
    }
}
