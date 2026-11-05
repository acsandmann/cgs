//! A non-activating AppKit host for CGS's native mode selector.
use std::{rc::Rc, sync::mpsc::Sender};

use cgs::{
    CGPoint, CGRect, CGSize, Insets, Ui, layout::VStack, tabs::TabSelector, view::NativeView,
};
use objc2::{MainThreadOnly, define_class, msg_send, rc::Retained};
use objc2_app_kit::{NSBackingStoreType, NSPanel, NSWindowCollectionBehavior, NSWindowStyleMask};
use objc2_core_graphics::{CGDisplayBounds, CGMainDisplayID};
use objc2_foundation::NSString;

use super::{Input, SwitcherLayout};

pub const MODES: [SwitcherLayout; 3] = [
    SwitcherLayout::Carousel,
    SwitcherLayout::Stack,
    SwitcherLayout::Grid,
];

define_class!(
    #[unsafe(super(NSPanel))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsExampleSwitcherModePanel"]
    struct ModePanel;
    impl ModePanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool { false }
        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool { false }
    }
);

pub struct ModeHud {
    panel: Retained<ModePanel>,
    selector: Rc<TabSelector>,
    _content: VStack,
}

impl ModeHud {
    pub fn new(ui: &Ui, tx: Sender<Input>) -> Self {
        let selector = Rc::new(
            TabSelector::choices(
                ui,
                &[
                    ("Carousel", Some("rectangle.stack")),
                    ("Stack", Some("square.stack.3d.up")),
                    ("Grid", Some("square.grid.2x2")),
                ],
            )
            .on_change(move |index| {
                if let Some(mode) = MODES.get(index) {
                    let _ = tx.send(Input::Layout(*mode));
                }
            }),
        );
        selector.accessibility_label("Window switcher layout");
        let content = VStack::new(ui)
            .insets(Insets {
                top: 10.,
                left: 12.,
                bottom: 10.,
                right: 12.,
            })
            .push(selector.clone());
        let panel: Retained<ModePanel> = unsafe {
            msg_send![super(ModePanel::alloc(ui.mtm()).set_ivars(())),
                initWithContentRect: CGRect::new(CGPoint::ZERO, CGSize::new(360., 58.)),
                styleMask: NSWindowStyleMask::Titled | NSWindowStyleMask::UtilityWindow | NSWindowStyleMask::NonactivatingPanel,
                backing: NSBackingStoreType::Buffered,
                defer: false
            ]
        };
        unsafe {
            panel.setReleasedWhenClosed(false);
        }
        panel.setTitle(&NSString::from_str("Switcher layout · L / Shift-L"));
        // Switcher previews sit at level 101. The controls must remain clickable above them.
        panel.setLevel(102);
        panel.setHidesOnDeactivate(false);
        panel.setBecomesKeyOnlyIfNeeded(true);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        panel.setContentView(Some(content.ns_view()));
        Self {
            panel,
            selector,
            _content: content,
        }
    }

    pub fn select(&self, layout: SwitcherLayout) {
        self.selector
            .set_selected(MODES.iter().position(|mode| *mode == layout).unwrap());
    }

    pub fn show(&self, bounds: CGRect, layout: SwitcherLayout) {
        self.select(layout);
        // Convert the active CG display's top-left to AppKit's bottom-left coordinates.
        let top = CGDisplayBounds(CGMainDisplayID()).size.height - bounds.origin.y;
        self.panel
            .setFrameTopLeftPoint(CGPoint::new(bounds.origin.x + 20., top - 44.));
        self.panel.orderFrontRegardless();
    }

    pub fn hide(&self) {
        self.panel.orderOut(None);
    }
}

impl Drop for ModeHud {
    fn drop(&mut self) {
        self.panel.orderOut(None);
        self.panel.setContentView(None);
        self.panel.close();
    }
}
