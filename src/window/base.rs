use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use objc2::rc::{Retained, Weak};
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::*;
use objc2_foundation::NSString;

use crate::bridge::{DelegateBridge, Event};
use crate::view::NativeView;
use crate::{CGPoint, CGRect, CGSize, Ui};

// Local toolbar navigation participates in the normal window responder chain.
define_class!(
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsWindow"]
    #[ivars = Cell<bool>]
    struct NativeWindow;
    impl NativeWindow {
        #[unsafe(method(close))]
        fn close_native(&self) {
            if !self.ivars().replace(true) {
                unsafe { let _: () = msg_send![super(self), close]; }
            }
        }
        #[unsafe(method(orderWindow:relativeTo:))]
        fn order(&self, mode: NSWindowOrderingMode, relative: isize) {
            if mode != NSWindowOrderingMode::Out { self.ivars().set(false); }
            unsafe { let _: () = msg_send![super(self), orderWindow: mode, relativeTo: relative]; }
        }
        #[unsafe(method(cgsGoBack:))]
        fn go_back(&self, _sender: Option<&objc2::runtime::AnyObject>) {
            if let Some(item) = self.back_item() {
                if let Some(action) = item.action() {
                unsafe { NSApplication::sharedApplication(self.mtm()).sendAction_to_from(action, item.target().as_deref(), Some(self)); }
                }
            }
        }
    }
    unsafe impl NSUserInterfaceValidations for NativeWindow {
        #[unsafe(method(validateUserInterfaceItem:))]
        fn validate(&self, item: &ProtocolObject<dyn NSValidatedUserInterfaceItem>) -> bool {
            if item.action() == Some(objc2::sel!(cgsGoBack:)) { self.back_item().is_some_and(|item| item.isEnabled()) }
            else { unsafe { msg_send![super(self), validateUserInterfaceItem: item] } }
        }
    }
);
impl NativeWindow {
    fn back_item(&self) -> Option<Retained<NSToolbarItem>> {
        self.toolbar().and_then(|toolbar| {
            toolbar
                .items()
                .iter()
                .find(|item| item.itemIdentifier().to_string() == "cgs.back")
        })
    }
}

pub struct Window {
    pub(crate) native: Retained<NSWindow>,
    pub(crate) content: RefCell<Option<Box<dyn NativeView>>>,
    pub(crate) delegate: Option<Retained<DelegateBridge>>,
    pub(crate) toolbar: OnceCell<Rc<crate::navigation::Toolbar>>,
}
impl Window {
    pub fn new(ui: &Ui) -> Self { Self::with_size(ui, CGSize::new(800.0, 600.0), false) }

    pub(crate) fn with_size(ui: &Ui, size: CGSize, full_size: bool) -> Self {
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable
            | if full_size {
                NSWindowStyleMask::FullSizeContentView
            } else {
                NSWindowStyleMask::empty()
            };
        let native: Retained<NativeWindow> = unsafe {
            msg_send![super(NativeWindow::alloc(ui.mtm()).set_ivars(Cell::new(false))),
                initWithContentRect: CGRect::new(CGPoint::new(0.0, 0.0), size),
                styleMask: style,
                backing: NSBackingStoreType::Buffered,
                defer: false
            ]
        };
        let native = native.into_super();
        unsafe {
            native.setReleasedWhenClosed(false);
        }
        Self {
            native,
            content: RefCell::new(None),
            delegate: None,
            toolbar: OnceCell::new(),
        }
    }

    pub fn ns_window(&self) -> &NSWindow { &self.native }

    pub fn title(self, title: &str) -> Self {
        self.native.setTitle(&NSString::from_str(title));
        self
    }

    pub fn size(self, size: CGSize) -> Self {
        self.native.setContentSize(size);
        self
    }

    pub fn min_size(self, size: CGSize) -> Self {
        self.native.setContentMinSize(size);
        self
    }

    pub fn content(self, content: impl NativeView) -> Self {
        self.set_content(content);
        self
    }

    pub fn set_content(&self, content: impl NativeView) {
        if let Some(controller) = content.view_controller() {
            self.native.setContentViewController(Some(controller));
        } else {
            self.native.setContentViewController(None);
            self.native.setContentView(Some(content.ns_view()));
        }
        if let Some(toolbar) = self.toolbar.get() {
            toolbar.attach(&self.native);
        }
        *self.content.borrow_mut() = Some(Box::new(content));
    }

    pub fn show(&self) { self.native.makeKeyAndOrderFront(None); }

    pub fn hide(&self) { self.native.orderOut(None); }

    pub fn close(&self) { self.native.close(); }

    pub fn center(&self) { self.native.center(); }

    pub fn make_key(&self) { self.native.makeKeyWindow(); }

    pub fn autosave_frame(self, name: &str) -> Self {
        self.native.setFrameAutosaveName(&NSString::from_str(name));
        self
    }

    pub fn on_close(mut self, mut f: impl FnMut() + 'static) -> Self {
        let ui = Ui::new(self.native.mtm());
        let bridge = DelegateBridge::new(&ui, move |event, _| {
            if event == Event::WindowClosed {
                f();
            }
        });
        bridge.for_window(&self.native);
        self.delegate = Some(bridge);
        self
    }

    pub fn appearance(&self, value: Appearance) {
        let name = match value {
            Appearance::System => None,
            Appearance::Light => Some(unsafe { NSAppearanceNameAqua }),
            Appearance::Dark => Some(unsafe { NSAppearanceNameDarkAqua }),
        };
        let appearance = name.and_then(NSAppearance::appearanceNamed);
        self.native.setAppearance(appearance.as_deref());
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        // End editing while native content and its Rust delegates are still alive.
        // Otherwise the cached field editor can retain its former client after teardown.
        unsafe {
            self.native.endEditingFor(None);
        }
        self.native.setDelegate(None);
        // End presentation before releasing Rust content; delegates cannot call back into it.
        if let Some(parent) = self.native.sheetParent() {
            parent.endSheet(&self.native);
        }
        self.native.close();
        self.native.setToolbar(None);
        self.native.setContentViewController(None);
        self.native.setContentView(None);
    }
}
/// A weak, cloneable window reference for presenting sheets without retaining the window.
#[derive(Clone, Default)]
pub struct WindowRef(pub(super) Weak<NSWindow>);
impl WindowRef {
    pub fn new(window: &NSWindow) -> Self { Self(Weak::new(window)) }

    pub fn is_open(&self) -> bool { self.0.load().is_some() }

    /// CoreGraphics ID of the display currently showing the window.
    pub fn display_id(&self) -> Option<u32> {
        let screen = self.0.load()?.screen()?;
        let number =
            screen.deviceDescription().objectForKey(&NSString::from_str("NSScreenNumber"))?;
        Some(number.downcast::<objc2_foundation::NSNumber>().ok()?.unsignedIntValue())
    }
}

#[derive(Clone, Copy, Default)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

pub struct ViewController {
    native: Retained<NSViewController>,
    content: Box<dyn NativeView>,
}
impl ViewController {
    pub fn new(ui: &Ui, content: impl NativeView) -> Self {
        let native = if let Some(controller) = content.view_controller() {
            controller.retain()
        } else {
            let controller = NSViewController::new(ui.mtm());
            controller.setView(content.ns_view());
            controller
        };
        Self {
            native,
            content: Box::new(content),
        }
    }

    pub fn ns_view_controller(&self) -> &NSViewController { &self.native }
}
impl NativeView for ViewController {
    fn ns_view(&self) -> &NSView { self.content.ns_view() }

    fn view_controller(&self) -> Option<&NSViewController> { Some(&self.native) }
}
