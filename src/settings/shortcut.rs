//! Native shortcut-manager presentation. Applications supply actions and persistence.
use std::rc::Rc;
use objc2::Message;
use objc2_app_kit::*;
use crate::{Insets, Ui};
use crate::collection::Table;
use crate::controls::{Button, Popup};
use crate::input::{KeyRecorder, SearchField};
use crate::layout::{HStack, VStack, uniform_insets};
use crate::text::{Caption, Label, SectionTitle, SecondaryLabel};
use crate::view::{NativeView, View};

pub struct PickerAction {
    pub group: String,
    pub title: String,
    pub keywords: String,
}
#[derive(Clone, PartialEq)]
enum PickerRow { Group(String), Action(usize) }

/// Searchable, grouped native picker; selection refers to the supplied action index.
pub struct ActionPicker { content: VStack, search: Rc<SearchField> }
impl ActionPicker {
    pub fn new(ui: &Ui, actions: Vec<PickerAction>, choose: impl Fn(usize) + 'static, cancel: impl Fn() + 'static) -> Self {
        let actions = Rc::new(actions);
        let choose: Rc<dyn Fn(usize)> = Rc::new(choose);
        let entries = {
            let actions = actions.clone();
            move |query: &str| {
                let mut rows = Vec::new();
                let mut group = None;
                for (index, action) in actions.iter().enumerate() {
                    let text = format!("{} {} {}", action.group, action.title, action.keywords).to_lowercase();
                    if !query.split_whitespace().all(|word| text.contains(word)) { continue; }
                    if group != Some(&action.group) {
                        rows.push(PickerRow::Group(action.group.clone()));
                        group = Some(&action.group);
                    }
                    rows.push(PickerRow::Action(index));
                }
                rows
            }
        };
        let labels = actions.clone();
        let cell_ui = *ui;
        let table = Rc::new(Table::new(ui).column("action", "Action", 0.0)
            .cells(move |row, _| match row {
                PickerRow::Group(title) => Box::new(Caption::new(&cell_ui, title)) as Box<dyn NativeView>,
                PickerRow::Action(index) => Box::new(Label::new(&cell_ui, &labels[*index].title)),
            })
            .group_rows(|row| matches!(row, PickerRow::Group(_)))
            .selectable(|row| matches!(row, PickerRow::Action(_)))
            .row_heights(|_| 28.0)
            .rows(entries("")));
        // A regular inset list, not a sidebar with a solid source-list background.
        table.ns_table_view().setHeaderView(None);
        table.ns_table_view().setStyle(NSTableViewStyle::Inset);
        table.ns_table_view().setFloatsGroupRows(false);
        table.ns_table_view().setBackgroundColor(&NSColor::clearColor());
        table.ns_scroll_view().setDrawsBackground(false);
        table.height(224.0);
        table.accessibility_label("Available shortcut actions");
        table.ns_table_view().setAccessibilityLabel(Some(&objc2_foundation::NSString::from_str("Available shortcut actions")));
        let (list, chosen) = (Rc::downgrade(&table), choose.clone());
        table.set_on_double_click(move |row| {
            if let Some(PickerRow::Action(index)) = list.upgrade().and_then(|list| list.row(row)) { chosen(index); }
        });
        let (list, chosen) = (Rc::downgrade(&table), choose);
        let next = Rc::new(Button::new(ui, "Continue").push().key_equivalent("\r").on_click(move || {
            if let Some(PickerRow::Action(index)) = list.upgrade().and_then(|list| list.selected_row()) { chosen(index); }
        }));
        next.ns_button().setEnabled(false);
        let (button, list) = (Rc::downgrade(&next), Rc::downgrade(&table));
        table.set_on_select(move |_| {
            if let Some(button) = button.upgrade() {
                button.ns_button().setEnabled(matches!(list.upgrade().and_then(|list| list.selected_row()), Some(PickerRow::Action(_))));
            }
        });
        let empty = Rc::new(SecondaryLabel::new(ui, "No matching actions. Try another name or category."));
        empty.set_hidden(true);
        let (list, empty_ref) = (table.clone(), empty.clone());
        let search = Rc::new(SearchField::new(ui).placeholder("Search actions or categories").on_change(move |query| {
            let selected = list.selected_row();
            list.set_selected(None);
            let rows = entries(&query.to_lowercase());
            empty_ref.set_hidden(!rows.is_empty());
            let index = selected.and_then(|item| rows.iter().position(|row| *row == item))
                .or_else(|| (!query.trim().is_empty()).then(|| rows.iter().position(|row| matches!(row, PickerRow::Action(_)))).flatten());
            list.set_rows(rows);
            list.set_selected(index);
        }));
        search.accessibility_label("Search actions");
        let cancel = Button::new(ui, "Cancel").push().key_equivalent("\u{1b}").on_click(cancel);
        cancel.identifier("cgs.sheet-cancel");
        let content = VStack::new(ui).spacing(12.0).insets(uniform_insets(20.0))
            .push(SectionTitle::new(ui, "Add Shortcut"))
            .push(search.clone()).push(table).push(empty)
            .push(HStack::new(ui).spacing(8.0).spacer(ui).push(cancel).push(next));
        content.width(400.0);
        Self { content, search }
    }
    pub fn focus_search(&self) { self.search.focus(); }
}
impl NativeView for ActionPicker { fn ns_view(&self) -> &NSView { self.content.ns_view() } }

