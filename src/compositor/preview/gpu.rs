use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::{CGPoint, CGSize};
use objc2_foundation::NSString;
use objc2_metal::*;
use objc2_quartz_core::CAMetalDrawable;

use super::capture::Captures;
use super::{Preview, PreviewCamera, PreviewError, PreviewResult, PreviewStats, Wake};

#[repr(C)]
struct Params {
    transform: [f32; 16],
    camera: [f32; 4],
    rect: [f32; 4],
    clip: [f32; 4],
    viewport: [f32; 4],
    uv: [f32; 4],
    border: [f32; 4],
    style: [f32; 4],
}
// Metal drawables are retained thread-safe framework objects. The completion
// callback transfers exclusive presentation ownership through this mailbox to
// the layer's owner thread. No layer mutation occurs on the Metal callback queue.
pub(super) struct Presentation {
    pub drawable: Retained<ProtocolObject<dyn CAMetalDrawable>>,
    pub offset: CGPoint,
    callback_time: f64,
    sequence: u64,
}
unsafe impl Send for Presentation {}
pub(super) struct Gpu {
    pub device: Retained<ProtocolObject<dyn MTLDevice>>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    sampler: Retained<ProtocolObject<dyn MTLSamplerState>>,
    in_flight: Arc<AtomicUsize>,
    wake: Arc<Wake>,
    stats: PreviewStats,
    metrics: Arc<Mutex<super::timing::Metrics>>,
    last_sources: Vec<(u32, u64)>,
    failure: Arc<Mutex<Option<String>>>,
    scheduled: Arc<Mutex<(u64, Option<Presentation>)>>,
    sequence: u64,
    session_submitted: bool,
    presented_sequence: u64,
    created: f64,
    presented_revision: Arc<AtomicU64>,
}
// All compositors are owner/main-thread confined. Share immutable expensive
// Metal state across displays and reopenings without cross-thread ObjC transfers.
#[derive(Clone)]
struct Resources {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    sampler: Retained<ProtocolObject<dyn MTLSamplerState>>,
}
thread_local! {
    static RESOURCES: std::cell::RefCell<Option<Resources>> = const { std::cell::RefCell::new(None) };
}
impl Resources {
    fn new() -> PreviewResult<Self> {
        let device = MTLCreateSystemDefaultDevice()
            .ok_or_else(|| PreviewError::Metal("no Metal device".into()))?;
        let queue = device
            .newCommandQueue()
            .ok_or_else(|| PreviewError::Metal("command queue".into()))?;
        // Compile once at build time. No shader compiler in startup/interactive paths.
        let data =
            dispatch2::DispatchData::from_static_bytes(include_bytes!("../preview.metallib"));
        let library = device
            .newLibraryWithData_error(&data)
            .map_err(|e| PreviewError::Metal(e.to_string()))?;
        let vertex = library
            .newFunctionWithName(&NSString::from_str("preview_vertex"))
            .ok_or_else(|| PreviewError::Metal("vertex function".into()))?;
        let fragment = library
            .newFunctionWithName(&NSString::from_str("preview_fragment"))
            .ok_or_else(|| PreviewError::Metal("fragment function".into()))?;
        let descriptor = MTLRenderPipelineDescriptor::new();
        descriptor.setVertexFunction(Some(&vertex));
        descriptor.setFragmentFunction(Some(&fragment));
        let color = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
        color.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        color.setBlendingEnabled(true);
        color.setSourceRGBBlendFactor(MTLBlendFactor::One);
        color.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
        color.setSourceAlphaBlendFactor(MTLBlendFactor::One);
        color.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
        let pipeline = device
            .newRenderPipelineStateWithDescriptor_error(&descriptor)
            .map_err(|e| PreviewError::Metal(e.to_string()))?;
        let sd = MTLSamplerDescriptor::new();
        sd.setMinFilter(MTLSamplerMinMagFilter::Linear);
        sd.setMagFilter(MTLSamplerMinMagFilter::Linear);
        sd.setSAddressMode(MTLSamplerAddressMode::ClampToEdge);
        sd.setTAddressMode(MTLSamplerAddressMode::ClampToEdge);
        let sampler = device
            .newSamplerStateWithDescriptor(&sd)
            .ok_or_else(|| PreviewError::Metal("sampler".into()))?;
        Ok(Self {
            device,
            queue,
            pipeline,
            sampler,
        })
    }
}
impl Gpu {
    pub fn new(wake: Arc<Wake>) -> PreviewResult<Self> {
        let created = objc2_quartz_core::CACurrentMediaTime();
        let Resources {
            device,
            queue,
            pipeline,
            sampler,
        } = RESOURCES.with(|cache| {
            let mut cache = cache.borrow_mut();
            if cache.is_none() {
                *cache = Some(Resources::new()?);
            }
            Ok::<_, PreviewError>(cache.as_ref().unwrap().clone())
        })?;
        Ok(Self {
            device,
            queue,
            pipeline,
            sampler,
            in_flight: Arc::new(AtomicUsize::new(0)),
            wake,
            stats: PreviewStats::default(),
            metrics: Arc::default(),
            last_sources: Vec::with_capacity(8),
            failure: Arc::default(),
            scheduled: Arc::default(),
            sequence: 0,
            session_submitted: false,
            presented_sequence: 0,
            created,
            presented_revision: Arc::default(),
        })
    }

