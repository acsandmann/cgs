//! WindowServer-hosted Metal previews. All control and presentation is owner-thread confined.
//! Capture callbacks only replace a single retained, immutable sample; they never render.
mod capture;
mod gpu;
mod pacing;
mod timing;

use std::cell::RefCell;
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

/// Capture inventory is independent of scene geometry and camera movement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewSource {
    pub window_id: u32,
    pub process_id: i32,
    pub mode: CaptureMode,
    pub resolution: CaptureResolution,
    pub priority: i32,
    /// Minimum interval for live frames; independent of display/animation cadence.
    pub frame_interval: Duration,
}
impl PreviewSource {
    fn valid(&self) -> bool {
        self.window_id != 0
            && self.process_id > 0
            && !self.frame_interval.is_zero()
            && self.frame_interval.as_secs() <= 60
            && !matches!(
                self.resolution,
                CaptureResolution::Thumbnail { width: 0, .. }
                    | CaptureResolution::Thumbnail { height: 0, .. }
            )
    }
}
impl From<&Preview> for PreviewSource {
    fn from(p: &Preview) -> Self {
        Self {
            window_id: p.window_id,
            process_id: p.process_id,
            mode: p.mode,
            resolution: p.resolution,
            priority: p.priority,
            frame_interval: Duration::from_secs_f64(1. / 120.),
        }
    }
}

/// Shared scene transform in display-local points; clipping remains in viewport coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewCamera {
    pub scale: f32,
    pub translation: [f32; 2],
}
impl Default for PreviewCamera {
    fn default() -> Self {
        Self {
            scale: 1.,
            translation: [0.; 2],
        }
    }
}

/// Immutable owner-thread frame handle. Cloning pins both the texture and its
/// capture-pool sample; retain it until any external GPU reads have completed.
#[derive(Clone)]
pub struct PreviewFrame {
    window_id: u32,
    process_id: i32,
    frame: capture::Frame,
    _owner: std::marker::PhantomData<Rc<()>>,
}
impl PreviewFrame {
    pub fn window_id(&self) -> u32 { self.window_id }

    pub fn process_id(&self) -> i32 { self.process_id }

    /// Read-only capture texture. Do not write pixels or mutate its IOSurface.
    pub fn texture(&self) -> &objc2::runtime::ProtocolObject<dyn objc2_metal::MTLTexture> {
        &self.frame.texture
    }

    pub fn texture_coordinates(&self) -> [f32; 4] { self.frame.uv }
}

/// Array order is back to front. Geometry is in canvas-local, top-left logical points.
/// Standalone canvases use display-local coordinates.
/// `process_id` guards against reusing an ID that now belongs to another application.
#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    pub window_id: u32,
    pub process_id: i32,
    pub frame: CGRect,
    /// Column-major homogeneous transform, applied in canvas-local logical coordinates.
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
    start: Instant,
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
    /// Hardware presentation timestamp relative to the latest show(), not
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

/// Additional diagnostics keep the existing PreviewStats layout/API unchanged.
#[derive(Default, Clone, Copy, Debug)]
pub struct PreviewDiagnostics {
    /// Shared high-water reservations, including asynchronously retiring captures.
    pub process_reserved_capture_bytes: usize,
    /// Hardware presentation intervals; includes intentional idle gaps.
    pub presentation_interval: FrameTimes,
    pub submission_to_present: FrameTimes,
    pub callback_to_owner: FrameTimes,
    /// Positive hardware timestamp minus the display link's target timestamp.
    pub presentation_lateness: FrameTimes,
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
    fn new(callback: impl Fn() + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            pending: AtomicBool::new(false),
            callback: Box::new(callback),
        })
    }

    fn notify(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            (self.callback)();
        }
    }
}

