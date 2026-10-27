use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, ClassType, DeclaredClass, define_class, msg_send, sel};
use objc2_core_foundation::{CFDictionary, CFRetained};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::*;
use objc2_foundation::{
    NSArray, NSDictionary, NSError, NSNumber, NSObject, NSObjectProtocol, NSProcessInfo, NSString,
    ns_string,
};
use objc2_metal::{
    MTLDevice, MTLPixelFormat, MTLStorageMode, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};
use objc2_screen_capture_kit::*;

use super::gpu::Gpu;
use super::{
    CaptureMode, CaptureResolution, Preview, PreviewError, PreviewResult, PreviewSource,
    PreviewStats, SourceStatus, Wake,
};

// A retained SCK sample is immutable here. No attachment mutation, base-address
// access, or CPU writes are allowed. Exclusive mailbox transfer hands the sample
// from the serial capture queue to the owner; Metal reads its IOSurface. Keeping
// the *sample*, not just the IOSurface/texture, pins the CV pool entry until GPU
// completion. CoreMedia retain/release may occur on either queue.
pub(super) struct Sample(pub CFRetained<CMSampleBuffer>);
unsafe impl Send for Sample {}
// Shared references only clone the retain or release after a command completes;
// the immutable sample and its attachments are never mutated.
unsafe impl Sync for Sample {}

#[derive(Clone, Copy, PartialEq)]
enum Lifecycle {
    Starting,
    Running,
    Updating,
    Stopping,
    Stopped,
    Failed,
}
struct Mailbox {
    lifecycle: Lifecycle,
    control_pending: bool,
    dimensions: (usize, usize),
    interval: Duration,
    latest: Option<Sample>,
    status: SourceStatus,
    last: Option<Instant>,
    sequence: u64,
    replaced: u64,
    closed: bool,
    started: Instant,
    first_frame_us: u64,
    last_callback_us: u64,
    capture_to_gpu_us: u64,
}
impl Mailbox {
    fn publish(&mut self, sample: &CMSampleBuffer) -> bool {
        if !unsafe { sample.is_valid() } {
            return false;
        }
        // Validate storage before accepting; idle buffers need not have images.
        let Some(image) = (unsafe { sample.image_buffer() }) else {
            return false;
        };
        if CVPixelBufferGetPixelFormatType(&image) != kCVPixelFormatType_32BGRA {
            return false;
        }
        let sample = Sample(unsafe { CFRetained::retain(std::ptr::NonNull::from(sample)) });
        if self.latest.replace(sample).is_some() {
            self.replaced += 1;
        }
        if self.sequence == 0 {
            self.first_frame_us = self.started.elapsed().as_micros() as u64;
        }
        self.sequence += 1;
        self.last = Some(Instant::now());
        self.status = SourceStatus::Complete { age: std::time::Duration::ZERO };
        true
    }
}
impl Default for Mailbox {
    fn default() -> Self {
        Self {
            lifecycle: Lifecycle::Starting,
            control_pending: false,
            dimensions: (0, 0),
            interval: Duration::ZERO,
            latest: None,
            status: SourceStatus::Starting,
            last: None,
            sequence: 0,
            replaced: 0,
            closed: false,
            started: Instant::now(),
            first_frame_us: 0,
            last_callback_us: 0,
            capture_to_gpu_us: 0,
        }
    }
}
struct OutputState {
    mailbox: Arc<Mutex<Mailbox>>,
    wake: Arc<Wake>,
}
define_class!(
    #[unsafe(super(NSObject))]
    #[ivars = OutputState]
    struct Output;
    unsafe impl NSObjectProtocol for Output {}
    unsafe impl SCStreamOutput for Output {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        unsafe fn sample(&self, _: &SCStream, sample: &CMSampleBuffer, kind: SCStreamOutputType) {
            let callback_start = Instant::now();
            if kind != SCStreamOutputType::Screen {
                return;
            }
            // SCK's sample attachments are a CFArray of CFDictionary. Status is
            // an NSNumber; absent/unknown metadata is never treated as a frame.
            let status = unsafe {
                sample.sample_attachments_array(false).and_then(|a| {
                    if a.count() == 0 {
                        return None;
                    }
                    let dict = &*a.value_at_index(0).cast::<CFDictionary>();
                    let value = dict.value(
                        (SCStreamFrameInfoStatus as *const objc2_foundation::NSString).cast(),
                    );
                    (!value.is_null()).then(|| (&*value.cast::<NSNumber>()).integerValue())
                })
            };
            let state = self.ivars();
            let mut mailbox = state.mailbox.lock().unwrap();
            if mailbox.closed {
                return;
            }
            match status {
                Some(0) if unsafe { sample.is_valid() } => {
                    if !mailbox.publish(sample) {
                        return;
                    }
                }
                Some(1) => {
                    if matches!(mailbox.status, SourceStatus::Idle { .. }) {
                        return;
                    }
                    mailbox.status = SourceStatus::Idle {
                        age: mailbox.last.map(|t| t.elapsed()),
                    };
                }
                Some(2 | 3) => {
                    if matches!(mailbox.status, SourceStatus::Blank) {
                        return;
                    }
                    mailbox.status = SourceStatus::Blank;
                }
                Some(5) => mailbox.status = SourceStatus::Stopped,
                _ => return,
            }
            mailbox.last_callback_us = callback_start.elapsed().as_micros() as u64;
            drop(mailbox);
            state.wake.notify();
        }
    }
    unsafe impl SCStreamDelegate for Output {
        #[unsafe(method(stream:didStopWithError:))]
        unsafe fn stopped(&self, _: &SCStream, error: &NSError) {
            let state = self.ivars();
            let mut mailbox = state.mailbox.lock().unwrap();
            if mailbox.closed {
                return;
            }
            mailbox.lifecycle = Lifecycle::Failed;
            mailbox.status = SourceStatus::Failed(error.to_string());
            drop(mailbox);
            state.wake.notify();
        }
    }
);

// Discovery returns immutable framework descriptions. Retained metadata crosses
// queues through a mutex; only read-only framework properties are accessed.
// Mutable capture state lives in mailboxes, not these shared descriptions.
#[derive(Clone)]
struct Content(Retained<SCShareableContent>);
unsafe impl Send for Content {}
#[derive(Default)]
struct Discovery {
    result: Option<Result<Content, String>>,
    pending: bool,
}

// Share discovery across displays and quick reopenings. This retains only
// immutable metadata, never streams or pixel buffers, and runs no idle timer.
#[derive(Default)]
struct Inventory {
    content: Option<(Instant, Content)>,
    pending: bool,
    waiters: Vec<(std::sync::Weak<Mutex<Discovery>>, std::sync::Weak<Wake>)>,
}
static INVENTORY: std::sync::LazyLock<Mutex<Inventory>> = std::sync::LazyLock::new(Mutex::default);
pub(super) fn prepare() { request_inventory(None, &[], false); }
fn request_inventory(
    waiter: Option<(&Arc<Mutex<Discovery>>, &Arc<Wake>)>,
    requested: &[PreviewSource],
    fresh: bool,
) {
    let mut inventory = INVENTORY.lock().unwrap();
    if !fresh {
        if let Some((time, content)) = &inventory.content {
            let current = time.elapsed() < std::time::Duration::from_secs(5)
                && requested.iter().all(|p| {
                    unsafe { content.0.windows() }.iter().any(|w| unsafe {
                        w.windowID() == p.window_id
                            && w.owningApplication().is_some_and(|a| a.processID() == p.process_id)
                    })
                });
            // Start known identities immediately, even when discovery must
            // refresh. Each source still checks the caller's owning PID. Missing
            // IDs stay pending until fresh discovery returns; stale metadata is
            // never used to manufacture an SCWindow for an unknown source.
            if let Some((discovery, wake)) = waiter {
                let mut discovery = discovery.lock().unwrap();
                discovery.pending = !current;
                discovery.result = Some(Ok(content.clone()));
                drop(discovery);
                wake.notify();
            }
            if current {
                return;
            }
        }
    }
    if let Some((discovery, wake)) = waiter {
        inventory.waiters.retain(|(d, _)| d.strong_count() != 0);
        inventory.waiters.push((Arc::downgrade(discovery), Arc::downgrade(wake)));
    }
    if inventory.pending {
        return;
    }
    inventory.pending = true;
    drop(inventory);
    let callback = RcBlock::new(|content: *mut SCShareableContent, error: *mut NSError| {
        let result = unsafe { Retained::retain(content) }.map(Content).ok_or_else(|| {
            unsafe { error.as_ref() }
                .map(ToString::to_string)
                .unwrap_or_else(|| "content discovery returned no content".into())
        });
        let mut inventory = INVENTORY.lock().unwrap();
        inventory.content = result.as_ref().ok().map(|c| (Instant::now(), c.clone()));
        inventory.pending = false;
        let waiters = std::mem::take(&mut inventory.waiters);
        drop(inventory);
        for (discovery, wake) in waiters {
            if let Some(discovery) = discovery.upgrade() {
                let mut discovery = discovery.lock().unwrap();
                discovery.result = Some(result.clone());
                discovery.pending = false;
            }
            if let Some(wake) = wake.upgrade() {
                wake.notify();
            }
        }
    });
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(true, false, &callback);
    }
}

