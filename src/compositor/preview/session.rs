//! Capture ownership for hosts that present IOSurfaces through Core Animation.
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use objc2::MainThreadMarker;

use super::capture::{Captures, Frame};
use super::{PreviewError, PreviewResult, PreviewSource, PreviewStats, SourceStatus, Wake};

/// An immutable retained capture sample for a Core Animation preview.
/// No Metal texture is created when acquiring or displaying this handle.
#[derive(Clone)]
pub struct PreviewSurfaceFrame {
    window_id: u32,
    process_id: i32,
    pub(super) frame: Frame,
}

impl PreviewSurfaceFrame {
    pub fn window_id(&self) -> u32 { self.window_id }
    pub fn process_id(&self) -> i32 { self.process_id }
    pub fn received_at(&self) -> Instant { self.frame.received }
    pub fn texture_coordinates(&self) -> [f32; 4] { self.frame.uv }
}

/// An owner-thread capture session without a Metal renderer or display clock.
/// Call pump after its coalesced wake, then present frames through PreviewLayer.
/// Clearing stops capture and retains the existing bounded thumbnail cache.
pub struct PreviewCaptureSession {
    captures: Captures,
    sources: Vec<PreviewSource>,
    wake: Arc<Wake>,
    _owner: PhantomData<Rc<()>>,
}

impl PreviewCaptureSession {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> PreviewResult<Self> {
        MainThreadMarker::new()
            .ok_or_else(|| PreviewError::Capture("create on main thread".into()))?;
        if !objc2_core_graphics::CGPreflightScreenCaptureAccess() {
            return Err(PreviewError::PermissionDenied);
        }
        let wake = Wake::new(wake);
        Ok(Self {
            captures: Captures::new(wake.clone()),
            sources: Vec::new(),
            wake,
            _owner: PhantomData,
        })
    }

    pub fn set_sources(&mut self, sources: &[PreviewSource]) -> PreviewResult<()> {
        if !super::valid_sources(sources) {
            return Err(PreviewError::InvalidScene);
        }
        if self.sources == sources {
            return Ok(());
        }
        self.sources.clear();
        self.sources.extend_from_slice(sources);
        self.captures.set_sources(sources.iter().copied())?;
        self.wake.notify();
        Ok(())
    }

    pub fn update_sources(&mut self, updates: &[PreviewSource]) -> PreviewResult<()> {
        let (changed, inventory_changed) = super::update_source_policies(&mut self.sources, updates)?;
        if changed {
            if inventory_changed {
                self.captures.set_sources(self.sources.iter().copied())?;
            } else {
                self.captures.update_sources(updates);
            }
            self.wake.notify();
        }
        Ok(())
    }

    pub fn pump(&mut self) -> PreviewResult<()> {
        self.wake.pending.store(false, Ordering::Release);
        self.captures.update_surfaces()
    }

    pub fn captured_frame(&self, window_id: u32) -> Option<PreviewSurfaceFrame> {
        let (process_id, frame) = self.captures.captured_frame(window_id)?;
        Some(PreviewSurfaceFrame { window_id, process_id, frame })
    }

    pub fn source_status(&self, window_id: u32) -> Option<SourceStatus> {
        self.captures.status(window_id)
    }

    pub fn stats(&self) -> PreviewStats {
        let mut stats = PreviewStats::default();
        self.captures.stats(&mut stats);
        stats
    }

    pub fn clear(&mut self) {
        self.captures.clear();
        self.sources.clear();
    }
}

impl Drop for PreviewCaptureSession {
    fn drop(&mut self) { self.clear(); }
}