/// Compact editor: a full-width action chooser above a grouped options/shortcut form.
pub struct ShortcutEditor(VStack);
impl ShortcutEditor {
    pub fn new(ui: &Ui, action: Popup, options: impl NativeView, recorder: &Rc<KeyRecorder>, feedback: impl NativeView, cancel: Button) -> Self {
        action.accessibility_label("Action");
        action.ns_popup_button().setControlSize(NSControlSize::Regular);
        let field = ShortcutPresentation::recorder(ui, recorder);
        let row = HStack::new(ui).spacing(12.0)
            .push(Label::new(ui, "Shortcut")).spacer(ui).push(field);
        let form = VStack::new(ui).spacing(14.0).insets(uniform_insets(14.0))
            .push(options).push(row);
        cancel.identifier("cgs.sheet-cancel");
        let group = View::settings_card(ui).content(form, uniform_insets(0.0));
        group.set_card_edges(true, true);
        let footer = VStack::new(ui).spacing(0.0).push(feedback)
            .push(HStack::new(ui).insets(Insets { top: 4.0, left: 0.0, bottom: 0.0, right: 0.0 }).spacer(ui).push(cancel));
        let content = VStack::new(ui).spacing(12.0).insets(uniform_insets(20.0))
            .push(SectionTitle::new(ui, "Add Shortcut"))
            .push(action).push(group).push(footer);
        content.width(400.0);
        Self(content)
    }
    pub fn feedback(ui: &Ui, parameters: impl NativeView, message: impl NativeView, conflicts: impl NativeView) -> VStack {
        VStack::new(ui).spacing(4.0).push(parameters).push(message).push(conflicts)
    }
    pub fn option(ui: &Ui, control: impl NativeView) -> VStack {
        control.accessibility_label("Options");
        let form = VStack::new(ui).spacing(6.0);
        // Composite forms already label their own fields. Text/code needs the
        // full form width; only a single compact selector belongs beside a label.
        if control.ns_view().downcast_ref::<NSStackView>().is_some() {
            form.add(control);
        } else if control.ns_view().downcast_ref::<NSTextField>().is_some() {
            form.add(Caption::new(ui, "Options"));
            form.add(control);
        } else {
            control.compression_resistance(250.0, NSLayoutConstraintOrientation::Horizontal);
            control.max_width(230.0);
            form.add(HStack::new(ui).spacing(12.0).push(SecondaryLabel::new(ui, "Options")).spacer(ui).push(control));
        }
        form
    }

}
impl NativeView for ShortcutEditor { fn ns_view(&self) -> &NSView { self.0.ns_view() } }

