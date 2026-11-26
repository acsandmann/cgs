use std::cell::RefCell;
use std::rc::Rc;

use objc2::Message;
use objc2::rc::Weak;
use objc2_app_kit::{NSGridCellPlacement, NSGridView, NSLayoutConstraintOrientation, NSView};

use crate::layout::{Divider, Grid, GroupBox, HStack, Spacer, VStack};
use crate::settings::SettingsRow;
use crate::text::{Caption, SectionTitle, SubsectionTitle};
use crate::view::{Metrics, NativeView};
use crate::{Insets, Ui};

/// Compact preference content; AppKit controls provide their own appearance.
pub struct SettingsGroup {
    surface: Option<GroupBox>,
    grid: Rc<Grid>,
    rows: Vec<SettingsRow>,
    dividers: Vec<Divider>,
    lines: Vec<HStack>,
}
impl SettingsGroup {
    pub fn new(ui: &Ui) -> Self {
        let grid = Rc::new(Grid::new(ui).spacing(11.0, 24.0));
        let surface = GroupBox::with_insets(ui, grid.clone(), Insets {
            top: 11.5,
            left: 10.0,
            bottom: 9.0,
            right: 10.0,
        });
        Self {
            surface: Some(surface),
            grid,
            rows: Vec::new(),
            dividers: Vec::new(),
            lines: Vec::new(),
        }
    }

    pub fn plain(ui: &Ui) -> Self {
        Self {
            surface: None,
            grid: Rc::new(Grid::new(ui).spacing(8.0, 20.0)),
            rows: Vec::new(),
            dividers: Vec::new(),
            lines: Vec::new(),
        }
    }

    pub fn row(mut self, row: impl Into<SettingsRow>) -> Self {
        let row = row.into();
        let grid = self.grid.ns_grid_view();
        if self.surface.is_some() && !self.rows.is_empty() {
            let divider = Divider::new(&row.ui);
            let separator = grid.addRowWithViews(&objc2_foundation::NSArray::from_slice(&[
                divider.ns_view(),
                &objc2_app_kit::NSGridCell::emptyContentView(row.ui.mtm()),
            ]));
            separator.mergeCellsInRange(objc2_foundation::NSRange::new(0, 2));
            grid.cellAtColumnIndex_rowIndex(0, grid.numberOfRows() - 1)
                .setXPlacement(objc2_app_kit::NSGridCellPlacement::Fill);
            self.dividers.push(divider);
        }
        if row.wide {
            // One merged cell, so other rows' labels don't set this row's column split.
            let [label, value] = row.take_form_cells();
            let line = HStack::new(&row.ui)
                .spacing(grid.columnSpacing())
                .push(label.retain())
                .push(Spacer::new(&row.ui))
                .push(value.retain());
            let cells = grid.addRowWithViews(&objc2_foundation::NSArray::from_slice(&[
                line.ns_view(),
                &objc2_app_kit::NSGridCell::emptyContentView(row.ui.mtm()),
            ]));
            cells.mergeCellsInRange(objc2_foundation::NSRange::new(0, 2));
            grid.cellAtColumnIndex_rowIndex(0, grid.numberOfRows() - 1)
                .setXPlacement(objc2_app_kit::NSGridCellPlacement::Fill);
            self.lines.push(line);
        } else {
            let cells = row.take_form_cells();
            let added = grid.addRowWithViews(&objc2_foundation::NSArray::from_slice(&cells));
            if let Some(anchor) = &row.baseline {
                let cell = added.cellAtIndex(0);
                cell.setYPlacement(objc2_app_kit::NSGridCellPlacement::None);
                let baseline = cells[0]
                    .firstBaselineAnchor()
                    .constraintEqualToAnchor(&anchor.firstBaselineAnchor());
                cell.setCustomPlacementConstraints(
                    &objc2_foundation::NSArray::from_retained_slice(&[baseline]),
                );
            }
            if self.surface.is_some() {
                // Rows with short controls, such as switches, keep the card's even rhythm.
                let height = cells.iter().map(|cell| cell.fittingSize().height).fold(0.0, f64::max);
                let padding = ((Metrics::ROW_MIN_HEIGHT - height) / 2.0).max(0.0);
                added.setTopPadding(padding);
                added.setBottomPadding(padding);
            }
        }
        if self.surface.is_some() && !row.wide {
            // Prefer enough room for explanations before compressing them to a
            // narrow strip. Yield to wide native controls and text field content.
            let label_width = row
                .label
                .ns_view()
                .widthAnchor()
                .constraintGreaterThanOrEqualToAnchor_multiplier(&grid.widthAnchor(), 0.5);
            label_width.setPriority(crate::text::LABEL_PREFERRED_WIDTH);
            label_width.setActive(true);
        }
        grid.columnAtIndex(0).setXPlacement(objc2_app_kit::NSGridCellPlacement::Fill);
        grid.columnAtIndex(1).setXPlacement(if self.surface.is_none() {
            objc2_app_kit::NSGridCellPlacement::Leading
        } else {
            objc2_app_kit::NSGridCellPlacement::Trailing
        });
        self.rows.push(row);
        self
    }
}
impl NativeView for SettingsGroup {
    fn ns_view(&self) -> &NSView {
        self.surface.as_ref().map_or(self.grid.ns_view(), NativeView::ns_view)
    }
}