/// Display-local preview canvas, optionally hosted in its own CGS window. Not Send or Sync.
///
/// Call `render` after the coalesced wake callback. A dirty-driven CAMetalDisplayLink
/// supplies drawables and animation timing; do not add another clock. The callback may run
/// on any framework queue and must
/// only schedule work on this object's owner. It must not call back into the compositor.
/// hide stops this canvas clock; hiding the source owner also retires shared capture.
pub struct PreviewCompositor {
    pacing: pacing::Pacing,
    gpu: Gpu,
    captures: Rc<RefCell<Captures>>,
    owns_captures: bool,
    layer: Retained<CAMetalLayer>,
    _root: Retained<CALayer>,
    // Unbind before destroying the window; GPU submissions retain their own source samples.
    surface: Option<WindowSurface>,
    window: Option<CgsWindow>,
    host_window_id: u32,
    bounds: CGRect,
    scale: f64,
    camera: PreviewCamera,
    scene: Vec<Preview>,
    sources: Option<Vec<PreviewSource>>,
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
        Self::create(bounds, scale, Wake::new(wake), None, None)
    }

    /// Attach a renderer to an existing top-left, display-local CA hierarchy.
    /// The caller owns window ordering, surface bounds, backing scale and lifetime.
    /// Call attach_to again to reuse this renderer with a new host after hide().
    pub fn new_attached(
        window_id: u32,
        parent: &CALayer,
        bounds: CGRect,
        scale: f64,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> PreviewResult<Self> {
        if window_id == 0 {
            return Err(PreviewError::InvalidScene);
        }
        Self::create(bounds, scale, Wake::new(wake), Some((window_id, parent)), None)
    }

    fn create(
        bounds: CGRect,
        scale: f64,
        wake: Arc<Wake>,
        host: Option<(u32, &CALayer)>,
        shared: Option<Rc<RefCell<Captures>>>,
    ) -> PreviewResult<Self> {
        static SUPPORTED: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
            objc2::runtime::AnyClass::get(c"CAMetalDisplayLink").is_some()
        });
        if !*SUPPORTED {
            return Err(PreviewError::Metal(
                "Metal previews require macOS 14 or later".into(),
            ));
        }
        let _main = MainThreadMarker::new()
            .ok_or_else(|| PreviewError::Metal("create on main thread".into()))?;
        if !objc2_core_graphics::CGPreflightScreenCaptureAccess() {
            return Err(PreviewError::PermissionDenied);
        }
        validate_bounds(bounds, scale)?;
        let owns_captures = shared.is_none();
        let captures = shared.unwrap_or_else(|| Rc::new(RefCell::new(Captures::new(wake.clone()))));
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
        root.setGeometryFlipped(host.is_none());
        root.setMasksToBounds(true);
        root.setZPosition(1.);
        root.addSublayer(&layer);
        let (window, surface, host_window_id) = if let Some((id, parent)) = host {
            parent.addSublayer(&root);
            (None, None, id)
        } else {
            let window = CgsWindow::new_compositor(bounds, 0.0)?;
            window.set_resolution(scale)?;
            let surface = WindowSurface::new_scaled(window.id(), local, &root, scale)?;
            if let Some(color_space) = &color_space {
                surface.set_color_space(color_space)?;
            }
            surface.flush();
            let id = window.id();
            (Some(window), Some(surface), id)
        };
        let pacing = pacing::Pacing::new(&layer, wake.clone());
        Ok(Self {
            pacing,
            gpu,
            captures,
            owns_captures,
            layer,
            _root: root,
            surface,
            window,
            host_window_id,
            bounds,
            scale,
            camera: PreviewCamera::default(),
            scene: Vec::new(),
            sources: None,
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
                .all(|p| self.captures.borrow().frame(p).is_some())
    }

    /// A usable captured frame is available, including a retained last-valid frame.
    pub fn has_frame(&self, window_id: u32) -> bool {
        self.scene
            .iter()
            .find(|p| p.window_id == window_id)
            .is_some_and(|p| self.captures.borrow().frame(p).is_some())
    }

    /// Independently movable canvas sharing this renderer's capture inventory.
    /// The owner controls sources and capture lifetime; a canvas never starts,
    /// reconfigures or stops a source. All canvases stay on the same owner thread.
    pub fn new_canvas(
        &self,
        window_id: u32,
        parent: &CALayer,
        bounds: CGRect,
        scale: f64,
    ) -> PreviewResult<Self> {
        if window_id == 0 {
            return Err(PreviewError::InvalidScene);
        }
        Self::create(
            bounds,
            scale,
            self.wake.clone(),
            Some((window_id, parent)),
            Some(self.captures.clone()),
        )
    }

    /// Change the shared camera without updating source inventory or per-window transforms.
    pub fn set_camera(&mut self, camera: PreviewCamera) -> PreviewResult<()> {
        if !camera.scale.is_finite()
            || camera.scale <= 0.
            || !camera.translation.iter().all(|v| v.is_finite())
        {
            return Err(PreviewError::InvalidScene);
        }
        if self.camera != camera {
            self.camera = camera;
            self.revision += 1;
            self.dirty = true;
            self.wake.notify();
        }
        Ok(())
    }

    pub fn captured_frame(&self, window_id: u32) -> Option<PreviewFrame> {
        let captures = self.captures.borrow();
        let (process_id, frame) = self
            .scene
            .iter()
            .find(|p| p.window_id == window_id)
            .and_then(|p| captures.frame(p).map(|f| (p.process_id, f.clone())))
            .or_else(|| captures.captured_frame(window_id))?;
        Some(PreviewFrame {
            window_id,
            process_id,
            frame,
            _owner: std::marker::PhantomData,
        })
    }

    /// Seed another renderer with last-valid pixels, without copying or recapturing.
    /// Identity is still validated against current SCK discovery before new capture.
    pub fn reuse_frame(&mut self, frame: &PreviewFrame) {
        if self
            .captures
            .borrow_mut()
            .reuse_frame(frame.window_id, frame.process_id, &frame.frame)
        {
            self.dirty = true;
            self.wake.notify();
        }
    }

    pub fn window_id(&self) -> u32 { self.host_window_id }

    pub fn set_level(&self, level: i32) -> PreviewResult<()> {
        let window = self.window.as_ref().ok_or_else(|| {
            PreviewError::Metal("attached renderer: host controls window level".into())
        })?;
        Ok(window.set_level(level)?)
    }

    /// Reparent an attached renderer without rebuilding Metal resources or drawables.
    pub fn attach_to(&mut self, window_id: u32, parent: &CALayer) -> PreviewResult<()> {
        if self.window.is_some() || window_id == 0 || self.visible {
            return Err(PreviewError::InvalidScene);
        }
        parent.addSublayer(&self._root);
        self.host_window_id = window_id;
        Ok(())
    }

    /// Replace capture policy without changing geometry or presentation state.
    pub fn set_sources(&mut self, sources: &[PreviewSource]) -> PreviewResult<()> {
        if !self.owns_captures
            || sources.len() > 256
            || sources
                .iter()
                .enumerate()
                .any(|(i, p)| !p.valid() || sources[..i].iter().any(|q| q.window_id == p.window_id))
        {
            return Err(PreviewError::InvalidScene);
        }
        if self.sources.as_deref() == Some(sources) {
            return Ok(());
        }
        let inventory = self.sources.get_or_insert_with(Vec::new);
        inventory.clear();
        inventory.extend_from_slice(sources);
        if self.visible {
            self.captures.borrow_mut().set_sources(sources.iter().copied())?;
        }
        self.wake.notify();
        Ok(())
    }

    /// Update existing source policies without rebuilding the inventory or discovering windows.
    pub fn update_sources(&mut self, updates: &[PreviewSource]) -> PreviewResult<()> {
        let sources = self.sources.as_mut().ok_or(PreviewError::InvalidScene)?;
        if !self.owns_captures
            || updates.len() > 256
            || updates.iter().any(|p| {
                !p.valid()
                    || !sources
                        .iter()
                        .any(|q| p.window_id == q.window_id && p.process_id == q.process_id)
            })
        {
            return Err(PreviewError::InvalidScene);
        }
        let mut changed = false;
        let mut inventory = false;
        for update in updates {
            let source = sources.iter_mut().find(|s| s.window_id == update.window_id).unwrap();
            inventory |=
                source.mode == CaptureMode::Inactive || update.mode == CaptureMode::Inactive;
            changed |= *source != *update;
            *source = *update;
        }
        if changed && self.visible {
            if inventory {
                self.captures.borrow_mut().set_sources(sources.iter().copied())?;
            } else {
                self.captures.borrow_mut().update_sources(updates);
            }
            self.wake.notify();
        }
        Ok(())
    }

    /// Replace visual state only. Capture inventory is controlled by set_sources.
    /// The compatibility set_scene API updates both responsibilities.
    pub fn set_visual_scene(&mut self, scene: &[Preview]) -> PreviewResult<()> {
        validate_scene(scene)?;
        if self.sources.is_none() {
            self.sources = Some(self.scene.iter().map(PreviewSource::from).collect());
        }
        self.replace_scene(scene, false)
    }

    pub fn set_scene(&mut self, scene: &[Preview]) -> PreviewResult<()> {
        validate_scene(scene)?;
        self.sources = None;
        self.replace_scene(scene, true)
    }

    fn replace_scene(&mut self, scene: &[Preview], capture: bool) -> PreviewResult<()> {
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
                if capture && self.owns_captures {
                    self.captures.borrow_mut().set_scene(&self.scene)?;
                }
                self.wake.notify();
            }
        }
        Ok(())
    }

    /// Edit one retained preview's border without touching capture policy or geometry.
    pub fn set_preview_style(
        &mut self,
        window_id: u32,
        width: f32,
        color: [f32; 4],
    ) -> PreviewResult<()> {
        if !width.is_finite() || width < 0. || !color.iter().all(|v| v.is_finite()) {
            return Err(PreviewError::InvalidScene);
        }
        let mut changed = false;
        for p in self
            .scene
            .iter_mut()
            .chain(self.animation.iter_mut().flat_map(|a| a.from.iter_mut().chain(&mut a.to)))
        {
            if p.window_id == window_id && (p.border_width != width || p.border_color != color) {
                p.border_width = width;
                p.border_color = color;
                changed = true;
            }
        }
        if changed {
            self.revision += 1;
            self.dirty = true;
            self.wake.notify();
        }
        Ok(())
    }

    /// Starts asynchronous capture and orders the transparent overlay. Call
    /// is_ready before hiding real source windows for a transition handoff.
    pub fn show(&mut self) -> PreviewResult<()> {
        if !self.visible {
            self.gpu.begin_session();
            self.visible = true;
            self.revision += 1;
            self._root.setHidden(true);
            let result: PreviewResult<()> = (|| {
                if self.owns_captures {
                    if let Some(sources) = &self.sources {
                        self.captures.borrow_mut().set_sources(sources.iter().copied())?;
                    } else {
                        self.captures.borrow_mut().set_scene(&self.scene)?;
                    }
                }
                self.dirty = true;
                self.render(Instant::now())?;
                if let Some(window) = &self.window {
                    window.order_above(None)?;
                }
                Ok(())
            })();
            if result.is_err() {
                let _ = self.hide();
            }
            result?;
        }
        Ok(())
    }

    pub fn hide(&mut self) -> PreviewResult<()> {
        if let Some(window) = &self.window {
            window.order_out()?;
        }
        self.visible = false;
        self._root.setHidden(true);
        if self.window.is_none() {
            self._root.removeFromSuperlayer();
        }
        self.pacing.pause();
        self.gpu.discard_presentation();
        self.animation = None;
        if self.owns_captures {
            self.captures.borrow_mut().clear();
        }
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
        if delta == CGPoint::ZERO {
            return Ok(());
        }
        let current = self.gpu.is_presented(self.revision);
        self.revision += 1;
        if current {
            self.gpu.mark_translated(self.revision);
        }
        self.offset.x += delta.x;
        self.offset.y += delta.y;
        let scale = self.camera.scale;
        let shift = |p: &mut Preview| {
            for column in 0..4 {
                let w = p.transform[column * 4 + 3];
                p.transform[column * 4] += delta.x as f32 / scale * w;
                p.transform[column * 4 + 1] += delta.y as f32 / scale * w;
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
        validate_bounds(bounds, scale)?;
        self.gpu.discard_presentation();
        self.pacing.pause();
        self._root.setHidden(true);
        self.revision += 1;
        let _layers = LayerTransaction::disabled().flush_on_commit();
        let transaction = WindowTransaction::new()?;
        if let Some(window) = &self.window {
            transaction.set_rounded_frame(window, bounds, 0.0)?;
        }
        let local = CGRect::new(CGPoint::ZERO, bounds.size);
        if let Some(surface) = &self.surface {
            surface.set_bounds_in(&transaction, local);
        }
        self._root.setFrame(local);
        self.layer.setFrame(local);
        self.layer.setContentsScale(scale);
        self.layer.setDrawableSize(objc2_core_foundation::CGSize::new(
            bounds.size.width * scale,
            bounds.size.height * scale,
        ));
        if let Some(window) = &self.window {
            window.set_resolution(scale)?;
        }
        if let Some(surface) = &self.surface {
            surface.set_scale(scale)?;
        }
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
        if self.visible && self.owns_captures && self.sources.is_none() {
            // Compatibility scenes capture their transition union. Explicit
            // inventories are independent of animation geometry.
            let mut union = to.clone();
            for p in &from {
                if !union.iter().any(|q| q.window_id == p.window_id) {
                    union.push(p.clone());
                }
            }
            self.captures.borrow_mut().set_scene(&union)?;
        }
        self.revision += 1;
        self.animation = Some(Animation {
            from,
            to,
            start: Instant::now(),
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

    pub fn is_visible(&self) -> bool { self.visible }

    pub fn is_animating(&self) -> bool {
        self.animation.as_ref().is_some_and(|a| a.progress.is_none())
    }

    fn interpolate(&mut self, now: Instant) {
        let Some(a) = &mut self.animation else {
            return;
        };
        let t = a.progress.unwrap_or_else(|| {
            (now.saturating_duration_since(a.start).as_secs_f32()
                / a.transition.duration.as_secs_f32().max(f32::EPSILON))
            .clamp(0., 1.)
        });
        let t = match a.transition.easing {
            Easing::Linear => t,
            Easing::EaseInOut => t * t * (3. - 2. * t),
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
            if self.visible && self.owns_captures && self.sources.is_none() {
                let _ = self.captures.borrow_mut().set_scene(&self.scene);
            }
        }
    }

    /// Prepare retained scene changes once a display-link drawable is available,
    /// before calling render. Neither method acquires a drawable or waits for GPU work.
    pub fn has_display_update(&self) -> bool { self.pacing.has_update() }

    pub fn request_frame(&self) {
        if self.visible {
            self.pacing.request();
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
        if self.sources.is_some() && self.scene.is_empty() && self.animation.is_none() {
            if self.dirty {
                let _transaction = LayerTransaction::disabled().flush_on_commit();
                self.gpu.discard_presentation();
                self._root.setHidden(true);
                self.pacing.pause();
                self.dirty = false;
            }
            self.captures.borrow_mut().update(&mut self.gpu, &self.scene)?;
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
        let update = self.pacing.take();
        let presentation = update
            .as_ref()
            .map(|u| {
                let target = u.targetPresentationTimestamp();
                let delta = (target - objc2_quartz_core::CACurrentMediaTime()).clamp(0.0, 0.1);
                Instant::now() + Duration::from_secs_f64(delta)
            })
            .unwrap_or(presentation);
        let changed = self.captures.borrow_mut().update(&mut self.gpu, &self.scene)?;
        self.dirty |= changed || self.gpu.sources_changed(&self.scene, &self.captures.borrow());
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
                .any(|p| p.opacity > 0. && self.captures.borrow().frame(p).is_some())
        {
            self.pacing.pause();
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
            self.camera,
            &self.captures.borrow(),
            self.offset,
            self.revision,
            update.targetPresentationTimestamp(),
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

    pub fn diagnostics(&self) -> PreviewDiagnostics {
        PreviewDiagnostics {
            process_reserved_capture_bytes: capture::reserved_bytes(),
            ..self.gpu.diagnostics()
        }
    }

    pub fn stats(&self) -> PreviewStats {
        let mut s = self.gpu.stats();
        self.captures.borrow().stats(&mut s);
        s
    }

    /// Per-source status distinguishes a stale/idle source from a failed stream.
    pub fn refresh_sources(&mut self) { self.captures.borrow_mut().refresh(); }

    pub fn source_status(&self, window_id: u32) -> Option<SourceStatus> {
        self.captures.borrow().status(window_id)
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

fn validate_bounds(bounds: CGRect, scale: f64) -> PreviewResult<()> {
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
    Ok(())
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