#[derive(Clone)]
pub(super) struct Frame {
    pub texture: Retained<ProtocolObject<dyn MTLTexture>>,
    pub sample: Arc<Sample>,
    pub uv: [f32; 4],
    pub sequence: u64,
    pub display_time: f64,
    pub bytes: usize,
    received: Instant,
}
static FRAME_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static WAITERS: Mutex<Vec<std::sync::Weak<Wake>>> = Mutex::new(Vec::new());
static STREAMS: AtomicUsize = AtomicUsize::new(0);
static LIVE_STREAMS: AtomicUsize = AtomicUsize::new(0);
static CAPTURE_BYTES: AtomicUsize = AtomicUsize::new(0);
const PROCESS_CAPTURE_BYTES: usize = 512 * 1024 * 1024;
pub(super) fn reserved_bytes() -> usize { CAPTURE_BYTES.load(Ordering::Acquire) }
struct StreamLease {
    bytes: AtomicUsize,
    stream: std::sync::atomic::AtomicBool,
}
impl StreamLease {
    // Reservations survive asynchronous stop and close, just like stream admission.
    // Keep the high-water reservation until retirement so an old configuration's
    // surfaces and GPU readers cannot be undercounted during an upgrade.
    fn reserve(&self, bytes: usize) -> bool {
        let previous = self.bytes.load(Ordering::Acquire);
        if bytes <= previous {
            return true;
        }
        let extra = bytes - previous;
        if CAPTURE_BYTES
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                n.checked_add(extra).filter(|n| *n <= PROCESS_CAPTURE_BYTES)
            })
            .is_err()
        {
            return false;
        }
        self.bytes.store(bytes, Ordering::Release);
        true
    }

    fn acquire(speculative: bool) -> Option<Arc<Self>> {
        STREAMS
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                // Leave two process-wide jobs for newly visible/selected sources.
                (n < if speculative { 14 } else { 16 }).then_some(n + 1)
            })
            .ok()
            .map(|_| {
                Arc::new(Self {
                    bytes: AtomicUsize::new(0),
                    stream: std::sync::atomic::AtomicBool::new(false),
                })
            })
    }
}
impl Drop for StreamLease {
    fn drop(&mut self) {
        CAPTURE_BYTES.fetch_sub(self.bytes.load(Ordering::Acquire), Ordering::AcqRel);
        STREAMS.fetch_sub(1, Ordering::AcqRel);
        if self.stream.load(Ordering::Acquire) {
            LIVE_STREAMS.fetch_sub(1, Ordering::AcqRel);
        }
        let waiters = std::mem::take(&mut *WAITERS.lock().unwrap());
        for waiter in waiters {
            if let Some(wake) = waiter.upgrade() {
                wake.notify();
            }
        }
    }
}
// Immutable handles are used only on this serial queue. The output's mutable
// state is confined to its mailbox. A completion pins the stream/output/lease
// until it runs; a close during start/update chains stop after that completion.
struct StreamControl {
    stream: Retained<SCStream>,
    _output: Retained<Output>,
    _lease: Arc<StreamLease>,
    mailbox: Arc<Mutex<Mailbox>>,
    wake: Arc<Wake>,
    queue: DispatchRetained<DispatchQueue>,
}
unsafe impl Send for StreamControl {}
unsafe impl Sync for StreamControl {}
impl StreamControl {
    fn submit(
        self: &Arc<Self>,
        action: Lifecycle,
        success: Lifecycle,
        dimensions: Option<((usize, usize), Duration)>,
    ) {
        let control = self.clone();
        self.queue.exec_async(move || {
            objc2::rc::autoreleasepool(|_| control.run(action, success, dimensions))
        });
    }

    fn retire(self: &Arc<Self>) {
        let mut m = self.mailbox.lock().unwrap();
        if m.control_pending || m.lifecycle == Lifecycle::Stopped {
            return;
        }
        m.control_pending = true;
        m.lifecycle = Lifecycle::Stopping;
        drop(m);
        self.submit(Lifecycle::Stopping, Lifecycle::Stopped, None);
    }

    fn run(
        self: Arc<Self>,
        action: Lifecycle,
        success: Lifecycle,
        dimensions: Option<((usize, usize), Duration)>,
    ) {
        {
            let mut m = self.mailbox.lock().unwrap();
            if m.closed && action != Lifecycle::Stopping {
                m.control_pending = false;
                drop(m);
                self.retire();
                return;
            }
        }
        let completion = Mutex::new(Some(self.clone()));
        let callback = RcBlock::new(move |error: *mut NSError| {
            let Some(control) = completion.lock().unwrap().take() else {
                return;
            };
            let mut m = control.mailbox.lock().unwrap();
            m.control_pending = false;
            if let Some(error) = unsafe { error.as_ref() } {
                m.lifecycle = Lifecycle::Failed;
                m.status = SourceStatus::Failed(error.to_string());
            } else if m.lifecycle != Lifecycle::Failed {
                m.lifecycle = success;
                if let Some((size, interval)) = dimensions {
                    m.dimensions = size;
                    m.interval = interval;
                }
            }
            let closed = m.closed;
            drop(m);
            if closed && action != Lifecycle::Stopping {
                control.retire();
            } else if !closed {
                control.wake.notify();
            }
        });
        unsafe {
            match action {
                Lifecycle::Starting => {
                    self.stream.startCaptureWithCompletionHandler(Some(&callback))
                }
                Lifecycle::Stopping => {
                    self.stream.stopCaptureWithCompletionHandler(Some(&callback))
                }
                Lifecycle::Updating => self.stream.updateConfiguration_completionHandler(
                    &configuration(dimensions.unwrap().0, dimensions.unwrap().1),
                    Some(&callback),
                ),
                _ => unreachable!(),
            }
        }
    }
}

struct Slot {
    _lease: Arc<StreamLease>,
    request: PreviewSource,
    native_dimensions: (usize, usize),
    control: Option<Arc<StreamControl>>,
    bootstrap: Option<Bootstrap>,
    mailbox: Arc<Mutex<Mailbox>>,
    frame: Option<Frame>,
    // Texture views of at most three capture surfaces. These retain IOSurfaces,
    // not pool samples. Old views are dropped on configuration changes.
    textures: Vec<(u32, Retained<ProtocolObject<dyn MTLTexture>>)>,
    dimensions: (usize, usize),
    desired: (usize, usize),
}
impl Drop for Slot {
    fn drop(&mut self) {
        let mut mailbox = self.mailbox.lock().unwrap();
        mailbox.closed = true;
        mailbox.latest = None;
        drop(mailbox);
        if let Some(control) = &self.control {
            control.retire();
        }
    }
}

