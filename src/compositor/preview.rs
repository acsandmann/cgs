//! WindowServer-hosted Metal previews. All control and presentation is owner-thread confined.
//! Capture callbacks only replace a single retained, immutable sample; they never render.
mod capture;
mod gpu;
mod pacing;
mod timing;

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use capture::Captures;
use gpu::Gpu;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_core_foundation::{CGPoint, CGRect};
use objc2_metal::MTLDrawable;
use objc2_quartz_core::{CALayer, CAMetalLayer};

use super::{CgsWindow, LayerTransaction, WindowSurface, WindowTransaction};

pub type PreviewResult<T> = Result<T, PreviewError>;
#[derive(Debug)]
pub enum PreviewError {
    PermissionDenied,
    Window(super::CgsWindowError),
    Capture(String),
    Metal(String),
    InvalidScene,
}
impl std::fmt::Display for PreviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{self:?}") }
}
impl std::error::Error for PreviewError {}
impl From<super::CgsWindowError> for PreviewError {
    fn from(e: super::CgsWindowError) -> Self { Self::Window(e) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureMode {
    Live,
    Snapshot,
    Inactive,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureResolution {
    Native,
    Thumbnail { width: usize, height: usize },
}

/// Array order is back to front. Geometry is in display-local, top-left logical points.
/// `process_id` guards against reusing an ID that now belongs to another application.
#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    pub window_id: u32,
    pub process_id: i32,
    pub frame: CGRect,
    /// Column-major homogeneous transform, applied in display-local logical coordinates.
    pub transform: [f32; 16],
    pub opacity: f32,
    pub corner_radius: f32,
    pub border_width: f32,
    /// Premultiplied RGBA in the same sRGB encoding as the scene.
    pub border_color: [f32; 4],
    pub clip: Option<CGRect>,
    pub mode: CaptureMode,
    pub resolution: CaptureResolution,
    /// Highest priorities receive the bounded live capture slots first.
    pub priority: i32,
}
impl Preview {
    pub fn window(window_id: u32, process_id: i32, frame: CGRect) -> Self {
        Self {
            window_id,
            process_id,
            frame,
            transform: IDENTITY,
            opacity: 1.0,
            corner_radius: 0.0,
            border_width: 0.0,
            border_color: [0.; 4],
            clip: None,
            mode: CaptureMode::Live,
            resolution: CaptureResolution::Native,
            priority: 0,
        }
    }

    fn valid(&self) -> bool {
        self.window_id != 0
            && self.process_id > 0
            && self.frame.size.width > 0.0
            && self.frame.size.height > 0.0
            && [
                self.frame.origin.x,
                self.frame.origin.y,
                self.frame.size.width,
                self.frame.size.height,
            ]
            .iter()
            .all(|v| v.is_finite())
            && self.transform.iter().all(|v| v.is_finite())
            && self.opacity.is_finite()
            && self.clip.is_none_or(|r| {
                [r.origin.x, r.origin.y, r.size.width, r.size.height]
                    .iter()
                    .all(|v| v.is_finite())
                    && r.size.width >= 0.
                    && r.size.height >= 0.
            })
            && self.border_width.is_finite()
            && self.border_width >= 0.
            && self.border_color.iter().all(|v| v.is_finite())
            && self.corner_radius.is_finite()
            && self.corner_radius >= 0.0
            && match self.resolution {
                CaptureResolution::Native => true,
                CaptureResolution::Thumbnail { width, height } => width > 0 && height > 0,
            }
    }
}
const IDENTITY: [f32; 16] = [
    1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
];
#[derive(Clone, Copy, Debug)]
pub enum Easing {
    Linear,
    EaseInOut,
    /// Critically damped motion, normalized to settle at the requested duration.
    Spring,
}
#[derive(Clone, Copy, Debug)]
pub struct Transition {
    pub duration: Duration,
    pub easing: Easing,
}
impl Transition {
    pub fn new(duration: Duration, easing: Easing) -> Self { Self { duration, easing } }
}
struct Animation {
    from: Vec<Preview>,
    to: Vec<Preview>,
    start: Option<Instant>,
    transition: Transition,
    progress: Option<f32>,
    sampled: Option<f32>,
}

/// Rolling timing percentiles cover the latest 1024 samples. Capture-to-present uses
/// WindowServer displayTime and MTLDrawable presentedTime; it excludes unchanged reused frames.
#[derive(Default, Clone, Copy, Debug)]
pub struct PreviewStats {
    pub captured_frames: u64,
    pub replaced_frames: u64,
    pub submitted_frames: u64,
    pub presented_frames: u64,
    /// Hardware presentation timestamp relative to Metal initialization, not
    /// when an event-loop observer happens to notice the presentation.
    pub first_present_us: u64,
    pub dropped_presentations: u64,
    pub streams: usize,
    /// Active latest samples plus session-cached frames; excludes SCK queues
    /// and additional samples held only by in-flight GPU commands.
    pub latest_frame_bytes: usize,
    /// Shared owner-thread thumbnail cache across reopenings (at most 32 MiB).
    /// May share storage already counted in latest_frame_bytes.
    pub reusable_frame_bytes: usize,
    /// Reserved active-source budget (eight aligned BGRA surfaces per source).
    pub reserved_capture_bytes: usize,
    pub process_streams: usize,
    pub process_capture_jobs: usize,
    pub last_capture_to_gpu_us: u64,
    pub last_drawable_wait_us: u64,
    pub last_cpu_render_us: u64,
    pub last_gpu_us: u64,
    pub cpu_render: FrameTimes,
    pub gpu_render: FrameTimes,
    pub capture_to_present: FrameTimes,
    pub first_frame_us: u64,
    pub max_capture_pump_us: u64,
    pub max_filter_us: u64,
    pub max_stream_create_us: u64,
    pub max_stream_start_us: u64,
    pub last_callback_us: u64,
}

#[derive(Default, Clone, Copy, Debug)]
pub struct FrameTimes {
    pub samples: usize,
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
}

pub(super) struct Wake {
    pending: AtomicBool,
    callback: Box<dyn Fn() + Send + Sync>,
}
impl Wake {
    fn notify(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            (self.callback)();
        }
    }
}

/// One transparent CGS presentation window per display. Not Send or Sync.
///
/// Call `render` after the coalesced wake callback. A dirty-driven CAMetalDisplayLink
/// supplies drawables and animation timing; do not add another clock. The callback may run
/// on any framework queue and must
/// only schedule work on this object's owner. It must not call back into the compositor.
/// No polling, capture, or display link remains active after `hide`.
pub struct PreviewCompositor {
    pacing: pacing::Pacing,
    gpu: Gpu,
    captures: Captures,
    layer: Retained<CAMetalLayer>,
    _root: Retained<CALayer>,
    // Unbind before destroying the window; GPU submissions retain their own source samples.
    surface: WindowSurface,
    window: CgsWindow,
    bounds: CGRect,
    scale: f64,
    scene: Vec<Preview>,
    animation: Option<Animation>,
    visible: bool,
    dirty: bool,
    wake: Arc<Wake>,
    revision: u64,
    offset: CGPoint,
    presented_offset: CGPoint,
    _owner: std::marker::PhantomData<Rc<()>>,
}
impl PreviewCompositor {
    /// Start shared content discovery early, before constructing the overview UI.
    /// Only immutable descriptions are cached briefly; no capture or display clock starts.
    pub fn prepare() -> PreviewResult<()> {
        if !objc2_core_graphics::CGPreflightScreenCaptureAccess() {
            return Err(PreviewError::PermissionDenied);
        }
        capture::prepare();
        Ok(())
    }

