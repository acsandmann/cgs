//! A real window switcher. Run `cargo run --release --example interactive_switcher`.
//! Option-Space opens; arrows/Tab select, L changes layout, Return commits, Escape cancels.
//! Four-finger horizontal swipe opens/scrubs; lifting commits. See readme.md for permissions.
#[path = "interactive_switcher/accessibility.rs"]
mod accessibility;
#[path = "interactive_switcher/hud.rs"]
mod hud;
#[path = "interactive_switcher/keyboard.rs"]
mod keyboard;

use cgs::CGPoint;
use cgs::compositor::switcher::{
    SwitcherCache, SwitcherCommand, SwitcherItem, SwitcherLayout, SwitcherOutcome, WindowSwitcher,
};
use multitouch::{
    GestureEndReason, GestureEvent, GesturePhase, GestureRecognizer, GestureTypes, Monitor,
};
use objc2::{
    MainThreadMarker, MainThreadOnly, Message, define_class, msg_send,
    rc::{Retained, autoreleasepool},
    runtime::AnyObject,
    sel,
};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSScreen};
use objc2_core_graphics::{
    CGDisplayBounds, CGMainDisplayID, CGWindowListCopyWindowInfo, CGWindowListOption,
    kCGNullWindowID,
};
use objc2_foundation::{
    NSArray, NSDate, NSDictionary, NSNumber, NSObject, NSObjectProtocol, NSRunLoop,
    NSRunLoopCommonModes, NSString, ns_string,
};
use objc2_quartz_core::{CACurrentMediaTime, CADisplayLink, CAFrameRateRange};
use objc2_screen_capture_kit::SCShareableContent;
use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
struct Target {
    id: u32,
    pid: i32,
}

enum Input {
    Open,
    Command(SwitcherCommand),
    Layout(SwitcherLayout),
    Gesture(u64, GestureEvent),
}
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }
define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    struct DisplayTarget;

    impl DisplayTarget {
        #[unsafe(method(switcherDisplayFrame:))]
        fn display_frame(&self, link: &CADisplayLink) {
            let now = Instant::now();
            let lead = (link.targetTimestamp() - CACurrentMediaTime()).clamp(0., 0.05);
            let presentation = now + Duration::from_secs_f64(lead);
            STATE.with(|state| {
                if let Ok(mut state) = state.try_borrow_mut() {
                    state.last_display_frame = Some(now);
                    state.drain_input();
                    state.advance(presentation);
                }
            });
        }
    }
);

fn display_link(screen: &NSScreen, mtm: MainThreadMarker) -> Option<Retained<CADisplayLink>> {
    if !screen.respondsToSelector(sel!(displayLinkWithTarget:selector:)) {
        return None;
    }
    let target: Retained<DisplayTarget> = unsafe { msg_send![DisplayTarget::alloc(mtm), init] };
    let link = unsafe { screen.displayLinkWithTarget_selector(&target, sel!(switcherDisplayFrame:)) };
    let max = screen.maximumFramesPerSecond().max(30) as f32;
    link.setPreferredFrameRateRange(CAFrameRateRange::new(30., max, max));
    unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };
    Some(link)
}

type Discovery = Arc<Mutex<Option<Result<Vec<SwitcherItem<Target>>, String>>>>;
#[derive(Default)]
struct State {
    switcher: Option<WindowSwitcher<Target>>,
    display_link: Option<Retained<CADisplayLink>>,
    last_display_frame: Option<Instant>,
    capture_pending: Arc<AtomicBool>,
    input_receiver: Option<Rc<mpsc::Receiver<Input>>>,
    activation_queue: Option<dispatch2::DispatchRetained<dispatch2::DispatchQueue>>,
    activation_done: Option<Arc<AtomicBool>>,
    cache: Option<SwitcherCache>,
    pending: Option<(Instant, Discovery)>,
    queued: Vec<Input>,
    gesture: Option<u64>,
    layout: SwitcherLayout,
    hud: Option<hud::ModeHud>,
}

fn z_order() -> Vec<u32> {
    let Some(list) =
        CGWindowListCopyWindowInfo(CGWindowListOption::OptionOnScreenOnly, kCGNullWindowID)
    else {
        return vec![];
    };
    let list =
        unsafe { &*(&*list as *const _ as *const NSArray<NSDictionary<NSString, AnyObject>>) };
    list.iter()
        .filter_map(|d| {
            d.objectForKey(ns_string!("kCGWindowNumber"))?
                .downcast::<NSNumber>()
                .ok()
                .map(|v| v.unsignedIntValue())
        })
        .collect()
}