/// Share a title width across a set of preference forms.
pub struct FormAlignment {
    minimum: f64,
    titles: RefCell<Vec<Weak<NSView>>>,
    grids: RefCell<Vec<Weak<NSGridView>>>,
}

impl FormAlignment {
    pub fn new(minimum: f64) -> Rc<Self> {
        Rc::new(Self { minimum, titles: RefCell::default(), grids: RefCell::default() })
    }

    /// Measure just the titles once the forms have been built.
    pub fn apply(&self) {
        let mut titles = self.titles.borrow_mut();
        titles.retain(|title| title.load().is_some());
        let width = titles.iter().filter_map(Weak::load)
            .map(|view| view.fittingSize().width)
            .fold(self.minimum, f64::max);
        self.grids.borrow_mut().retain(|grid| {
            if let Some(grid) = grid.load() {
                grid.columnAtIndex(0).setWidth(width);
                true
            } else {
                false
            }
        });
    }
}

pub struct Section {
    ui: Ui,
    stack: VStack,
    header: Rc<VStack>,
    header_container: Rc<HStack>,
    group: Option<SettingsGroup>,
    plain: bool,
    trailing_controls: bool,
    alignment: Option<Rc<FormAlignment>>,
}
impl Section {
    pub fn new(ui: &Ui, title: &str) -> Self {
        let header = Rc::new(VStack::new(ui).spacing(2.0));
        if !title.is_empty() {
            header.add(SectionTitle::new(ui, title));
        }
        let header_container = Rc::new(
            HStack::new(ui)
                .insets(Insets {
                    top: 0.0,
                    left: 10.0,
                    bottom: 0.0,
                    right: 0.0,
                })
                .push(header.clone()),
        );
        header_container.set_hidden(title.is_empty());
        let stack = VStack::new(ui).spacing(8.0).push(header_container.clone());
        Self {
            ui: *ui,
            stack,
            header,
            header_container,
            group: None,
            plain: false,
            trailing_controls: true,
            alignment: None,
        }
    }

    /// An aligned native preferences form without a grouped card or row dividers.
    pub fn form(ui: &Ui, title: &str) -> Self {
        Self {
            plain: true,
            trailing_controls: false,
            ..Self::new(ui, title)
        }
    }

    /// Register the form with a shared title column. Call before adding rows.
    pub fn aligned(mut self, alignment: &Rc<FormAlignment>) -> Self {
        self.alignment = Some(alignment.clone());
        self
    }

