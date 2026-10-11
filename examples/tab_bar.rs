//! Standalone native preview: `cargo run --example tab_bar` from this repository.
use std::cell::RefCell;
use std::rc::Rc;

use cgs::*;
use objc2::Message;
use objc2_app_kit::NSApplicationActivationPolicy;
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
    window.center();
    window.show();
    let reference = std::env::args().any(|arg| arg == "--reference");
    let titles = Rc::new(RefCell::new(if reference {
        vec!["Downloads", "Start Page"]
    } else {
        vec![
            "Rift",
            "Long document title with native ellipsis",
            "窗口 — 日本語",
            "Fourth window",
            "Fifth window",
        ]
    }));
    let selected = Rc::new(std::cell::Cell::new(0));
    let slot = Rc::new(RefCell::new(None::<TabBar>));
    let weak = Rc::downgrade(&slot);
    let names = titles.clone();
    let selection = selected.clone();
    let bar = TabBar::new(&ui).on_action(move |action| {
        match action {
            TabBarAction::Select(i) => selection.set(i),
            TabBarAction::Close(i) => {
                if i < names.borrow().len() {
                    names.borrow_mut().remove(i);
                    selection.set(i.saturating_sub(1));
                }
            }
        }
        if let Some(slot) = weak.upgrade() {
            if let Some(bar) = slot.borrow().as_ref() {
                bar.set_tabs(
                    names
                        .borrow()
                        .iter()
                        .map(|s| TabBarItem {
                            title: (*s).into(),
                            icon: Application::icon_for_bundle(if *s == "Start Page" {
                                "com.apple.Safari"
                            } else {
                                "com.apple.finder"
                            }),
                        })
                        .collect(),
                    selection.get(),
                );
            }
        }
    });
    bar.set_tabs(
        titles
            .borrow()
            .iter()
            .map(|s| TabBarItem {
                title: (*s).into(),
                icon: Application::icon_for_bundle(if *s == "Start Page" {
                    "com.apple.Safari"
                } else {
                    "com.apple.finder"
                }),
            })
            .collect(),
        0,
    );
    let frame = window.ns_window().frame();
    bar.set_frame(CGRect::new(
        CGPoint::new(frame.origin.x + 10.0, frame.origin.y + 285.0),
        CGSize::new(660.0, 28.0),
    ));
    if std::env::args().any(|arg| arg == "--embedded") {
        // Useful for window-only screenshot tools: retain exactly the same glass content.
        let strip = bar.ns_panel().contentView().unwrap();
        bar.ns_panel().setContentView(None);
        strip.height(28.0);
        window.set_content(VStack::new(&ui).push(strip).push(Label::new(
            &ui,
            "Native glass, Unicode truncation, hover, close and overflow preview",
        )));
    } else {
        bar.show_above(window.ns_window().windowNumber());
    }
    println!("Tab panel window: {}", bar.ns_panel().windowNumber());
    *slot.borrow_mut() = Some(bar);
    app.ns_application().run();
}
