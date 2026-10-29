use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::*;
use objc2_foundation::NSString;

use crate::Ui;
use crate::collection::Table;
use crate::layout::VStack;
use crate::text::Label;
use crate::view::{NativeControl, NativeView};

struct SettingsCellContent {
    content: crate::layout::HStack,
    title: Rc<Label>,
    summary: Option<Rc<crate::text::Caption>>,
    trailing: Option<Rc<Label>>,
    icon: Option<Retained<NSImageView>>,
    button: Option<Rc<crate::controls::Button>>,
    divider: Rc<crate::layout::Divider>,
    index: Rc<Cell<usize>>,
}
define_class!(
    #[unsafe(super(NSTableCellView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgUiSettingsListCell"]
    #[ivars = SettingsCellContent]
    struct SettingsListCell;
);
impl SettingsListCell {
    fn new(
        ui: &Ui,
        open: Rc<RefCell<Option<Box<dyn FnMut(usize)>>>>,
        inline: bool,
        has_icon: bool,
        navigation: bool,
        external: bool,
        opens: bool,
    ) -> Retained<Self> {
        let title = Rc::new(Label::new(ui, ""));
        let summary = (!inline).then(|| Rc::new(crate::text::Caption::single_line(ui, "")));
        let trailing = inline.then(|| Rc::new(Label::new(ui, "")));
        let icon = has_icon.then(|| {
            let image = NSImageView::new(ui.mtm());
            image.widthAnchor().constraintEqualToConstant(22.0).setActive(true);
            image.heightAnchor().constraintEqualToConstant(22.0).setActive(true);
            image.setContentTintColor(Some(&crate::style::Color::secondary_label()));
            image
        });
        let chevron = navigation.then(|| {
            let image = NSImageView::new(ui.mtm());
            image.setImage(
                crate::image::Symbol::named(if external {
                    "arrow.up.right"
                } else {
                    "chevron.forward"
                })
                .as_deref(),
            );
            image.widthAnchor().constraintEqualToConstant(12.0).setActive(true);
            image.setContentTintColor(Some(&crate::style::Color::secondary_label()));
            image.setAccessibilityElement(false);
            image
        });
        let index = Rc::new(Cell::new(0));
        let action_index = index.clone();
        let button = (opens && !navigation).then(|| {
            let button = Rc::new(
                crate::controls::Button::new(ui, "Open")
                    .symbol("chevron.forward")
                    .borderless()
                    .on_click(move || {
                        if let Some(open) = open.borrow_mut().as_mut() {
                            open(action_index.get());
                        }
                    }),
            );
            button
                .ns_button()
                .setContentTintColor(Some(&crate::style::Color::secondary_label()));
            button.control_size(NSControlSize::Small);
            button.width(16.0);
            button
        });
        let mut row = crate::layout::HStack::new(ui).insets(crate::Insets {
            top: 8.0,
            left: 12.0,
            bottom: 8.0,
            right: 12.0,
        });
        if let Some(icon) = &icon {
            row = row.push(icon.clone().into_super().into_super());
        }
        row = if let Some(summary) = &summary {
            row.push(VStack::new(ui).spacing(2.0).push(title.clone()).push(summary.clone()))
        } else {
            row.push(title.clone())
        };
        row = row.spacer(ui);
        if let Some(trailing) = &trailing {
            row = row.push(trailing.clone());
        }
        if let Some(chevron) = &chevron {
            row = row.push(chevron.clone().into_super().into_super());
        }
        if let Some(button) = &button {
            row = row.push(button.clone());
        }
        let divider = Rc::new(crate::layout::Divider::new(ui));
        let content = row;
        let native: Retained<Self> = unsafe {
            msg_send![
                super(Self::alloc(ui.mtm()).set_ivars(SettingsCellContent {
                    content,
                    title,
                    summary,
                    trailing,
                    icon,
                    button,
                    divider,
                    index,
                })),
                init
            ]
        };
        native.addSubview(native.ivars().content.ns_view());
        native.addSubview(native.ivars().divider.ns_view());
        let line = native.ivars().divider.ns_view();
        crate::view::prepare(line);
        line.leadingAnchor()
            .constraintEqualToAnchor_constant(&native.leadingAnchor(), 12.0)
            .setActive(true);
        line.trailingAnchor()
            .constraintEqualToAnchor_constant(&native.trailingAnchor(), -12.0)
            .setActive(true);
        line.bottomAnchor()
            .constraintEqualToAnchor(&native.bottomAnchor())
            .setActive(true);
        line.heightAnchor().constraintEqualToConstant(1.0).setActive(true);
        crate::view::pin(&native, native.ivars().content.ns_view(), crate::Insets {
            top: 0.0,
            left: 0.0,
            bottom: 0.0,
            right: 0.0,
        });
        unsafe {
            native.setTextField(Some(native.ivars().title.ns_text_field()));
            native.setImageView(native.ivars().icon.as_deref());
        }
        native
    }

