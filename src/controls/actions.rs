use objc2::Message;
use objc2_app_kit::*;

use crate::Ui;
use crate::controls::{Button, IconButton};
use crate::layout::HStack;
use crate::view::{NativeControl, NativeView};

pub struct AddRemoveControl {
    stack: HStack,
    add: IconButton,
    remove: IconButton,
}
impl AddRemoveControl {
    pub fn new(ui: &Ui) -> Self {
        let add = IconButton::new(ui, "plus", "Add");
        let remove = IconButton::new(ui, "minus", "Remove");
        for button in [&add, &remove] {
            button.ns_button().setBordered(false);
            button.width(28.0);
            button.height(24.0);
        }
        let divider = NSBox::new(ui.mtm());
        divider.setBoxType(NSBoxType::Separator);
        divider.widthAnchor().constraintEqualToConstant(1.0).setActive(true);
        divider.heightAnchor().constraintEqualToConstant(16.0).setActive(true);
        let stack = HStack::new(ui)
            .spacing(0.0)
            .push(add.ns_view().retain())
            .push(divider.into_super())
            .push(remove.ns_view().retain());
        Self { stack, add, remove }
    }

    pub fn on_add(mut self, f: impl FnMut() + 'static) -> Self {
        self.add = self.add.on_click(f);
        self
    }

    pub fn on_remove(mut self, f: impl FnMut() + 'static) -> Self {
        self.remove = self.remove.on_click(f);
        self
    }

    pub fn set_remove_enabled(&self, value: bool) { self.remove.set_enabled(value); }

    pub(crate) fn show_add(&self, visible: bool) {
        self.add.set_hidden(!visible);
        self.stack
            .ns_stack_view()
            .arrangedSubviews()
            .objectAtIndex(1)
            .setHidden(!visible);
    }

    pub(crate) fn remove_button(&self) -> &NSButton { self.remove.ns_button() }
}
impl NativeView for AddRemoveControl {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}

/// Native trailing sheet actions. Cancel is visible only while the draft differs
/// from its initial value; clients report draft changes without managing buttons.
pub struct SheetActions {
    row: HStack,
    done: std::rc::Rc<Button>,
    cancel: std::rc::Rc<Button>,
}
impl SheetActions {
    pub fn new(ui: &Ui) -> Self {
        let done = std::rc::Rc::new(Button::new(ui, "Done").key_equivalent("\r"));
        let cancel = std::rc::Rc::new(Button::new(ui, "Cancel").key_equivalent("\u{1b}"));
        cancel.set_hidden(true);
        cancel.identifier("cgs.sheet-cancel");
        done.identifier("cgs.sheet-primary");
        let row = HStack::new(ui).spacing(8.0).spacer(ui).push(cancel.clone()).push(done.clone());
        Self { row, done, cancel }
    }

    pub fn set_changed(&self, changed: bool) { self.cancel.set_hidden(!changed); }

    pub fn set_primary_title(&self, title: &str) { self.done.set_title(title); }

    pub fn set_on_done(&self, mut f: impl FnMut() + 'static) { self.done.target.set(move |_| f()); }

    pub fn set_on_cancel(&self, mut f: impl FnMut() + 'static) {
        self.cancel.target.set(move |_| f());
    }
}
impl NativeView for SheetActions {
    fn ns_view(&self) -> &NSView { self.row.ns_view() }
}