impl Slot {
    fn drive(&mut self) {
        if let Some(initial) = self.bootstrap.take() {
            DispatchQueue::global_queue(dispatch2::GlobalQueueIdentifier::QualityOfService(
                dispatch2::DispatchQoS::UserInitiated,
            ))
            .exec_async(move || {
                objc2::rc::autoreleasepool(|_| initial.run());
            });
        }
        let Some(control) = &self.control else { return };
        let mut m = self.mailbox.lock().unwrap();
        if m.control_pending {
            return;
        }
        if self.dimensions != m.dimensions && m.dimensions != (0, 0) {
            self.dimensions = m.dimensions;
            self.textures.clear();
            // Keep the last valid frame while a new-size buffer is pending.
        }
        let previous = m.lifecycle;
        let action = if matches!(previous, Lifecycle::Running | Lifecycle::Stopped)
            && (m.dimensions != self.desired || m.interval != self.request.frame_interval)
        {
            Some(Lifecycle::Updating)
        } else if previous == Lifecycle::Stopped
            && (self.request.mode == CaptureMode::Live || self.frame.is_none())
        {
            Some(Lifecycle::Starting)
        } else if previous == Lifecycle::Running
            && self.request.mode == CaptureMode::Snapshot
            && self.frame.is_some()
        {
            Some(Lifecycle::Stopping)
        } else {
            None
        };
        let Some(action) = action else {
            return;
        };
        m.lifecycle = action;
        m.control_pending = true;
        drop(m);
        let (success, dimensions) = match action {
            Lifecycle::Updating => (previous, Some((self.desired, self.request.frame_interval))),
            Lifecycle::Starting => (Lifecycle::Running, None),
            Lifecycle::Stopping => (Lifecycle::Stopped, None),
            _ => unreachable!(),
        };
        control.submit(action, success, dimensions);
    }
}

// Filter/configuration are immutable and transferred once to this capture job.
// Admission happens on the owner before this job can allocate its first frame.
struct Bootstrap {
    mailbox: Arc<Mutex<Mailbox>>,
    wake: Arc<Wake>,
    filter: Retained<SCContentFilter>,
    config: Retained<SCStreamConfiguration>,
    lease: Arc<StreamLease>,
    snapshot: bool,
}
unsafe impl Send for Bootstrap {}
impl Bootstrap {
    fn run(self) {
        let Self {
            mailbox,
            wake,
            filter,
            config,
            lease,
            snapshot,
        } = self;
        {
            let m = mailbox.lock().unwrap();
            if m.closed || m.sequence != 0 {
                return;
            }
        }
        let pending = Arc::downgrade(&mailbox);
        let timeout_wake = Arc::downgrade(&wake);
        let _ = DispatchQueue::global_queue(dispatch2::GlobalQueueIdentifier::QualityOfService(
            dispatch2::DispatchQoS::UserInitiated,
        ))
        .after(
            std::time::Duration::from_secs(2).try_into().unwrap(),
            move || {
                if let Some(mailbox) = pending.upgrade() {
                    let mut m = mailbox.lock().unwrap();
                    if !m.closed && m.sequence == 0 && m.lifecycle != Lifecycle::Failed {
                        m.lifecycle = Lifecycle::Failed;
                        m.status = SourceStatus::Failed(
                            "no initial capture frame within 2 seconds".into(),
                        );
                        drop(m);
                        if let Some(wake) = timeout_wake.upgrade() {
                            wake.notify();
                        }
                    }
                }
            },
        );

        let initial = mailbox.clone();
        let ready = wake.clone();
        let keep_filter = filter.clone();
        let keep_config = config.clone();
        let keep_lease = lease.clone();
        let callback = RcBlock::new(move |sample: *mut CMSampleBuffer, error: *mut NSError| {
            let _retained = (&keep_filter, &keep_config, &keep_lease);
            let mut m = initial.lock().unwrap();
            if m.closed || m.sequence != 0 {
                return;
            }
            let valid = unsafe { sample.as_ref() }.is_some_and(|sample| m.publish(sample));
            if !valid && snapshot {
                m.lifecycle = Lifecycle::Failed;
                m.status = SourceStatus::Failed(
                    unsafe { error.as_ref() }
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "initial sample unavailable".into()),
                );
            }
            drop(m);
            ready.notify();
        });
        unsafe {
            SCScreenshotManager::captureSampleBufferWithFilter_configuration_completionHandler(
                &filter,
                &config,
                Some(&callback),
            );
        }
    }
}

// These immutable window descriptions and unstarted streams use AnyThread APIs.
// Opened crosses queues exactly once, before it contains any frame/Metal texture;
// after transfer the owner alone controls the stream. Samples still use mailboxes.
struct WindowDescription(Retained<SCWindow>);
unsafe impl Send for WindowDescription {}

#[derive(Default)]
struct DirectDiscovery {
    started: bool,
    result: Option<Vec<WindowDescription>>,
}
// This private initializer was validated on macOS 27. Other releases use normal
// SCK discovery. It only constructs metadata: capture still uses authorized SCK.
fn direct_discovery_supported() -> bool {
    NSProcessInfo::processInfo().operatingSystemVersion().majorVersion == 27
        && unsafe { msg_send![SCWindow::class(), instancesRespondToSelector: sel!(initWithDict:)] }
}
fn direct_windows(requested: &[(u32, i32)]) -> Vec<WindowDescription> {
    use objc2::runtime::AnyObject;
    use objc2_core_graphics::{CGWindowListCopyWindowInfo, CGWindowListOption, kCGNullWindowID};
    objc2::exception::catch(std::panic::AssertUnwindSafe(|| {
        let Some(list) = CGWindowListCopyWindowInfo(CGWindowListOption::OptionAll, kCGNullWindowID)
        else {
            return Vec::new();
        };
        // CFArray/CFDictionary are toll-free bridged. The retained CF list owns
        // every dictionary throughout this read-only traversal.
        let list =
            unsafe { &*(&*list as *const _ as *const NSArray<NSDictionary<NSString, AnyObject>>) };
        let mut windows = Vec::with_capacity(requested.len());
        for entry in list {
            let Some(id) = entry.objectForKey(ns_string!("kCGWindowNumber")) else {
                continue;
            };
            let Some(pid) = entry.objectForKey(ns_string!("kCGWindowOwnerPID")) else {
                continue;
            };
            let (Some(id_number), Some(pid_number)) =
                (id.downcast_ref::<NSNumber>(), pid.downcast_ref::<NSNumber>())
            else {
                continue;
            };
            let identity = (id_number.unsignedIntValue(), pid_number.intValue());
            if !requested.contains(&identity) {
                continue;
            }
            let Some(bounds) = entry.objectForKey(ns_string!("kCGWindowBounds")) else {
                continue;
            };
            let Some(bounds) = bounds.downcast_ref::<NSDictionary>() else {
                continue;
            };
            let Some(x) = bounds.objectForKey(ns_string!("X")) else {
                continue;
            };
            let Some(y) = bounds.objectForKey(ns_string!("Y")) else {
                continue;
            };
            let Some(width) = bounds.objectForKey(ns_string!("Width")) else {
                continue;
            };
            let Some(height) = bounds.objectForKey(ns_string!("Height")) else {
                continue;
            };
            let Some(layer) = entry.objectForKey(ns_string!("kCGWindowLayer")) else {
                continue;
            };
            let active = NSNumber::new_bool(true);
            let on_screen = entry.objectForKey(ns_string!("kCGWindowIsOnscreen"));
            let dict = NSDictionary::<NSString, AnyObject>::from_slices(
                &[
                    ns_string!("SCWindowID"),
                    ns_string!("SCRunningApplicationPID"),
                    ns_string!("SCWindowLayer"),
                    ns_string!("SCWindowName"),
                    ns_string!("SCWindowBoundOriginX"),
                    ns_string!("SCWindowBoundOriginY"),
                    ns_string!("SCWindowBoundWidth"),
                    ns_string!("SCWindowBoundHeight"),
                    ns_string!("SCWindowIsOnScreen"),
                    ns_string!("SCWindowIsActive"),
                ],
                &[
                    &id,
                    &pid,
                    &layer,
                    ns_string!(""),
                    &x,
                    &y,
                    &width,
                    &height,
                    on_screen.as_deref().unwrap_or(&active),
                    &active,
                ],
            );
            let window: Option<Retained<SCWindow>> =
                unsafe { msg_send![SCWindow::alloc(), initWithDict: &*dict] };
            if let Some(window) = window {
                if unsafe {
                    window.windowID() == identity.0
                        && window
                            .owningApplication()
                            .is_some_and(|app| app.processID() == identity.1)
                } {
                    windows.push(WindowDescription(window));
                }
            }
        }
        windows
    }))
    .unwrap_or_default()
}

