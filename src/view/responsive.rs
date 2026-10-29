use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::NSView;

use crate::Ui;
use crate::view::NativeView;

struct ResponsiveContent {
    wide: Box<dyn NativeView>,
    compact: Box<dyn NativeView>,
    size: crate::CGSize,
}
define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsResponsiveView"]
    #[ivars = ResponsiveContent]
    struct ResponsiveNativeView;
    impl ResponsiveNativeView {
        #[unsafe(method(intrinsicContentSize))]
        fn intrinsic_size(&self) -> crate::CGSize {
            crate::CGSize::new(-1.0, self.ivars().size.height)
        }
        #[unsafe(method(layout))]
        fn layout_content(&self) {
            unsafe { let _: () = msg_send![super(self), layout]; }
            let content = self.ivars();
            let width = self.bounds().size.width;
            let compact = width < content.size.width;
            content.wide.set_hidden(compact);
            content.compact.set_hidden(!compact);
            let view = if compact { content.compact.ns_view() } else { content.wide.ns_view() };
            let size = view.fittingSize();
            view.setFrame(crate::CGRect::new(crate::CGPoint::new(0.0, 0.0), crate::CGSize::new(size.width.min(width), content.size.height)));
        }
    }
);

/// Uses a compact native alternative when the preferred content no longer fits.
pub struct ResponsiveView(Retained<ResponsiveNativeView>);
impl ResponsiveView {
    pub fn new(ui: &Ui, wide: impl NativeView, compact: impl NativeView) -> Self {
        let mut size = wide.ns_view().fittingSize();
        size.height = size.height.max(compact.ns_view().fittingSize().height);
        let native: Retained<ResponsiveNativeView> = unsafe {
            msg_send![
                super(
                    ResponsiveNativeView::alloc(ui.mtm()).set_ivars(ResponsiveContent {
                        wide: Box::new(wide),
                        compact: Box::new(compact),
                        size,
                    })
                ),
                init
            ]
        };
        native.addSubview(native.ivars().wide.ns_view());
        native.addSubview(native.ivars().compact.ns_view());
        native.ivars().compact.set_hidden(true);
        Self(native)
    }
}
impl NativeView for ResponsiveView {
    fn ns_view(&self) -> &NSView { &self.0 }
}
