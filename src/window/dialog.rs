use std::cell::RefCell;
use std::path::{Path, PathBuf};

use block2::RcBlock;
use objc2::rc::{Retained, Weak};
use objc2_app_kit::*;
use objc2_foundation::{NSString, NSURL};

use crate::Ui;
use crate::bridge::callback;

pub struct Alert(Retained<NSAlert>);
impl Alert {
    pub fn new(ui: &Ui, title: &str, message: &str) -> Self {
        let native = NSAlert::new(ui.mtm());
        native.setMessageText(&NSString::from_str(title));
        native.setInformativeText(&NSString::from_str(message));
        Self(native)
    }

    pub fn button(self, title: &str) -> Self {
        self.0.addButtonWithTitle(&NSString::from_str(title));
        self
    }

    pub fn show_sheet(&self, parent: &NSWindow, f: impl FnOnce(isize) + 'static) {
        let f = RefCell::new(Some(f));
        let completion = RcBlock::new(move |response| {
            if let Some(f) = f.borrow_mut().take() {
                callback(|| f(response));
            }
        });
        self.0.beginSheetModalForWindow_completionHandler(parent, Some(&completion));
    }

    /// Run as an app-modal alert, returning the chosen button's response.
    pub fn run_modal(&self) -> isize { self.0.runModal() }

    pub fn ns_alert(&self) -> &NSAlert { &self.0 }
}
fn path_url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
}
fn url_path(url: Option<Retained<NSURL>>) -> Option<PathBuf> {
    url?.path().map(|s| PathBuf::from(s.to_string()))
}
macro_rules! panel {
    ($wrapper:ident,$native:ident,$ctor:ident,$accessor:ident) => {
        pub struct $wrapper(Retained<$native>);
        impl $wrapper {
            pub fn new(ui: &Ui) -> Self { Self($native::$ctor(ui.mtm())) }

            pub fn directory(self, path: &Path) -> Self {
                self.0.setDirectoryURL(Some(&path_url(path)));
                self
            }

            pub fn choose(&self) -> Option<PathBuf> {
                if self.0.runModal() == NSModalResponseOK {
                    url_path(self.0.URL())
                } else {
                    None
                }
            }

            pub fn show_sheet(&self, parent: &NSWindow, f: impl FnOnce(Option<PathBuf>) + 'static) {
                let f = RefCell::new(Some(f));
                let panel = Weak::new(&*self.0);
                let completion = RcBlock::new(move |response| {
                    if let Some(f) = f.borrow_mut().take() {
                        let path = if response == NSModalResponseOK {
                            panel.load().and_then(|panel| url_path(panel.URL()))
                        } else {
                            None
                        };
                        callback(|| f(path));
                    }
                });
                self.0.beginSheetModalForWindow_completionHandler(parent, &completion);
            }

            pub fn $accessor(&self) -> &$native { &self.0 }
        }
    };
}
panel!(OpenPanel, NSOpenPanel, openPanel, ns_open_panel);
panel!(SavePanel, NSSavePanel, savePanel, ns_save_panel);
impl OpenPanel {
    pub fn directories(self) -> Self {
        self.0.setCanChooseDirectories(true);
        self.0.setCanChooseFiles(false);
        self
    }

    pub fn files(self) -> Self {
        self.0.setCanChooseFiles(true);
        self.0.setCanChooseDirectories(false);
        self
    }
}
impl SavePanel {
    pub fn filename(self, name: &str) -> Self {
        self.0.setNameFieldStringValue(&NSString::from_str(name));
        self
    }
}