struct Opened {
    slot: Slot,
    setup_us: [u64; 3],
}
unsafe impl Send for Opened {}
struct Opening {
    window_id: u32,
    process_id: i32,
    result: Arc<Mutex<Option<Result<Opened, String>>>>,
}
fn open_source(
    window: WindowDescription,
    request: PreviewSource,
    lease: Arc<StreamLease>,
    queue: DispatchRetained<DispatchQueue>,
    controls: DispatchRetained<DispatchQueue>,
    wake: Arc<Wake>,
    native: Option<(usize, usize)>,
    bootstrap: bool,
) -> Result<Opened, String> {
    let mut setup_us = [0; 3];
    let begin = Instant::now();
    let filter = unsafe {
        SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &window.0)
    };
    let size = unsafe { filter.contentRect().size };
    let scale = unsafe { filter.pointPixelScale() } as f64;
    let native_dimensions = (
        (size.width * scale).ceil() as usize,
        (size.height * scale).ceil() as usize,
    );
    setup_us[0] = begin.elapsed().as_micros() as u64;
    let dimensions = match request.resolution {
        CaptureResolution::Native => native.unwrap_or(native_dimensions),
        CaptureResolution::Thumbnail { width, height } => (width, height),
    };
    if dimensions.0 == 0 || dimensions.1 == 0 || capture_bytes(dimensions) > 256 * 1024 * 1024 {
        return Err("capture memory limit".into());
    }
    if !lease.reserve(capture_bytes(dimensions)) {
        return Err("process capture memory limit".into());
    }
    let mailbox = Arc::new(Mutex::new(Mailbox {
        lifecycle: Lifecycle::Stopped,
        dimensions,
        interval: request.frame_interval,
        ..Mailbox::default()
    }));
    let config = configuration(dimensions, request.frame_interval);
    let initial = bootstrap.then(|| Bootstrap {
        mailbox: mailbox.clone(),
        wake: wake.clone(),
        filter: filter.clone(),
        config: config.clone(),
        lease: lease.clone(),
        snapshot: request.mode == CaptureMode::Snapshot,
    });
    let control = if request.mode == CaptureMode::Live {
        let output = Output::alloc().set_ivars(OutputState {
            mailbox: mailbox.clone(),
            wake: wake.clone(),
        });
        let output: Retained<Output> = unsafe { msg_send![super(output), init] };
        let create_start = Instant::now();
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &config,
                Some(ProtocolObject::from_ref(&*output)),
            )
        };
        unsafe {
            stream.addStreamOutput_type_sampleHandlerQueue_error(
                ProtocolObject::from_ref(&*output),
                SCStreamOutputType::Screen,
                Some(&queue),
            )
        }
        .map_err(|e| e.to_string())?;
        setup_us[1] = create_start.elapsed().as_micros() as u64;
        lease.stream.store(true, Ordering::Release);
        LIVE_STREAMS.fetch_add(1, Ordering::AcqRel);
        Some(Arc::new(StreamControl {
            stream,
            _output: output,
            _lease: lease.clone(),
            mailbox: mailbox.clone(),
            wake: wake.clone(),
            queue: controls,
        }))
    } else {
        None
    };
    Ok(Opened {
        slot: Slot {
            _lease: lease,
            request,
            native_dimensions,
            control,
            bootstrap: initial,
            mailbox,
            frame: None,
            textures: Vec::with_capacity(3),
            dimensions,
            desired: dimensions,
        },
        setup_us,
    })
}

// Keep one last frame per source until hide/clear, without retaining its stream
// or texture-view cache. Scene admission still bounds active capture memory.
#[derive(Clone)]
struct CachedFrame {
    window_id: u32,
    process_id: i32,
    frame: Frame,
    refresh: bool,
    reusable: bool,
    priority: i32,
}
// Owner-thread texture/sample reuse only: never transfer Metal objects across
// threads. No streams or clocks survive close. Bound the shared closed-session
// cache independently of the bounded 128 MiB open-session cache.
thread_local! {
    static REUSABLE: std::cell::RefCell<Vec<CachedFrame>> = const { std::cell::RefCell::new(Vec::new()) };
}
const REUSABLE_BYTES: usize = 32 * 1024 * 1024;
const SESSION_FRAME_BYTES: usize = 128 * 1024 * 1024;
fn retain_reusable(frame: &CachedFrame) {
    // Native/oversized buffers are deliberately excluded from this small cache.
    if !frame.reusable || frame.frame.bytes > 4 * 1024 * 1024 {
        return;
    }
    REUSABLE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.retain(|f| f.window_id != frame.window_id);
        cache.push(frame.clone());
        let mut bytes: usize = cache.iter().map(|f| f.frame.bytes).sum();
        while bytes > REUSABLE_BYTES {
            bytes -= cache.remove(0).frame.bytes;
        }
    });
}
fn cache_frame(
    cache: &mut Vec<CachedFrame>,
    slot: &mut Slot,
    unavailable: &mut std::collections::HashMap<u32, String>,
) {
    let Some(frame) = slot.frame.take() else { return };
    cache.retain(|f| f.window_id != slot.request.window_id);
    cache.push(CachedFrame {
        window_id: slot.request.window_id,
        process_id: slot.request.process_id,
        frame,
        refresh: false,
        reusable: slot.request.resolution != CaptureResolution::Native,
        priority: slot.request.priority,
    });
    trim_frames(cache, unavailable);
}
fn trim_frames(
    cache: &mut Vec<CachedFrame>,
    unavailable: &mut std::collections::HashMap<u32, String>,
) {
    let mut bytes: usize = cache.iter().map(|f| f.frame.bytes).sum();
    while bytes > SESSION_FRAME_BYTES {
        let index = cache.iter().enumerate().min_by_key(|(_, f)| f.priority).unwrap().0;
        let evicted = cache.remove(index);
        bytes -= evicted.frame.bytes;
        unavailable.insert(evicted.window_id, "retained frame memory limit".into());
    }
}

pub(super) struct Captures {
    cached: Vec<CachedFrame>,
    slots: Vec<Slot>,
    opening: Vec<Opening>,
    requested: Vec<PreviewSource>,
    windows: Option<Vec<Retained<SCWindow>>>,
    discovery: Arc<Mutex<Discovery>>,
    direct: Arc<Mutex<DirectDiscovery>>,
    direct_used: bool,
    queue: DispatchRetained<DispatchQueue>,
    controls: DispatchRetained<DispatchQueue>,
    wake: Arc<Wake>,
    unavailable: std::collections::HashMap<u32, String>,
    reconcile: bool,
    waiting_for_capacity: bool,
    retired: (u64, u64),
    setup_us: [u64; 3],
    pump_us: u64,
    reused_dirty: bool,
    native_sizes: std::collections::HashMap<u32, (usize, usize)>,
}
impl Captures {
    pub fn new(owner_wake: Arc<Wake>) -> Self {
        let wake = Wake::new(move || owner_wake.notify());
        let captures = Self {
            cached: Default::default(),
            slots: Vec::with_capacity(8),
            opening: Vec::with_capacity(8),
            requested: Vec::new(),
            windows: None,
            discovery: Arc::default(),
            direct: Arc::default(),
            direct_used: false,
            queue: DispatchQueue::new("cgs.preview.capture", None),
            controls: DispatchQueue::new("cgs.preview.control", None),
            wake,
            unavailable: Default::default(),
            reconcile: false,
            waiting_for_capacity: false,
            retired: (0, 0),
            setup_us: [0; 3],
            pump_us: 0,
            reused_dirty: false,
            native_sizes: Default::default(),
        };
        captures.discover();
        captures
    }

