use std::cell::RefCell;
use std::rc::Rc;

use objc2::rc::Weak;
use objc2_app_kit::*;

use crate::Ui;
use crate::collection::Table;
use crate::layout::VStack;
use crate::text::Label;
use crate::view::NativeView;

#[derive(Clone)]
pub struct PickerItem<T> {
    pub id: T,
    pub title: String,
    pub group: Option<String>,
}
#[derive(Clone)]
enum PickerRow<T> {
    Group(String),
    Item(PickerItem<T>),
}
fn picker_rows<T: Clone>(items: &[PickerItem<T>], query: &str) -> Vec<PickerRow<T>> {
    let query = query.to_lowercase();
    let mut groups: Vec<(Option<String>, Vec<PickerItem<T>>)> = Vec::new();
    for item in items.iter().filter(|i| i.title.to_lowercase().contains(&query)) {
        if let Some((_, items)) = groups.iter_mut().find(|(g, _)| *g == item.group) {
            items.push(item.clone());
        } else {
            groups.push((item.group.clone(), vec![item.clone()]));
        }
    }
    groups
        .into_iter()
        .flat_map(|(group, items)| {
            group
                .map(PickerRow::Group)
                .into_iter()
                .chain(items.into_iter().map(PickerRow::Item))
        })
        .collect()
}
pub struct SearchablePicker<T: Clone + 'static> {
    stack: VStack,
    search: crate::input::SearchField,
    table: Table<PickerRow<T>>,
    items: Rc<RefCell<Vec<PickerItem<T>>>>,
}
impl<T: Clone + 'static> SearchablePicker<T> {
    pub fn new(ui: &Ui, items: Vec<PickerItem<T>>) -> Self {
        let items = Rc::new(RefCell::new(items));
        let ui_copy = *ui;
        let table = Table::new(ui)
            .column("item", "", 300.0)
            .cells(move |row, _| match row {
                PickerRow::Group(title) => {
                    Box::new(crate::text::SectionTitle::new(&ui_copy, title))
                }
                PickerRow::Item(item) => Box::new(Label::new(&ui_copy, &item.title)),
            })
            .selectable(|row| matches!(row, PickerRow::Item(_)))
            .rows(picker_rows(&items.borrow(), ""));
        table.native.setHeaderView(None);
        let all = items.clone();
        let rows = table.rows.clone();
        let native = Weak::new(&*table.native);
        let bridge = Weak::new(&*table.bridge);
        let search =
            crate::input::SearchField::new(ui)
                .placeholder("Search")
                .on_change(move |query| {
                    *rows.borrow_mut() = picker_rows(&all.borrow(), &query);
                    if let (Some(table), Some(bridge)) = (native.load(), bridge.load()) {
                        bridge.reload(&table);
                    }
                });
        let stack = VStack::new(ui);
        stack.ns_stack_view().addArrangedSubview(search.ns_view());
        stack.ns_stack_view().addArrangedSubview(table.ns_view());
        Self { stack, search, table, items }
    }

    pub fn on_select(mut self, mut f: impl FnMut(T) + 'static) -> Self {
        let rows = self.table.rows.clone();
        self.table = self.table.on_select(move |index| {
            let id = index.and_then(|i| match rows.borrow().get(i) {
                Some(PickerRow::Item(item)) => Some(item.id.clone()),
                _ => None,
            });
            if let Some(id) = id {
                f(id);
            }
        });
        self
    }

    pub fn set_items(&self, items: Vec<PickerItem<T>>) {
        *self.items.borrow_mut() = items;
        self.table.set_rows(picker_rows(&self.items.borrow(), &self.search.get_value()));
    }

    pub fn set_selected(&self, id: &T)
    where T: PartialEq {
        let index = self
            .table
            .rows
            .borrow()
            .iter()
            .position(|row| matches!(row,PickerRow::Item(item) if &item.id==id));
        self.table.set_selected(index);
    }

    pub fn ns_search_field(&self) -> &NSSearchField { self.search.ns_search_field() }

    pub fn ns_table_view(&self) -> &NSTableView { self.table.ns_table_view() }
}
impl<T: Clone + 'static> NativeView for SearchablePicker<T> {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}
