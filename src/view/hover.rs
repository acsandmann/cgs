use std::cell::OnceCell;
use objc2::rc::Retained;
use objc2::{AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSEvent, NSTrackingArea, NSTrackingAreaOptions, NSView};
use crate::{CGRect, Ui};
use super::{NativeView, pin};

struct HoverContent {
    content: Box<dyn NativeView>,
    action: Retained<NSView>,
    tracking: OnceCell<Retained<NSTrackingArea>>,
}
define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsHoverReveal"]
    #[ivars = HoverContent]
    struct HoverNativeView;
    impl HoverNativeView {
        #[unsafe(method(updateTrackingAreas))]
        fn update_tracking(&self) {
            unsafe { let _: () = msg_send![super(self), updateTrackingAreas]; }
            self.ivars().tracking.get_or_init(|| {
                let area = unsafe { NSTrackingArea::initWithRect_options_owner_userInfo(
                    NSTrackingArea::alloc(), CGRect::ZERO,
                    NSTrackingAreaOptions::MouseEnteredAndExited | NSTrackingAreaOptions::ActiveInKeyWindow | NSTrackingAreaOptions::InVisibleRect,
                    Some(self), None,
                ) };
                self.addTrackingArea(&area);
                area
            });
        }
        #[unsafe(method(mouseEntered:))]
        fn entered(&self, _: &NSEvent) { self.ivars().action.setHidden(false); }
        #[unsafe(method(mouseExited:))]
        fn exited(&self, _: &NSEvent) { self.ivars().action.setHidden(true); }
        #[unsafe(method(viewDidHide))]
        fn hidden(&self) {
            unsafe { let _: () = msg_send![super(self), viewDidHide]; }
            self.ivars().action.setHidden(true);
        }
    }
);

/// Reveal a secondary row action on hover. Give the action a fixed-size container
/// inside the content to keep layout stable while the action itself is hidden.
pub struct HoverReveal(Retained<HoverNativeView>);
impl HoverReveal {
    pub fn new(ui: &Ui, content: impl NativeView, action: Retained<NSView>) -> Self {
        action.setHidden(true);
        let native: Retained<HoverNativeView> = unsafe {
            msg_send![super(HoverNativeView::alloc(ui.mtm()).set_ivars(HoverContent {
                content: Box::new(content), action, tracking: OnceCell::new(),
            })), init]
        };
        let content = native.ivars().content.ns_view();
        native.addSubview(content);
        pin(&native, content, crate::layout::uniform_insets(0.0));
        Self(native)
    }
}
impl NativeView for HoverReveal {
    fn ns_view(&self) -> &NSView { &self.0 }
}
