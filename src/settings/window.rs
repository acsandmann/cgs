use std::rc::Rc;

use objc2::MainThreadOnly;
use objc2_app_kit::*;

use crate::view::NativeView;
use crate::window::{Window, WindowRef};
use crate::{CGPoint, CGRect, CGSize, Ui};

pub struct SettingsWindow(Window);
impl SettingsWindow {
    pub fn new(ui: &Ui, title: &str) -> Self {
        Self::from_window(
            Window::with_size(ui, CGSize::new(800.0, 600.0), true),
            title,
            true,
        )
    }

    /// Use explicit initial geometry without restoring and then resizing an autosaved frame.
    pub fn with_size(ui: &Ui, title: &str, size: CGSize) -> Self {
        Self::from_window(Window::with_size(ui, size, true), title, false)
    }

    fn from_window(window: Window, title: &str, autosave: bool) -> Self {
        let window = window.title(title);
        let window = if autosave {
            window.autosave_frame(title)
        } else {
            window
        };
        window
            .native
            .setBackgroundColor(Some(&crate::style::Color::window_background()));
        window.native.setTitlebarAppearsTransparent(true);
        window.native.setTitlebarSeparatorStyle(NSTitlebarSeparatorStyle::None);
        window.native.setToolbarStyle(NSWindowToolbarStyle::Automatic);
        window.native.setTitleVisibility(NSWindowTitleVisibility::Visible);
        if !window.native.styleMask().contains(NSWindowStyleMask::FullSizeContentView) {
            window
                .native
                .setStyleMask(window.native.styleMask() | NSWindowStyleMask::FullSizeContentView);
        }
        Self(window)
    }

    pub fn page_title(mut self, ui: &Ui, title: Rc<crate::text::Label>) -> Self {
        title
            .ns_text_field()
            .setFont(Some(&NSFont::systemFontOfSize_weight(15.0, unsafe {
                NSFontWeightSemibold
            })));
        title.max_width(180.0);
        title.height(20.0);
        self.0.native.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        let toolbar =
            crate::navigation::Toolbar::navigation_title(ui, "cgs.settings.pages", Some(title));
        toolbar.attach(&self.0.native);
        self.0.toolbar.take();
        self.0.toolbar.set(Rc::new(toolbar)).ok();
        self
    }

    pub fn toolbar(&self) -> &Rc<crate::navigation::Toolbar> {
        self.0.toolbar.get_or_init(|| {
            let toolbar = Rc::new(crate::navigation::Toolbar::navigation(
                &Ui::new(self.0.native.mtm()),
                "cgs.settings",
            ));
            toolbar.attach(&self.0.native);
            toolbar
        })
    }

    pub fn content(self, content: impl NativeView) -> Self {
        // NSWindow adopts a controller view's frame on attachment. Keep the requested
        // size rather than laying out at the controller's default size first.
        if let Some(controller) = content.view_controller() {
            let size = self.0.native.contentRectForFrameRect(self.0.native.frame()).size;
            controller.view().setFrame(CGRect::new(CGPoint::new(0.0, 0.0), size));
        }
        Self(self.0.content(content))
    }

    pub fn show(&self) {
        self.toolbar();
        self.0.show();
    }

    pub fn hide(&self) { self.0.hide(); }

    pub fn close(&self) { self.0.close(); }

    pub fn ns_window(&self) -> &NSWindow {
        self.toolbar();
        self.0.ns_window()
    }

    pub fn on_close(self, f: impl FnMut() + 'static) -> Self { Self(self.0.on_close(f)) }

    pub fn handle(&self) -> WindowRef { WindowRef::new(self.ns_window()) }

    /// Bring the window forward; accessory apps must activate to accept keyboard input.
    pub fn present(&self) {
        #[allow(deprecated)]
        NSApplication::sharedApplication(self.0.native.mtm()).activateIgnoringOtherApps(true);
        self.show();
    }

    pub fn is_visible(&self) -> bool { self.ns_window().isVisible() }

    pub fn initial_focus(&self, view: &impl NativeView) {
        self.ns_window().setInitialFirstResponder(Some(view.ns_view()));
    }

    pub fn set_size_centered(&self, size: CGSize) {
        self.ns_window().setContentSize(size);
        self.ns_window().center();
    }
}
