use std::sync::Arc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_core_video::CVPixelBufferGetIOSurface;
use objc2_quartz_core::{CALayer, CATransaction};

use super::PreviewFrame;
use super::capture::Sample;
use crate::compositor::LayerTransaction;

// Keep the capture pool entry and its surface in use through CA consumption.
// The completion block can release this immutable sample on another thread.
struct LayerSample(Arc<Sample>);
impl Drop for LayerSample {
    fn drop(&mut self) {
        if let Some(image) = unsafe { self.0.0.image_buffer() }
            && let Some(surface) = CVPixelBufferGetIOSurface(Some(&image))
        {
            surface.decrement_use_count();
        }
    }
}

/// A retained, zero-copy capture layer. Geometry belongs to its host's CA tree;
/// changing it never acquires a drawable, renders pixels, or changes capture policy.
/// Like `PreviewCompositor`, this object is confined to its owner thread.
pub struct PreviewLayer {
    layer: Retained<CALayer>,
    sample: Option<Arc<LayerSample>>,
    sequence: u64,
    _owner: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl Default for PreviewLayer {
    fn default() -> Self { Self::new() }
}
impl PreviewLayer {
    pub fn new() -> Self {
        Self {
            layer: CALayer::layer(),
            sample: None,
            sequence: 0,
            _owner: std::marker::PhantomData,
        }
    }

    pub fn layer(&self) -> &CALayer { &self.layer }

    pub fn has_frame(&self) -> bool { self.sample.is_some() }

    /// Replace pixels only when a new sample arrives. Last-valid pixels survive
    /// paused capture. The previous pool entry lives until this CA commit completes.
    pub fn set_frame(&mut self, frame: &PreviewFrame) -> bool {
        if self.sequence == frame.frame.sequence {
            return false;
        }
        let Some(image) = (unsafe { frame.frame.sample.0.image_buffer() }) else {
            return false;
        };
        let Some(surface) = CVPixelBufferGetIOSurface(Some(&image)) else {
            return false;
        };
        let _tx = LayerTransaction::disabled();
        surface.increment_use_count();
        let previous = self.sample.replace(Arc::new(LayerSample(frame.frame.sample.clone())));
        unsafe {
            self.layer.setContents(Some(
                &*((&*surface as *const objc2_io_surface::IOSurfaceRef).cast()),
            ));
        }
        let [x, y, width, height] = frame.frame.uv;
        self.layer.setContentsRect(objc2_core_foundation::CGRect::new(
            objc2_core_foundation::CGPoint::new(x as f64, y as f64),
            objc2_core_foundation::CGSize::new(width as f64, height as f64),
        ));
        self.sequence = frame.frame.sequence;
        unsafe {
            CATransaction::setCompletionBlock(Some(&RcBlock::new(move || {
                let _ = &previous;
            })));
        }
        true
    }

    pub fn clear(&mut self) {
        let Some(previous) = self.sample.take() else { return };
        let _tx = LayerTransaction::disabled();
        unsafe { self.layer.setContents(None) };
        self.sequence = 0;
        unsafe {
            CATransaction::setCompletionBlock(Some(&RcBlock::new(move || {
                let _ = &previous;
            })));
        }
    }
}
impl Drop for PreviewLayer {
    fn drop(&mut self) {
        self.clear();
        self.layer.removeFromSuperlayer();
    }
}