    fn configure(
        &self,
        name: &str,
        summary: &str,
        symbol: Option<&str>,
        index: usize,
        count: usize,
    ) {
        let cell = self.ivars();
        cell.index.set(index);
        cell.title.set_text(name);
        cell.title.tooltip(name);
        if let Some(caption) = &cell.summary {
            caption.set_text(summary);
            caption.tooltip(summary);
            caption.set_hidden(summary.is_empty());
        }
        if let Some(trailing) = &cell.trailing {
            trailing.set_text(summary);
            trailing.tooltip(summary);
            trailing.set_hidden(summary.is_empty());
        }
        if let Some(icon) = &cell.icon {
            icon.setContentTintColor(Some(&crate::style::Color::secondary_label()));
            icon.setImage(symbol.and_then(crate::image::Symbol::named).as_deref());
            icon.setHidden(symbol.is_none());
        }
        if let Some(button) = &cell.button {
            button.accessibility_label(&format!("Open {name}"));
        }
        cell.divider.set_hidden(index + 1 == count);
    }
}
impl NativeView for Retained<SettingsListCell> {
    fn ns_view(&self) -> &NSView { self }
}

/// A native settings collection with primary text, a summary, and optional disclosure.
/// NSTableView owns selection, keyboard navigation, scrolling, and drag reordering.
pub struct SettingsList<T: 'static> {
    table: Table<T>,
    surface: crate::layout::GroupBox,
    open: Rc<RefCell<Option<Box<dyn FnMut(usize)>>>>,
    symbol: Rc<RefCell<Option<Box<dyn Fn(&T) -> String>>>>,
    image: Rc<RefCell<Option<Box<dyn Fn(&T) -> Option<Retained<NSImage>>>>>>,
    fitted_height: Option<(Retained<NSLayoutConstraint>, f64)>,
    row_count: Rc<Cell<usize>>,
    empty: crate::text::SecondaryLabel,
    trailing_summary: Rc<Cell<bool>>,
    navigation: Rc<Cell<bool>>,
    external: Rc<Cell<bool>>,
}
impl<T: 'static> SettingsList<T> {
    pub fn new(
        ui: &Ui,
        title: impl Fn(&T) -> String + 'static,
        summary: impl Fn(&T) -> String + 'static,
    ) -> Self {
        let open: Rc<RefCell<Option<Box<dyn FnMut(usize)>>>> = Rc::new(RefCell::new(None));
        let symbol: Rc<RefCell<Option<Box<dyn Fn(&T) -> String>>>> = Rc::new(RefCell::new(None));
        let image: Rc<RefCell<Option<Box<dyn Fn(&T) -> Option<Retained<NSImage>>>>>> =
            Rc::new(RefCell::new(None));
        let row_image = image.clone();
        let row_open = open.clone();
        let row_symbol = symbol.clone();
        let ui_copy = *ui;
        let double_open = open.clone();
        let row_count = Rc::new(Cell::new(0));
        let cell_count = row_count.clone();
        let trailing_summary = Rc::new(Cell::new(false));
        let inline = trailing_summary.clone();
        let navigation = Rc::new(Cell::new(false));
        let row_navigation = navigation.clone();
        let external = Rc::new(Cell::new(false));
        let row_external = external.clone();
        let table = Table::new(ui).column("item", "", 0.0).on_double_click(move |index| {
            if let Some(open) = double_open.borrow_mut().as_mut() {
                open(index);
            }
        });
        let rows = table.rows.clone();
        let opens = open.clone();
        *table.bridge.ivars().native_cell.borrow_mut() = Some(Box::new(move |table, row, _| {
            let identifier = NSString::from_str("cgs.settings-list");
            let cell = unsafe { table.makeViewWithIdentifier_owner(&identifier, None) }
                .and_then(|view| view.downcast::<SettingsListCell>().ok())
                .unwrap_or_else(|| {
                    let cell = SettingsListCell::new(
                        &ui_copy,
                        row_open.clone(),
                        inline.get(),
                        row_symbol.borrow().is_some() || row_image.borrow().is_some(),
                        row_navigation.get(),
                        row_external.get(),
                        // Opening a sheet can synchronously ask AppKit to rebuild this cell
                        // while the open callback holds its mutable borrow.
                        opens.try_borrow().map_or(true, |open| open.is_some()),
                    );
                    cell.setIdentifier(Some(&identifier));
                    cell
                });
            let rows = rows.borrow();
            let item = &rows[row];
            let symbol = row_symbol.borrow().as_ref().map(|symbol| symbol(item));
            cell.configure(
                &title(item),
                &summary(item),
                symbol.as_deref(),
                row,
                cell_count.get(),
            );
            if let Some(provider) = row_image.borrow().as_ref() {
                if let Some(image) = provider(item) {
                    if let Some(icon) = &cell.ivars().icon {
                        icon.setContentTintColor(None);
                        icon.setImage(Some(&image));
                        icon.setHidden(false);
                    }
                }
            }
            cell.into_super().into_super()
        }));
        // The enclosing group supplies the surface; avoid NSTableView's extra inset margins.
        table.ns_table_view().setStyle(NSTableViewStyle::Plain);
        table.ns_table_view().setHeaderView(None);
        table.ns_table_view().setRowHeight(44.0);
        table.ns_table_view().setBackgroundColor(&NSColor::clearColor());
        table.ns_scroll_view().setDrawsBackground(false);
        let surface =
            crate::layout::GroupBox::with_insets(ui, table.ns_view().retain(), crate::Insets {
                top: 0.0,
                left: 0.0,
                bottom: 0.0,
                right: 0.0,
            });
        let empty = crate::text::SecondaryLabel::new(ui, "No items");
        empty.ns_text_field().setAlignment(NSTextAlignment::Center);
        surface.ns_view().addSubview(empty.ns_view());
        crate::view::prepare(empty.ns_view());
        empty
            .ns_view()
            .centerXAnchor()
            .constraintEqualToAnchor(&surface.ns_view().centerXAnchor())
            .setActive(true);
        empty
            .ns_view()
            .centerYAnchor()
            .constraintEqualToAnchor(&surface.ns_view().centerYAnchor())
            .setActive(true);
        empty
            .ns_view()
            .widthAnchor()
            .constraintLessThanOrEqualToAnchor_multiplier(&surface.ns_view().widthAnchor(), 0.9)
            .setActive(true);
        Self {
            table,
            surface,
            open,
            symbol,
            image,
            fitted_height: None,
            row_count,
            empty,
            trailing_summary,
            navigation,
            external,
        }
    }

    /// Expand to every row so an enclosing settings page owns scrolling.
    pub fn full_length(self) -> Self {
        self.table.scroll.defer_scrolling();
        self.fit_content(f64::MAX)
    }

    /// Small inventories size to their rows; larger inventories scroll within the cap.

    pub fn fit_content(mut self, maximum_height: f64) -> Self {
        if let Some((_, maximum)) = &mut self.fitted_height {
            *maximum = maximum_height.max(64.0);
        } else {
            let height = self.ns_view().heightAnchor().constraintEqualToConstant(64.0);
            height.setActive(true);
            self.fitted_height = Some((height, maximum_height.max(64.0)));
        }
        self
    }

    pub fn set_rows_if_changed(&self, rows: Vec<T>)
    where T: PartialEq {
        if *self.table.rows.borrow() != rows {
            self.set_rows(rows);
        }
    }

    pub fn set_rows(&self, rows: Vec<T>) {
        let count = rows.len();
        self.row_count.set(count);
        self.empty.set_hidden(count != 0);
        self.table.set_rows(rows);
        if let Some((height, maximum)) = &self.fitted_height {
            let table = self.table.ns_table_view();
            // Inset tables add native padding before the first and after the last row.
            let native_padding = if count == 0 {
                0.0
            } else {
                table.rectOfRow(0).origin.y * 2.0
            };
            let total = (count as f64 * (table.rowHeight() + table.intercellSpacing().height)
                + native_padding)
                .max(if count > 0 { table.rowHeight() } else { 64.0 });
            let fitted = total.min(*maximum);
            if height.constant() != fitted {
                height.setConstant(fitted);
            }
            self.table.ns_scroll_view().setHasVerticalScroller(total > *maximum);
        }
    }

    /// Open destinations with one click, without a persistent selection highlight.
    pub fn navigation(self) -> Self {
        self.navigation.set(true);
        let table = self.table.ns_table_view();
        table.setUsesAutomaticRowHeights(false);
        table.setRowHeight(56.0);
        table.setSelectionHighlightStyle(NSTableViewSelectionHighlightStyle::None);
        unsafe {
            table.setAction(Some(objc2::sel!(invoke:)));
        }
        self
    }

    /// Native single-click resource rows with an external-link indicator.
    pub fn external_links(self) -> Self {
        self.external.set(true);
        self.navigation().trailing_summary()
    }

    /// Display a short value beside the title, as in a native keyboard shortcuts list.
    pub fn trailing_summary(self) -> Self {
        self.trailing_summary.set(true);
        self.table.ns_table_view().setRowHeight(44.0);
        self
    }

    pub fn empty_message(self, message: &str) -> Self {
        self.set_empty_message(message);
        self
    }

    /// Update guidance when a collection's search or filters change.
    pub fn set_empty_message(&self, message: &str) { self.empty.set_text(message); }

    pub fn on_select(self, f: impl FnMut(Option<usize>) + 'static) -> Self {
        self.table.set_on_select(f);
        self
    }

    /// A copy of a displayed row, without repeating its source's filtering.
    pub fn row(&self, index: usize) -> Option<T>
    where T: Clone {
        self.table.row(index)
    }

    pub fn on_select_item(self, mut f: impl FnMut(Option<T>) + 'static) -> Self
    where T: Clone {
        let rows = self.table.rows.clone();
        self.table
            .set_on_select(move |index| f(index.and_then(|i| rows.borrow().get(i).cloned())));
        self
    }

    pub fn on_open(self, f: impl FnMut(usize) + 'static) -> Self {
        *self.open.borrow_mut() = Some(Box::new(f));
        // Use AppKit's click action, preserving selection and drag handling.
        unsafe {
            self.table.ns_table_view().setAction(Some(objc2::sel!(invoke:)));
            self.table.ns_table_view().setDoubleAction(None);
        }
        self
    }

    pub fn symbol(self, name: &str) -> Self {
        let name = name.to_owned();
        *self.symbol.borrow_mut() = Some(Box::new(move |_| name.clone()));
        self
    }

    /// Provide a native image, with the configured symbol as a fallback.
    pub fn images(self, image: impl Fn(&T) -> Option<crate::image::Image> + 'static) -> Self {
        *self.image.borrow_mut() = Some(Box::new(image));
        self
    }

    pub fn symbols(self, symbol: impl Fn(&T) -> String + 'static) -> Self {
        *self.symbol.borrow_mut() = Some(Box::new(symbol));
        self
    }

    pub fn reorderable(mut self, value: bool) -> Self {
        self.table = self.table.reorderable(value);
        self
    }

    pub fn on_reorder(mut self, f: impl FnMut(usize, usize) + 'static) -> Self {
        self.table = self.table.on_reorder(f);
        self
    }
}
impl<T: 'static> std::ops::Deref for SettingsList<T> {
    type Target = Table<T>;

    fn deref(&self) -> &Self::Target { &self.table }
}
impl<T: 'static> NativeView for SettingsList<T> {
    fn ns_view(&self) -> &NSView { self.surface.ns_view() }
}
