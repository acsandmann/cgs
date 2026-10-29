use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::NSView;

use super::pin;
use crate::view::NativeView;
use crate::{Insets, Ui};

/// Apple's native glass surface, falling back to unmodified content before macOS 26.
pub struct GlassEffectView {
    native: Option<Retained<objc2_app_kit::NSGlassEffectView>>,
    content: Box<dyn NativeView>,
}
#[derive(Clone, Copy, Debug, Default)]
pub enum GlassStyle {
    #[default]
    Regular,
    Clear,
}
impl GlassEffectView {
    pub fn new(ui: &Ui, content: impl NativeView) -> Self {
        let native = objc2::runtime::AnyClass::get(c"NSGlassEffectView").map(|_| {
            let view = objc2_app_kit::NSGlassEffectView::new(ui.mtm());
            view.setContentView(Some(content.ns_view()));
            pin(&view, content.ns_view(), Insets {
                top: 0.0,
                left: 0.0,
                bottom: 0.0,
                right: 0.0,
            });
            view
        });
        Self {
            native,
            content: Box::new(content),
        }
    }

    pub fn style(self, style: GlassStyle) -> Self {
        if let Some(view) = &self.native {
            view.setStyle(match style {
                GlassStyle::Regular => objc2_app_kit::NSGlassEffectViewStyle::Regular,
                GlassStyle::Clear => objc2_app_kit::NSGlassEffectViewStyle::Clear,
            });
        }
        self
    }

    pub fn corner_radius(self, radius: f64) -> Self {
        if let Some(view) = &self.native {
            view.setCornerRadius(radius);
        }
        self
    }

    /// Interactive glass is available on macOS 27 and newer.
    pub fn interactive(self, interactive: bool) -> Self {
        if let Some(view) = &self.native {
            let supported: bool = unsafe {
                msg_send![view, respondsToSelector: objc2::sel!(setEffectIsInteractive:)]
            };
            if supported {
                let _: () = unsafe { msg_send![view, setEffectIsInteractive: interactive] };
            }
        }
        self
    }

    pub fn ns_glass_effect_view(&self) -> Option<&objc2_app_kit::NSGlassEffectView> {
        self.native.as_deref()
    }
}
/// Detach borrowed content from a glass wrapper. Resolving the class before macOS 26 aborts.
pub(crate) fn detach_glass_content(view: Option<Retained<NSView>>) {
    if objc2::runtime::AnyClass::get(c"NSGlassEffectView").is_some()
        && let Some(glass) =
            view.and_then(|view| view.downcast::<objc2_app_kit::NSGlassEffectView>().ok())
    {
        glass.setContentView(None);
    }
}
impl NativeView for GlassEffectView {
    fn ns_view(&self) -> &NSView {
        self.native
            .as_ref()
            .map_or_else(|| self.content.ns_view(), |view| view.as_ref())
    }
}

/// Groups descendant native glass surfaces for AppKit's shared compositing and
/// proximity-based merging. Falls back to the content on systems before macOS 26.
pub struct GlassEffectContainerView {
    native: Option<Retained<objc2_app_kit::NSGlassEffectContainerView>>,
    _content: Box<dyn NativeView>,
}
impl GlassEffectContainerView {
    pub fn new(ui: &Ui, content: impl NativeView) -> Self {
        let native = objc2::runtime::AnyClass::get(c"NSGlassEffectContainerView").map(|_| {
            let view = objc2_app_kit::NSGlassEffectContainerView::new(ui.mtm());
            view.setContentView(Some(content.ns_view()));
            pin(&view, content.ns_view(), Insets {
                top: 0.0,
                left: 0.0,
                bottom: 0.0,
                right: 0.0,
            });
            view
        });
        Self {
            native,
            _content: Box::new(content),
        }
    }

    /// Distance at which compatible glass surfaces start to merge.
    pub fn spacing(self, spacing: f64) -> Self {
        if let Some(view) = &self.native {
            view.setSpacing(spacing.max(0.0));
        }
        self
    }

    pub fn ns_glass_effect_container_view(
        &self,
    ) -> Option<&objc2_app_kit::NSGlassEffectContainerView> {
        self.native.as_deref()
    }
}
impl NativeView for GlassEffectContainerView {
    fn ns_view(&self) -> &NSView {
        self.native.as_deref().map_or(self._content.ns_view(), |view| view)
    }
}
