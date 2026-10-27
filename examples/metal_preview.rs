//! Standalone native integration/benchmark harness (Screen Recording permission required).
//! --self-test [SECONDS] [WINDOWS] | WINDOW_ID PID [SECONDS]
use std::cell::RefCell;
use std::time::Instant;

use cgs::{
    CGPoint, CGRect, CGSize, CaptureMode, CaptureResolution, Easing, Preview, PreviewCompositor,
    Transition,
};
use objc2_core_graphics::{CGColor, CGDisplayBounds, CGMainDisplayID};
use objc2_foundation::{NSDate, NSRunLoop, NSString};
use objc2_metal::MTLTexture;
use objc2_quartz_core::{CABasicAnimation, CALayer, CAMediaTiming};
thread_local! { static COMPOSITOR: RefCell<Option<PreviewCompositor>> = const { RefCell::new(None) }; }
thread_local! { static CANVASES: RefCell<Vec<PreviewCompositor>> = const { RefCell::new(Vec::new()) }; }
fn render() {
    // A shared canvas must drive capture and pacing without rendering the source owner.
    if CANVASES.with(|c| c.borrow().is_empty()) {
        COMPOSITOR.with(|c| {
            if let Some(c) = c.borrow_mut().as_mut() {
                if let Err(e) = c.render(Instant::now()) {
                    eprintln!("render error: {e}");
                }
            }
        });
    }
    CANVASES.with(|canvases| {
        for canvas in &mut *canvases.borrow_mut() {
            canvas.render(Instant::now()).expect("canvas render");
        }
    });
}