impl State {
    fn open(&mut self) {
        if self.switcher.is_some() || self.pending.is_some() {
            return;
        }
        let result = Arc::new(Mutex::new(None));
        self.pending = Some((Instant::now(), result.clone()));
        let callback = block2::RcBlock::new(
            move |content: *mut SCShareableContent, error: *mut objc2_foundation::NSError| {
                let discovered = if let Some(content) = unsafe { content.as_ref() } {
                    let order: HashMap<_, _> = z_order()
                        .into_iter()
                        .enumerate()
                        .map(|(rank, id)| (id, rank))
                        .collect();
                    let mut items: Vec<_> = unsafe { content.windows() }
                        .iter()
                        .filter_map(|w| unsafe {
                            let app = w.owningApplication()?;
                            let frame = w.frame();
                            if w.windowLayer() != 0
                                || !w.isOnScreen()
                                || app.processID() == std::process::id() as i32
                                || frame.size.width < 80.
                                || frame.size.height < 60.
                            {
                                return None;
                            }
                            let title = w
                                .title()
                                .map(|t| t.to_string())
                                .filter(|t| !t.is_empty())
                                .unwrap_or_else(|| app.applicationName().to_string());
                            Some(SwitcherItem {
                                window_id: w.windowID(),
                                process_id: app.processID(),
                                frame,
                                on_screen: true,
                                local: true,
                                title,
                                data: Target {
                                    id: w.windowID(),
                                    pid: app.processID(),
                                },
                            })
                        })
                        .collect();
                    items.sort_by_key(|w| order.get(&w.window_id).copied().unwrap_or(usize::MAX));
                    Ok(items)
                } else {
                    Err(unsafe { error.as_ref() }
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "Window discovery failed".into()))
                };
                *result.lock().unwrap() = Some(discovered);
            },
        );
        unsafe {
            SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(true, true, &callback);
        }
    }

    fn drain_input(&mut self) {
        if let Some(receiver) = self.input_receiver.clone() {
            self.inputs(receiver.try_iter());
        }
    }

    fn inputs(&mut self, inputs: impl Iterator<Item = Input>) {
        let mut inputs = inputs.peekable();
        while let Some(input) = inputs.next() {
            if let Input::Command(SwitcherCommand::Step(mut delta)) = input {
                // Apply a burst once so layout and capture priorities follow
                // the final destination. Other input keeps its original order.
                while let Some(Input::Command(SwitcherCommand::Step(next))) = inputs.peek() {
                    if next.signum() != delta.signum() {
                        break;
                    }
                    delta = delta.saturating_add(*next);
                    inputs.next();
                }
                self.input(Input::Command(SwitcherCommand::Step(delta)));
            } else {
                self.input(input);
            }
        }
    }

    fn input(&mut self, input: Input) {
        if matches!(input, Input::Open) {
            self.open();
            return;
        }
        if self.pending.is_some() {
            if matches!(input, Input::Command(SwitcherCommand::Dismiss)) {
                self.pending = None;
                self.queued.clear();
                self.gesture = None;
            } else {
                self.queued.push(input);
            }
            return;
        }
        match input {
            Input::Command(command) => self.command(command),
            Input::Layout(layout) => {
                while self.layout != layout {
                    if self.switcher.is_none() {
                        self.layout = layout;
                        break;
                    }
                    self.command(SwitcherCommand::Layout(true));
                }
            }
            Input::Gesture(device, GestureEvent::Swipe(swipe)) => {
                match swipe.phase {
                    GesturePhase::Began => {
                        if self.gesture.is_some() {
                            return;
                        }
                        // Leave vertical system gestures alone.
                        if swipe.translation.x.abs() < swipe.translation.y.abs() {
                            return;
                        }
                        self.gesture = Some(device);
                        if self.switcher.is_none() {
                            self.open();
                            self.queued
                                .push(Input::Gesture(device, GestureEvent::Swipe(swipe)));
                            self.gesture = None;
                            return;
                        }
                        self.switcher.as_mut().unwrap().begin_scrub();
                    }
                    GesturePhase::Changed if self.gesture == Some(device) => {}
                    GesturePhase::Ended(reason) if self.gesture == Some(device) => {
                        self.gesture = None;
                        self.command(
                            if matches!(
                                reason,
                                GestureEndReason::Lifted | GestureEndReason::FingerCountChanged(_)
                            ) {
                                SwitcherCommand::Commit { release: false }
                            } else {
                                SwitcherCommand::Dismiss
                            },
                        );
                        return;
                    }
                    GesturePhase::Cancelled | GesturePhase::Failed
                        if self.gesture == Some(device) =>
                    {
                        self.gesture = None;
                        self.command(SwitcherCommand::Dismiss);
                        return;
                    }
                    _ => return,
                }
                if let Some(s) = self.switcher.as_mut() {
                    s.scrub(-f64::from(swipe.translation.x));
                }
            }
            Input::Gesture(device, event)
                if self.gesture == Some(device)
                    && matches!(
                        event.phase(),
                        GesturePhase::Cancelled | GesturePhase::Ended(_)
                    ) =>
            {
                self.gesture = None;
                self.command(SwitcherCommand::Dismiss);
            }
            _ => {}
        }
    }

    fn command(&mut self, command: SwitcherCommand) {
        let Some(s) = self.switcher.as_mut() else {
            return;
        };
        let outcome = s.command(command);
        if let SwitcherCommand::Layout(forward) = command {
            self.layout = self.layout.next(forward);
            if let Some(hud) = &self.hud {
                hud.select(self.layout);
            }
        }
        match outcome {
            Some(SwitcherOutcome::Activate) => {
                let target = *s.selected();
                let done = Arc::new(AtomicBool::new(false));
                self.activation_done = Some(done.clone());
                // Exact-window AX requests can wait on another application.
                // Keep those requests serialized while the main thread animates.
                let queue = self.activation_queue.get_or_insert_with(|| {
                    let target = dispatch2::DispatchQueue::global_queue(
                        dispatch2::GlobalQueueIdentifier::QualityOfService(
                            dispatch2::DispatchQoS::UserInitiated,
                        ),
                    );
                    dispatch2::DispatchQueue::new_with_target(
                        "cgs.switcher.activation",
                        None,
                        Some(&target),
                    )
                });
                queue.exec_async(move || {
                    autoreleasepool(|_| {
                        if let Err(error) = accessibility::activate(target.id, target.pid) {
                            eprintln!("Cannot activate window: {error}");
                        }
                        done.store(true, Ordering::Release);
                    });
                });
                // A quick commit while capture is still waiting has no exit animation.
                if s.activation_stamp().is_none() {
                    self.close();
                }
            }
            Some(SwitcherOutcome::Close) => self.close(),
            None => {}
        }
    }

    fn close(&mut self) {
        if let Some(link) = self.display_link.take() {
            link.invalidate();
        }
        self.last_display_frame = None;
        self.activation_done = None;
        if let Some(hud) = &self.hud {
            hud.hide();
        }
        if let Some(s) = self.switcher.take() {
            s.retain(&mut self.cache);
        }
        self.gesture = None;
    }

    fn tick(&mut self) {
        let discovered = self
            .pending
            .as_ref()
            .and_then(|(_, result)| result.lock().unwrap().take());
        if let Some(result) = discovered {
            self.pending = None;
            match result {
                Ok(mut items) if !items.is_empty() => {
                    // Use the display containing the pointer; CG frames have top-left origins.
                    let mtm = MainThreadMarker::new().unwrap();
                    let pointer = objc2_app_kit::NSEvent::mouseLocation();
                    let screens = NSScreen::screens(mtm);
                    let main_height = CGDisplayBounds(CGMainDisplayID()).size.height;
                    let point = CGPoint::new(pointer.x, main_height - pointer.y);
                    let mut display = CGMainDisplayID();
                    let mut scale = 1.;
                    let mut display_screen = NSScreen::mainScreen(mtm);
                    for screen in &screens {
                        if let Some(n) = screen.deviceDescription().objectForKey(
                            &objc2_app_kit::NSDeviceDescriptionKey::from_str("NSScreenNumber"),
                        ) {
                            if let Some(n) = n.downcast_ref::<NSNumber>() {
                                let bounds = CGDisplayBounds(n.unsignedIntValue());
                                if point.x >= bounds.origin.x
                                    && point.x < bounds.max().x
                                    && point.y >= bounds.origin.y
                                    && point.y < bounds.max().y
                                {
                                    display = n.unsignedIntValue();
                                    scale = screen.backingScaleFactor();
                                    display_screen = Some(screen.retain());
                                    break;
                                }
                            }
                        }
                    }
                    let bounds = CGDisplayBounds(display);
                    for item in &mut items {
                        let center = item.frame.mid();
                        item.local = center.x >= bounds.origin.x
                            && center.x < bounds.max().x
                            && center.y >= bounds.origin.y
                            && center.y < bounds.max().y;
                        item.on_screen = item.local;
                        item.frame.origin.x -= bounds.origin.x;
                        item.frame.origin.y -= bounds.origin.y;
                    }
                    if let Some(mut s) =
                        WindowSwitcher::new(display, bounds, scale, self.layout, items)
                    {
                        let capture_pending = self.capture_pending.clone();
                        s.start(
                            true,
                            move || capture_pending.store(true, Ordering::Release),
                            &mut self.cache,
                        );
                        self.switcher = Some(s);
                        self.display_link =
                            display_screen.as_deref().and_then(|screen| display_link(screen, mtm));
                        self.last_display_frame = self.display_link.as_ref().map(|_| Instant::now());
                        if let Some(hud) = &self.hud {
                            hud.show(bounds, self.layout);
                        }
                    }
                    let queued = std::mem::take(&mut self.queued);
                    self.inputs(queued.into_iter());
                }
                Ok(_) => {
                    eprintln!("No switchable windows on the current desktop");
                    self.queued.clear();
                }
                Err(error) => {
                    eprintln!("{error}. Enable Screen Recording in System Settings, then restart.");
                    self.queued.clear();
                }
            }
        } else if self
            .pending
            .as_ref()
            .is_some_and(|(start, _)| start.elapsed() > Duration::from_secs(10))
        {
            eprintln!("Window discovery timed out; check Screen Recording permission.");
            self.pending = None;
            self.queued.clear();
            self.gesture = None;
        }
        // A disconnected or sleeping display can stop its callback mid-session.
        if self.last_display_frame.is_some_and(|last| last.elapsed() > Duration::from_millis(250)) {
            if let Some(link) = self.display_link.take() {
                link.invalidate();
            }
            self.last_display_frame = None;
        }
        if self.display_link.is_none() {
            self.advance(Instant::now());
        }
    }

    fn advance(&mut self, presentation: Instant) {
        if let Some(s) = self.switcher.as_mut() {
            if self.capture_pending.swap(false, Ordering::AcqRel) {
                s.pump_at(presentation);
            } else if s.animating() {
                s.tick_at(presentation);
            }
            let activated = self
                .activation_done
                .as_ref()
                .is_none_or(|done| done.load(Ordering::Acquire));
            if s.handoff_ready(|_, _| activated) {
                self.close();
            }
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!(
            "Interactive window switcher\nOption-Space: open\nArrows / Tab / Shift-Tab: select\nL / Shift-L or top-left HUD: change Carousel / Stack / Grid layout\nReturn / Space: activate selected window\nEscape: cancel\nFour-finger horizontal swipe: open and scrub; lift: activate\nCtrl-C in terminal: quit\nRequires Accessibility and Screen Recording. Disable conflicting four-finger macOS gestures."
        );
        return Ok(());
    }
    let mtm = MainThreadMarker::new().ok_or("Run on the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    app.finishLaunching();
    let _ = cgs::compositor::preview::PreviewCompositor::prepare();
    if !accessibility::trusted(true) {
        return Err("Grant Accessibility permission in System Settings > Privacy & Security, then run again".into());
    }
    let (tx, rx) = mpsc::channel();
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.input_receiver = Some(Rc::new(rx));
        state.hud = Some(hud::ModeHud::new(&cgs::Ui::new(mtm), tx.clone()));
    });
    let active = Arc::new(AtomicBool::new(false));
    let _keyboard = keyboard::Keyboard::new(tx.clone(), active.clone())?;
    let monitor = Monitor::with_gesture_handler(
        |_| {
            GestureRecognizer::new(4)
                .with_gesture_types(GestureTypes::SWIPE)
                .with_exact_finger_count(true)
        },
        move |device, event| {
            if let Some(id) = device.device_id() {
                let _ = tx.send(Input::Gesture(id, event));
            }
        },
    );
    if !monitor.start() {
        eprintln!("No multitouch device available; Option-Space still works.");
    }
    eprintln!("Switcher ready: Option-Space or a four-finger horizontal swipe. Ctrl-C to quit.");
    let mut next_tick = Instant::now();
    loop {
        autoreleasepool(|_| {
            let was_active = active.load(Ordering::Acquire);
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                s.drain_input();
                let now = Instant::now();
                let is_active = s.switcher.is_some() || s.pending.is_some();
                if now >= next_tick || (is_active && !was_active) {
                    s.tick();
                    let interval = if is_active {
                        Duration::from_secs_f64(1. / 120.)
                    } else {
                        Duration::from_millis(50)
                    };
                    next_tick = if was_active && is_active && now >= next_tick {
                        (next_tick + interval).max(now + interval / 2)
                    } else {
                        now + interval
                    };
                }
                active.store(
                    s.switcher.is_some() || s.pending.is_some(),
                    Ordering::Release,
                );
            });
            let delay = next_tick.saturating_duration_since(Instant::now()).as_secs_f64();
            let date = NSDate::dateWithTimeIntervalSinceNow(delay);
            // Run AppKit dispatch as well as CF sources so the HUD receives mouse actions.
            if let Some(event) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
                objc2_app_kit::NSEventMask::Any,
                Some(&date),
                unsafe { objc2_foundation::NSDefaultRunLoopMode },
                true,
            ) {
                app.sendEvent(&event);
            }
            app.updateWindows();
        });
    }
}