    pub fn begin_session(&mut self) {
        self.discard_presentation();
        self.session_submitted = false;
        self.created = objc2_quartz_core::CACurrentMediaTime();
        let mut metrics = self.metrics.lock().unwrap();
        metrics.session += 1;
        metrics.presented_frames = 0;
        metrics.first_present_us = 0;
        metrics.last_presented_time = 0.;
    }

    pub fn has_submitted(&self) -> bool { self.session_submitted }

    pub fn is_presented(&self, revision: u64) -> bool {
        self.presented_revision.load(Ordering::Acquire) == revision
    }

    pub fn mark_translated(&self, revision: u64) {
        self.presented_revision.fetch_max(revision, Ordering::AcqRel);
    }

    pub fn take_presentation(&mut self) -> Option<Presentation> {
        let presentation = self.scheduled.lock().unwrap().1.take()?;
        if presentation.sequence <= self.presented_sequence {
            return None;
        }
        self.presented_sequence = presentation.sequence;
        self.metrics.lock().unwrap().callback_to_owner.push(
            ((objc2_quartz_core::CACurrentMediaTime() - presentation.callback_time).max(0.) * 1e6)
                as u64,
        );
        Some(presentation)
    }

    pub fn discard_presentation(&mut self) {
        let mut s = self.scheduled.lock().unwrap();
        s.0 += 1;
        s.1 = None;
    }

    pub fn diagnostics(&self) -> super::PreviewDiagnostics {
        let m = self.metrics.lock().unwrap();
        super::PreviewDiagnostics {
            presentation_interval: m.presentation_interval.summary(),
            submission_to_present: m.submission_to_present.summary(),
            callback_to_owner: m.callback_to_owner.summary(),
            presentation_lateness: m.presentation_lateness.summary(),
            ..Default::default()
        }
    }

    pub fn stats(&self) -> PreviewStats {
        let mut s = self.stats;
        let m = self.metrics.lock().unwrap();
        s.presented_frames = m.presented_frames;
        s.first_present_us = m.first_present_us;
        s.cpu_render = m.cpu.summary();
        s.gpu_render = m.gpu.summary();
        s.capture_to_present = m.capture_to_present.summary();
        s.last_gpu_us = m.last_gpu_us;
        s
    }

    pub fn reap(&mut self) -> PreviewResult<()> {
        if let Some(error) = self.failure.lock().unwrap().take() {
            Err(PreviewError::Metal(error))
        } else {
            Ok(())
        }
    }

