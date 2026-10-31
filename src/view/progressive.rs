//! Rust/AppKit adaptation of the three-layer ProgressiveBlurHeader design:
//! https://github.com/dominikmartn/ProgressiveBlurHeader (MIT, Dominik Martin).
//! License: third-party/ProgressiveBlurHeader.LICENSE.
//! Uses the existing compositor's CABackdropLayer, with the gradient-mask
//! variableBlur technique documented by https://github.com/nikstar/VariableBlur.
use std::cell::Cell;
use std::ptr;

use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::*;
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{CGBitmapContextCreate, CGBitmapContextCreateImage, CGColor, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo};
use objc2_foundation::{NSArray, NSNumber};
use objc2_quartz_core::{CALayer, CAGradientLayer};

use crate::compositor::{LayerTransaction, progressive_backdrop, backdrop_scale};
use crate::{CGRect, CGPoint, CGSize, Ui};

fn mask() -> Option<CFRetained<CGImage>> {
    let rgb = CGColorSpace::new_device_rgb()?;
    let context = unsafe { CGBitmapContextCreate(ptr::null_mut(), 1, 256, 8, 4, Some(&rgb), CGImageAlphaInfo::PremultipliedLast.0) }?;
    for y in 0..256 {
        CGContext::set_rgb_fill_color(Some(&context), 0.0, 0.0, 0.0, y as f64 / 255.0);
        CGContext::fill_rect(Some(&context), CGRect::new(CGPoint::new(0.0, y as f64), CGSize::new(1.0, 1.0)));
    }
    CGBitmapContextCreateImage(Some(&context))
}
thread_local! { static MASK: Option<CFRetained<CGImage>> = mask(); }

struct State {
    backdrop: Option<Retained<CALayer>>,
    tint: Retained<CAGradientLayer>,
    dark: Cell<Option<bool>>,
}
define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsProgressiveBlurView"]
    #[ivars = State]
    struct BlurView;
    impl BlurView {
        #[unsafe(method(hitTest:))]
        fn hit(&self, _: CGPoint) -> *mut NSView { ptr::null_mut() }

        #[unsafe(method(viewDidMoveToWindow))]
        fn moved(&self) {
            unsafe { let _: () = msg_send![super(self), viewDidMoveToWindow]; }
            self.update_tint();
        }

        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn appearance(&self) {
            unsafe { let _: () = msg_send![super(self), viewDidChangeEffectiveAppearance]; }
            self.update_tint();
        }
    }
);
impl BlurView {
    fn update_tint(&self) {
        let names = NSArray::from_slice(&[unsafe { NSAppearanceNameAqua }, unsafe { NSAppearanceNameDarkAqua }]);
        let dark = self.effectiveAppearance().bestMatchFromAppearancesWithNames(&names)
            .is_some_and(|name| &*name == unsafe { NSAppearanceNameDarkAqua });
        if self.ivars().dark.replace(Some(dark)) == Some(dark) { return; }
        let value = if dark { 0.0 } else { 1.0 };
        let colors = [0.65, 0.35, 0.0].map(|a| CGColor::new_generic_rgb(value, value, value, a));
        let colors: Vec<&AnyObject> = colors.iter().map(|color| unsafe { &*((&**color as *const CGColor).cast()) }).collect();
        unsafe { self.ivars().tint.setColors(Some(&NSArray::from_slice(&colors))) };
    }
}

/// A mouse-transparent, full-width backdrop. No snapshots or image work while scrolling.
pub(crate) struct ProgressiveBlur {
    native: Retained<BlurView>,
    stop: Cell<f64>,
    scale: Cell<f64>,
}
impl ProgressiveBlur {
    pub(crate) fn new(ui: &Ui) -> Self {
        let backdrop = MASK.with(|mask| mask.as_ref().and_then(|mask| progressive_backdrop(5.0, mask)));
        let tint = CAGradientLayer::layer();
        // Layer content uses AppKit’s bottom-up coordinates, independently of
        // the flipped geometry used to place its sublayers. Fade down to clear.
        tint.setStartPoint(CGPoint::new(0.5, 1.0));
        tint.setEndPoint(CGPoint::new(0.5, 0.0));
        let native: Retained<BlurView> = unsafe { msg_send![super(BlurView::alloc(ui.mtm()).set_ivars(State {
            backdrop, tint, dark: Cell::new(None),
        })), init] };
        native.setWantsLayer(true);
        native.setAccessibilityElement(false);
        if let Some(root) = native.layer() {
            root.setGeometryFlipped(true);
            if let Some(backdrop) = &native.ivars().backdrop { root.addSublayer(backdrop); }
            root.addSublayer(&native.ivars().tint);
        }
        native.update_tint();
        Self { native, stop: Cell::new(-1.0), scale: Cell::new(0.0) }
    }

    pub(crate) fn ns_view(&self) -> &NSView { &self.native }

    pub(crate) fn update(&self, frame: CGRect, header: f64, amount: f64, scale: f64) {
        let _tx = LayerTransaction::disabled();
        if self.native.frame() != frame { self.native.setFrame(frame); }
        let bounds = CGRect::new(CGPoint::ZERO, frame.size);
        if let Some(backdrop) = &self.native.ivars().backdrop {
            if backdrop.frame() != bounds { backdrop.setFrame(bounds); }
            if self.scale.replace(scale) != scale { backdrop_scale(backdrop, scale); }
        }
        if self.native.ivars().tint.frame() != bounds { self.native.ivars().tint.setFrame(bounds); }
        let stop = (header / frame.size.height).clamp(0.0, 0.95);
        if self.stop.replace(stop) != stop {
            let locations = [0.0, stop, 1.0].map(NSNumber::new_f64);
            self.native.ivars().tint.setLocations(Some(&NSArray::from_retained_slice(&locations)));
        }
        self.native.setHidden(amount <= 0.0);
        if self.native.alphaValue() != amount { self.native.setAlphaValue(amount); }
    }
}
