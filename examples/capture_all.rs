//! Capture first-frame latency for every normal application window. No titles logged.
//! cargo run --release --example capture_all -- [thumbnail|native] [CONCURRENCY] [TIMEOUT_SECONDS] [WINDOW_IDS_FILE] [FOREGROUND_COUNT]
use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cgs::{CaptureMode, CaptureResolution, Preview, PreviewCompositor, SourceStatus};
use objc2_core_graphics::{CGDisplayBounds, CGMainDisplayID};
use objc2_foundation::{NSDate, NSRunLoop};
use objc2_screen_capture_kit::{SCShareableContent, SCWindow};

struct Batch {
    start: Instant,
    windows: Vec<(u32, Option<f64>)>,
}
thread_local! {
    static COMPOSITOR: RefCell<Option<PreviewCompositor>> = const { RefCell::new(None) };
    static BATCH: RefCell<Option<Batch>> = const { RefCell::new(None) };
}
fn render() {
    COMPOSITOR.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(c) = slot.as_mut() else { return };
        if let Err(e) = c.render(Instant::now()) {
            eprintln!("render_error={e}");
        }
        BATCH.with(|batch| {
            if let Some(batch) = batch.borrow_mut().as_mut() {
                for (id, elapsed) in &mut batch.windows {
                    if elapsed.is_none() && c.has_frame(*id)
                        && matches!(c.source_status(*id),
                            Some(SourceStatus::Complete { age } | SourceStatus::Idle { age: Some(age) })
                            if age <= batch.start.elapsed())
                    {
                        *elapsed = Some(batch.start.elapsed().as_secs_f64() * 1000.);
                    }
                }
            }
        });
    });
}
fn run_until(deadline: Instant, mut done: impl FnMut() -> bool) {
    while !done() && Instant::now() < deadline {
        objc2::rc::autoreleasepool(|_| {
            // Bound only the benchmark's deadline checks; rendering is wake-driven.
            let remaining = deadline.saturating_duration_since(Instant::now());
            let date = NSDate::dateWithTimeIntervalSinceNow(remaining.as_secs_f64().min(0.1));
            NSRunLoop::currentRunLoop()
                .runMode_beforeDate(unsafe { objc2_foundation::NSDefaultRunLoopMode }, &date);
        });
    }
}
fn eligible(w: &SCWindow) -> Option<(u32, i32)> {
    unsafe {
        let app = w.owningApplication()?;
        let size = w.frame().size;
        (w.windowLayer() == 0
            && size.width >= 32.
            && size.height >= 32.
            && app.processID() != std::process::id() as i32)
            .then_some((w.windowID(), app.processID()))
    }
}
fn scene_for(
    windows: &[(u32, i32)],
    foreground: Option<usize>,
    native: bool,
    mixed: bool,
) -> Vec<Preview> {
    windows
        .iter()
        .enumerate()
        .map(|(index, &(id, pid))| {
            let mut p = Preview::window(
                id,
                pid,
                cgs::CGRect::new(cgs::CGPoint::ZERO, cgs::CGSize::new(320., 240.)),
            );
            p.mode = if foreground.is_some_and(|n| index < n) {
                CaptureMode::Live
            } else {
                CaptureMode::Snapshot
            };
            p.opacity = if p.mode == CaptureMode::Live { 1. } else { 0. };
            p.priority = if p.mode == CaptureMode::Live { 1 } else { 0 };
            p.resolution = if native || (mixed && index == 0) {
                CaptureResolution::Native
            } else {
                CaptureResolution::Thumbnail { width: 640, height: 480 }
            };
            p
        })
        .collect()
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let _ = CGDisplayBounds(CGMainDisplayID());
    if args.first().is_some_and(|s| s == "sample") {
        return sample_benchmark(&args);
    }
    let present = args.iter().any(|s| s == "present");
    let native = args.first().is_some_and(|s| s == "native");
    let mixed = args.first().is_some_and(|s| s == "mixed");
    let foreground: Option<usize> = args.get(4).map(|s| s.parse()).transpose()?;
    let concurrency: usize = args.get(1).map(|s| s.parse()).transpose()?.unwrap_or(2);
    if !(1..=8).contains(&concurrency) {
        return Err("concurrency must be 1..8".into());
    }
    let timeout: f64 = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(5.);
    if !timeout.is_finite() || timeout <= 0. {
        return Err("timeout must be positive".into());
    }
    let start = Instant::now();
    let supplied: Option<Vec<(u32, i32)>> = args
        .get(3)
        .map(|path| std::fs::read_to_string(path))
        .transpose()?
        .and_then(|text| {
            let lines: Vec<_> = text.lines().filter(|s| !s.trim().is_empty()).collect();
            if lines.iter().all(|s| s.split_whitespace().count() == 2) {
                lines
                    .iter()
                    .map(|line| {
                        let mut parts = line.split_whitespace();
                        Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
                    })
                    .collect()
            } else {
                None
            }
        });
    let windows = if let Some(windows) = supplied {
        windows
    } else {
        let inventory = Arc::new(Mutex::new(None));
        let result = inventory.clone();
        let callback = block2::RcBlock::new(
            move |content: *mut SCShareableContent, error: *mut objc2_foundation::NSError| {
                // Only plain IDs and strings cross this queue boundary, never ObjC objects.
                let windows = unsafe { content.as_ref() }
                    .map(|c| unsafe {
                        c.windows().iter().filter_map(|w| eligible(&w)).collect::<Vec<_>>()
                    })
                    .ok_or_else(|| {
                        unsafe { error.as_ref() }
                            .map(ToString::to_string)
                            .unwrap_or_else(|| "discovery returned no content".into())
                    });
                *result.lock().unwrap() = Some(windows);
            },
        );
        unsafe {
            SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            true, false, &callback);
        }
        run_until(Instant::now() + Duration::from_secs(15), || {
            inventory.lock().unwrap().is_some()
        });
        let mut windows = inventory.lock().unwrap().take().ok_or("discovery timed out")??;
        if let Some(path) = args.get(3) {
            let requested: std::collections::HashSet<u32> = std::fs::read_to_string(path)?
                .split_whitespace()
                .map(str::parse)
                .collect::<Result<_, _>>()?;
            windows.retain(|(id, _)| requested.contains(id));
            if windows.len() != requested.len() {
                return Err(format!(
                    "only {} of {} requested windows were discoverable",
                    windows.len(),
                    requested.len()
                )
                .into());
            }
        }
        windows
    };
    let discovery_ms = start.elapsed().as_secs_f64() * 1000.;
    if windows.is_empty() {
        return Err("no eligible application windows".into());
    }
    let bounds = CGDisplayBounds(CGMainDisplayID());
    let mut c = PreviewCompositor::new(bounds, 2., || {
        dispatch2::DispatchQueue::main().exec_async(render);
    })?;
    c.set_level(if present { 25 } else { -1 })?; // Optional on-screen presentation measurement.
    c.set_scene(&[])?;
    let cold_setup_ms = start.elapsed().as_secs_f64() * 1000.;
    c.show()?;
    COMPOSITOR.with(|slot| slot.replace(Some(c)));
    let mut latencies = Vec::new();
    let mut failed = 0;
    let mut ready_times = Vec::new();
    let mut foreground_ms = 0f64;
    let capture_start = Instant::now();
    for chunk in windows.chunks(if foreground.is_some() {
        windows.len()
    } else {
        concurrency
    }) {
        let scene = scene_for(chunk, foreground, native, mixed);
        BATCH.with(|b| {
            b.replace(Some(Batch {
                start: Instant::now(),
                windows: chunk.iter().map(|&(id, _)| (id, None)).collect(),
            }))
        });
        COMPOSITOR.with(|slot| slot.borrow_mut().as_mut().unwrap().set_scene(&scene))?;
        run_until(Instant::now() + Duration::from_secs_f64(timeout), || {
            BATCH.with(|b| b.borrow().as_ref().unwrap().windows.iter().all(|(_, t)| t.is_some()))
        });
        let batch = BATCH.with(|b| b.take().unwrap());
        for (index, (id, elapsed)) in batch.windows.into_iter().enumerate() {
            if let Some(ms) = elapsed {
                latencies.push(ms);
                if foreground.is_some_and(|n| index < n) {
                    foreground_ms = foreground_ms.max(ms);
                }
                let cumulative_ms =
                    batch.start.saturating_duration_since(start).as_secs_f64() * 1000. + ms;
                ready_times.push(cumulative_ms);
                println!(
                    "window_id={id} first_frame_ms={ms:.3} cumulative_ms={:.3}",
                    cumulative_ms
                );
            } else {
                failed += 1;
                COMPOSITOR.with(|slot| {
                    println!(
                        "window_id={id} timeout status={:?}",
                        slot.borrow().as_ref().unwrap().source_status(id)
                    )
                });
            }
        }
    }
    let capture_ms = capture_start.elapsed().as_secs_f64() * 1000.;
    let total_ms = start.elapsed().as_secs_f64() * 1000.;
    latencies.sort_by(f64::total_cmp);
    let percentile = |p: f64| {
        latencies
            .get(((latencies.len().saturating_sub(1)) as f64 * p).round() as usize)
            .copied()
            .unwrap_or(0.)
    };
    let last_valid_frame_ms = ready_times.iter().copied().fold(0., f64::max);
    if present {
        run_until(Instant::now() + Duration::from_secs_f64(timeout), || {
            COMPOSITOR.with(|slot| slot.borrow().as_ref().unwrap().stats().presented_frames > 0)
        });
    }
    let stats = COMPOSITOR.with(|slot| slot.borrow().as_ref().unwrap().stats());
    println!(
        "cold_presentation first_ms={:.3} presented={}",
        cold_setup_ms + stats.first_present_us as f64 / 1000.,
        usize::from(stats.presented_frames > 0)
    );
    let bytes = stats.latest_frame_bytes;
    println!(
        "setup max_filter_us={} max_create_us={} max_start_us={} max_owner_pump_us={}",
        stats.max_filter_us,
        stats.max_stream_create_us,
        stats.max_stream_start_us,
        stats.max_capture_pump_us
    );
    println!(
        "summary quality={} concurrency={} windows={} captured={} failed={} discovery_ms={discovery_ms:.3} total_ms={total_ms:.3} first_frame_p50_ms={:.3} first_frame_p95_ms={:.3} first_frame_max_ms={:.3} cached_frame_bytes={bytes} last_valid_frame_ms={last_valid_frame_ms:.3} capture_ms={capture_ms:.3} foreground_ms={foreground_ms:.3}",
        if native {
            "native"
        } else if mixed {
            "mixed"
        } else {
            "thumbnail"
        },
        concurrency,
        windows.len(),
        latencies.len(),
        failed,
        percentile(0.5),
        percentile(0.95),
        percentile(1.)
    );
    COMPOSITOR.with(|slot| slot.borrow_mut().as_mut().unwrap().hide())?;
    let retained =
        COMPOSITOR.with(|slot| slot.borrow().as_ref().unwrap().stats().latest_frame_bytes);
    println!("after_hide_frame_bytes={retained}");
    println!(
        "after_hide_reusable_bytes={}",
        COMPOSITOR.with(|slot| slot.borrow().as_ref().unwrap().stats().reusable_frame_bytes)
    );
    let reuse_context = args.get(5).is_some_and(|s| s == "reuse");
    let mut retained = COMPOSITOR.with(|slot| slot.take());
    if !reuse_context {
        retained = None;
    }
    if args.get(5).is_some_and(|s| s == "warm" || s == "reuse") {
        if let Some(delay) = args.get(6).filter(|s| s.as_str() != "present") {
            let seconds: f64 = delay.parse()?;
            if !seconds.is_finite() || seconds < 0. {
                return Err("reopen delay must be nonnegative".into());
            }
            let before = retained.as_ref().map(|c| c.stats().submitted_frames);
            run_until(Instant::now() + Duration::from_secs_f64(seconds), || false);
            if let Some(c) = &retained {
                let stats = c.stats();
                println!(
                    "idle_summary seconds={seconds} submissions={} streams={} jobs={} active_frame_bytes={} reusable_frame_bytes={}",
                    stats.submitted_frames - before.unwrap(),
                    stats.process_streams,
                    stats.process_capture_jobs,
                    stats.latest_frame_bytes,
                    stats.reusable_frame_bytes
                );
            }
        }
        let warm_start = Instant::now();
        let mut c = if let Some(c) = retained.take() {
            c
        } else {
            PreviewCompositor::new(bounds, 2., || {
                dispatch2::DispatchQueue::main().exec_async(render);
            })?
        };
        c.set_level(if present { 25 } else { -1 })?;
        c.set_scene(&scene_for(&windows, foreground, native, mixed))?;
        BATCH.with(|b| {
            b.replace(Some(Batch {
                start: warm_start,
                windows: windows.iter().map(|&(id, _)| (id, None)).collect(),
            }))
        });
        let setup_ms = warm_start.elapsed().as_secs_f64() * 1000.;
        c.show()?;
        let ready = windows.iter().filter(|(id, _)| c.has_frame(*id)).count();
        let foreground_ready = windows
            .iter()
            .take(foreground.unwrap_or(0))
            .filter(|(id, _)| c.has_frame(*id))
            .count();
        let ready_ms = warm_start.elapsed().as_secs_f64() * 1000.;
        COMPOSITOR.with(|slot| slot.replace(Some(c)));
        if present {
            run_until(Instant::now() + Duration::from_secs_f64(timeout), || {
                COMPOSITOR.with(|slot| slot.borrow().as_ref().unwrap().stats().presented_frames > 0)
            });
        }
        let presented =
            COMPOSITOR.with(|slot| slot.borrow().as_ref().unwrap().stats().presented_frames > 0);
        println!(
            "reuse_summary windows={} ready={ready} foreground_ready={foreground_ready} ready_ms={ready_ms:.3} presented={} first_present_ms={:.3}",
            windows.len(),
            usize::from(presented),
            COMPOSITOR.with(|slot| setup_ms
                + slot.borrow().as_ref().unwrap().stats().first_present_us as f64 / 1000.)
        );
        run_until(Instant::now() + Duration::from_secs_f64(timeout), || {
            BATCH.with(|b| b.borrow().as_ref().unwrap().windows.iter().all(|(_, t)| t.is_some()))
        });
        let batch = BATCH.with(|b| b.take().unwrap());
        let visible: Vec<_> = batch
            .windows
            .iter()
            .take(foreground.unwrap_or(0))
            .filter_map(|(_, t)| *t)
            .collect();
        println!(
            "warm_summary windows={} captured={} all_ms={:.3} foreground_ms={:.3} first_ms={:.3}",
            windows.len(),
            batch.windows.iter().filter(|(_, t)| t.is_some()).count(),
            warm_start.elapsed().as_secs_f64() * 1000.,
            visible.iter().copied().fold(0., f64::max),
            visible.iter().copied().fold(f64::INFINITY, f64::min)
        );
        COMPOSITOR.with(|slot| slot.borrow_mut().as_mut().unwrap().hide())?;
        COMPOSITOR.with(|slot| slot.take());
    }
    if failed != 0 {
        return Err("some windows did not produce a valid frame; see statuses above".into());
    }
    Ok(())
}

