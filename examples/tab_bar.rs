//! Native preview: `cargo run --example tab_bar -- --embedded --light`.
//! Compare --dark, --many, --clear and --bright / --dim / --colorful backgrounds;
//! --embedded --wallpaper=PATH previews the strip over busy imagery.
//! --vertical lays tabs top to bottom (--length=N sets its height, --right reads downward); --thickness=N changes
//! the strip's thickness. --hover selects tabs when the pointer enters them.
//! --panel-only captures the actual non-key panel; --embedded is an ordinary-window
//! reference (click its window to make the reference active).
//! --anchor=WINDOW_NUMBER orders above another application's window without activating it.
//! --frame=X,Y,WIDTH positions that overlay in AppKit screen coordinates.
//! --active-reference --embedded activates only the separate reference window.
//! --diagnostics reports actual focus state on display and every action.
//! Drag a tab along the strip to reorder it. Preview controls add a tab, rapidly retarget selection, and resize the strip.
use std::cell::RefCell;
use std::rc::Rc;

use cgs::*;
use objc2::{AnyThread, Message};
use objc2_app_kit::{NSAppearanceCustomization, NSApplicationActivationPolicy};
fn main() {
    let ui = Ui::new(MainThreadMarker::new().unwrap());
    let app = Application::shared(&ui);
    app.ns_application()
        .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let app_native = app.ns_application().retain();
    let window = Window::new(&ui)
        .on_close(move || app_native.stop(None))
        .title("cgs native tabs")
        .size(CGSize::new(680.0, 340.0))
        .content(Label::new(
            &ui,
            "Resize the strip in code to exercise overflow. Close and select tabs above.",
        ));
    window.ns_window().setAcceptsMouseMovedEvents(true);
    let background = if std::env::args().any(|arg| arg == "--bright") {
        Some(objc2_app_kit::NSColor::whiteColor())
    } else if std::env::args().any(|arg| arg == "--dim") {
        Some(objc2_app_kit::NSColor::blackColor())
    } else if std::env::args().any(|arg| arg == "--colorful") {
        Some(objc2_app_kit::NSColor::systemTealColor())
    } else {
        None
    };
    window.ns_window().setBackgroundColor(background.as_deref());
    window.center();
    let panel_only = std::env::args().any(|arg| arg == "--panel-only");
    if !panel_only {
        window.ns_window().orderFront(None);
    }
    let appearance = if std::env::args().any(|arg| arg == "--light") {
        Some(unsafe { objc2_app_kit::NSAppearanceNameAqua })
    } else if std::env::args().any(|arg| arg == "--dark") {
        Some(unsafe { objc2_app_kit::NSAppearanceNameDarkAqua })
    } else {
        None
    };
    if let Some(name) = appearance {
        window
            .ns_window()
            .setAppearance(objc2_app_kit::NSAppearance::appearanceNamed(name).as_deref());
    }
    let reference = std::env::args().any(|arg| arg == "--reference");
    let titles = Rc::new(RefCell::new(if std::env::args().any(|arg| arg == "--same") {
        vec![
            ".../Documents/schoolgithub/rift",
            ".../Documents/schoolgithub/rift",
        ]
    } else if reference {
        vec!["Downloads", "Start Page"]
    } else if std::env::args().any(|arg| arg == "--many") {
        vec![
            "Rift",
            "/Projects/Rift/a-very-long-window-title/src/tab_bar.rs",
            "窗口 — 日本語",
            "Fourth window",
            "Fifth window",
            "Sixth window",
            "Seventh window",
        ]
    } else {
        vec![
            "Downloads",
            "Start Page",
            "Long document title with native ellipsis — 窗口",
        ]
    }));
    let selected = Rc::new(std::cell::Cell::new(0));
    let slot = Rc::new(RefCell::new(None::<TabBar>));
    let weak = Rc::downgrade(&slot);
    let names = titles.clone();
    let selection = selected.clone();
    let diagnostics = std::env::args().any(|arg| arg == "--diagnostics");
    let bar = TabBar::new(&ui).on_action(move |action| {
        match action {
            TabBarAction::Select(i) => selection.set(i),
            TabBarAction::Close(i) => {
                if i < names.borrow().len() {
                    names.borrow_mut().remove(i);
                    let previous = selection.get();
                    selection.set(if previous > i {
                        previous - 1
                    } else if previous == i {
                        i.min(names.borrow().len().saturating_sub(1))
                    } else {
                        previous
                    });
                }
            }
            TabBarAction::Move { from, to } => {
                let mut names = names.borrow_mut();
                let title = names.remove(from);
                names.insert(to, title);
                let previous = selection.get();
                selection.set(if previous == from {
                    to
                } else if from < previous && previous <= to {
                    previous - 1
                } else if to <= previous && previous < from {
                    previous + 1
                } else {
                    previous
                });
            }
        }
        if diagnostics {
            println!("{action:?}");
        }
        if let Some(slot) = weak.upgrade() {
            if let Some(bar) = slot.borrow().as_ref() {
                bar.set_tabs(preview_items(&names.borrow()), selection.get());
                if diagnostics {
                    report_focus(bar, &ui);
                }
            }
        }
    });
    bar.ns_panel()
        .setTitle(&objc2_foundation::NSString::from_str("cgs floating glass"));
    bar.set_tabs(preview_items(&titles.borrow()), 0);
    if diagnostics {
        println!("Material: {:?}", bar.ns_panel().contentView());
    }
    if let Some(name) = appearance {
        bar.ns_panel()
            .setAppearance(objc2_app_kit::NSAppearance::appearanceNamed(name).as_deref());
    }
    if std::env::args().any(|arg| arg == "--clear") {
        if let Some(surface) = bar.ns_panel().contentView() {
            let supported: bool =
                unsafe { objc2::msg_send![&surface, respondsToSelector: objc2::sel!(setStyle:)] };
            if supported {
                let _: () = unsafe {
                    objc2::msg_send![&surface, setStyle: objc2_app_kit::NSGlassEffectViewStyle::Clear]
                };
            }
        }
    }
    let thickness = std::env::args()
        .find_map(|arg| arg.strip_prefix("--thickness=").and_then(|v| v.parse::<f64>().ok()))
        .unwrap_or(TabBar::HEIGHT);
    let vertical = std::env::args().any(|arg| arg == "--vertical");
    let length = std::env::args()
        .find_map(|arg| arg.strip_prefix("--length=").and_then(|v| v.parse::<f64>().ok()))
        .unwrap_or(300.0);
    bar.set_edge(if !vertical {
        TabBarEdge::Top
    } else if std::env::args().any(|arg| arg == "--right") {
        TabBarEdge::Right
    } else {
        TabBarEdge::Left
    });
    bar.set_select_on_hover(std::env::args().any(|arg| arg == "--hover"));
    let frame = window.ns_window().frame();
    bar.set_frame(CGRect::new(
        CGPoint::new(frame.origin.x + 10.0, frame.origin.y + 285.0),
        if vertical {
            CGSize::new(thickness, length)
        } else {
            CGSize::new(660.0, thickness)
        },
    ));
    if let Some(values) =
        std::env::args().find_map(|arg| arg.strip_prefix("--frame=").map(str::to_owned))
    {
        let values: Vec<f64> = values.split(',').filter_map(|value| value.parse().ok()).collect();
        if let [x, y, width] = values.as_slice() {
            bar.set_frame(CGRect::new(
                CGPoint::new(*x, *y),
                CGSize::new(*width, TabBar::HEIGHT),
            ));
        } else {
            panic!("--frame requires X,Y,WIDTH in AppKit screen coordinates");
        }
    }
    let controls = {
        let add_slot = slot.clone();
        let add_titles = titles.clone();
        let add_selected = selected.clone();
        let rapid_slot = slot.clone();
        let rapid_titles = titles.clone();
        let rapid_selected = selected.clone();
        let resize_slot = slot.clone();
        HStack::new(&ui)
            .push(Button::new(&ui, "Add window").on_click(move || {
                add_titles.borrow_mut().push("New window");
                if let Some(bar) = add_slot.borrow().as_ref() {
                    bar.set_tabs(preview_items(&add_titles.borrow()), add_selected.get());
                }
            }))
            .push(Button::new(&ui, "Rapid selection").on_click(move || {
                if let Some(bar) = rapid_slot.borrow().as_ref() {
                    let titles = rapid_titles.borrow();
                    for index in (0..titles.len()).rev() {
                        rapid_selected.set(index);
                        bar.set_tabs(preview_items(&titles), index);
                    }
                }
            }))
            .push(Button::new(&ui, "Resize strip").on_click(move || {
                if let Some(bar) = resize_slot.borrow().as_ref() {
                    let mut frame = bar.ns_panel().frame();
                    frame.size.width = if frame.size.width > 400.0 {
                        380.0
                    } else {
                        660.0
                    };
                    bar.set_frame(frame);
                }
            }))
    };
    if std::env::args().any(|arg| arg == "--embedded") {
        // Useful for window-only screenshot tools: retain exactly the same glass content.
        let strip = bar.ns_panel().contentView().unwrap();
        bar.ns_panel().setContentView(None);
        let label = Label::new(
            &ui,
            "Native glass, Unicode truncation, hover, close and overflow preview",
        );
        if vertical {
            strip.width(thickness);
            strip.height(length);
            window.set_content(
                HStack::new(&ui).push(strip).push(VStack::new(&ui).push(label).push(controls)),
            );
        } else {
            strip.height(thickness);
            window.set_content(VStack::new(&ui).push(strip).push(label).push(controls));
        }
        // Busy imagery is the realistic backdrop for an overlay strip.
        if let Some(image) = std::env::args()
            .find_map(|arg| arg.strip_prefix("--wallpaper=").map(str::to_owned))
            .and_then(|path| {
                objc2_app_kit::NSImage::initWithContentsOfFile(
                    objc2_app_kit::NSImage::alloc(),
                    &objc2_foundation::NSString::from_str(&path),
                )
            })
        {
            let root = window.ns_window().contentView().unwrap();
            let container = objc2_app_kit::NSView::new(ui.mtm());
            window.ns_window().setContentView(Some(&container));
            let backdrop = objc2_app_kit::NSImageView::imageViewWithImage(&image, ui.mtm());
            backdrop.setImageScaling(objc2_app_kit::NSImageScaling::ScaleAxesIndependently);
            for view in [&*backdrop as &objc2_app_kit::NSView, &root] {
                view.setTranslatesAutoresizingMaskIntoConstraints(false);
                view.setContentCompressionResistancePriority_forOrientation(
                    1.0,
                    objc2_app_kit::NSLayoutConstraintOrientation::Horizontal,
                );
                view.setContentCompressionResistancePriority_forOrientation(
                    1.0,
                    objc2_app_kit::NSLayoutConstraintOrientation::Vertical,
                );
                container.addSubview(view);
                for (a, b) in [
                    (view.leadingAnchor(), container.leadingAnchor()),
                    (view.trailingAnchor(), container.trailingAnchor()),
                ] {
                    a.constraintEqualToAnchor(&b).setActive(true);
                }
                for (a, b) in [
                    (view.topAnchor(), container.topAnchor()),
                    (view.bottomAnchor(), container.bottomAnchor()),
                ] {
                    a.constraintEqualToAnchor(&b).setActive(true);
                }
            }
            window.ns_window().setContentSize(CGSize::new(680.0, 340.0));
        }
    } else {
        window.set_content(
            VStack::new(&ui)
                .push(Label::new(&ui, "Select or close tabs in the floating strip."))
                .push(controls),
        );
        let anchor = std::env::args()
            .find_map(|arg| {
                arg.strip_prefix("--anchor=").and_then(|value| value.parse::<isize>().ok())
            })
            .unwrap_or(window.ns_window().windowNumber());
        bar.show_above(anchor);
    }
    println!(
        "Tab panel window: {}; anchor window: {}",
        bar.ns_panel().windowNumber(),
        window.ns_window().windowNumber()
    );
    if std::env::args().any(|arg| arg == "--active-reference") {
        assert!(
            std::env::args().any(|arg| arg == "--embedded"),
            "--active-reference requires --embedded"
        );
        // This ordinary comparison window belongs to the disposable preview app.
        // The TabPanel is never made key or used to activate an application.
        app.ns_application().finishLaunching();
        #[allow(deprecated)]
        app.ns_application().activateIgnoringOtherApps(true);
        window.ns_window().makeKeyWindow();
    }
    if diagnostics {
        report_focus(&bar, &ui);
    }
    *slot.borrow_mut() = Some(bar);
    app.ns_application().run();
}

