use std::cell::OnceCell;
use std::rc::Rc;

use objc2::Message;
use objc2::rc::Retained;
use objc2_app_kit::{NSTextField, NSView};

use crate::Ui;
use crate::controls::{Button, Checkbox, InfoButton};
use crate::layout::{Disclosure, Grid, HStack, Spacer, VStack};
use crate::settings::{Validation, ValidationMessage};
use crate::text::{Caption, SecondaryLabel, WrappingLabel};
use crate::view::{NativeControl, NativeView};

pub struct Badge(Caption);
impl Badge {
    pub fn new(ui: &Ui, text: &str) -> Self { Self(Caption::new(ui, text)) }

    pub fn set_text(&self, text: &str) { self.0.set_text(text); }

    pub fn ns_text_field(&self) -> &NSTextField { self.0.ns_text_field() }
}
impl NativeView for Badge {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }
}

pub struct SettingsRow {
    pub(super) ui: Ui,
    title: String,
    grid: OnceCell<Grid>,
    pub(super) label: Rc<dyn NativeView>,
    value: VStack,
    line: OnceCell<Rc<HStack>>,
    control: Box<dyn NativeView>,
    validation: Rc<ValidationMessage>,
    pub(super) wide: bool,
    pub(super) baseline: Option<Retained<NSView>>,
}
impl SettingsRow {
    pub fn new(ui: &Ui, title: &str, control: impl NativeView) -> Self {
        Self::with_validation(ui, title, control, Rc::new(ValidationMessage::new(ui)))
    }

    pub fn with_validation(
        ui: &Ui,
        title: &str,
        control: impl NativeView,
        validation: Rc<ValidationMessage>,
    ) -> Self {
        // Settings cards use compact native controls, including when wrapped
        // in a composite control.
        fn compact_controls(view: &NSView) {
            if let Some(switch) = view.downcast_ref::<objc2_app_kit::NSSwitch>() {
                switch.setControlSize(objc2_app_kit::NSControlSize::Mini);
            }
            if let Some(popup) = view.downcast_ref::<crate::controls::SettingsPopup>() {
                popup.settings_style();
            }
            for child in view.subviews().iter() {
                compact_controls(&child);
            }
        }
        compact_controls(control.ns_view());
        let label = Rc::new(WrappingLabel::new(ui, title));
        control.accessibility_label(title);
        let value = VStack::new(ui)
            .spacing(4.0)
            .push(control.ns_view().retain())
            .push(validation.clone());
        Self {
            ui: *ui,
            title: title.to_owned(),
            grid: OnceCell::new(),
            label,
            value,
            line: OnceCell::new(),
            control: Box::new(control),
            validation,
            wide: false,
            baseline: None,
        }
    }

    /// Line the label up with `view`’s first baseline, rather than centering it on the
    /// whole control, such as when a caption or other lines sit beneath the control.
    pub fn label_baseline(mut self, view: &NSView) -> Self {
        self.baseline = Some(view.retain());
        self
    }

    /// Let the control extend into the label column, past the group's shared
    /// column widths, so long values such as paths show whole. The label keeps
    /// only the width its text needs.
    pub fn wide(mut self) -> Self {
        self.wide = true;
        self
    }

    /// Add a discoverable description beside the label, without expanding the row.
    pub fn help(mut self, text: &str) -> Self {
        if !text.trim().is_empty() {
            self.label = Rc::new(
                HStack::new(&self.ui)
                    .spacing(6.0)
                    .push(self.label)
                    .push(InfoButton::new(&self.ui, &self.title, text))
                    .push(Spacer::new(&self.ui)),
            );
        }
        self
    }

    /// Explain the setting in a native help tag on its label and control.
    pub fn tooltip(self, text: &str) -> Self {
        if !text.trim().is_empty() {
            let text = objc2_foundation::NSString::from_str(text);
            self.label.ns_view().setToolTip(Some(&text));
            self.control.ns_view().setToolTip(Some(&text));
        }
        self
    }

