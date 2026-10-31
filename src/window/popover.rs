use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::*;
use objc2_foundation::NSRectEdge;

use crate::Ui;
use crate::bridge::{DelegateBridge, Event};
use crate::view::{NativeView, View};
use crate::window::ViewController;

pub struct Popover {
    native: Retained<NSPopover>,
    controller: ViewController,
    delegate: Option<Retained<DelegateBridge>>,
}
impl Popover {
    pub fn new(ui: &Ui, content: impl NativeView) -> Self {
        let native = NSPopover::new(ui.mtm());
        native.setBehavior(NSPopoverBehavior::Transient);
        let controller = ViewController::new(ui, content);
        native.setContentViewController(Some(controller.ns_view_controller()));
        Self {
            native,
            controller,
            delegate: None,
        }
    }

    pub fn show(&self, view: &impl NativeView) {
        self.native.showRelativeToRect_ofView_preferredEdge(
            view.ns_view().bounds(),
            view.ns_view(),
            NSRectEdge::MaxY,
        );
    }

    /// Refit when an option reveals or hides additional controls.
    pub fn fit_content(&self) {
        let content = self.controller.ns_view_controller().view();
        content.layoutSubtreeIfNeeded();
        self.native.setContentSize(content.fittingSize());
    }

    pub fn close(&self) { self.native.close(); }

    pub fn on_close(mut self, mut f: impl FnMut() + 'static) -> Self {
        let bridge = DelegateBridge::new(&Ui::new(self.native.mtm()), move |e, _| {
            if e == Event::PopoverClosed {
                f();
            }
        });
        self.native.setDelegate(Some(ProtocolObject::from_ref(&*bridge)));
        self.delegate = Some(bridge);
        self
    }

    pub fn ns_popover(&self) -> &NSPopover { &self.native }

    pub fn ns_view_controller(&self) -> &NSViewController { self.controller.ns_view_controller() }
}
impl Drop for Popover {
    fn drop(&mut self) {
        self.native.setDelegate(None);
        self.native.close();
        self.native.setContentViewController(None);
    }
}
pub struct HelpPopover(Popover);
impl HelpPopover {
    pub fn new(ui: &Ui, text: &str) -> Self {
        Self::with_content(ui, crate::text::WrappingLabel::new(ui, text))
    }

    pub fn with_content(ui: &Ui, content: impl NativeView) -> Self {
        Self(Popover::new(
            ui,
            View::new(ui).content(content, crate::Insets {
                top: 12.0,
                left: 12.0,
                bottom: 12.0,
                right: 12.0,
            }),
        ))
    }

    pub fn show(&self, view: &impl NativeView) { self.0.show(view); }

    pub fn ns_popover(&self) -> &NSPopover { self.0.ns_popover() }
}