/// Shared appearance for shortcut rows, toolbars, and contextual editors.
pub struct ShortcutPresentation;
impl ShortcutPresentation {
    pub fn name(ui: &Ui, title: &str) -> Label { Label::new(ui, title).truncating() }
    pub fn description(ui: &Ui, text: &str) -> Label { Label::new(ui, text).wrapping() }
    pub fn body(ui: &Ui, content: impl NativeView) -> View {
        View::settings_background(ui).content(content, uniform_insets(0.0))
    }
    pub fn list<T: 'static>(ui: &Ui) -> Table<T> {
        let table = Table::new(ui).column("shortcut", "", 0.0).row_heights(|_| 40.0);
        Self::table(&table);
        table
    }
    pub fn sheet(ui: &Ui) -> VStack {
        let content = VStack::new(ui).spacing(12.0).insets(uniform_insets(20.0));
        content.width(400.0);
        content
    }
    pub fn actions(ui: &Ui, first: impl NativeView, last: impl NativeView) -> HStack {
        HStack::new(ui).spacing(8.0).spacer(ui).push(first).push(last)
    }
    pub fn conflicts(ui: &Ui) -> VStack {
        VStack::new(ui).insets(Insets { top: 0.0, left: 12.0, bottom: 8.0, right: 12.0 })
    }
    pub fn welcome_container(ui: &Ui) -> VStack {
        VStack::new(ui).insets(Insets { top: 0.0, left: 20.0, bottom: 0.0, right: 20.0 })
    }
    pub fn welcome(ui: &Ui, add: Button, browse: Button, restore: Button, restore_row: Rc<HStack>) -> crate::layout::GroupBox {
        restore_row.add(crate::layout::Spacer::new(ui));
        restore_row.add(restore);
        restore_row.add(crate::layout::Spacer::new(ui));
        let content = VStack::new(ui).spacing(12.0)
            .push(SectionTitle::new(ui, "No shortcuts yet"))
            .push(Label::new(ui, "Choose an action, then record the keys you want to use.")
                .color(&NSColor::secondaryLabelColor()).wrapping())
            .push(HStack::new(ui).spacing(8.0).push(add).push(browse).spacer(ui))
            .push(restore_row);
        crate::layout::GroupBox::with_insets(ui, content, uniform_insets(20.0))
    }
    pub fn advanced(ui: &Ui, title: &str, content: impl NativeView) -> View {
        View::new(ui).content(crate::layout::Disclosure::new(ui, title, content),
            Insets { top: 0.0, left: 20.0, bottom: 20.0, right: 20.0 })
    }
    pub fn empty(ui: &Ui, text: &str) -> SecondaryLabel {
        let label = SecondaryLabel::new(ui, text);
        label.ns_text_field().setAlignment(NSTextAlignment::Center);
        label
    }
    pub fn number_field(ui: &Ui, value: f64, integer: bool) -> crate::controls::NumberField {
        let field = crate::controls::NumberField::new(ui).value(value);
        let field = if integer { field.integer() } else { field };
        field.width(crate::view::Metrics::VALUE_FIELD_WIDTH);
        field
    }
    pub fn text_field(ui: &Ui, value: &str, placeholder: &str, code: bool) -> crate::input::TextField {
        let field = crate::input::TextField::new(ui).value(value).placeholder(placeholder);
        field.min_width(80.0);
        field.hugging_priority(1.0, NSLayoutConstraintOrientation::Horizontal);
        if code { Self::code_field(field.ns_text_field()); }
        field
    }
    pub fn options(ui: &Ui, first: impl NativeView, second: impl NativeView, third: impl NativeView) -> VStack {
        VStack::new(ui).spacing(10.0).push(first).push(second).push(third)
    }
    pub fn flexible(view: &impl NativeView) {
        view.compression_resistance(490.0, NSLayoutConstraintOrientation::Horizontal);
    }
    pub fn validation(ui: &Ui) -> Rc<super::ValidationMessage> {
        let message = Rc::new(super::ValidationMessage::new(ui));
        let field = message.ns_text_field();
        field.setUsesSingleLineMode(false);
        field.setMaximumNumberOfLines(0);
        if let Some(cell) = field.cell() { cell.setWraps(true); cell.setScrollable(false); }
        Self::flexible(&*message);
        message
    }
    pub fn choices(ui: &Ui, labels: impl IntoIterator<Item = impl AsRef<str>>) -> Popup {
        let popup = Popup::new(ui).items(labels);
        popup.ns_popup_button().setControlSize(NSControlSize::Small);
        popup.ns_popup_button().setFont(Some(&crate::style::Font::body()));
        popup
    }
    fn code_field(field: &NSTextField) {
        field.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(12.0, unsafe { NSFontWeightRegular })));
    }
    pub fn summary(ui: &Ui, text: &str) -> Button {
        let button = Button::new(ui, text).borderless();
        let native = button.ns_button();
        native.setImage(crate::image::Symbol::named("chevron.down").as_deref());
        native.setImagePosition(NSCellImagePosition::ImageTrailing);
        native.setBezelStyle(NSBezelStyle::AccessoryBar);
        native.setBordered(true);
        native.setShowsBorderOnlyWhileMouseInside(true);
        native.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        if let Some(cell) = native.cell() { cell.setLineBreakMode(NSLineBreakMode::ByTruncatingTail); }
        button.tooltip(text);
        button
    }
    pub fn recorder(ui: &Ui, recorder: &Rc<KeyRecorder>) -> impl NativeView {
        let field = recorder.flat_field(ui);
        field.width(148.0);
        field.height(24.0);
        field
    }
    pub fn option(ui: &Ui, label: &str, control: Box<dyn NativeView>) -> HStack {
        control.width(180.0);
        HStack::new(ui).spacing(16.0).push(SecondaryLabel::new(ui, label)).spacer(ui).push(control)
    }
    pub fn popover(ui: &Ui, title: &str, control: impl NativeView, wide: bool) -> VStack {
        let content = VStack::new(ui).spacing(12.0).insets(uniform_insets(16.0))
            .push(SectionTitle::new(ui, title)).push(control);
        content.width(if wide { 420.0 } else { 340.0 });
        content
    }
    fn table<T: 'static>(table: &Table<T>) {
        let native = table.ns_table_view();
        native.setHeaderView(None);
        native.setStyle(NSTableViewStyle::Plain);
        native.setFloatsGroupRows(false);
        native.setUsesAutomaticRowHeights(true);
        native.setIntercellSpacing(crate::CGSize::ZERO);
        native.setBackgroundColor(&NSColor::clearColor());
        native.setSelectionHighlightStyle(NSTableViewSelectionHighlightStyle::None);
        table.ns_scroll_view().setDrawsBackground(false);
        table.hugging_priority(1.0, NSLayoutConstraintOrientation::Vertical);
    }
    pub fn section(ui: &Ui, title: &str) -> HStack {
        HStack::new(ui).insets(Insets { top: 14.0, bottom: 8.0, left: 32.0, right: 32.0 })
            .push(SectionTitle::new(ui, title))
    }
    pub fn pane_item(ui: &Ui, content: impl NativeView) -> View {
        View::new(ui).content(content, Insets { top: 0.0, left: 20.0, bottom: 0.0, right: 20.0 })
    }
    pub fn filters(ui: &Ui, search: SearchField, tabs: impl NativeView) -> HStack {
        search.hugging_priority(1.0, NSLayoutConstraintOrientation::Horizontal);
        HStack::new(ui).spacing(12.0).insets(Insets { top: 4.0, left: 20.0, bottom: 8.0, right: 20.0 })
            .push(search).push(tabs)
    }
    pub fn toolbar(ui: &Ui, add: Button, management: Popup) -> crate::navigation::HeaderControls {
        let icon = crate::image::Symbol::header("ellipsis");
        let native = management.ns_popup_button();
        native.setImage(icon.as_deref());
        native.setImagePosition(NSCellImagePosition::ImageOnly);
        let display = crate::menu::MenuItem::new(ui, "");
        display.ns_menu_item().setImage(icon.as_deref());
        if let Some(cell) = native.cell().and_then(|cell| cell.downcast::<NSPopUpButtonCell>().ok()) {
            cell.setUsesItemFromMenu(false);
            cell.setMenuItem(Some(display.ns_menu_item()));
        }
        native.setBordered(false);
        native.setAlignment(NSTextAlignment::Center);
        management.width(36.0);
        management.height(36.0);
        let management = crate::view::GlassEffectView::new(ui, management).corner_radius(18.0).interactive(true);
        add.ns_button().setBordered(false);
        add.ns_button().setImageHugsTitle(true);
        add.ns_button().setContentTintColor(Some(&NSColor::labelColor()));
        add.width(36.0);
        add.height(36.0);
        let add = crate::view::GlassEffectView::new(ui, add).corner_radius(18.0).interactive(true);
        let controls = HStack::new(ui).spacing(8.0).push(add).push(management);
        controls.height(36.0);
        let controls = crate::view::GlassEffectContainerView::new(ui, controls).spacing(4.0);
        crate::navigation::HeaderControls::content(ui, "Shortcut controls", controls)
    }
}