fn run_for(seconds: f64) {
    let deadline = NSDate::dateWithTimeIntervalSinceNow(seconds);
    while deadline.timeIntervalSinceNow() > 0. {
        // Drain native autoreleases per event-loop turn, not once per benchmark.
        // runMode blocks on real events/timers; this is not a rendering poll.
        objc2::rc::autoreleasepool(|_| {
            NSRunLoop::currentRunLoop()
                .runMode_beforeDate(unsafe { objc2_foundation::NSDefaultRunLoopMode }, &deadline);
        });
    }
}
fn compositor<T>(f: impl FnOnce(&mut PreviewCompositor) -> T) -> T {
    COMPOSITOR.with(|c| f(c.borrow_mut().as_mut().unwrap()))
}
fn frame(x: f64, y: f64, w: f64, h: f64) -> CGRect {
    CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
}
#[allow(dead_code)]
fn main() -> Result<(), Box<dyn std::error::Error>> { run() }
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--scroll-probe") {
        let _ = std::fs::remove_file(std::path::Path::new(&args[i + 1]).join("presented.txt"));
    }
    let self_test = args.is_empty() || args[0] == "--self-test";
    if args.first().is_some_and(|a| a == "--list") {
        let callback = block2::RcBlock::new(
            |content: *mut objc2_screen_capture_kit::SCShareableContent,
             error: *mut objc2_foundation::NSError| {
                if let Some(error) = unsafe { error.as_ref() } {
                    eprintln!("{error}");
                }
                if let Some(content) = unsafe { content.as_ref() } {
                    for w in unsafe { content.windows() } {
                        if let Some(app) = unsafe { w.owningApplication() } {
                            println!(
                                "{} {} {:?} {} {:?}",
                                unsafe { w.windowID() },
                                unsafe { app.processID() },
                                unsafe { w.frame() },
                                unsafe { app.applicationName() },
                                unsafe { w.title() }
                            );
                        }
                    }
                }
            },
        );
        unsafe {
            objc2_screen_capture_kit::SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(true,false,&callback);
        }
        run_for(2.);
        return Ok(());
    }
    let bounds = CGDisplayBounds(CGMainDisplayID());
    let mut sources = Vec::new();
    let mut scene = Vec::new();
    let mut parked_frames = Vec::new();
    let seconds;
    if self_test {
        seconds = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(3.);
        let count: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5);
        assert!((1..=20).contains(&count));
        for i in 0..count {
            let (w, h) = if i == 0 {
                (960., 640.)
            } else {
                (480. + (i % 3) as f64 * 160., 320. + (i % 2) as f64 * 120.)
            };
            let position = if count == 1 {
                frame(bounds.origin.x + 20., bounds.origin.y + 40., w, h)
            } else {
                match i % 4 {
                    0 => frame(bounds.max().x - 1., bounds.max().y - 1., w, h), // Rift's one-pixel parking
                    1 => frame(bounds.max().x + 4000., bounds.max().y + 4000., w, h), // fully outside all local displays
                    2 => frame(bounds.origin.x - w + 1., bounds.max().y - 1., w, h),
                    _ => frame(bounds.origin.x + 20., bounds.origin.y + 40., w, h), // visible, occluded by overlay
                }
            };
            let window = cgs::CgsWindow::new_compositor(
                frame(bounds.origin.x + 20., bounds.origin.y + 40., w, h),
                0.,
            )?;
            parked_frames.push(position);
            let root = CALayer::layer();
            root.setFrame(frame(0., 0., w, h));
            root.setBackgroundColor(Some(&CGColor::new_generic_rgb(0.1, 0.7, 0.3, 1.)));
            let stripe = CALayer::layer();
            stripe.setFrame(frame(0., 0., w / 3., h));
            stripe.setBackgroundColor(Some(&CGColor::new_generic_rgb(0.8, 0.1, 0.2, 1.)));
            root.addSublayer(&stripe);
            let animation =
                CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str("opacity")));
            unsafe {
                animation.setFromValue(Some(&objc2_foundation::NSNumber::new_f64(0.2)));
                animation.setToValue(Some(&objc2_foundation::NSNumber::new_f64(1.)));
            }
            animation.setDuration(0.5);
            animation.setAutoreverses(true);
            animation.setRepeatCount(f32::INFINITY);
            stripe.addAnimation_forKey(&animation, None);
            let surface = cgs::WindowSurface::new_scaled(window.id(), root.bounds(), &root, 2.)?;
            window.order_above(None)?;
            surface.flush();
            let target = if count == 1 {
                frame(100., 100., 640., 480.)
            } else {
                frame(
                    30. + (i % 5) as f64 * 260.,
                    60. + (i / 5) as f64 * 190.,
                    240.,
                    160.,
                )
            };
            let mut p = Preview::window(window.id(), std::process::id() as i32, target);
            p.corner_radius = 12.;
            p.border_width = 2.;
            p.border_color = [0.2, 0.4, 1., 1.];
            if i > 0 {
                p.resolution = CaptureResolution::Thumbnail { width: 480, height: 320 };
            }
            p.priority = -(i as i32);
            scene.push(p);
            sources.push((surface, window, root));
        }
        // A just-created CA context may never draw when born offscreen. Let the
        // fixture produce its first backing contents before parking, as a real
        // application does before Rift hides it. Capture starts only afterwards;
        // the compositor never moves or receives access to source windows.
        run_for(0.15);
        let transaction = cgs::WindowTransaction::new()?;
        for ((_, window, _), position) in sources.iter().zip(parked_frames) {
            transaction.move_window(window, position.origin);
        }
        transaction.commit();
    } else {
        if args.len() < 2 {
            return Err(
                "expected WINDOW_ID PID [SECONDS] or --self-test [SECONDS] [WINDOWS]".into(),
            );
        }
        seconds = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10.);
        scene.push(Preview::window(
            args[0].parse()?,
            args[1].parse()?,
            frame(100., 100., 960., 640.),
        ));
    }
    if let Some(i) = args.iter().position(|a| a == "--canvas-probe") {
        canvas_probe(bounds, &scene, std::path::Path::new(&args[i + 1]))?;
        drop(sources);
        return Ok(());
    }
    let start = Instant::now();
    // Exercise the same renderer in a caller-owned CAContext, including its
    // existing native scroll probe and lifecycle/transition assertions.
    let host = if args.iter().any(|a| a == "--attached") {
        let window = cgs::CgsWindow::new_compositor(bounds, 0.)?;
        window.set_resolution(2.)?;
        window.set_level(25)?;
        let root = CALayer::layer();
        root.setFrame(frame(0., 0., bounds.size.width, bounds.size.height));
        root.setGeometryFlipped(true);
        let surface = cgs::WindowSurface::new_scaled(window.id(), root.bounds(), &root, 2.)?;
        window.order_above(None)?;
        Some((surface, window, root))
    } else {
        None
    };
    let wake = || {
        dispatch2::DispatchQueue::main().exec_async(render);
    };
    let mut c = if let Some((_, window, root)) = &host {
        PreviewCompositor::new_attached(window.id(), root, bounds, 2., wake)?
    } else {
        let c = PreviewCompositor::new(bounds, 2., wake)?;
        c.set_level(25)?;
        c
    };
    if args.iter().any(|a| a == "--adaptive") {
        let policies: Vec<_> = scene
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mut source = cgs::PreviewSource::from(p);
                source.frame_interval =
                    std::time::Duration::from_secs_f64(1. / if i == 0 { 60. } else { 30. });
                source
            })
            .collect();
        c.set_sources(&policies)?;
        c.set_visual_scene(&scene)?;
    } else {
        c.set_scene(&scene)?;
    }
    c.show()?;
    println!(
        "overlay_window_id={} sources={:?}",
        c.window_id(),
        scene.iter().map(|p| (p.window_id, p.process_id)).collect::<Vec<_>>()
    );
    COMPOSITOR.with(|slot| slot.replace(Some(c)));
    run_for(seconds);
    let stats = compositor(|c| c.stats());
    println!(
        "elapsed_ms={} stats={stats:?} diagnostics={:?}",
        start.elapsed().as_millis(),
        compositor(|c| c.diagnostics())
    );
    for p in &scene {
        println!(
            "source={} status={:?}",
            p.window_id,
            compositor(|c| c.source_status(p.window_id))
        );
    }
    assert!(stats.captured_frames > 0, "no valid capture frames");
    assert!(
        stats.presented_frames > 0,
        "no actual Metal presentations: unlock the graphical session before running native presentation tests"
    );
    assert!(stats.streams <= 8, "capture pool exceeded bound");
    if args.iter().any(|a| a == "--measure-only") {
        compositor(|c| c.hide())?;
        run_for(0.3);
        assert_eq!(compositor(|c| c.stats().process_capture_jobs), 0);
        drop(COMPOSITOR.with(|c| c.take()));
        drop(sources);
        return Ok(());
    }
    if self_test {
        if let Some(i) = args.iter().position(|a| a == "--scroll-probe") {
            let dir = std::path::Path::new(&args[i + 1]);
            std::fs::create_dir_all(dir)?;
            let capture = |name: &str| {
                let id = compositor(|c| c.window_id()).to_string();
                assert!(
                    std::process::Command::new("screencapture")
                        .args(["-x", "-l", &id])
                        .arg(dir.join(name))
                        .status()
                        .unwrap()
                        .success()
                );
            };
            capture("before.png");
            let submitted = compositor(|c| c.stats().submitted_frames);
            {
                let _transaction = cgs::LayerTransaction::disabled().flush_on_commit();
                compositor(|c| c.translate(CGPoint::new(0., 200.)))?;
            }
            // No owner run-loop turn and therefore no GPU submission can occur
            // between translation and this actual WindowServer screenshot.
            capture("translated.png");
            assert_eq!(compositor(|c| c.stats().submitted_frames), submitted);
            run_for(0.4);
            assert!(
                compositor(|c| c.stats().submitted_frames) > submitted,
                "replacement frame was not submitted"
            );
            assert!(
                compositor(|c| c.is_ready()),
                "replacement frame was not presented"
            );
            capture("rebased.png");
            std::fs::write(
                dir.join("presented.txt"),
                format!("{:?}\n", compositor(|c| c.stats())),
            )?;
            compositor(|c| c.translate(CGPoint::new(0., -200.)))?;
            run_for(0.2);
        }
        for p in scene.iter().take(8) {
            assert!(
                matches!(
                    compositor(|c| c.source_status(p.window_id)),
                    Some(cgs::SourceStatus::Complete { .. } | cgs::SourceStatus::Idle { .. })
                ),
                "source never became usable"
            );
        }
        let retained =
            compositor(|c| c.captured_frame(scene[0].window_id)).expect("retained source frame");
        let mut consumer = PreviewCompositor::new(bounds, 2., || {})?;
        consumer.set_scene(&scene[..1])?;
        consumer.reuse_frame(&retained);
        assert!(
            consumer.has_frame(scene[0].window_id),
            "frame transfer lost last-valid pixels"
        );
        assert_eq!(
            consumer.captured_frame(scene[0].window_id).unwrap().texture_coordinates(),
            retained.texture_coordinates()
        );
        drop(consumer);
        drop(retained);
        // The shared camera must not restart paused source capture.
        let mut destination = scene.clone();
        for p in &mut destination {
            p.frame.origin.x += 70.;
            p.opacity = 0.6;
        }
        compositor(|c| {
            c.transition_to(
                &destination,
                Transition::new(std::time::Duration::from_millis(200), Easing::EaseInOut),
            )
        })?;
        run_for(0.4);
        assert!(!compositor(|c| c.is_animating()));
        compositor(|c| {
            c.transition_to(
                &scene,
                Transition::new(std::time::Duration::from_secs(1), Easing::Linear),
            )
        })?;
        for progress in [0.2, 0.7, 0.3, 0., 1.] {
            compositor(|c| c.set_progress(progress))?;
            run_for(0.05);
        }
        for (_, _, root) in &sources {
            if let Some(children) = unsafe { root.sublayers() } {
                for child in children {
                    child.removeAllAnimations();
                }
            }
        }
        run_for(0.5);
        let static_submissions = compositor(|c| c.stats().submitted_frames);
        run_for(0.2);
        println!(
            "static_live_submissions={}",
            compositor(|c| c.stats().submitted_frames) - static_submissions
        );
        let mut snapshot = scene.clone();
        for p in &mut snapshot {
            p.mode = CaptureMode::Snapshot;
        }
        compositor(|c| c.set_scene(&snapshot))?;
        run_for(0.5);
        assert_eq!(
            compositor(|c| c.stats().streams),
            0,
            "snapshot streams failed to stop"
        );
        let idle_submissions = compositor(|c| c.stats().submitted_frames);
        run_for(0.2);
        assert_eq!(
            compositor(|c| c.stats().submitted_frames),
            idle_submissions,
            "unchanged snapshot scene kept rendering"
        );
        // Visual-only updates preserve a paused capture and last-valid pixels.
        let policies: Vec<_> = snapshot.iter().map(cgs::PreviewSource::from).collect();
        compositor(|c| c.set_sources(&policies))?;
        let before_policy = compositor(|c| c.stats().submitted_frames);
        let mut priority = policies[0];
        priority.priority += 1;
        compositor(|c| c.update_sources(&[priority]))?;
        run_for(0.1);
        assert_eq!(
            compositor(|c| c.stats().submitted_frames),
            before_policy,
            "capture priority alone submitted a GPU frame"
        );
        if policies.len() > 1 {
            let mut quality = policies[1];
            quality.resolution = CaptureResolution::Thumbnail { width: 800, height: 600 };
            compositor(|c| c.update_sources(&[quality]))?;
            assert!(
                compositor(|c| c.has_frame(quality.window_id)),
                "quality upgrade lost last-valid pixels"
            );
            run_for(0.5);
            assert_eq!(
                compositor(|c| c.captured_frame(quality.window_id).unwrap().texture().width()),
                800,
                "paused snapshot failed to upgrade resolution"
            );
            assert_eq!(compositor(|c| c.stats().streams), 0);
        }
        snapshot[0].border_width = 4.;
        compositor(|c| c.set_visual_scene(&snapshot))?;
        run_for(0.2);
        assert!(compositor(|c| c.has_frame(snapshot[0].window_id)));
        assert_eq!(
            compositor(|c| c.stats().streams),
            0,
            "visual edit resumed capture"
        );
        compositor(|c| {
            c.set_camera(cgs::PreviewCamera {
                scale: 0.9,
                translation: [4., 8.],
            })
        })?;
        run_for(0.1);
        assert_eq!(
            compositor(|c| c.stats().streams),
            0,
            "camera edit resumed capture"
        );
        compositor(|c| c.set_camera(cgs::PreviewCamera::default()))?;
        compositor(|c| c.set_scene(&scene))?;
        run_for(0.5);
        assert!(
            compositor(|c| c.stats().streams) > 0,
            "warm streams failed to restart"
        );
        // Resize an offscreen source and refresh descriptions: native capture
        // must resize without replacing its stream or losing other live sources.
        let resized = frame(bounds.max().x + 4000., bounds.max().y + 4000., 480., 320.);
        let transaction = cgs::WindowTransaction::new()?;
        transaction.set_rounded_frame(&sources[0].1, resized, 0.)?;
        sources[0].0.set_bounds_in(&transaction, frame(0., 0., 480., 320.));
        sources[0].2.setFrame(frame(0., 0., 480., 320.));
        transaction.commit();
        sources[0].0.flush();
        run_for(0.6);
        let resized_stats = compositor(|c| c.stats());
        assert!(
            resized_stats.latest_frame_bytes < stats.latest_frame_bytes,
            "native source resize did not reconfigure capture: {resized_stats:?}"
        );
        assert!(matches!(
            compositor(|c| c.source_status(scene[0].window_id)),
            Some(cgs::SourceStatus::Complete { .. } | cgs::SourceStatus::Idle { .. })
        ));
        // Destroy a captured source while commands may still reference it.
        let destroyed = sources.remove(0);
        drop(destroyed);
        compositor(|c| c.refresh_sources());
        run_for(0.5);
        assert!(matches!(
            compositor(|c| c.source_status(scene[0].window_id)),
            Some(
                cgs::SourceStatus::Failed(_)
                    | cgs::SourceStatus::Stopped
                    | cgs::SourceStatus::Blank
            )
        ));
        compositor(|c| c.hide())?;
        run_for(0.3);
        assert_eq!(
            compositor(|c| c.stats().latest_frame_bytes),
            0,
            "hide retained captured samples"
        );
        assert_eq!(compositor(|c| c.stats().streams), 0);
        assert_eq!(
            compositor(|c| c.stats().process_capture_jobs),
            0,
            "capture jobs survived close"
        );
        assert_eq!(
            compositor(|c| c.diagnostics().process_reserved_capture_bytes),
            0,
            "reservations survived close"
        );
        if let Some((_, window, root)) = &host {
            compositor(|c| c.attach_to(window.id(), root))?;
            compositor(|c| c.set_scene(if scene.len() > 1 { &scene[1..2] } else { &[] }))?;
            compositor(|c| c.show())?;
            run_for(0.4);
            if scene.len() > 1 {
                assert!(compositor(|c| c.is_ready()), "attached reopen did not present");
            }
            compositor(|c| c.hide())?;
            run_for(0.2);
        }
        let hidden = compositor(|c| c.stats().submitted_frames);
        run_for(0.2);
        assert_eq!(
            compositor(|c| c.stats().submitted_frames),
            hidden,
            "hidden compositor rendered"
        );
        // Teardown then re-create while earlier command completion callbacks may still run.
        let old = COMPOSITOR.with(|c| c.take());
        drop(old);
        for _ in 0..3 {
            let mut c = PreviewCompositor::new(bounds, 2., || {
                dispatch2::DispatchQueue::main().exec_async(render)
            })?;
            c.set_scene(if scene.len() > 1 { &scene[1..2] } else { &[] })?;
            c.show()?;
            COMPOSITOR.with(|slot| slot.replace(Some(c)));
            run_for(0.25);
            drop(COMPOSITOR.with(|c| c.take()));
            run_for(0.1);
        }
        println!("native lifecycle/transition assertions passed");
    } else {
        drop(COMPOSITOR.with(|c| c.take()));
    }
    drop(sources);
    run_for(0.1);
    Ok(())
}

