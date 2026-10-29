use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use objc2::MainThreadOnly;
use objc2::rc::Weak;
use objc2_app_kit::*;
use objc2_foundation::NSString;

use crate::Ui;
use crate::view::NativeView;
use crate::window::OpenPanel;

pub struct PathField {
    stack: crate::layout::HStack,
    field: crate::input::TextField,
    choose: crate::controls::Button,
    directories: Rc<Cell<bool>>,
}
impl PathField {
    pub fn new(ui: &Ui) -> Self {
        let field = crate::input::TextField::new(ui);
        field.min_width(112.0);
        let weak = Weak::new(field.ns_text_field());
        let ui_copy = *ui;
        let directories = Rc::new(Cell::new(false));
        let directory_mode = directories.clone();
        let choose = crate::controls::Button::new(ui, "Choose…").push().on_click(move || {
            let panel = OpenPanel::new(&ui_copy);
            let panel = if directory_mode.get() {
                panel.directories()
            } else {
                panel
            };
            if let Some(path) = panel.choose() {
                if let Some(field) = weak.load() {
                    field.setStringValue(&NSString::from_str(&path.to_string_lossy()));
                }
            }
        });
        let stack = crate::layout::HStack::new(ui);
        stack.ns_stack_view().addArrangedSubview(field.ns_view());
        stack.ns_stack_view().addArrangedSubview(choose.ns_view());
        Self {
            stack,
            field,
            choose,
            directories,
        }
    }

    pub fn directories(self) -> Self {
        self.directories.set(true);
        self
    }

    pub fn set_value(&self, path: &Path) {
        let path = path.to_string_lossy();
        self.field.set_value(&path);
        self.field.tooltip(&path);
    }

    pub fn on_change(mut self, mut f: impl FnMut(PathBuf) + 'static) -> Self {
        use std::rc::Rc;
        let cb = Rc::new(RefCell::new(move |path| f(path)));
        let c = cb.clone();
        self.field = self.field.on_commit(move |s| (c.borrow_mut())(PathBuf::from(s)));
        let weak = Weak::new(self.field.ns_text_field());
        let ui = Ui::new(self.field.ns_text_field().mtm());
        let directories = self.directories.clone();
        self.choose = self.choose.on_click(move || {
            if let Some(field) = weak.load() {
                let cb = cb.clone();
                let weak = Weak::new(&*field);
                let apply = move |path: Option<PathBuf>| {
                    if let Some(path) = path {
                        if let Some(field) = weak.load() {
                            field.setStringValue(&NSString::from_str(&path.to_string_lossy()));
                        }
                        (cb.borrow_mut())(path);
                    }
                };
                let panel = OpenPanel::new(&ui);
                let panel = if directories.get() {
                    panel.directories()
                } else {
                    panel
                };
                if let Some(window) = field.window() {
                    panel.show_sheet(&window, apply);
                } else {
                    apply(panel.choose());
                }
            }
        });
        self
    }

    pub fn ns_text_field(&self) -> &NSTextField { self.field.ns_text_field() }
}
impl NativeView for PathField {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}
