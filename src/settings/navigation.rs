use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::rc::{Retained, Weak};
use objc2_app_kit::*;
use objc2_foundation::{NSArray, NSString};

use crate::collection::Table;
use crate::input::SearchField;
use crate::layout::{VStack, uniform_insets};
use crate::text::{Caption, Label, SubsectionTitle};
use crate::view::{NativeControl, NativeView, PageHost, View};
use crate::{CGSize, Insets, Ui};

/// Back and forward history for typed settings destinations.
pub struct SettingsHistory<T> {
    destinations: Vec<T>,
    index: usize,
}

impl<T: Copy + PartialEq> SettingsHistory<T> {
    pub fn new(initial: T) -> Self { Self { destinations: vec![initial], index: 0 } }

    pub fn current(&self) -> T { self.destinations[self.index] }

    pub fn navigate(&mut self, destination: T) {
        if destination != self.current() {
            self.destinations.truncate(self.index + 1);
            self.destinations.push(destination);
            self.index += 1;
        }
    }

    pub fn step(&mut self, offset: isize) -> Option<T> {
        let index = self.index.checked_add_signed(offset)?;
        let destination = *self.destinations.get(index)?;
        self.index = index;
        Some(destination)
    }

    pub fn can_go_back(&self) -> bool { self.index > 0 }

    pub fn can_go_forward(&self) -> bool { self.index + 1 < self.destinations.len() }
}

/// A muted badge color for a destination in a settings sidebar.
#[derive(Clone, Copy)]
pub enum SettingsBadge {
    Gray,
    Blue,
    Violet,
    Brown,
    Slate,
    Teal,
}

impl SettingsBadge {
    fn color(self) -> Retained<NSColor> {
        let (r, g, b) = match self {
            Self::Gray => (0.38, 0.40, 0.43),
            Self::Blue => (0.32, 0.42, 0.52),
            Self::Violet => (0.40, 0.41, 0.51),
            Self::Brown => (0.49, 0.43, 0.35),
            Self::Slate => (0.40, 0.41, 0.46),
            Self::Teal => (0.34, 0.44, 0.46),
        };
        NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, 1.0)
    }
}

/// One destination, with an optional visual separation before its group.
#[derive(Clone)]
pub struct SettingsDestination<T> {
    pub id: T,
    pub title: &'static str,
    pub symbol: &'static str,
    pub badge: SettingsBadge,
    pub starts_group: bool,
}

#[derive(Clone)]
enum SettingsRow<T> {
    Spacer,
    Destination(SettingsDestination<T>),
}

/// A compact settings navigation list with a neutral, rounded selection.
pub struct SettingsNavigation<T: Clone + PartialEq + 'static> {
    table: crate::collection::Table<SettingsRow<T>>,
}

