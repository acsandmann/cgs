use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{AnyThread, extern_class, extern_methods};
use objc2_foundation::{NSArray, NSNumber, NSObjectNSKeyValueCoding, NSString};
use objc2_quartz_core::CALayer;

extern_class!(
    #[unsafe(super(CALayer))]
    #[thread_kind = AnyThread]
    struct CABackdropLayer;
);

impl CABackdropLayer {
    extern_methods!(
        #[unsafe(method(layer))]
        #[unsafe(method_family = none)]
        fn layer() -> Retained<Self>;

        #[unsafe(method(setScale:))]
        fn set_scale(&self, scale: f64);

        #[unsafe(method(setBleedAmount:))]
        fn set_bleed_amount(&self, amount: f64);

        #[unsafe(method(setWindowServerAware:))]
        fn set_window_server_aware(&self, aware: bool);
    );
}

extern_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = AnyThread]
    struct CAFilter;
);

impl CAFilter {
    extern_methods!(
        #[unsafe(method(filterWithType:))]
        #[unsafe(method_family = none)]
        fn with_type(filter_type: &NSString) -> Option<Retained<Self>>;
    );
}

/// Creates a compositor-native backdrop blur layer, or `None` when the private
/// Core Animation filter is unavailable on the running macOS version.
pub fn backdrop_blur(radius: f64) -> Option<Retained<CALayer>> {
    let backdrop = CABackdropLayer::layer();
    let filters = blur_filters(radius)?;
    unsafe { backdrop.setFilters(Some(&filters)) };
    backdrop.set_scale(0.25);
    backdrop.set_bleed_amount(5.0);
    backdrop.set_window_server_aware(true);

    Some(unsafe { Retained::cast_unchecked(backdrop) })
}

/// The render server's gaussian blur, for a layer's own `filters`.
pub(crate) fn blur_filters(radius: f64) -> Option<Retained<NSArray<AnyObject>>> {
    let filter = CAFilter::with_type(&NSString::from_str("gaussianBlur"))?;
    unsafe {
        filter.setValue_forKey(
            Some(&NSNumber::new_f64(radius)),
            &NSString::from_str("inputRadius"),
        );
        filter.setValue_forKey(
            Some(&NSNumber::new_bool(true)),
            &NSString::from_str("inputNormalizeEdges"),
        );
    }
    let filter: Retained<AnyObject> = unsafe { Retained::cast_unchecked(filter) };
    Some(NSArray::from_retained_slice(&[filter]))
}

/// A true variable-radius backdrop, driven by mask alpha (opaque = maximum blur).
pub(crate) fn progressive_backdrop(radius: f64, mask: &objc2_core_graphics::CGImage) -> Option<Retained<CALayer>> {
    if objc2::runtime::AnyClass::get(c"CABackdropLayer").is_none()
        || objc2::runtime::AnyClass::get(c"CAFilter").is_none() { return None; }
    let filter = CAFilter::with_type(&NSString::from_str("variableBlur"))?;
    unsafe {
        filter.setValue_forKey(Some(&NSNumber::new_f64(radius)), &NSString::from_str("inputRadius"));
        let image = &*(mask as *const objc2_core_graphics::CGImage as *const AnyObject);
        filter.setValue_forKey(Some(image), &NSString::from_str("inputMaskImage"));
        filter.setValue_forKey(Some(&NSNumber::new_bool(true)), &NSString::from_str("inputNormalizeEdges"));
    }
    let backdrop = CABackdropLayer::layer();
    let filter: Retained<AnyObject> = unsafe { Retained::cast_unchecked(filter) };
    unsafe { backdrop.setFilters(Some(&NSArray::from_retained_slice(&[filter]))) };
    backdrop.set_bleed_amount(0.0);
    backdrop.set_window_server_aware(false);
    Some(unsafe { Retained::cast_unchecked(backdrop) })
}

pub(crate) fn backdrop_scale(layer: &CALayer, scale: f64) {
    // Only called with layers made by progressive_backdrop.
    let backdrop = unsafe { &*(layer as *const CALayer as *const CABackdropLayer) };
    backdrop.set_scale(scale);
}