pub struct ShortcutRow {
    pub view: Rc<View>,
    pub cell: View,
    pub divider: Rc<HStack>,
}
impl ShortcutRow {
    pub fn new(ui: &Ui, name: Rc<Label>, parameter: Option<Box<dyn NativeView>>, recorder: &Rc<KeyRecorder>, another: Button,
        parameter_error: impl NativeView, error: impl NativeView, conflicts: impl NativeView) -> Self {
        ShortcutPresentation::flexible(&*name);
        another.ns_button().setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        another.width(14.0); another.height(20.0);
        let action = another.ns_view().retain();
        let slot = View::new(ui).content(another, uniform_insets(0.0));
        slot.width(14.0); slot.height(20.0);
        let controls = crate::view::HoverReveal::new(ui,
            HStack::new(ui).spacing(6.0).push(slot).push(ShortcutPresentation::recorder(ui, recorder)), action);
        let line = HStack::new(ui).spacing(12.0)
            .insets(Insets { top: 8.0, left: 12.0, bottom: 8.0, right: 12.0 })
            .push(name).spacer(ui);
        if let Some(parameter) = parameter {
            parameter.compression_resistance(250.0, NSLayoutConstraintOrientation::Horizontal);
            parameter.max_width(180.0);
            line.add(parameter);
        }
        line.add(controls); line.min_height(40.0);
        ShortcutPresentation::flexible(&parameter_error); ShortcutPresentation::flexible(&error);
        let errors = VStack::new(ui).spacing(2.0)
            .insets(Insets { top: 0.0, left: 12.0, bottom: 0.0, right: 12.0 }).push(parameter_error).push(error);
        let divider = Rc::new(HStack::new(ui).insets(Insets { top: 2.0, left: 12.0, bottom: 2.0, right: 12.0 })
            .push(crate::layout::Divider::new(ui)));
        let content = VStack::new(ui).spacing(0.0).push(divider.clone()).push(line).push(errors).push(conflicts);
        let view = Rc::new(View::settings_card(ui).content(content, uniform_insets(0.0)));
        let cell = ShortcutPresentation::pane_item(ui, view.clone());
        Self { view, cell, divider }
    }
}