impl<T: Clone + PartialEq + 'static> SettingsNavigation<T> {
    pub fn new(
        ui: &Ui,
        destinations: Vec<SettingsDestination<T>>,
        mut select: impl FnMut(T) + 'static,
    ) -> Self {
        let mut rows = Vec::new();
        for destination in destinations {
            if destination.starts_group && !rows.is_empty() {
                rows.push(SettingsRow::Spacer);
            }
            rows.push(SettingsRow::Destination(destination));
        }
        let ui_copy = *ui;
        let table_ref = Rc::new(RefCell::new(None::<Weak<NSTableView>>));
        let cell_table = table_ref.clone();
        let table = crate::collection::Table::new(ui)
            .source_list()
            .column("destination", "", 0.0)
            .cells_with_index(move |row, _, index| match row {
                SettingsRow::Spacer => Box::new(crate::view::View::new(&ui_copy)),
                SettingsRow::Destination(item) => {
                    let cell =
                        settings_destination_cell(&ui_copy, item.title, item.symbol, item.badge);
                    let selected = cell_table
                        .borrow()
                        .as_ref()
                        .and_then(Weak::load)
                        .is_some_and(|table| table.selectedRow() == index as isize);
                    style_settings_cell(&cell.0, selected);
                    Box::new(cell)
                }
            })
            .selectable(|row| matches!(row, SettingsRow::Destination(_)))
            .row_heights(|row| {
                if matches!(row, SettingsRow::Spacer) {
                    12.0
                } else {
                    32.0
                }
            })
            .rows(rows);
        *table_ref.borrow_mut() = Some(Weak::new(table.ns_table_view()));
        let native = Weak::new(table.ns_table_view());
        let table = table.on_select_item(move |row| {
            if let Some(table) = native.load() {
                update_settings_selection(&table);
            }
            if let Some(SettingsRow::Destination(item)) = row {
                select(item.id);
            }
        });
        let native = table.ns_table_view();
        // Plain tables do not add the source-list indentation around our own inset.
        native.setStyle(NSTableViewStyle::Plain);
        native.setSelectionHighlightStyle(NSTableViewSelectionHighlightStyle::None);
        native.setIntercellSpacing(CGSize::new(0.0, 0.0));
        native.setBackgroundColor(&NSColor::clearColor());
        table.min_width(210.0);
        let width = table.ns_view().widthAnchor().constraintEqualToConstant(215.0);
        width.setPriority(499.0);
        width.setActive(true);
        table.max_width(280.0);
        Self { table }
    }

    pub fn set_selected(&self, id: &T) {
        let index = (0..self.table.ns_table_view().numberOfRows() as usize).find(|&index| {
            matches!(self.table.row(index), Some(SettingsRow::Destination(item)) if &item.id == id)
        });
        self.table.set_selected(index);
        update_settings_selection(self.table.ns_table_view());
    }

    pub fn accessibility_label(&self, label: &str) {
        self.table
            .ns_table_view()
            .setAccessibilityLabel(Some(&NSString::from_str(label)));
    }

    pub fn ns_table_view(&self) -> &NSTableView { self.table.ns_table_view() }
}

impl<T: Clone + PartialEq + 'static> NativeView for SettingsNavigation<T> {
    fn ns_view(&self) -> &NSView { self.table.ns_view() }
}

fn style_settings_cell(cell: &NSTableCellView, selected: bool) {
    if let Some(highlight) = cell.subviews().firstObject() {
        if let Some(layer) = highlight.layer() {
            layer.setOpacity(if selected { 1.0 } else { 0.0 });
        }
    }
    if let Some(field) = unsafe { cell.textField() } {
        field.setTextColor(Some(&NSColor::labelColor()));
        field.setFont(Some(&*if selected {
            crate::style::Font::body_emphasized()
        } else {
            crate::style::Font::body()
        }));
    }
}

fn update_settings_selection(table: &NSTableView) {
    let selected = table.selectedRow();
    for row in 0..table.numberOfRows() {
        if let Some(outer) = table.viewAtColumn_row_makeIfNecessary(0, row, row == selected)
            && let Some(cell) = outer
                .subviews()
                .firstObject()
                .and_then(|view| view.downcast::<NSTableCellView>().ok())
        {
            style_settings_cell(&cell, row == selected);
        }
    }
}

struct SettingsDestinationCell(Retained<NSTableCellView>);
impl NativeView for SettingsDestinationCell {
    fn ns_view(&self) -> &NSView { &self.0 }
}