    /// Align natural-width controls at the trailing edge of a plain preferences form.
    pub fn trailing_controls(mut self) -> Self {
        self.trailing_controls = true;
        if let Some(group) = &self.group {
            group
                .grid
                .ns_grid_view()
                .columnAtIndex(1)
                .setXPlacement(objc2_app_kit::NSGridCellPlacement::Trailing);
        }
        self
    }

    pub fn subsection(ui: &Ui, title: &str) -> Self {
        Self::new(ui, "").content(SubsectionTitle::new(ui, title))
    }

    pub fn description(self, text: &str) -> Self {
        self.header_container.set_hidden(false);
        self.header.add(Caption::new(&self.ui, text));
        self
    }

    pub fn row(mut self, row: impl Into<SettingsRow>) -> Self {
        let group = self.group.take().unwrap_or_else(|| {
            let group = if self.plain {
                SettingsGroup::plain(&self.ui)
            } else {
                SettingsGroup::new(&self.ui)
            };
            if let Some(alignment) = &self.alignment {
                alignment.grids.borrow_mut().push(Weak::new(group.grid.ns_grid_view()));
            }
            self.stack.ns_stack_view().addArrangedSubview(group.ns_view());
            group
                .ns_view()
                .widthAnchor()
                .constraintEqualToAnchor(&self.stack.ns_view().widthAnchor())
                .setActive(true);
            group
        });
        let row = row.into();
        let placement = if let Some(alignment) = &self.alignment {
            alignment.titles.borrow_mut().push(Weak::new(row.title_label.ns_view()));
            if let Some(slider) = &row.fill_slider {
                slider.widthAnchor().constraintGreaterThanOrEqualToConstant(140.0).setActive(true);
                slider.setContentHuggingPriority_forOrientation(
                    1.0, NSLayoutConstraintOrientation::Horizontal,
                );
                Some(NSGridCellPlacement::Fill)
            } else {
                Some(NSGridCellPlacement::Trailing)
            }
        } else {
            None
        };
        let group = group.row(row);
        if let Some(placement) = placement {
            let grid = group.grid.ns_grid_view();
            grid.cellAtColumnIndex_rowIndex(1, grid.numberOfRows() - 1).setXPlacement(placement);
        }
        if self.trailing_controls {
            group
                .grid
                .ns_grid_view()
                .columnAtIndex(1)
                .setXPlacement(objc2_app_kit::NSGridCellPlacement::Trailing);
        }
        self.group = Some(group);
        self
    }

    pub fn footer(self, content: impl NativeView) -> Self {
        self.stack.add(content);
        self
    }

    pub fn content(self, content: impl NativeView) -> Self {
        self.stack.add(content);
        self
    }
}
impl NativeView for Section {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}

/// User edits to a native editable string list.
pub enum StringListEdit {
    Add(String),
    Remove(usize, String),
}

/// Build the add field and native list that make up a settings string editor.
pub fn editable_strings(
    ui: &Ui,
    edit: impl Fn(StringListEdit) + 'static,
) -> (HStack, Rc<crate::collection::EditableList<String>>) {
    use crate::collection::EditableList;
    use crate::controls::Button;
    use crate::input::TextField;

    let edit = Rc::new(edit);
    let field = Rc::new(TextField::new(ui).placeholder("Add an entry"));
    let input = Rc::downgrade(&field);
    let add_edit = edit.clone();
    let add = Button::new(ui, "Add").on_click(move || {
        if let Some(input) = input.upgrade() {
            let value = input.get_value();
            if !value.trim().is_empty() {
                add_edit(StringListEdit::Add(value));
                input.set_value("");
            }
        }
    });
    let list = Rc::new(
        EditableList::new(ui, String::clone)
            .full_length()
            .on_remove_item(move |index, item| edit(StringListEdit::Remove(index, item))),
    );
    (HStack::new(ui).push(field).push(add), list)
}