pub enum FormValue {
    Text { value: String, placeholder: String },
    Path(String),
    Boolean(bool),
    Choice { labels: Vec<String>, selected: usize },
}
pub struct FormField { pub label: String, pub value: FormValue }
pub enum FormEdit { Text(String), Boolean(bool), Choice(usize) }

/// Labeled native controls for structured command arguments. Data parsing stays in the app.
pub struct CommandForm(VStack);
impl CommandForm {
    pub fn new(ui: &Ui, fields: Vec<FormField>, draft: bool, changed: impl Fn(usize, FormEdit) + 'static) -> Self {
        let changed: Rc<dyn Fn(usize, FormEdit)> = Rc::new(changed);
        let form = VStack::new(ui).spacing(10.0);
        for (index, field) in fields.into_iter().enumerate() {
            let notify = changed.clone();
            let wide = matches!(&field.value, FormValue::Path(_));
            let control: Box<dyn NativeView> = match field.value {
                FormValue::Text { value, placeholder } => {
                    let field = ShortcutPresentation::text_field(ui, &value, &placeholder, false);
                    let changed = move |value| notify(index, FormEdit::Text(value));
                    Box::new(if draft { field.on_change(changed) } else { field.on_commit(changed) })
                }
                FormValue::Path(value) => {
                    let field = ShortcutPresentation::text_field(ui, &value, "File path", false);
                    let changed = move |value| notify(index, FormEdit::Text(value));
                    Box::new(if draft { field.on_change(changed) } else { field.on_commit(changed) })
                }
                FormValue::Boolean(value) => Box::new(crate::controls::Checkbox::new(ui, "").value(value)
                    .on_change(move |value| notify(index, FormEdit::Boolean(value)))),
                FormValue::Choice { labels, selected } => {
                    let popup = ShortcutPresentation::choices(ui, labels);
                    popup.set_selected(selected);
                    Box::new(popup.on_change(move |index_value| notify(index, FormEdit::Choice(index_value))))
                }
            };
            control.accessibility_label(&field.label);
            if wide {
                form.add(VStack::new(ui).spacing(6.0).push(Caption::new(ui, &field.label)).push(control));
                continue;
            }
            control.width(180.0);
            let label = SecondaryLabel::new(ui, &field.label);
            label.compression_resistance(750.0, NSLayoutConstraintOrientation::Horizontal);
            form.add(HStack::new(ui).spacing(12.0).push(label).spacer(ui).push(control));
        }
        Self(form)
    }
}
impl NativeView for CommandForm { fn ns_view(&self) -> &NSView { self.0.ns_view() } }

