use std::cell::RefCell;
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{
    AnyThread, DeclaredClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSRunLoop, NSRunLoopCommonModes};
use objc2_quartz_core::{
    CAFrameRateRange, CAMetalDisplayLink, CAMetalDisplayLinkDelegate, CAMetalDisplayLinkUpdate,
    CAMetalLayer,
};

use super::Wake;
struct State {
    update: RefCell<Option<Retained<CAMetalDisplayLinkUpdate>>>,
    wake: Arc<Wake>,
}
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = State]
    struct Target;
    unsafe impl NSObjectProtocol for Target {}
    unsafe impl CAMetalDisplayLinkDelegate for Target {
        #[unsafe(method(metalDisplayLink:needsUpdate:))]
        fn update(&self, link: &CAMetalDisplayLink, update: &CAMetalDisplayLinkUpdate) {
            // Keep cadence while changing frames arrive. If the owner falls
            // behind, replace its unsubmitted drawable and pause until consumed.
            if self.ivars().update.replace(Some(update.retain())).is_some() {
                link.setPaused(true);
            }
            self.ivars().wake.notify();
        }
    }
);
pub(super) struct Pacing {
    link: Retained<CAMetalDisplayLink>,
    target: Retained<Target>,
}
impl Pacing {
    pub fn new(layer: &CAMetalLayer, wake: Arc<Wake>) -> Self {
        let mtm = MainThreadMarker::new().expect("compositor owner");
        let target = Target::alloc(mtm).set_ivars(State {
            update: RefCell::default(),
            wake,
        });
        let target: Retained<Target> = unsafe { msg_send![super(target), init] };
        let link = CAMetalDisplayLink::initWithMetalLayer(CAMetalDisplayLink::alloc(), layer);
        link.setDelegate(Some(ProtocolObject::from_ref(&*target)));
        link.setPreferredFrameLatency(1.0);
        link.setPreferredFrameRateRange(CAFrameRateRange::new(30., 120., 120.));
        link.setPaused(true);
        unsafe {
            link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes);
        }
        Self { link, target }
    }

    pub fn has_update(&self) -> bool { self.target.ivars().update.borrow().is_some() }

    pub fn take(&self) -> Option<Retained<CAMetalDisplayLinkUpdate>> {
        self.target.ivars().update.borrow_mut().take()
    }

    pub fn request(&self) {
        if self.link.isPaused() {
            self.link.setPaused(false);
        }
    }

    pub fn pause(&self) {
        self.link.setPaused(true);
        self.target.ivars().update.borrow_mut().take();
    }
}
impl Drop for Pacing {
    fn drop(&mut self) {
        self.link.invalidate();
        self.link.setDelegate(None);
    }
}