    pub fn clear(&mut self) {
        for slot in &mut self.slots {
            cache_frame(&mut self.cached, slot, &mut self.unavailable);
        }
        for frame in &self.cached {
            retain_reusable(frame);
        }
        self.requested.clear();
        self.retire_all();
        self.opening.clear();
        self.cached.clear();
        self.windows = None;
        self.direct = Arc::default();
        self.direct_used = false;
        self.unavailable.clear();
        self.native_sizes.clear();
        self.waiting_for_capacity = false;
        self.reconcile = false;
    }

    fn retire_all(&mut self) {
        for s in &self.slots {
            let m = s.mailbox.lock().unwrap();
            self.retired.0 += m.sequence;
            self.retired.1 += m.replaced;
        }
        self.slots.clear();
    }

    pub fn refresh(&mut self) {
        self.native_sizes.clear();
        self.unavailable.clear();
        for frame in &mut self.cached {
            frame.refresh = true;
        }
        self.discover_fresh(true);
    }

    pub fn set_scene(&mut self, scene: &[Preview]) -> PreviewResult<()> {
        self.set_sources(scene.iter().map(PreviewSource::from))
    }

    pub fn set_sources(
        &mut self,
        sources: impl Iterator<Item = PreviewSource> + Clone,
    ) -> PreviewResult<()> {
        let active = sources.filter(|p| p.mode != CaptureMode::Inactive);
        // Visual edits never copy/sort the inventory, discover metadata, or pump streams.
        if active.clone().count() == self.requested.len()
            && active.clone().all(|p| self.requested.contains(&p))
        {
            return Ok(());
        }
        let identities_changed = active.clone().any(|p| {
            !self
                .requested
                .iter()
                .any(|q| p.window_id == q.window_id && p.process_id == q.process_id)
        });
        self.reconcile = true;
        self.unavailable.clear();
        self.native_sizes.retain(|id, _| active.clone().any(|p| p.window_id == *id));
        self.requested.clear();
        self.requested.extend(active);
        self.requested.sort_by_key(|p| std::cmp::Reverse(p.priority));
        // Caller-supplied WindowServer ID + owning PID also key pixel reuse.
        // Do not put an asynchronous metadata refresh on the first-pixel path.
        // The completed discovery below prunes missing/changed-owner sources.
        REUSABLE.with(|cache| {
            for frame in cache.borrow().iter().filter(|f| {
                self.requested
                    .iter()
                    .any(|p| p.window_id == f.window_id && p.process_id == f.process_id)
            }) {
                if !self.cached.iter().any(|f| f.window_id == frame.window_id)
                    && !self
                        .slots
                        .iter()
                        .any(|s| s.request.window_id == frame.window_id && s.frame.is_some())
                {
                    let mut frame = frame.clone();
                    frame.refresh = true;
                    self.reused_dirty = true;
                    self.cached.push(frame);
                }
            }
        });
        for frame in &mut self.cached {
            frame.priority = self
                .requested
                .iter()
                .find(|p| p.window_id == frame.window_id && p.process_id == frame.process_id)
                .map_or(i32::MIN, |p| p.priority);
        }
        trim_frames(&mut self.cached, &mut self.unavailable);
        self.slots.retain_mut(|s| {
            let keep = self.requested.iter().any(|p| {
                p.window_id == s.request.window_id && p.process_id == s.request.process_id
            });
            if !keep {
                let m = s.mailbox.lock().unwrap();
                self.retired.0 += m.sequence;
                self.retired.1 += m.replaced;
                drop(m);
                cache_frame(&mut self.cached, s, &mut self.unavailable);
            }
            keep
        });
        if self.windows.as_ref().is_none_or(|windows| {
            identities_changed
                && self.requested.iter().any(|p| {
                    !windows.iter().any(|w| unsafe {
                        w.windowID() == p.window_id
                            && w.owningApplication().is_some_and(|a| a.processID() == p.process_id)
                    })
                })
        }) {
            self.discover();
        }
        self.pump()
    }

    pub fn update_sources(&mut self, updates: &[PreviewSource]) {
        for update in updates {
            if let Some(source) =
                self.requested.iter_mut().find(|p| p.window_id == update.window_id)
            {
                if source != update {
                    *source = *update;
                    if let Some(frame) =
                        self.cached.iter_mut().find(|f| f.window_id == update.window_id)
                    {
                        frame.priority = update.priority;
                    }
                    self.unavailable.remove(&update.window_id);
                    self.reconcile = true;
                }
            }
        }
        if self.reconcile {
            self.requested.sort_by_key(|p| std::cmp::Reverse(p.priority));
        }
    }

    fn discover(&self) { self.discover_fresh(false); }

    fn discover_fresh(&self, fresh: bool) {
        if self.windows.is_none() && !self.requested.is_empty() && direct_discovery_supported() {
            let mut direct = self.direct.lock().unwrap();
            if !direct.started {
                direct.started = true;
                drop(direct);
                let result = self.direct.clone();
                let requested: Vec<_> =
                    self.requested.iter().map(|p| (p.window_id, p.process_id)).collect();
                let wake = self.wake.clone();
                DispatchQueue::global_queue(dispatch2::GlobalQueueIdentifier::QualityOfService(
                    dispatch2::DispatchQoS::UserInitiated,
                ))
                .exec_async(move || {
                    let windows = objc2::rc::autoreleasepool(|_| direct_windows(&requested));
                    result.lock().unwrap().result = Some(windows);
                    wake.notify();
                });
            }
        }
        let mut d = self.discovery.lock().unwrap();
        if d.pending {
            return;
        }
        d.pending = true;
        drop(d);
        request_inventory(Some((&self.discovery, &self.wake)), &self.requested, fresh);
    }