    pub fn description(self, text: &str) -> Self {
        if text.trim().is_empty() {
            return self;
        }
        let caption = Caption::new(&self.ui, text);
        self.description_view(caption)
    }

    /// Show `view` below the label, such as a caption its owner keeps current.
    pub fn description_view(mut self, view: impl NativeView) -> Self {
        self.label = Rc::new(VStack::new(&self.ui).spacing(4.0).push(self.label).push(view));
        self
    }

    pub fn suffix(self, text: &str) -> Self {
        let line = self.line.get_or_init(|| {
            self.value.ns_stack_view().removeArrangedSubview(self.control.ns_view());
            self.control.ns_view().removeFromSuperview();
            let line =
                Rc::new(HStack::new(&self.ui).spacing(6.0).push(self.control.ns_view().retain()));
            self.value.ns_stack_view().insertArrangedSubview_atIndex(line.ns_view(), 0);
            line.ns_view()
                .widthAnchor()
                .constraintEqualToAnchor(&self.value.ns_view().widthAnchor())
                .setActive(true);
            line
        });
        line.add(SecondaryLabel::new(&self.ui, text));
        self
    }

    pub fn set_validation(&self, value: &Validation) { self.validation.set_validation(value); }

    pub fn control_view(&self) -> &NSView { self.control.ns_view() }

    /// A handle for showing or hiding this row, with its divider, after it joins a section.
    pub fn visibility(&self) -> RowVisibility {
        RowVisibility(objc2::rc::Weak::new(self.label.ns_view()))
    }

    /// A weak handle to the row's control, for enabling it from sync callbacks.
    pub fn control(&self) -> crate::view::WeakView {
        crate::view::WeakView(objc2::rc::Weak::new(self.control_view()))
    }

    pub(super) fn take_form_cells(&self) -> [&NSView; 2] {
        if let Some(grid) = self.grid.get() {
            if grid.ns_grid_view().numberOfRows() > 0 {
                grid.ns_grid_view().removeRowAtIndex(0);
            }
        }
        [self.label.ns_view(), self.value.ns_view()]
    }
}
impl NativeView for SettingsRow {
    fn ns_view(&self) -> &NSView {
        self.grid
            .get_or_init(|| {
                let grid = Grid::new(&self.ui).spacing(10.0, 20.0);
                grid.ns_grid_view().addRowWithViews(&objc2_foundation::NSArray::from_slice(&[
                    self.label.ns_view(),
                    self.value.ns_view(),
                ]));
                grid.ns_grid_view()
                    .columnAtIndex(0)
                    .setXPlacement(objc2_app_kit::NSGridCellPlacement::Fill);
                grid.ns_grid_view()
                    .columnAtIndex(1)
                    .setXPlacement(objc2_app_kit::NSGridCellPlacement::Trailing);
                grid
            })
            .ns_view()
    }
}
macro_rules! setting_row {
    ($name:ident) => {
        pub struct $name(SettingsRow);
        impl $name {
            pub fn new(ui: &Ui, title: &str, control: impl NativeControl) -> Self {
                Self(SettingsRow::new(ui, title, control))
            }

            pub fn description(self, text: &str) -> Self { Self(self.0.description(text)) }

            pub fn help(self, text: &str) -> Self { Self(self.0.help(text)) }

            pub fn suffix(self, text: &str) -> Self { Self(self.0.suffix(text)) }

            pub fn set_validation(&self, value: &Validation) { self.0.set_validation(value); }

            pub fn control_view(&self) -> &NSView { self.0.control_view() }
        }
        impl NativeView for $name {
            fn ns_view(&self) -> &NSView { self.0.ns_view() }
        }
        impl From<$name> for SettingsRow {
            fn from(row: $name) -> Self { row.0 }
        }
    };
}
/// Shows or hides one settings row in place; a grouped card hides the row's divider too.
#[derive(Clone)]
pub struct RowVisibility(objc2::rc::Weak<NSView>);
impl RowVisibility {
    pub fn set_hidden(&self, hidden: bool) {
        let Some(label) = self.0.load() else { return };
        let Some(grid) = (unsafe { label.superview() })
            .and_then(|view| view.downcast::<objc2_app_kit::NSGridView>().ok())
        else {
            label.setHidden(hidden);
            return;
        };
        let Some(row) = grid.cellForView(&label).and_then(|cell| cell.row()) else {
            return;
        };
        let index = grid.indexOfRow(&row);
        row.setHidden(hidden);
        // A divider sits above every row but the first; the first row's follows it.
        let divider = if index > 0 { index - 1 } else { index + 1 };
        if divider < grid.numberOfRows()
            && grid
                .cellAtColumnIndex_rowIndex(0, divider)
                .contentView()
                .is_some_and(|view| view.downcast_ref::<objc2_app_kit::NSBox>().is_some())
        {
            grid.rowAtIndex(divider).setHidden(hidden);
        }
    }
}