fn settings_destination_cell(
    ui: &Ui,
    title: &str,
    symbol: &str,
    badge_color: SettingsBadge,
) -> SettingsDestinationCell {
    let native = NSTableCellView::new(ui.mtm());
    let highlight = crate::view::View::settings_color(ui, 2);
    highlight.ns_view().setWantsLayer(true);
    if let Some(layer) = highlight.ns_view().layer() {
        layer.setCornerRadius(9.0);
    }
    native.addSubview(highlight.ns_view());
    highlight.ns_view().setTranslatesAutoresizingMaskIntoConstraints(false);
    let label = crate::text::Label::new(ui, title);
    label.ns_view().setTranslatesAutoresizingMaskIntoConstraints(false);
    native.addSubview(label.ns_view());
    unsafe {
        native.setTextField(Some(label.ns_text_field()));
    }
    let badge = crate::view::View::new(ui);
    badge.width(20.0);
    badge.height(20.0);
    badge.ns_view().setWantsLayer(true);
    if let Some(layer) = badge.ns_view().layer() {
        layer.setBackgroundColor(Some(&badge_color.color().CGColor()));
        layer.setCornerRadius(5.0);
        layer.setBorderWidth(0.5);
        layer.setBorderColor(Some(&NSColor::colorWithWhite_alpha(1.0, 0.10).CGColor()));
    }
    if let Some(image) = crate::image::ImageView::symbol(ui, symbol) {
        image
            .ns_image_view()
            .setContentTintColor(Some(&NSColor::colorWithWhite_alpha(0.94, 1.0)));
        image.width(14.0);
        image.height(14.0);
        badge.ns_view().addSubview(image.ns_view());
        image.ns_view().setTranslatesAutoresizingMaskIntoConstraints(false);
        image
            .ns_view()
            .centerXAnchor()
            .constraintEqualToAnchor(&badge.ns_view().centerXAnchor())
            .setActive(true);
        image
            .ns_view()
            .centerYAnchor()
            .constraintEqualToAnchor(&badge.ns_view().centerYAnchor())
            .setActive(true);
    }
    badge.ns_view().setTranslatesAutoresizingMaskIntoConstraints(false);
    native.addSubview(badge.ns_view());
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        highlight
            .ns_view()
            .leadingAnchor()
            .constraintEqualToAnchor_constant(&native.leadingAnchor(), 10.0),
        highlight
            .ns_view()
            .trailingAnchor()
            .constraintEqualToAnchor_constant(&native.trailingAnchor(), -10.0),
        highlight.ns_view().topAnchor().constraintEqualToAnchor(&native.topAnchor()),
        highlight
            .ns_view()
            .bottomAnchor()
            .constraintEqualToAnchor(&native.bottomAnchor()),
        badge
            .ns_view()
            .leadingAnchor()
            .constraintEqualToAnchor_constant(&native.leadingAnchor(), 16.0),
        badge.ns_view().centerYAnchor().constraintEqualToAnchor(&native.centerYAnchor()),
        label
            .ns_view()
            .leadingAnchor()
            .constraintEqualToAnchor_constant(&badge.ns_view().trailingAnchor(), 8.0),
        label
            .ns_view()
            .trailingAnchor()
            .constraintLessThanOrEqualToAnchor_constant(&native.trailingAnchor(), -15.0),
        label.ns_view().centerYAnchor().constraintEqualToAnchor(&native.centerYAnchor()),
    ]));
    SettingsDestinationCell(native)
}

/// Standard settings sidebar chrome, including the search field and background.
pub fn settings_sidebar_pane(
    ui: &Ui,
    search: crate::input::SearchField,
    content: impl NativeView,
) -> crate::view::View {
    search.control_size(NSControlSize::Large);
    search.height(28.0);
    let pane = crate::layout::VStack::new(ui)
        .spacing(4.0)
        .push(
            crate::layout::HStack::new(ui)
                .insets(crate::Insets {
                    top: 6.0,
                    left: 10.0,
                    bottom: 0.0,
                    right: 10.0,
                })
                .push(search),
        )
        .push(content);
    let background = crate::view::View::settings_color(ui, 1).safe_area_content(pane);
    background
}