    pub fn pump(&mut self) -> PreviewResult<()> {
        let pump_start = Instant::now();
        if let Some(windows) = self.direct.lock().unwrap().result.take() {
            if self.windows.is_none() && !windows.is_empty() {
                self.windows = Some(windows.into_iter().map(|w| w.0).collect());
                self.direct_used = true;
                self.reconcile = true;
            }
        }
        let mut i = 0;
        while i < self.opening.len() {
            let result = self.opening[i].result.lock().unwrap().take();
            let Some(result) = result else {
                i += 1;
                continue;
            };
            let opening = self.opening.swap_remove(i);
            match result {
                Ok(opened)
                    if self.requested.iter().any(|p| {
                        p.window_id == opening.window_id
                            && p.process_id == opening.process_id
                            && p.resolution == opened.slot.request.resolution
                    }) =>
                {
                    for (value, sample) in self.setup_us.iter_mut().zip(opened.setup_us) {
                        *value = (*value).max(sample);
                    }
                    self.slots.push(opened.slot);
                }
                Ok(_) => {}
                Err(error) => {
                    self.unavailable.insert(opening.window_id, error);
                }
            }
            self.reconcile = true;
        }
        for slot in &self.slots {
            let mailbox = slot.mailbox.lock().unwrap();
            if mailbox.lifecycle == Lifecycle::Failed {
                self.unavailable.entry(slot.request.window_id).or_insert_with(|| {
                    match &mailbox.status {
                        SourceStatus::Failed(message) => message.clone(),
                        _ => "capture failed".into(),
                    }
                });
                self.reconcile = true;
            }
        }
        if self.waiting_for_capacity
            && self.slots.len() + self.opening.len() < 8
            && STREAMS.load(Ordering::Acquire) < 16
        {
            self.reconcile = true;
        }
        if let Some(result) = self.discovery.lock().unwrap().result.take() {
            let content = result.map_err(PreviewError::Capture)?.0;
            if std::mem::take(&mut self.direct_used) {
                // Any shortcut that has not produced pixels yields to real SCK
                // descriptions. Already-usable captures continue without churn.
                self.opening.clear();
                self.slots
                    .retain(|s| s.frame.is_some() || s.mailbox.lock().unwrap().latest.is_some());
            }
            self.unavailable.clear();
            let inventory = unsafe { content.windows() };
            let cached_count = self.cached.len();
            self.cached.retain(|f| {
                inventory.iter().any(|w| unsafe {
                    w.windowID() == f.window_id
                        && w.owningApplication().is_some_and(|a| a.processID() == f.process_id)
                })
            });
            self.reused_dirty |= self.cached.len() != cached_count;
            REUSABLE.with(|cache| {
                cache.borrow_mut().retain(|f| {
                    inventory.iter().any(|w| unsafe {
                        w.windowID() == f.window_id
                            && w.owningApplication().is_some_and(|a| a.processID() == f.process_id)
                    })
                });
            });
            // Drop the full desktop inventory immediately; retain only the
            // bounded requested descriptions, not thousands of system windows.
            self.windows = Some(
                unsafe { content.windows() }
                    .iter()
                    .filter(|w| {
                        self.requested.iter().any(|p| unsafe { w.windowID() == p.window_id })
                    })
                    .collect(),
            );
            self.reconcile = true;
        }
        if self.reconcile && self.windows.is_some() {
            self.reconcile()?;
            self.reconcile = false;
        }
        for slot in &mut self.slots {
            let start = Instant::now();
            let previous = slot.dimensions;
            slot.drive();
            if previous != slot.dimensions {
                self.reconcile = true;
                self.wake.notify();
            }
            self.setup_us[2] = self.setup_us[2].max(start.elapsed().as_micros() as u64);
        }
        self.pump_us = self.pump_us.max(pump_start.elapsed().as_micros() as u64);
        Ok(())
    }

    fn reconcile(&mut self) -> PreviewResult<()> {
        self.waiting_for_capacity = false;
        let windows = self.windows.as_ref().unwrap();
        // Reserve eight surface-sized buffers per admitted source (queue, latest,
        // in-flight reads and texture-view cache), including 64-byte row alignment.
        // GPU reads of the previous scene may briefly outlive active admission.
        let mut budget = 256usize * 1024 * 1024;
        let mut desired = Vec::with_capacity(8);
        let pending = self
            .requested
            .iter()
            .filter(|p| p.mode == CaptureMode::Snapshot)
            .chain(self.requested.iter().filter(|p| p.mode == CaptureMode::Live).skip(6))
            .any(|p| self.needs_sample(p) && !self.unavailable.contains_key(&p.window_id));
        let live_limit = if pending { 6 } else { 8 };
        let foreground_ready = self
            .requested
            .iter()
            .filter(|p| p.mode == CaptureMode::Live)
            .take(live_limit)
            .all(|p| !self.needs_sample(p) || self.unavailable.contains_key(&p.window_id));
        let mut live_index = 0;
        for requested in &self.requested {
            if self.unavailable.contains_key(&requested.window_id) {
                continue;
            }
            let mut request = requested.clone();
            if request.mode == CaptureMode::Live {
                live_index += 1;
            }
            if request.mode == CaptureMode::Snapshot || live_index > live_limit {
                if !foreground_ready
                    || !self.needs_sample(&request)
                    || self.unavailable.contains_key(&request.window_id)
                {
                    continue;
                }
                request.mode = CaptureMode::Snapshot;
            }
            if desired.len() == 8 {
                break;
            }
            // Get initial pixels without waiting for SCStream construction. Once
            // imported, this one-shot slot is upgraded to Live using its cached
            // frame, so stream startup cannot delay the first presentation.
            if request.mode == CaptureMode::Live && self.source_frame(&request).is_none() {
                request.mode = CaptureMode::Snapshot;
            }
            let request = &request;
            let window = windows.iter().find(|w| unsafe {
                w.windowID() == request.window_id
                    && w.owningApplication().is_some_and(|a| a.processID() == request.process_id)
            });
            let Some(window) = window else {
                if !self.discovery.lock().unwrap().pending {
                    self.unavailable
                        .insert(request.window_id, "window unavailable or owner changed".into());
                }
                continue;
            };
            if !self.slots.iter().any(|s| {
                s.request.window_id == request.window_id
                    && s.request.process_id == request.process_id
                    && (request.mode != CaptureMode::Live
                        || s.control.is_some()
                        || s.frame.is_none())
            }) {
                if self
                    .opening
                    .iter()
                    .any(|s| s.window_id == request.window_id && s.process_id == request.process_id)
                {
                    continue;
                }
                if self.slots.len() + self.opening.len() >= 8 {
                    self.waiting_for_capacity = true;
                    continue;
                }
                let speculative = requested.mode == CaptureMode::Snapshot && requested.priority < 0;
                let Some(lease) = StreamLease::acquire(speculative) else {
                    self.waiting_for_capacity = true;
                    continue;
                };
                let result = Arc::new(Mutex::new(None));
                self.opening.push(Opening {
                    window_id: request.window_id,
                    process_id: request.process_id,
                    result: result.clone(),
                });
                let window = WindowDescription(window.clone());
                let request = request.clone();
                let native = self.native_sizes.get(&request.window_id).copied();
                let bootstrap =
                    request.mode == CaptureMode::Snapshot || self.needs_sample(&request);
                let wake = self.wake.clone();
                let queue = self.queue.clone();
                let controls = self.controls.clone();
                dispatch2::DispatchQueue::global_queue(
                    dispatch2::GlobalQueueIdentifier::QualityOfService(
                        dispatch2::DispatchQoS::UserInitiated,
                    ),
                )
                .exec_async(move || {
                    let opened = objc2::rc::autoreleasepool(|_| {
                        open_source(
                            window,
                            request,
                            lease,
                            queue,
                            controls,
                            wake.clone(),
                            native,
                            bootstrap,
                        )
                    });
                    *result.lock().unwrap() = Some(opened);
                    wake.notify();
                });
                continue;
            }
            let native_dimensions = self
                .slots
                .iter()
                .find(|s| {
                    s.request.window_id == request.window_id
                        && s.request.process_id == request.process_id
                })
                .unwrap()
                .native_dimensions;
            let dimensions = match request.resolution {
                CaptureResolution::Native => {
                    self.native_sizes.get(&request.window_id).copied().unwrap_or(native_dimensions)
                }
                CaptureResolution::Thumbnail { width, height } => (width, height),
            };
            let previous = self
                .slots
                .iter()
                .find(|s| s.request.window_id == request.window_id)
                .unwrap()
                .dimensions;
            let dimensions = if request.resolution != CaptureResolution::Native
                && dimensions.0 as f64 <= previous.0 as f64 * 1.125
                && dimensions.1 as f64 <= previous.1 as f64 * 1.125
                && self
                    .slots
                    .iter()
                    .find(|s| s.request.window_id == request.window_id)
                    .is_some_and(|s| request.priority <= s.request.priority)
                && dimensions.0 as f64 >= previous.0 as f64 * 0.6
                && dimensions.1 as f64 >= previous.1 as f64 * 0.6
                && ((dimensions.0 as f64 / dimensions.1 as f64)
                    / (previous.0 as f64 / previous.1 as f64)
                    - 1.)
                    .abs()
                    < 0.02
            {
                previous
            } else {
                dimensions
            };
            let bytes = capture_bytes(dimensions).max(capture_bytes(previous));
            if dimensions.0 == 0 || dimensions.1 == 0 || bytes > 256 * 1024 * 1024 {
                self.unavailable.insert(request.window_id, "capture memory limit".into());
                continue;
            }
            if bytes > budget {
                if request.mode == CaptureMode::Live {
                    self.unavailable
                        .insert(request.window_id, "active capture memory limit".into());
                }
                continue;
            }
            budget -= bytes;
            desired.push((request.clone(), dimensions));
            self.unavailable.remove(&request.window_id);
        }
        if self.waiting_for_capacity {
            let mut waiters = WAITERS.lock().unwrap();
            let weak = Arc::downgrade(&self.wake);
            if !waiters.iter().any(|w| w.ptr_eq(&weak)) {
                waiters.push(weak);
            }
        }
        // Retire rejected old sources before allocating replacement streams.
        self.slots.retain_mut(|s| {
            let m = s.mailbox.lock().unwrap();
            let keep = m.lifecycle != Lifecycle::Failed
                && desired.iter().any(|(p, _)| {
                    p.window_id == s.request.window_id && p.process_id == s.request.process_id
                });
            if !keep {
                self.retired.0 += m.sequence;
                self.retired.1 += m.replaced;
                drop(m);
                if windows.iter().any(|w| unsafe {
                    w.windowID() == s.request.window_id
                        && w.owningApplication()
                            .is_some_and(|a| a.processID() == s.request.process_id)
                }) {
                    cache_frame(&mut self.cached, s, &mut self.unavailable);
                }
            }
            keep
        });
        for (request, dimensions) in desired {
            if let Some(slot) =
                self.slots.iter_mut().find(|s| s.request.window_id == request.window_id)
            {
                if slot._lease.reserve(capture_bytes(dimensions)) {
                    slot.request = request;
                    slot.desired = dimensions;
                } else {
                    self.unavailable
                        .insert(request.window_id, "process capture memory limit".into());
                }
                continue;
            }
        }
        Ok(())
    }