    fn encode_preview(
        &self,
        encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>,
        p: &Preview,
        camera: PreviewCamera,
        source: &ProtocolObject<dyn MTLTexture>,
        uv: [f32; 4],
        size: CGSize,
    ) {
        let clip = p
            .clip
            .map(|r| {
                [
                    r.origin.x as f32,
                    r.origin.y as f32,
                    r.max().x as f32,
                    r.max().y as f32,
                ]
            })
            .unwrap_or([0., 0., size.width as f32, size.height as f32]);
        let params = Params {
            transform: p.transform,
            camera: [
                camera.scale,
                camera.translation[0],
                camera.translation[1],
                0.,
            ],
            rect: [
                p.frame.origin.x as f32,
                p.frame.origin.y as f32,
                p.frame.size.width as f32,
                p.frame.size.height as f32,
            ],
            clip,
            viewport: [
                size.width as f32,
                size.height as f32,
                p.opacity.clamp(0., 1.),
                p.corner_radius,
            ],
            uv,
            border: p.border_color,
            style: [p.border_width, 0., 0., 0.],
        };
        unsafe {
            // Metal copies these <4KiB constants into command-owned storage.
            // No persistently mapped CPU/GPU buffer race or per-quad heap buffer.
            let ptr = NonNull::from(&params).cast();
            encoder.setVertexBytes_length_atIndex(ptr, size_of::<Params>(), 0);
            encoder.setFragmentBytes_length_atIndex(ptr, size_of::<Params>(), 0);
            encoder.setFragmentTexture_atIndex(Some(source), 0);
            encoder.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::TriangleStrip, 0, 4);
        }
    }

    pub fn sources_changed(&self, scene: &[Preview], captures: &Captures) -> bool {
        scene.iter().filter(|p| p.opacity > 0.).any(|p| {
            let previous =
                self.last_sources.iter().find(|(id, _)| *id == p.window_id).map(|(_, seq)| *seq);
            previous != captures.frame(p).map(|f| f.sequence)
        })
    }

    pub fn render(
        &mut self,
        drawable: &ProtocolObject<dyn CAMetalDrawable>,
        size: CGSize,
        scene: &[Preview],
        camera: PreviewCamera,
        captures: &Captures,
        offset: CGPoint,
        revision: u64,
        target: f64,
    ) -> PreviewResult<bool> {
        if self.in_flight.load(Ordering::Acquire) >= 2 {
            self.stats.dropped_presentations += 1;
            return Ok(false);
        }
        let start = Instant::now();
        // CAMetalDisplayLink supplies an available drawable; never call blocking nextDrawable.
        let command = self
            .queue
            .commandBuffer()
            .ok_or_else(|| PreviewError::Metal("command buffer".into()))?;
        let pass = MTLRenderPassDescriptor::renderPassDescriptor();
        let color = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        color.setTexture(Some(&drawable.texture()));
        color.setLoadAction(MTLLoadAction::Clear);
        color.setStoreAction(MTLStoreAction::Store);
        color.setClearColor(MTLClearColor {
            red: 0.,
            green: 0.,
            blue: 0.,
            alpha: 0.,
        });
        let encoder = command
            .renderCommandEncoderWithDescriptor(&pass)
            .ok_or_else(|| PreviewError::Metal("render encoder".into()))?;
        encoder.setRenderPipelineState(&self.pipeline);
        unsafe {
            encoder.setFragmentSamplerState_atIndex(Some(&self.sampler), 0);
        }
        let mut samples = Vec::with_capacity(scene.len());
        let mut capture_time = f64::INFINITY;
        for p in scene {
            if p.opacity <= 0.0 {
                continue;
            }
            let Some(frame) = captures.frame(p) else {
                continue;
            };
            self.encode_preview(&encoder, p, camera, &frame.texture, frame.uv, size);
            if !self
                .last_sources
                .iter()
                .any(|&(id, seq)| id == p.window_id && seq == frame.sequence)
            {
                if frame.display_time > 0. {
                    capture_time = capture_time.min(frame.display_time);
                }
            }
            samples.push(frame.sample.clone());
        }
        encoder.endEncoding();
        self.last_sources.clear();
        for p in scene {
            if let Some(f) = captures.frame(p) {
                self.last_sources.push((p.window_id, f.sequence));
            }
        }
        let scene_complete = scene
            .iter()
            .filter(|p| p.mode != super::CaptureMode::Inactive && p.opacity > 0.)
            .all(|p| captures.frame(p).is_some());
        let submitted_time = objc2_quartz_core::CACurrentMediaTime();
        let presentation_metrics = self.metrics.clone();
        let epoch = self.scheduled.lock().unwrap().0;
        let session = presentation_metrics.lock().unwrap().session;
        let presentation_state = Arc::downgrade(&self.scheduled);
        let presented_revision = self.presented_revision.clone();
        let presentation_wake = self.wake.clone();
        let created = self.created;
        let presented = RcBlock::new(move |d: NonNull<ProtocolObject<dyn MTLDrawable>>| {
            let presented = unsafe { d.as_ref() }.presentedTime();
            if presented <= 0.
                || presentation_state.upgrade().is_none_or(|s| s.lock().unwrap().0 != epoch)
            {
                return;
            }
            if scene_complete {
                presented_revision.fetch_max(revision, Ordering::AcqRel);
            }
            presentation_wake.notify();
            {
                let mut m = presentation_metrics.lock().unwrap();
                if m.session != session {
                    return;
                }
                if m.presented_frames == 0 {
                    m.first_present_us = ((presented - created).max(0.) * 1e6) as u64;
                }
                m.submission_to_present
                    .push(((presented - submitted_time).max(0.) * 1e6) as u64);
                m.presentation_lateness.push(((presented - target).max(0.) * 1e6) as u64);
                m.presented_frames += 1;
                if m.last_presented_time > 0. {
                    let interval = ((presented - m.last_presented_time).max(0.) * 1e6) as u64;
                    m.presentation_interval.push(interval);
                }
                m.last_presented_time = presented;
            }
            if capture_time.is_finite() && presented >= capture_time {
                presentation_metrics
                    .lock()
                    .unwrap()
                    .capture_to_present
                    .push(((presented - capture_time) * 1e6) as u64);
            }
        });
        unsafe {
            drawable.addPresentedHandler(RcBlock::as_ptr(&presented));
        }

        let count = self.in_flight.clone();
        let wake = self.wake.clone();
        self.in_flight.fetch_add(1, Ordering::AcqRel);
        let metrics = self.metrics.clone();
        let failure = self.failure.clone();
        let samples = Mutex::new(Some(samples));
        let completed = RcBlock::new(move |c: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
            let c = unsafe { c.as_ref() };
            let gpu_us = ((c.GPUEndTime() - c.GPUStartTime()).max(0.) * 1e6) as u64;
            {
                let mut m = metrics.lock().unwrap();
                m.gpu.push(gpu_us);
                m.last_gpu_us = gpu_us;
            }
            let failed = c.status() == MTLCommandBufferStatus::Error;
            if failed {
                *failure.lock().unwrap() = Some(
                    c.error()
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "Metal command failed".into()),
                );
            }
            // Captured samples pin capture-pool entries even if compositor/scene
            // is destroyed while this command is still executing.
            samples.lock().unwrap().take();
            // Completion only needs an owner wake for backpressure or errors.
            // Metal retains committed commands; no owner-side command list/reap is needed.
            if count.fetch_sub(1, Ordering::AcqRel) >= 2 || failed {
                wake.notify();
            }
        });
        unsafe {
            command.addCompletedHandler(RcBlock::as_ptr(&completed));
        }
        self.sequence += 1;
        let packet = Mutex::new(Some(Presentation {
            drawable: unsafe { Retained::retain(drawable as *const _ as *mut _) }.unwrap(),
            offset,
            callback_time: 0.,
            sequence: self.sequence,
        }));
        let scheduled = Arc::downgrade(&self.scheduled);
        let wake = self.wake.clone();
        let scheduled_handler =
            RcBlock::new(move |c: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
                if unsafe { c.as_ref() }.status() == MTLCommandBufferStatus::Error {
                    return;
                }
                let Some(mut packet) = packet.lock().unwrap().take() else {
                    return;
                };
                packet.callback_time = objc2_quartz_core::CACurrentMediaTime();
                let Some(scheduled) = scheduled.upgrade() else {
                    return;
                };
                let mut pending = scheduled.lock().unwrap();
                if pending.0 == epoch
                    && pending.1.as_ref().is_none_or(|p| p.sequence < packet.sequence)
                {
                    pending.1 = Some(packet);
                }
                drop(pending);
                wake.notify();
            });
        unsafe {
            command.addScheduledHandler(RcBlock::as_ptr(&scheduled_handler));
        }
        // Transactional presentation occurs on the owner after scheduling, in its current CA
        // transaction. GPU completion only releases captured samples. This is Apple's presentsWithTransaction contract without
        // blocking the owner in waitUntilScheduled ; neither drawable acquisition nor GPU work blocks the owner.
        command.commit();
        self.session_submitted = true;
        self.stats.submitted_frames += 1;
        self.stats.last_cpu_render_us = start.elapsed().as_micros() as u64;
        self.metrics.lock().unwrap().cpu.push(self.stats.last_cpu_render_us);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use objc2_core_foundation::CGRect;

    use super::*;

    /// Readback is confined to this test. Production imports IOSurfaces and never
    /// reads captured pixels on the CPU. This tests the actual shipped metallib.
    #[test]
    fn metal_pixels_obey_transforms_clip_rounding_and_premultiplied_stacking() {
        objc2::rc::autoreleasepool(|_| {
            let wake = Arc::new(Wake {
                pending: std::sync::atomic::AtomicBool::new(false),
                callback: Box::new(|| {}),
            });
            let gpu = Gpu::new(wake).unwrap();
            let texture = |width, height| {
                let d = unsafe {
                    MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                        MTLPixelFormat::BGRA8Unorm,
                        width,
                        height,
                        false,
                    )
                };
                d.setStorageMode(MTLStorageMode::Shared);
                d.setUsage(MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget);
                gpu.device.newTextureWithDescriptor(&d).unwrap()
            };
            let red = texture(4, 4);
            let blue = texture(4, 4);
            let target = texture(32, 32);
            let command = gpu.queue.commandBuffer().unwrap();
            for (image, color) in [
                (&red, MTLClearColor {
                    red: 0.5,
                    green: 0.,
                    blue: 0.,
                    alpha: 0.5,
                }),
                (&blue, MTLClearColor {
                    red: 0.,
                    green: 0.,
                    blue: 0.5,
                    alpha: 0.5,
                }),
            ] {
                let pass = MTLRenderPassDescriptor::renderPassDescriptor();
                let color_attachment =
                    unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
                color_attachment.setTexture(Some(image));
                color_attachment.setLoadAction(MTLLoadAction::Clear);
                color_attachment.setStoreAction(MTLStoreAction::Store);
                color_attachment.setClearColor(color);
                command.renderCommandEncoderWithDescriptor(&pass).unwrap().endEncoding();
            }
            let pass = MTLRenderPassDescriptor::renderPassDescriptor();
            let color = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
            color.setTexture(Some(&target));
            color.setLoadAction(MTLLoadAction::Clear);
            color.setStoreAction(MTLStoreAction::Store);
            color.setClearColor(MTLClearColor {
                red: 0.,
                green: 0.,
                blue: 0.,
                alpha: 0.,
            });
            let encoder = command.renderCommandEncoderWithDescriptor(&pass).unwrap();
            encoder.setRenderPipelineState(&gpu.pipeline);
            unsafe {
                encoder.setFragmentSamplerState_atIndex(Some(&gpu.sampler), 0);
            }
            let mut p = Preview::window(1, 1, CGRect::new(CGPoint::ZERO, CGSize::new(20., 32.)));
            gpu.encode_preview(
                &encoder,
                &p,
                PreviewCamera::default(),
                &blue,
                [0., 0., 1., 1.],
                CGSize::new(32., 32.),
            );
            p.frame = CGRect::new(CGPoint::new(8., 8.), CGSize::new(16., 16.));
            p.transform[12] = 4.;
            p.opacity = 0.5;
            p.corner_radius = 4.;
            p.clip = Some(CGRect::new(CGPoint::ZERO, CGSize::new(20., 32.)));
            gpu.encode_preview(
                &encoder,
                &p,
                PreviewCamera::default(),
                &red,
                [0., 0., 1., 1.],
                CGSize::new(32., 32.),
            );
            let p = Preview::window(1, 1, CGRect::new(CGPoint::ZERO, CGSize::new(16., 16.)));
            gpu.encode_preview(
                &encoder,
                &p,
                PreviewCamera {
                    scale: 0.5,
                    translation: [22., 24.],
                },
                &blue,
                [0., 0., 1., 1.],
                CGSize::new(32., 32.),
            );
            encoder.endEncoding();
            command.commit();
            command.waitUntilCompleted();
            assert_eq!(command.status(), MTLCommandBufferStatus::Completed);
            let mut pixels = [0u8; 32 * 32 * 4];
            unsafe {
                target.getBytes_bytesPerRow_fromRegion_mipmapLevel(
                    NonNull::from(&mut pixels).cast(),
                    32 * 4,
                    MTLRegion {
                        origin: MTLOrigin { x: 0, y: 0, z: 0 },
                        size: MTLSize {
                            width: 32,
                            height: 32,
                            depth: 1,
                        },
                    },
                    0,
                );
            }
            let pixel = |x: usize, y: usize| {
                <[u8; 4]>::try_from(&pixels[(y * 32 + x) * 4..(y * 32 + x + 1) * 4]).unwrap()
            };
            let near = |actual: [u8; 4], expected: [u8; 4]| {
                for (a, e) in actual.into_iter().zip(expected) {
                    assert!(a.abs_diff(e) <= 1, "actual={actual:?} expected={expected:?}");
                }
            };
            near(pixel(16, 16), [96, 0, 64, 160]); // red over blue, premultiplied alpha
            near(pixel(12, 8), [128, 0, 0, 128]); // transformed rounded corner exposes blue
            near(pixel(21, 16), [0, 0, 0, 0]); // scissor-equivalent screen-space clipping
            near(pixel(26, 28), [128, 0, 0, 128]); // shared camera zoom and translation
            near(pixel(30, 28), [0, 0, 0, 0]); // camera scales quad width
            near(pixel(31, 31), [0, 0, 0, 0]); // transparent target outside all quads
        });
    }
}