/// A grouped result displayed in the settings sidebar.
#[derive(Clone, Copy, PartialEq)]
pub enum SettingsSearchRow<T> {
    Heading(&'static str),
    Setting { destination: T, title: &'static str, description: &'static str },
}

struct SettingsSearchCell {
    content: VStack,
    heading: Option<Rc<SubsectionTitle>>,
    result: Option<(Rc<Label>, Rc<Caption>)>,
}

impl SettingsSearchCell {
    fn new(ui: &Ui, is_heading: bool) -> Self {
        let content = VStack::new(ui).spacing(3.0).insets(Insets {
            top: 6.0,
            left: 12.0,
            bottom: 6.0,
            right: 12.0,
        });
        if is_heading {
            let heading = Rc::new(SubsectionTitle::new(ui, ""));
            content.add(heading.clone());
            Self { content, heading: Some(heading), result: None }
        } else {
            let label = Rc::new(Label::new(ui, ""));
            let caption = Rc::new(Caption::new(ui, "").max_lines(2));
            content.add(label.clone());
            content.add(caption.clone());
            Self { content, heading: None, result: Some((label, caption)) }
        }
    }
}

impl NativeView for SettingsSearchCell {
    fn ns_view(&self) -> &NSView { self.content.ns_view() }
}

/// Replace the settings navigation with grouped results while searching.
/// The first destination returned by the lookup is opened when Return is pressed.
pub fn searchable_settings_sidebar<T: Copy + PartialEq + 'static>(
    ui: Ui,
    sidebar: Rc<impl NativeView>,
    lookup: impl Fn(&str) -> (Option<T>, Vec<SettingsSearchRow<T>>) + 'static,
    key: impl Fn(T) -> usize + 'static,
    open: impl Fn(T) + 'static,
) -> View {
    let content = Rc::new(PageHost::new(&ui));
    content.set_cached_page(sidebar.clone());
    let open = Rc::new(open);
    let mut results = None;
    let mut empty = None;
    let first = Rc::new(Cell::new(None::<T>));
    let top = first.clone();
    let open_results = open.clone();
    let weak_content = Rc::downgrade(&content);
    let search = SearchField::new(&ui)
        .placeholder("Search")
        .on_change(move |query| {
            let Some(content) = weak_content.upgrade() else {
                return;
            };
            let (first, rows) = lookup(&query);
            top.set(first);
            if rows.is_empty() {
                content.set_cached_page(if query.trim().is_empty() {
                    sidebar.clone()
                } else {
                    empty
                        .get_or_insert_with(|| {
                            Rc::new(
                                VStack::new(&ui)
                                    .insets(uniform_insets(12.0))
                                    .push(Caption::new(&ui, "No matching settings")),
                            )
                        })
                        .clone()
                });
                return;
            }
            let results = results.get_or_insert_with(|| {
                let pick = open_results.clone();
                Rc::new(
                    Table::new(&ui)
                        .source_list()
                        .column("setting", "", 0.0)
                        .reusable_cells(
                        |row: &SettingsSearchRow<T>, _| usize::from(matches!(row, SettingsSearchRow::Heading(_))),
                        |ui, _, kind| SettingsSearchCell::new(ui, kind == 1),
                        |cell, row, _, _| match row {
                            SettingsSearchRow::Heading(title) => cell.heading.as_ref().unwrap().set_text(title),
                            SettingsSearchRow::Setting { title, description, .. } => {
                                let (label, caption) = cell.result.as_ref().unwrap();
                                label.set_text(title);
                                caption.set_text(description);
                            }
                        },
                    )
                    .group_rows(|row| matches!(row, SettingsSearchRow::Heading(_)))
                        .selectable(|row| matches!(row, SettingsSearchRow::Setting { .. }))
                        .row_heights(|row| {
                            if matches!(row, SettingsSearchRow::Heading(_)) {
                                30.0
                            } else {
                                54.0
                            }
                        })
                        .on_select_item(move |row| {
                            if let Some(SettingsSearchRow::Setting { destination, .. }) = row {
                                pick(destination);
                            }
                        }),
                )
            });
            results.set_keyed_rows(rows, |row| match *row {
                SettingsSearchRow::Heading(title) => Ok(title),
                SettingsSearchRow::Setting { destination, .. } => Err(key(destination)),
            });
            results.scroll_to(0);
            content.set_cached_page(results.clone());
        })
        .on_commit(move |_| {
            if let Some(entry) = first.get() {
                open(entry);
            }
        });
    settings_sidebar_pane(&ui, search, content)
}