    /// All canvases share this mailbox drain. Display/geometry wakes alone do
    /// not revisit stream control or acquire every source's mailbox mutex.
    pub fn update(&mut self, gpu: &mut Gpu, scene: &[Preview]) -> PreviewResult<bool> {
        if !self.wake.pending.swap(false, Ordering::AcqRel) && !self.reconcile {
            return Ok(false);
        }
        self.pump()?;
        self.import(gpu, scene)
    }

    fn import(&mut self, gpu: &mut Gpu, scene: &[Preview]) -> PreviewResult<bool> {
        let mut changed = std::mem::take(&mut self.reused_dirty);
        for slot in &mut self.slots {
            let sample = slot.mailbox.lock().unwrap().latest.take();
            let Some(sample) = sample else {
                continue;
            };
            let image = unsafe { sample.0.image_buffer() }
                .ok_or_else(|| PreviewError::Capture("missing image".into()))?;
            let surface: CFRetained<objc2_io_surface::IOSurfaceRef> =
                CVPixelBufferGetIOSurface(Some(&image))
                    .ok_or_else(|| PreviewError::Capture("frame is not IOSurface-backed".into()))?;
            let id = surface.id();
            let texture = if let Some((_, t)) = slot.textures.iter().find(|(key, _)| *key == id) {
                t.clone()
            } else {
                let descriptor = unsafe {
                    MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                        MTLPixelFormat::BGRA8Unorm,
                        CVPixelBufferGetWidth(&image),
                        CVPixelBufferGetHeight(&image),
                        false,
                    )
                };
                descriptor.setStorageMode(MTLStorageMode::Shared);
                descriptor.setUsage(MTLTextureUsage::ShaderRead);
                let t = gpu
                    .device
                    .newTextureWithDescriptor_iosurface_plane(&descriptor, &surface, 0)
                    .ok_or_else(|| PreviewError::Metal("IOSurface texture import failed".into()))?;
                if slot.textures.len() == 3 {
                    slot.textures.remove(0);
                }
                slot.textures.push((id, t.clone()));
                t
            };
            let bytes =
                CVPixelBufferGetBytesPerRow(&image).saturating_mul(CVPixelBufferGetHeight(&image));
            let (uv, native_size) = content_geometry(
                &sample.0,
                CVPixelBufferGetWidth(&image),
                CVPixelBufferGetHeight(&image),
            );
            if slot.request.resolution == CaptureResolution::Native {
                if let Some(size) = native_size {
                    if size != slot.desired {
                        self.native_sizes.insert(slot.request.window_id, size);
                        self.reconcile = true;
                        self.wake.notify();
                    }
                }
            }
            let display_time = display_time(&sample.0);
            if display_time > 0. {
                slot.mailbox.lock().unwrap().capture_to_gpu_us =
                    ((CVGetCurrentHostTime() as f64 / CVGetHostClockFrequency() - display_time)
                        .max(0.)
                        * 1e6) as u64;
            }
            let first = slot.frame.is_none();
            self.cached.retain(|f| f.window_id != slot.request.window_id);
            slot.frame = Some(Frame {
                texture,
                sample: Arc::new(sample),
                bytes,
                uv,
                sequence: FRAME_SEQUENCE.fetch_add(1, Ordering::Relaxed),
                display_time,
                received: Instant::now(),
            });
            changed |= scene.iter().any(|p| {
                p.window_id == slot.request.window_id
                    && p.process_id == slot.request.process_id
                    && p.opacity > 0.
            });
            if first || slot.request.mode == CaptureMode::Snapshot {
                self.reconcile = true;
                self.wake.notify();
            }
            if slot.request.mode != CaptureMode::Snapshot {
                slot.drive();
            }
        }
        Ok(changed)
    }

    fn needs_sample(&self, p: &PreviewSource) -> bool {
        self.source_frame(p).is_none_or(|frame| match p.resolution {
            CaptureResolution::Native => false,
            CaptureResolution::Thumbnail { width, height } => {
                width as f64 > frame.texture.width() as f64 * 1.125
                    || height as f64 > frame.texture.height() as f64 * 1.125
            }
        }) || self
            .cached
            .iter()
            .any(|f| f.window_id == p.window_id && f.process_id == p.process_id && f.refresh)
    }

    pub fn captured_frame(&self, window_id: u32) -> Option<(i32, Frame)> {
        let source = self.requested.iter().find(|p| p.window_id == window_id)?;
        Some((source.process_id, self.source_frame(source)?.clone()))
    }

    pub fn reuse_frame(&mut self, window_id: u32, process_id: i32, frame: &Frame) -> bool {
        let newer = |old: &Frame| {
            if frame.display_time > 0. && old.display_time > 0. {
                frame.display_time > old.display_time
            } else {
                frame.received > old.received
            }
        };
        if let Some(slot) = self
            .slots
            .iter_mut()
            .find(|s| s.request.window_id == window_id && s.request.process_id == process_id)
        {
            if let Some(current) = &slot.frame {
                if !newer(current) {
                    return false;
                }
                slot.frame = Some(frame.clone());
                self.reconcile = true;
                self.reused_dirty = true;
                return true;
            }
        }
        if self
            .cached
            .iter()
            .any(|f| f.window_id == window_id && f.process_id == process_id && !newer(&f.frame))
        {
            return false;
        }
        self.cached.retain(|f| f.window_id != window_id);
        self.cached.push(CachedFrame {
            window_id,
            process_id,
            frame: frame.clone(),
            refresh: true,
            reusable: false,
            priority: self
                .requested
                .iter()
                .find(|p| p.window_id == window_id && p.process_id == process_id)
                .map_or(i32::MAX, |p| p.priority),
        });
        trim_frames(&mut self.cached, &mut self.unavailable);
        self.reconcile = true;
        self.reused_dirty = true;
        true
    }

    pub fn frame(&self, p: &Preview) -> Option<&Frame> {
        self.source_frame(&PreviewSource::from(p))
    }

    fn source_frame(&self, p: &PreviewSource) -> Option<&Frame> {
        if p.mode == CaptureMode::Inactive {
            return None;
        }
        self.slots
            .iter()
            .find(|s| s.request.window_id == p.window_id && s.request.process_id == p.process_id)
            .and_then(|s| s.frame.as_ref())
            .or_else(|| {
                self.cached
                    .iter()
                    .find(|f| f.window_id == p.window_id && f.process_id == p.process_id)
                    .map(|f| &f.frame)
            })
    }

    pub fn stats(&self, s: &mut PreviewStats) {
        s.reusable_frame_bytes =
            REUSABLE.with(|cache| cache.borrow().iter().map(|f| f.frame.bytes).sum());
        s.latest_frame_bytes += self.cached.iter().map(|f| f.frame.bytes).sum::<usize>();
        s.max_capture_pump_us = self.pump_us;
        s.max_filter_us = self.setup_us[0];
        s.max_stream_create_us = self.setup_us[1];
        s.max_stream_start_us = self.setup_us[2];
        s.process_capture_jobs = STREAMS.load(Ordering::Acquire);
        s.process_streams = LIVE_STREAMS.load(Ordering::Acquire);
        s.captured_frames += self.retired.0;
        s.replaced_frames += self.retired.1;
        s.streams = self
            .slots
            .iter()
            .filter(|s| {
                s.control.is_some() && s.mailbox.lock().unwrap().lifecycle != Lifecycle::Stopped
            })
            .count();
        for slot in &self.slots {
            let m = slot.mailbox.lock().unwrap();
            s.captured_frames += m.sequence;
            s.replaced_frames += m.replaced;
            s.latest_frame_bytes += slot.frame.as_ref().map_or(0, |f| f.bytes);
            s.reserved_capture_bytes +=
                capture_bytes(slot.desired).max(capture_bytes(slot.dimensions));
            s.last_capture_to_gpu_us = s.last_capture_to_gpu_us.max(m.capture_to_gpu_us);
            s.first_frame_us = s.first_frame_us.max(m.first_frame_us);
            s.last_callback_us = s.last_callback_us.max(m.last_callback_us);
        }
    }

    pub fn status(&self, id: u32) -> Option<SourceStatus> {
        if let Some(reason) = self.unavailable.get(&id) {
            return Some(SourceStatus::Failed(reason.clone()));
        }
        let Some(slot) = self.slots.iter().find(|s| s.request.window_id == id) else {
            if let Some(frame) = self.cached.iter().find(|f| f.window_id == id) {
                return Some(SourceStatus::Complete {
                    age: frame.frame.received.elapsed(),
                });
            }
            return self
                .requested
                .iter()
                .any(|p| p.window_id == id)
                .then_some(SourceStatus::Starting);
        };
        let m = slot.mailbox.lock().unwrap();
        Some(match &m.status {
            SourceStatus::Complete { .. } => {
                SourceStatus::Complete { age: m.last.unwrap().elapsed() }
            }
            SourceStatus::Idle { .. } => SourceStatus::Idle {
                age: m.last.map(|t| t.elapsed()),
            },
            s => s.clone(),
        })
    }
}
static CLEAR: std::sync::LazyLock<CFRetained<objc2_core_graphics::CGColor>> =
    std::sync::LazyLock::new(|| objc2_core_graphics::CGColor::new_generic_rgb(0., 0., 0., 0.));