/// Raw configuration is a deliberate advanced editor, never a clipped single-line field.
pub struct AdvancedCommandEditor(crate::layout::Disclosure);
impl AdvancedCommandEditor {
    pub fn new(ui: &Ui, value: &str, draft: bool, apply: impl Fn(String) + 'static) -> Self {
        let apply: Rc<dyn Fn(String)> = Rc::new(apply);
        let text = Rc::new(std::cell::RefCell::new(value.to_string()));
        let (edited, changed) = (text.clone(), apply.clone());
        let input = crate::input::TextArea::new(ui).on_change(move |value| {
            *edited.borrow_mut() = value.clone();
            if draft { changed(value); }
        });
        input.set_value(value);
        input.height(140.0);
        input.accessibility_label("Command configuration");
        input.ns_text_view().setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(12.0, unsafe { NSFontWeightRegular })));
        let content = VStack::new(ui).spacing(8.0).push(input);
        if !draft {
            content.add(HStack::new(ui).spacer(ui).push(Button::new(ui, "Apply").push()
                .on_click(move || apply(text.borrow().clone()))));
        }
        Self(crate::layout::Disclosure::new(ui, "Advanced configuration", content))
    }
}
impl NativeView for AdvancedCommandEditor { fn ns_view(&self) -> &NSView { self.0.ns_view() } }