fn report_focus(bar: &TabBar, ui: &Ui) {
    fn animations(layer: &objc2_quartz_core::CALayer) -> usize {
        layer.animationKeys().map_or(0, |keys| keys.len())
            + unsafe { layer.sublayers() }
                .map_or(0, |layers| layers.iter().map(|layer| animations(&layer)).sum())
    }
    fn view_animations(view: &objc2_app_kit::NSView) -> usize {
        if let Some(layer) = view.layer() {
            animations(&layer)
        } else {
            view.subviews().iter().map(|view| view_animations(&view)).sum()
        }
    }
    let motion =
        objc2_app_kit::NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion();
    let active = bar.ns_panel().contentView().map(|view| view_animations(&view));
    println!("reduce motion={motion}; native animations={active:?}");
    let app = Application::shared(ui);
    let frontmost = objc2_app_kit::NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .and_then(|app| app.bundleIdentifier())
        .map(|id| id.to_string());
    println!(
        "panel key={}, main={}; preview active={}; reference key={:?}; frontmost={:?}",
        bar.ns_panel().isKeyWindow(),
        bar.ns_panel().isMainWindow(),
        app.ns_application().isActive(),
        app.ns_application().keyWindow().map(|window| window.windowNumber()),
        frontmost
    );
}

fn preview_items(titles: &[&str]) -> Vec<TabBarItem> {
    titles
        .iter()
        .map(|title| TabBarItem {
            title: (*title).into(),
            icon: Application::icon_for_bundle(if *title == "Start Page" {
                "com.apple.Safari"
            } else if title.starts_with("Long") || title.starts_with('/') {
                "com.apple.TextEdit"
            } else {
                "com.apple.finder"
            }),
        })
        .collect()
}