setting_row!(SwitchRow);
setting_row!(PopupRow);
setting_row!(TextRow);
setting_row!(NumberRow);
setting_row!(SliderRow);
setting_row!(ColorRow);
setting_row!(ButtonRow);
pub type DisclosureRow = Disclosure;

pub struct ResettableRow {
    stack: HStack,
    reset: Button,
    row: Box<dyn NativeView>,
}
impl ResettableRow {
    pub fn new(ui: &Ui, row: impl NativeView) -> Self {
        let reset = Button::new(ui, "Reset");
        let stack = HStack::new(ui);
        stack.ns_stack_view().addArrangedSubview(row.ns_view());
        stack.ns_stack_view().addArrangedSubview(reset.ns_view());
        Self {
            stack,
            reset,
            row: Box::new(row),
        }
    }

    pub fn on_reset(mut self, f: impl FnMut() + 'static) -> Self {
        self.reset = self.reset.on_click(f);
        self
    }

    pub fn set_reset_enabled(&self, value: bool) { self.reset.set_enabled(value); }

    pub fn row_view(&self) -> &NSView { self.row.ns_view() }
}
impl NativeView for ResettableRow {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}
pub struct OverrideRow {
    stack: VStack,
    toggle: Checkbox,
    row: Box<dyn NativeView>,
    inherited: SecondaryLabel,
}
impl OverrideRow {
    pub fn new(ui: &Ui, row: impl NativeView, inherited: &str) -> Self {
        let label = SecondaryLabel::new(ui, inherited);
        let weak = objc2::rc::Weak::new(row.ns_view());
        row.set_hidden(true);
        let inherited_label = objc2::rc::Weak::new(label.ns_view());
        let toggle = Checkbox::new(ui, "Use custom value").on_change(move |v| {
            if let Some(row) = weak.load() {
                row.setHidden(!v);
            }
            if let Some(label) = inherited_label.load() {
                label.setHidden(v);
            }
        });
        let stack = VStack::new(ui);
        for view in [toggle.ns_view(), label.ns_view(), row.ns_view()] {
            stack.ns_stack_view().addArrangedSubview(view);
        }
        Self {
            stack,
            toggle,
            row: Box::new(row),
            inherited: label,
        }
    }

    pub fn set_overridden(&self, value: bool) {
        self.toggle.set_value(value);
        self.row.set_hidden(!value);
        self.inherited.set_hidden(value);
    }

    pub fn set_inherited_text(&self, text: &str) { self.inherited.set_text(text); }

    pub fn on_change(mut self, mut f: impl FnMut(bool) + 'static) -> Self {
        let weak = objc2::rc::Weak::new(self.row.ns_view());
        let label = objc2::rc::Weak::new(self.inherited.ns_view());
        self.toggle = self.toggle.on_change(move |v| {
            if let Some(row) = weak.load() {
                row.setHidden(!v);
            }
            if let Some(label) = label.load() {
                label.setHidden(v);
            }
            f(v);
        });
        self
    }
}
impl NativeView for OverrideRow {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}