// Benchmark only: compare a one-shot IOSurface sample with stream startup.
// This does not replace the compositor's SCStream live capture path.
struct ShotWindow(objc2::rc::Retained<SCWindow>);
impl ShotWindow {
    fn window(&self) -> &SCWindow { &self.0 }
}
// Immutable SCK window descriptions transfer once to an AnyThread filter factory.
unsafe impl Send for ShotWindow {}
struct ShotJobs {
    pending: Vec<ShotWindow>,
    finished: Vec<(u32, f64, usize)>,
    total: usize,
    discovered: bool,
}
fn request_shot(jobs: Arc<Mutex<ShotJobs>>, start: Instant) {
    let window = jobs.lock().unwrap().pending.pop();
    let Some(window) = window else { return };
    dispatch2::DispatchQueue::global_queue(dispatch2::GlobalQueueIdentifier::QualityOfService(
        dispatch2::DispatchQoS::UserInitiated,
    ))
    .exec_async(move || {
        objc2::rc::autoreleasepool(|_| unsafe {
            use objc2::AnyThread;
            use objc2_core_video::*;
            use objc2_screen_capture_kit::*;
            let id = window.window().windowID();
            let filter = SCContentFilter::initWithDesktopIndependentWindow(
                SCContentFilter::alloc(),
                window.window(),
            );
            let config = SCStreamConfiguration::new();
            config.setWidth(640);
            config.setHeight(480);
            config.setPixelFormat(kCVPixelFormatType_32BGRA);
            config.setShowsCursor(false);
            config.setIgnoreShadowsSingleWindow(true);
            config.setIgnoreGlobalClipSingleWindow(true);
            let completed = jobs.clone();
            let callback = block2::RcBlock::new(
                move |sample: *mut objc2_core_media::CMSampleBuffer,
                      _: *mut objc2_foundation::NSError| {
                    let bytes = sample
                        .as_ref()
                        .and_then(|s| s.image_buffer())
                        .and_then(|image| {
                            CVPixelBufferGetIOSurface(Some(&image)).map(|_| {
                                CVPixelBufferGetBytesPerRow(&image) * CVPixelBufferGetHeight(&image)
                            })
                        })
                        .unwrap_or(0);
                    completed.lock().unwrap().finished.push((
                        id,
                        start.elapsed().as_secs_f64() * 1000.,
                        bytes,
                    ));
                    request_shot(completed.clone(), start);
                },
            );
            SCScreenshotManager::captureSampleBufferWithFilter_configuration_completionHandler(
                &filter,
                &config,
                Some(&callback),
            );
        });
    });
}
fn sample_benchmark(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let concurrency: usize = args.get(1).map(|s| s.parse()).transpose()?.unwrap_or(2);
    if !(1..=8).contains(&concurrency) {
        return Err("concurrency must be 1..8".into());
    }
    let requested: std::collections::HashSet<u32> =
        std::fs::read_to_string(args.get(3).ok_or("sample mode needs WINDOW_IDS_FILE")?)?
            .lines()
            .filter_map(|line| line.split_whitespace().next())
            .map(str::parse)
            .collect::<Result<_, _>>()?;
    let jobs = Arc::new(Mutex::new(ShotJobs {
        pending: Vec::new(),
        finished: Vec::new(),
        total: 0,
        discovered: false,
    }));
    let completion = jobs.clone();
    let start = Instant::now();
    let callback = block2::RcBlock::new(
        move |content: *mut SCShareableContent, _: *mut objc2_foundation::NSError| {
            let mut state = completion.lock().unwrap();
            if let Some(content) = unsafe { content.as_ref() } {
                state.pending = unsafe { content.windows() }
                    .iter()
                    .filter(|w| requested.contains(&unsafe { w.windowID() }))
                    .map(ShotWindow)
                    .collect();
            }
            state.total = state.pending.len();
            state.discovered = true;
            drop(state);
            for _ in 0..concurrency {
                request_shot(completion.clone(), start);
            }
        },
    );
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(true, false, &callback);
    }
    run_until(Instant::now() + Duration::from_secs(15), || {
        let state = jobs.lock().unwrap();
        state.discovered && state.finished.len() == state.total
    });
    let state = jobs.lock().unwrap();
    let valid = state.finished.iter().filter(|(_, _, bytes)| *bytes != 0).count();
    let first_ms = state
        .finished
        .iter()
        .filter(|(_, _, b)| *b != 0)
        .map(|(_, t, _)| *t)
        .fold(f64::INFINITY, f64::min);
    println!(
        "sample_summary windows={} finished={} iosurface_frames={} first_ms={first_ms:.3} all_ms={:.3}",
        state.total,
        state.finished.len(),
        valid,
        start.elapsed().as_secs_f64() * 1000.
    );
    if valid != state.total || state.total == 0 {
        return Err("one-shot sample capture failed or timed out".into());
    }
    Ok(())
}
