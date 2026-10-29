use std::cell::RefCell;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadOnly};
use objc2_app_kit::*;
use objc2_foundation::{NSIndexSet, NSNumber, NSString};

use super::bridge::{CollectionBridge, Node};
use crate::Ui;
use crate::layout::ScrollView;
use crate::text::Label;
use crate::view::NativeView;

pub struct OutlineItem<T> {
    pub id: T,
    pub title: String,
    pub children: Vec<OutlineItem<T>>,
}
pub struct Outline<T: 'static> {
    native: Retained<NSOutlineView>,
    scroll: ScrollView,
    bridge: Retained<CollectionBridge>,
    items: Rc<RefCell<Vec<(T, String)>>>,
    column: Retained<NSTableColumn>,
}
impl<T: 'static> Outline<T> {
    pub fn new(ui: &Ui) -> Self {
        let native = NSOutlineView::new(ui.mtm());
        let column = NSTableColumn::initWithIdentifier(
            NSTableColumn::alloc(ui.mtm()),
            &NSString::from_str("item"),
        );
        native.addTableColumn(&column);
        unsafe {
            native.setOutlineTableColumn(Some(&column));
        }
        native.setHeaderView(None);
        native.setStyle(NSTableViewStyle::SourceList);
        let items: Rc<RefCell<Vec<(T, String)>>> = Rc::new(RefCell::new(Vec::new()));
        let i = items.clone();
        let j = items.clone();
        let ui_copy = *ui;
        let bridge = CollectionBridge::new(
            ui,
            move || i.borrow().len(),
            move |row, _| Box::new(Label::new(&ui_copy, &j.borrow()[row].1)),
        );
        unsafe {
            native.setDataSource(Some(ProtocolObject::from_ref(&*bridge)));
            native.setDelegate(Some(ProtocolObject::from_ref(&*bridge)));
        }
        let view: Retained<NSView> = native.clone().into_super().into_super().into_super();
        let scroll = ScrollView::new(ui, view);
        Self {
            native,
            scroll,
            bridge,
            items,
            column,
        }
    }

    pub fn items(self, items: Vec<OutlineItem<T>>) -> Self {
        self.set_items(items);
        self
    }

    pub fn set_items(&self, items: Vec<OutlineItem<T>>) {
        fn flatten<T>(
            items: Vec<OutlineItem<T>>,
            values: &mut Vec<(T, String)>,
            nodes: &mut Vec<Node>,
        ) -> Vec<usize> {
            items
                .into_iter()
                .map(|item| {
                    let index = values.len();
                    values.push((item.id, item.title));
                    nodes.push(Node {
                        object: NSNumber::new_usize(index),
                        children: Vec::new(),
                    });
                    let children = flatten(item.children, values, nodes);
                    nodes[index].children = children;
                    index
                })
                .collect()
        }
        let mut values = Vec::new();
        let mut nodes = Vec::new();
        let roots = flatten(items, &mut values, &mut nodes);
        *self.items.borrow_mut() = values;
        *self.bridge.ivars().nodes.borrow_mut() = nodes;
        *self.bridge.ivars().roots.borrow_mut() = roots;
        self.native.reloadData();
    }

    pub fn on_select(self, mut f: impl FnMut(Option<T>) + 'static) -> Self
    where T: Clone {
        let items = self.items.clone();
        *self.bridge.ivars().selection.borrow_mut() = Some(Box::new(move |i| {
            let id = i.and_then(|i| items.borrow().get(i).map(|(id, _)| id.clone()));
            f(id);
        }));
        self
    }

    pub fn cells(self, mut cell: impl FnMut(&T, &str) -> Box<dyn NativeView> + 'static) -> Self {
        let items = self.items.clone();
        *self.bridge.ivars().cell.borrow_mut() = Box::new(move |index, _| {
            let items = items.borrow();
            let (item, title) = &items[index];
            cell(item, title)
        });
        self.native.reloadData();
        self
    }

    /// Select a stable item index, revealing its parent if collapsed.
    pub fn set_selected(&self, index: usize) {
        let nodes = self.bridge.ivars().nodes.borrow();
        let Some(node) = nodes.get(index) else {
            return;
        };
        for parent in nodes.iter().filter(|node| node.children.contains(&index)) {
            unsafe {
                self.native.expandItem(Some(&parent.object));
            }
        }
        let row = unsafe { self.native.rowForItem(Some(&node.object)) };
        if row >= 0 {
            self.native.selectRowIndexes_byExtendingSelection(
                &NSIndexSet::indexSetWithIndex(row as usize),
                false,
            );
        }
    }

    /// Treat parent rows as native, nonselectable source-list section headings.
    pub fn group_parents(&self) {
        self.bridge.ivars().group_parents.set(true);
        self.native.reloadData();
        self.expand_all();
    }

    pub fn expand_all(&self) {
        unsafe {
            self.native.expandItem_expandChildren(None, true);
        }
    }

    pub fn ns_outline_view(&self) -> &NSOutlineView { &self.native }

    pub fn ns_table_column(&self) -> &NSTableColumn { &self.column }
}
impl<T: 'static> NativeView for Outline<T> {
    fn ns_view(&self) -> &NSView { self.scroll.ns_view() }
}
impl<T: 'static> Drop for Outline<T> {
    fn drop(&mut self) {
        unsafe {
            self.native.setDelegate(None);
            self.native.setDataSource(None);
            self.native.setOutlineTableColumn(None);
        }
    }
}