fn capture_bytes((width, height): (usize, usize)) -> usize {
    width
        .checked_mul(4)
        .and_then(|n| n.checked_add(63))
        .map(|n| n / 64 * 64)
        .and_then(|n| n.checked_mul(height))
        .and_then(|n| n.checked_mul(8))
        .unwrap_or(usize::MAX)
}
fn configuration(
    (width, height): (usize, usize),
    interval: Duration,
) -> Retained<SCStreamConfiguration> {
    let config = unsafe { SCStreamConfiguration::new() };
    unsafe {
        config.setWidth(width);
        config.setHeight(height);
        config.setPixelFormat(kCVPixelFormatType_32BGRA);
        config.setQueueDepth(3);
        config.setMinimumFrameInterval(CMTime {
            value: interval.as_nanos().min(i64::MAX as u128) as i64,
            timescale: 1_000_000_000,
            flags: objc2_core_media::CMTimeFlags::Valid,
            epoch: 0,
        });
        config.setBackgroundColor(&CLEAR);
        config.setColorSpaceName(objc2_core_graphics::kCGColorSpaceSRGB);
        config.setShowsCursor(false);
        config.setCapturesAudio(false);
        config.setIgnoreShadowsSingleWindow(true);
        config.setIgnoreGlobalClipSingleWindow(true);
        config.setScalesToFit(true);
        config.setPreservesAspectRatio(true);
    }
    config
}

fn content_geometry(
    sample: &CMSampleBuffer,
    width: usize,
    height: usize,
) -> ([f32; 4], Option<(usize, usize)>) {
    unsafe {
        let Some(a) = sample.sample_attachments_array(false) else {
            return ([0., 0., 1., 1.], None);
        };
        if a.count() == 0 {
            return ([0., 0., 1., 1.], None);
        }
        let dict = &*a.value_at_index(0).cast::<CFDictionary>();
        let raw =
            dict.value((SCStreamFrameInfoContentRect as *const objc2_foundation::NSString).cast());
        let mut rect = objc2_core_foundation::CGRect::ZERO;
        if raw.is_null()
            || !objc2_core_graphics::CGRectMakeWithDictionaryRepresentation(
                Some(&*raw.cast::<CFDictionary>()),
                &mut rect,
            )
        {
            return ([0., 0., 1., 1.], None);
        }
        let raw_scale =
            dict.value((SCStreamFrameInfoScaleFactor as *const objc2_foundation::NSString).cast());
        let scale = if raw_scale.is_null() {
            1.
        } else {
            (&*raw_scale.cast::<NSNumber>()).doubleValue()
        };
        if !scale.is_finite() || scale <= 0. {
            return ([0., 0., 1., 1.], None);
        }
        let x = (rect.origin.x * scale / width as f64).clamp(0., 1.) as f32;
        let y = (rect.origin.y * scale / height as f64).clamp(0., 1.) as f32;
        let right = (rect.max().x * scale / width as f64).clamp(x as f64, 1.) as f32;
        let bottom = (rect.max().y * scale / height as f64).clamp(y as f64, 1.) as f32;
        let raw_content_scale =
            dict.value((SCStreamFrameInfoContentScale as *const objc2_foundation::NSString).cast());
        let content_scale = if raw_content_scale.is_null() {
            0.
        } else {
            (&*raw_content_scale.cast::<NSNumber>()).doubleValue()
        };
        let native = if content_scale.is_finite()
            && content_scale > 0.
            && rect.size.width > 0.
            && rect.size.height > 0.
        {
            Some((
                (rect.size.width * scale / content_scale).round() as usize,
                (rect.size.height * scale / content_scale).round() as usize,
            ))
        } else {
            None
        };
        ([x, y, right - x, bottom - y], native)
    }
}

fn display_time(sample: &CMSampleBuffer) -> f64 {
    unsafe {
        let Some(a) = sample.sample_attachments_array(false) else {
            return 0.;
        };
        if a.count() == 0 {
            return 0.;
        }
        let dict = &*a.value_at_index(0).cast::<CFDictionary>();
        let raw =
            dict.value((SCStreamFrameInfoDisplayTime as *const objc2_foundation::NSString).cast());
        if raw.is_null() {
            0.
        } else {
            (&*raw.cast::<NSNumber>()).unsignedLongLongValue() as f64 / CVGetHostClockFrequency()
        }
    }
}
