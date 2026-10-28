//! Drive the window switcher over the current on-screen windows and screenshot each step.
//! cargo run --release --example switcher -- OUT_DIR [SCRIPT]
//! SCRIPT is comma separated: `n` next, `p` previous, `l` next layout, `w<ms>` wait,
//! `s` screenshot, `c` commit, `d` dismiss, `f<ms>` screenshot burst every 1/60 s for ms.
use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cgs::{SwitcherCommand, SwitcherItem, SwitcherLayout, WindowSwitcher};
use objc2_core_graphics::{CGDisplayBounds, CGMainDisplayID};
use objc2_foundation::{NSDate, NSRunLoop};
use objc2_screen_capture_kit::SCShareableContent;

thread_local! {
    static SWITCHER: RefCell<Option<WindowSwitcher<()>>> = const { RefCell::new(None) };
}

fn spin(duration: Duration) {
    let end = Instant::now() + duration;
    while Instant::now() < end {
        objc2::rc::autoreleasepool(|_| {
            let date = NSDate::dateWithTimeIntervalSinceNow(0.008);
            NSRunLoop::currentRunLoop()
                .runMode_beforeDate(unsafe { objc2_foundation::NSDefaultRunLoopMode }, &date);
        });
        // Stands in for a display link: one frame per run-loop pass while animating.
        SWITCHER.with(|s| {
            if let Some(s) = s.borrow_mut().as_mut()
                && s.animating()
            {
                s.pump()
            }
        });
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(args.first().ok_or("OUT_DIR required")?);
    std::fs::create_dir_all(&out)?;
    let script = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "w600,s,n,w400,s,l,w500,s,l,w500,s,d,w400".into());
    let bounds = CGDisplayBounds(CGMainDisplayID());

    let inventory = Arc::new(Mutex::new(None));
    let result = inventory.clone();
    let callback = block2::RcBlock::new(
        move |content: *mut SCShareableContent, _: *mut objc2_foundation::NSError| {
            let windows = unsafe { content.as_ref() }.map(|c| unsafe {
                c.windows()
                    .iter()
                    .filter_map(|w| {
                        let app = w.owningApplication()?;
                        let f = w.frame();
                        (w.windowLayer() == 0
                            && w.isOnScreen()
                            && f.size.width >= 120.
                            && f.size.height >= 120.
                            && app.processID() != std::process::id() as i32)
                            .then(|| {
                                let title = w.title().map(|t| t.to_string()).unwrap_or_default();
                                (w.windowID(), app.processID(), f, title)
                            })
                    })
                    .collect::<Vec<_>>()
            });
            *result.lock().unwrap() = Some(windows.unwrap_or_default());
        },
    );
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            true, true, &callback);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while inventory.lock().unwrap().is_none() && Instant::now() < deadline {
        spin(Duration::from_millis(20));
    }
    let windows = inventory.lock().unwrap().take().ok_or("discovery timed out")?;
    let max: usize = std::env::var("MAX").ok().and_then(|s| s.parse().ok()).unwrap_or(usize::MAX);
    let windows: Vec<_> = windows.into_iter().filter(|w| !w.3.is_empty()).take(max).collect();
    for (id, pid, f, title) in &windows {
        eprintln!(
            "{id} {pid} {:?} {title}",
            (f.origin.x, f.origin.y, f.size.width, f.size.height)
        );
    }
    let items: Vec<_> = windows
        .into_iter()
        .filter(|(_, _, f, _)| f.origin.x < bounds.size.width && f.origin.y < bounds.size.height)
        .map(|(id, pid, frame, title)| SwitcherItem {
            window_id: id,
            process_id: pid,
            frame,
            on_screen: frame.origin.x + frame.size.width <= bounds.size.width + 1.,
            local: true,
            title,
            data: (),
        })
        .collect();
    let layout = match std::env::var("LAYOUT").as_deref() {
        Ok("stack") => SwitcherLayout::Stack,
        Ok("grid") => SwitcherLayout::Grid,
        _ => SwitcherLayout::Carousel,
    };
    let mut s =
        WindowSwitcher::new(CGMainDisplayID(), bounds, 2., layout, items).ok_or("no switcher")?;
    let mut cache = None;
    s.start(
        true,
        || {
            dispatch2::DispatchQueue::main().exec_async(|| {
                SWITCHER.with(|s| {
                    if let Some(s) = s.borrow_mut().as_mut() {
                        s.pump()
                    }
                })
            })
        },
        &mut cache,
    );
    SWITCHER.with(|slot| slot.replace(Some(s)));
    let command = |c| SWITCHER.with(|s| s.borrow_mut().as_mut().unwrap().command(c));
    let mut shot = 0;
    let snap = |shot: &mut usize| {
        let path = out.join(format!("s{:03}.png", *shot));
        *shot += 1;
        let _ = std::process::Command::new("screencapture")
            .arg("-x")
            .arg("-C")
            .arg(&path)
            .status();
    };
    for step in script.split(',') {
        let (op, arg) = step.split_at(1);
        let ms = arg.parse::<u64>().unwrap_or(0);
        match op {
            "n" => drop(command(SwitcherCommand::Step(1))),
            "p" => drop(command(SwitcherCommand::Step(-1))),
            "l" => drop(command(SwitcherCommand::Layout(true))),
            "c" => drop(command(SwitcherCommand::Commit { release: false })),
            "d" => drop(command(SwitcherCommand::Dismiss)),
            "w" => spin(Duration::from_millis(ms)),
            "s" => snap(&mut shot),
            "f" => {
                let end = Instant::now() + Duration::from_millis(ms);
                while Instant::now() < end {
                    snap(&mut shot);
                    spin(Duration::from_millis(1));
                }
            }
            _ => {}
        }
    }
    SWITCHER.with(|slot| slot.borrow_mut().take());
    spin(Duration::from_millis(100));
    Ok(())
}