    /// Requires Screen Recording permission already granted. Does not prompt for TCC access.
    pub fn new(
        bounds: CGRect,
        scale: f64,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> PreviewResult<Self> {
        if objc2::runtime::AnyClass::get(c"CAMetalDisplayLink").is_none() {
            return Err(PreviewError::Metal(
                "Metal previews require macOS 14 or later".into(),
            ));
        }
        let _main = MainThreadMarker::new()
            .ok_or_else(|| PreviewError::Metal("create on main thread".into()))?;
        if !objc2_core_graphics::CGPreflightScreenCaptureAccess() {
            return Err(PreviewError::PermissionDenied);
        }
        if bounds.size.width * scale > 16384.
            || bounds.size.height * scale > 16384.
            || !scale.is_finite()
            || scale <= 0.0
            || ![
                bounds.origin.x,
                bounds.origin.y,
                bounds.size.width,
                bounds.size.height,
            ]
            .iter()
            .all(|v| v.is_finite())
            || bounds.size.width <= 0.0
            || bounds.size.height <= 0.0
        {
            return Err(PreviewError::InvalidScene);
        }
        let wake = Arc::new(Wake {
            pending: AtomicBool::new(false),
            callback: Box::new(wake),
        });
        let captures = Captures::new(wake.clone());
        let gpu = Gpu::new(wake.clone())?;
        let layer = CAMetalLayer::layer();
        layer.setDevice(Some(&gpu.device));
        layer.setPixelFormat(objc2_metal::MTLPixelFormat::BGRA8Unorm);
        layer.setFramebufferOnly(true);
        let color_space = objc2_core_graphics::CGColorSpace::with_name(Some(unsafe {
            objc2_core_graphics::kCGColorSpaceSRGB
        }));
        layer.setColorspace(color_space.as_deref());
        layer.setOpaque(false);
        layer.setMaximumDrawableCount(3);
        layer.setAllowsNextDrawableTimeout(true);
        layer.setPresentsWithTransaction(true);
        layer.setDisplaySyncEnabled(false);
        let local = CGRect::new(CGPoint::ZERO, bounds.size);
        layer.setFrame(local);
        layer.setContentsScale(scale);
        layer.setDrawableSize(objc2_core_foundation::CGSize::new(
            bounds.size.width * scale,
            bounds.size.height * scale,
        ));
        // Keep an ordinary CALayer root, with CAMetalLayer as the hosted child. This
        // follows the established CAContext/SLSBindSurface route, not renderInContext.
        let root = CALayer::layer();
        root.setFrame(local);
        root.setGeometryFlipped(true);
        root.setMasksToBounds(true);
        root.addSublayer(&layer);
        let window = CgsWindow::new_compositor(bounds, 0.0)?;
        window.set_resolution(scale)?;
        let surface = WindowSurface::new_scaled(window.id(), local, &root, scale)?;
        if let Some(color_space) = &color_space {
            surface.set_color_space(color_space)?;
        }
        surface.flush();
        let pacing = pacing::Pacing::new(&layer, wake.clone());
        Ok(Self {
            pacing,
            gpu,
            captures,
            layer,
            _root: root,
            surface,
            window,
            bounds,
            scale,
            scene: Vec::new(),
            animation: None,
            visible: false,
            dirty: true,
            wake,
            revision: 1,
            offset: CGPoint::ZERO,
            presented_offset: CGPoint::ZERO,
            _owner: std::marker::PhantomData,
        })
    }

    pub fn is_ready(&self) -> bool {
        self.visible
            && !self._root.isHidden()
            && self.gpu.is_presented(self.revision)
            && self
                .scene
                .iter()
                .filter(|p| p.mode != CaptureMode::Inactive && p.opacity > 0.)
                .all(|p| self.captures.frame(p).is_some())
    }

    /// A usable captured frame is available, including a retained last-valid frame.
    pub fn has_frame(&self, window_id: u32) -> bool {
        self.scene
            .iter()
            .find(|p| p.window_id == window_id)
            .is_some_and(|p| self.captures.frame(p).is_some())
    }

    pub fn window_id(&self) -> u32 { self.window.id() }

    pub fn set_level(&self, level: i32) -> PreviewResult<()> { Ok(self.window.set_level(level)?) }

    pub fn set_scene(&mut self, scene: &[Preview]) -> PreviewResult<()> {
        validate_scene(scene)?;
        // Re-submission and hover-border changes must not cancel movement.
        // Capture settings and geometry changes still replace the scene immediately.
        if let Some(a) = &mut self.animation {
            if a.to.len() == scene.len()
                && a.to.iter().zip(scene).all(|(old, new)| {
                    let mut styled = old.clone();
                    styled.border_width = new.border_width;
                    styled.border_color = new.border_color;
                    styled == *new
                })
            {
                if a.to != scene {
                    a.to.clone_from_slice(scene);
                    a.sampled = None;
                    self.dirty = true;
                    self.wake.notify();
                }
                return Ok(());
            }
        }
        if self.scene != scene || self.animation.is_some() {
            if self.scene.iter().any(|old| {
                !scene
                    .iter()
                    .any(|p| p.window_id == old.window_id && p.process_id == old.process_id)
            }) {
                // Cancel stale queued work, but keep the displayed canvas until
                // its replacement arrives. Viewport culling must not blank every
                // other preview while newly visible streams start.
                self.gpu.discard_presentation();
            }
            self.scene.clear();
            self.scene.extend_from_slice(scene);
            self.animation = None;
            self.revision += 1;
            self.dirty = true;
            if self.visible {
                self.captures.set_scene(&self.scene)?;
                self.wake.notify();
            }
        }
        Ok(())
    }

    /// Starts asynchronous capture and orders the transparent overlay. Call
    /// is_ready before hiding real source windows for a transition handoff.
    pub fn show(&mut self) -> PreviewResult<()> {
        if !self.visible {
            self.visible = true;
            self.revision += 1;
            self._root.setHidden(true);
            let result: PreviewResult<()> = (|| {
                self.captures.set_scene(&self.scene)?;
                self.dirty = true;
                self.render(Instant::now())?;
                self.window.order_above(None)?;
                Ok(())
            })();
            if result.is_err() {
                let _ = self.hide();
            }
            result?;
        }
        Ok(())
    }

    /// Open the current scene from supplied source geometry. Capture quality
    /// comes from the destination scene; the timeline starts when a visible
    /// source has pixels, so asynchronous capture does not consume the animation.
    pub fn show_animated(&mut self, from: &[Preview], transition: Transition) -> PreviewResult<()> {
        validate_scene(from)?;
        let destination = self.scene.clone();
        let mut origin = from.to_vec();
        for p in &mut origin {
            if let Some(to) = destination
                .iter()
                .find(|q| p.window_id == q.window_id && p.process_id == q.process_id)
            {
                p.mode = to.mode;
                p.resolution = to.resolution;
                p.priority = to.priority;
            }
        }
        self.set_scene(&origin)?;
        self.transition_to(&destination, transition)?;
        self.animation.as_mut().unwrap().start = None;
        self.show()
    }

    pub fn hide(&mut self) -> PreviewResult<()> {
        self.window.order_out()?;
        self.visible = false;
        self._root.setHidden(true);
        self.pacing.pause();
        self.gpu.discard_presentation();
        self.animation = None;
        self.captures.clear();
        self.gpu.reap()?;
        Ok(())
    }

    /// Move an already-presented scene with sibling CA layers in the caller's
    /// LayerTransaction, without waiting for capture, a drawable, or GPU work.
    /// Geometry and clipping move together. A replacement GPU frame atomically
    /// rebases this translation when it is presented.
    pub fn translate(&mut self, delta: CGPoint) -> PreviewResult<()> {
        if !delta.x.is_finite() || !delta.y.is_finite() {
            return Err(PreviewError::InvalidScene);
        }
        let current = self.gpu.is_presented(self.revision);
        self.revision += 1;
        if current {
            self.gpu.mark_translated(self.revision);
        }
        self.offset.x += delta.x;
        self.offset.y += delta.y;
        let shift = |p: &mut Preview| {
            for column in 0..4 {
                let w = p.transform[column * 4 + 3];
                p.transform[column * 4] += delta.x as f32 * w;
                p.transform[column * 4 + 1] += delta.y as f32 * w;
            }
            if let Some(clip) = &mut p.clip {
                clip.origin.x += delta.x;
                clip.origin.y += delta.y;
            }
        };
        for p in &mut self.scene {
            shift(p);
        }
        if let Some(a) = &mut self.animation {
            for p in a.from.iter_mut().chain(&mut a.to) {
                shift(p);
            }
        }
        self.dirty = true;
        self.position_layer();
        self.wake.notify();
        Ok(())
    }

    fn position_layer(&self) {
        let _transaction = LayerTransaction::disabled();
        self.layer.setFrame(CGRect::new(
            CGPoint::new(
                self.offset.x - self.presented_offset.x,
                self.offset.y - self.presented_offset.y,
            ),
            self.bounds.size,
        ));
    }

    pub fn resize(&mut self, bounds: CGRect, scale: f64) -> PreviewResult<()> {
        if bounds.size.width * scale > 16384.
            || bounds.size.height * scale > 16384.
            || !scale.is_finite()
            || scale <= 0.0
            || ![
                bounds.origin.x,
                bounds.origin.y,
                bounds.size.width,
                bounds.size.height,
            ]
            .iter()
            .all(|v| v.is_finite())
            || bounds.size.width <= 0.0
            || bounds.size.height <= 0.0
        {
            return Err(PreviewError::InvalidScene);
        }
        self.gpu.discard_presentation();
        self.pacing.pause();
        self.pacing.pause();
        self._root.setHidden(true);
        self.revision += 1;
        let _layers = LayerTransaction::disabled().flush_on_commit();
        let transaction = WindowTransaction::new()?;
        transaction.set_rounded_frame(&self.window, bounds, 0.0)?;
        let local = CGRect::new(CGPoint::ZERO, bounds.size);
        self.surface.set_bounds_in(&transaction, local);
        self._root.setFrame(local);
        self.layer.setFrame(local);
        self.layer.setContentsScale(scale);
        self.layer.setDrawableSize(objc2_core_foundation::CGSize::new(
            bounds.size.width * scale,
            bounds.size.height * scale,
        ));
        self.window.set_resolution(scale)?;
        self.surface.set_scale(scale)?;
        transaction.commit();
        self.bounds = bounds;
        self.scale = scale;
        self.position_layer();
        self.dirty = true;
        self.wake.notify();
        Ok(())
    }

    pub fn transition_to(
        &mut self,
        destination: &[Preview],
        transition: Transition,
    ) -> PreviewResult<()> {
        // Materialize the current interpolated scene before reversing or cancelling.
        self.interpolate(Instant::now());
        let from = self.scene.clone();
        validate_scene(destination)?;
        let to = destination.to_vec();
        // A transition's union remains captured until its final presentation.
        let mut union = to.clone();
        for p in &from {
            if !union.iter().any(|q| q.window_id == p.window_id) {
                union.push(p.clone());
            }
        }
        if self.visible {
            self.captures.set_scene(&union)?;
        }
        self.revision += 1;
        self.animation = Some(Animation {
            from,
            to,
            start: Some(Instant::now()),
            transition,
            progress: None,
            sampled: None,
        });
        self.dirty = true;
        self.wake.notify();
        Ok(())
    }

    /// Interactive progress is clamped; use 0 to reverse and 1 to finish. Calling
    /// set_scene cancels immediately without changing any source window geometry.
    pub fn set_progress(&mut self, progress: f32) -> PreviewResult<()> {
        if !progress.is_finite() {
            return Err(PreviewError::InvalidScene);
        }
        if let Some(a) = &mut self.animation {
            let progress = Some(progress.clamp(0.0, 1.0));
            if a.progress == progress {
                return Ok(());
            }
            a.progress = progress;
            self.dirty = true;
            self.wake.notify();
        }
        Ok(())
    }

    pub fn is_animating(&self) -> bool {
        self.animation
            .as_ref()
            .is_some_and(|a| a.progress.is_none() && a.start.is_some())
    }

    fn interpolate(&mut self, now: Instant) {
        let Some(a) = &mut self.animation else {
            return;
        };
        let t = a.progress.unwrap_or_else(|| {
            (a.start.map_or(0., |start| now.saturating_duration_since(start).as_secs_f32())
                / a.transition.duration.as_secs_f32().max(f32::EPSILON))
            .clamp(0., 1.)
        });
        let t = match a.transition.easing {
            Easing::Linear => t,
            Easing::EaseInOut => t * t * (3. - 2. * t),
            Easing::Spring => {
                // Unit step of a critically damped oscillator. Normalizing its
                // endpoint avoids a discontinuity when the timeline completes.
                let step = |x: f32| 1. - (1. + 8. * x) * (-8. * x).exp();
                step(t) / step(1.)
            }
        };
        if a.sampled == Some(t) {
            return;
        }
        a.sampled = Some(t);
        self.revision += 1;
        self.scene.clear();
        for to in &a.to {
            let mut p = to.clone();
            if let Some(from) = a
                .from
                .iter()
                .find(|p| p.window_id == to.window_id && p.process_id == to.process_id)
            {
                let lerp = |a: f64, b: f64| a + (b - a) * t as f64;
                p.frame = CGRect::new(
                    CGPoint::new(
                        lerp(from.frame.origin.x, to.frame.origin.x),
                        lerp(from.frame.origin.y, to.frame.origin.y),
                    ),
                    objc2_core_foundation::CGSize::new(
                        lerp(from.frame.size.width, to.frame.size.width),
                        lerp(from.frame.size.height, to.frame.size.height),
                    ),
                );
                for i in 0..16 {
                    p.transform[i] = from.transform[i] + (to.transform[i] - from.transform[i]) * t;
                }
                p.opacity = from.opacity + (to.opacity - from.opacity) * t;
                p.corner_radius = from.corner_radius + (to.corner_radius - from.corner_radius) * t;
                p.border_width = from.border_width + (to.border_width - from.border_width) * t;
                if t < 1. && from.clip != to.clip {
                    p.clip = match (from.clip, to.clip) {
                        (Some(a), Some(b)) => Some(CGRect::new(
                            CGPoint::new(a.origin.x.min(b.origin.x), a.origin.y.min(b.origin.y)),
                            objc2_core_foundation::CGSize::new(
                                a.max().x.max(b.max().x) - a.origin.x.min(b.origin.x),
                                a.max().y.max(b.max().y) - a.origin.y.min(b.origin.y),
                            ),
                        )),
                        _ => None,
                    };
                }
            } else {
                p.opacity *= t;
            }
            self.scene.push(p);
        }
        for from in &a.from {
            if !a.to.iter().any(|p| p.window_id == from.window_id) {
                let mut p = from.clone();
                p.opacity *= 1. - t;
                self.scene.push(p);
            }
        }
        self.dirty = true;
        if t >= 1.0 {
            self.scene.clone_from(&a.to);
            self.animation = None;
            if self.visible {
                let _ = self.captures.set_scene(&self.scene);
            }
        }
    }

    /// Returns true when a GPU frame was submitted. The display link supplies
    /// predicted timing when a drawable is available. Never waits for GPU completion.
    pub fn render(&mut self, presentation: Instant) -> PreviewResult<bool> {
        objc2::rc::autoreleasepool(|_| self.render_inner(presentation))
    }

    fn render_inner(&mut self, presentation: Instant) -> PreviewResult<bool> {
        self.wake.pending.store(false, Ordering::Release);
        self.gpu.reap()?;
        if !self.visible {
            self.gpu.discard_presentation();
            return Ok(false);
        }
        if let Some(presentation) = self.gpu.take_presentation() {
            let _transaction = LayerTransaction::disabled().flush_on_commit();
            if self.presented_offset != presentation.offset {
                self.presented_offset = presentation.offset;
                self.position_layer();
            }
            self._root.setHidden(false);
            presentation.drawable.present();
        }
        self.captures.pump()?;
        let update = self.pacing.take();
        let presentation = update
            .as_ref()
            .map(|u| {
                let target = u.targetPresentationTimestamp();
                let delta = (target - objc2_quartz_core::CACurrentMediaTime()).clamp(0.0, 0.1);
                Instant::now() + Duration::from_secs_f64(delta)
            })
            .unwrap_or(presentation);
        let changed = self.captures.import(&mut self.gpu)?;
        self.dirty |= changed;
        if let Some(a) = &mut self.animation {
            if a.start.is_none() && update.is_some() {
                let visible_origin = a.from.iter().any(|p| p.opacity > 0.);
                let ready = if visible_origin { &a.from } else { &a.to }
                    .iter()
                    .any(|p| p.opacity > 0. && self.captures.frame(p).is_some());
                let origin_unavailable = a.from.iter().filter(|p| p.opacity > 0.).all(|p| {
                    matches!(
                        self.captures.status(p.window_id),
                        Some(SourceStatus::Failed(_) | SourceStatus::Blank | SourceStatus::Stopped)
                    )
                });
                let fallback_ready = origin_unavailable
                    && a.to.iter().any(|p| p.opacity > 0. && self.captures.frame(p).is_some());
                if ready || fallback_ready {
                    a.start = Some(presentation);
                }
            }
        }
        if update.is_some() || !self.is_animating() {
            self.interpolate(presentation);
        }
        if !self.dirty {
            if update.is_some() {
                self.pacing.pause();
            }
            return Ok(false);
        }
        // Do not spend the startup drawable on a blank capture scene. The
        // capture callback wakes us when pixels exist; no timer/polling is needed.
        if !self.gpu.has_submitted()
            && !self
                .animation
                .as_ref()
                .map_or(&self.scene, |a| &a.to)
                .iter()
                .any(|p| p.opacity > 0. && self.captures.frame(p).is_some())
        {
            return Ok(false);
        }
        let Some(update) = update else {
            self.pacing.request();
            return Ok(false);
        };
        let submitted = self.gpu.render(
            &update.drawable(),
            self.bounds.size,
            &self.scene,
            &self.captures,
            self.offset,
            self.revision,
        )?;
        if submitted {
            self.dirty = false;
        }
        if !submitted {
            self.pacing.pause();
        } else {
            self.pacing.request();
        }
        Ok(submitted)
    }

    pub fn stats(&self) -> PreviewStats {
        let mut s = self.gpu.stats();
        self.captures.stats(&mut s);
        s
    }

    /// Per-source status distinguishes a stale/idle source from a failed stream.
    pub fn refresh_sources(&mut self) { self.captures.refresh(); }

    pub fn source_status(&self, window_id: u32) -> Option<SourceStatus> {
        self.captures.status(window_id)
    }
}
impl Drop for PreviewCompositor {
    fn drop(&mut self) { let _ = self.hide(); }
}

#[derive(Clone, Debug)]
pub enum SourceStatus {
    Starting,
    Complete { age: Duration },
    Idle { age: Option<Duration> },
    Blank,
    Stopped,
    Failed(String),
}

fn validate_scene(scene: &[Preview]) -> PreviewResult<()> {
    if scene.len() > 256
        || scene.iter().any(|p| !p.valid())
        || scene
            .iter()
            .enumerate()
            .any(|(i, p)| scene[..i].iter().any(|q| q.window_id == p.window_id))
    {
        Err(PreviewError::InvalidScene)
    } else {
        Ok(())
    }
}