// A caller-owned nested CA hierarchy and two independent canvases share one
// source inventory. Screenshots are taken without yielding the owner between
// input, sibling movement and cached-pixel movement.
fn canvas_probe(
    bounds: CGRect,
    scene: &[Preview],
    out: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(out)?;
    let window = cgs::CgsWindow::new_compositor(bounds, 0.)?;
    window.set_resolution(2.)?;
    window.set_level(25)?;
    let root = CALayer::layer();
    root.setFrame(frame(0., 0., bounds.size.width, bounds.size.height));
    root.setGeometryFlipped(true);
    let surface = cgs::WindowSurface::new_scaled(window.id(), root.bounds(), &root, 2.)?;
    let row = CALayer::layer();
    row.setFrame(frame(0., 60., bounds.size.width, 400.));
    row.setMasksToBounds(true);
    root.addSublayer(&row);
    let mut owner = PreviewCompositor::new_attached(window.id(), &root, bounds, 2., || {
        dispatch2::DispatchQueue::main().exec_async(render);
    })?;
    owner.set_sources(&scene.iter().map(cgs::PreviewSource::from).collect::<Vec<_>>())?;
    owner.set_visual_scene(&[])?;
    owner.show()?;
    let mut canvas =
        owner.new_canvas(window.id(), &row, frame(0., 0., bounds.size.width, 400.), 2.)?;
    let mut local = scene[0].clone();
    local.frame = frame(100., 100., 240., 160.);
    canvas.set_visual_scene(&[local.clone()])?;
    canvas.show()?;
    // A second canvas proves shared capture has no extra streams or factories.
    let mut sibling =
        owner.new_canvas(window.id(), &row, frame(0., 0., bounds.size.width, 400.), 2.)?;
    let mut second = local.clone();
    second.frame.origin.x += 400.;
    sibling.set_visual_scene(&[second])?;
    sibling.show()?;
    COMPOSITOR.with(|slot| slot.replace(Some(owner)));
    CANVASES.with(|slot| slot.replace(vec![canvas, sibling]));
    window.order_above(None)?;
    surface.flush();
    run_for(2.);
    assert!(
        CANVASES.with(|s| s.borrow().iter().all(|c| c.is_ready())),
        "shared canvases never presented"
    );
    let before = compositor(|c| c.stats());
    assert!(
        before.streams <= 8 && before.process_capture_jobs <= 8,
        "canvases duplicated capture"
    );
    let screenshot = |name: &str| {
        assert!(
            std::process::Command::new("screencapture")
                .args(["-x", "-l", &window.id().to_string()])
                .arg(out.join(name))
                .status()
                .unwrap()
                .success()
        );
    };
    screenshot("before.png");
    let submitted =
        CANVASES.with(|s| s.borrow().iter().map(|c| c.stats().submitted_frames).sum::<u64>());
    {
        let _tx = cgs::LayerTransaction::disabled().flush_on_commit();
        CANVASES.with(|s| s.borrow_mut()[0].translate(CGPoint::new(80., 0.)))?;
    }
    screenshot("horizontal.png");
    assert_eq!(
        submitted,
        CANVASES.with(|s| s.borrow().iter().map(|c| c.stats().submitted_frames).sum::<u64>())
    );
    {
        let _tx = cgs::LayerTransaction::disabled().flush_on_commit();
        row.setFrame(frame(0., 140., bounds.size.width, 400.));
    }
    screenshot("vertical.png");
    assert_eq!(
        submitted,
        CANVASES.with(|s| s.borrow().iter().map(|c| c.stats().submitted_frames).sum::<u64>())
    );
    run_for(0.4);
    screenshot("rebased.png");
    // Immediate direction reversal while an earlier replacement may be queued.
    {
        let _tx = cgs::LayerTransaction::disabled().flush_on_commit();
        CANVASES.with(|s| s.borrow_mut()[0].translate(CGPoint::new(-80., 0.)))?;
        row.setFrame(frame(0., 60., bounds.size.width, 400.));
    }
    screenshot("reversed.png");
    run_for(0.3);
    screenshot("reversed-rebased.png");
    CANVASES.with(|s| s.borrow_mut().clear());
    compositor(|c| c.hide())?;
    run_for(0.3);
    let after = compositor(|c| c.stats());
    assert_eq!(after.process_capture_jobs, 0);
    assert_eq!(compositor(|c| c.diagnostics().process_reserved_capture_bytes), 0);
    drop(COMPOSITOR.with(|s| s.take()));
    std::fs::write(
        out.join("presented.txt"),
        format!("shared_canvas_before={before:?} after={after:?}\n"),
    )?;
    println!("shared_canvas_before={before:?} after={after:?}");
    Ok(())
}
