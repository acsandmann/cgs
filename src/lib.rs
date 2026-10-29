//! A native macOS UI library built on AppKit, plus a window-server compositor.
//!
//! Widgets live in topical modules:
//!
//! - [`view`], [`layout`], [`style`], [`text`], [`image`]: the `NativeView` abstraction, containers and content
//! - [`controls`], [`input`], [`collection`]: interactive controls, text and shortcut entry, tables and outlines
//! - [`window`], [`menu`], [`app`]: windows, sheets and popovers, menus and status items, the application
//! - [`navigation`], [`tabs`], [`settings`]: split views and toolbars, tab strips, settings-window building blocks
//! - [`drawing`], [`feedback`]: custom drawing surfaces, progress and empty states
//! - [`compositor`]: SkyLight windows, layer surfaces, live window previews and the window switcher

mod bridge;

pub mod app;
pub mod collection;
pub mod compositor;
pub mod controls;
pub mod drawing;
pub mod feedback;
pub mod image;
pub mod input;
pub mod layout;
pub mod menu;
pub mod navigation;
pub mod settings;
pub mod style;
pub mod tabs;
pub mod text;
pub mod view;
pub mod window;

pub use objc2::MainThreadMarker;
pub use objc2::rc::Weak;
pub use objc2_core_foundation::{CGPoint, CGRect, CGSize};
pub use objc2_foundation::NSEdgeInsets as Insets;

#[derive(Clone, Copy)]
pub struct Ui {
    mtm: MainThreadMarker,
}
impl Ui {
    pub fn new(mtm: MainThreadMarker) -> Self { Self { mtm } }

    pub fn mtm(&self) -> MainThreadMarker { self.mtm }
}
