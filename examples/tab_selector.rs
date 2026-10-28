//! Interactive reference for the settings tab selector.
use cgs::*;
use objc2::Message;
use objc2_app_kit::NSApplicationActivationPolicy;
fn main() {
    let ui = Ui::new(MainThreadMarker::new().unwrap());
    let app = Application::shared(&ui);
    app.ns_application().setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let native_app = app.ns_application().retain();
    let status = std::rc::Rc::new(Label::new(&ui, "Selected: Comfortable / Snappy"));
    let pane_status = status.clone();
    let pane = TabSelector::choices(&ui, &[
        ("Comfortable", Some("square.grid.2x2")),
        ("Compact", Some("rectangle.split.2x2")),
    ])
    .on_change(move |index| {
        pane_status.set_text(&format!("Pane callback: {}", ["Comfortable", "Compact"][index]))
    });
    pane.accessibility_label("Pane style");
    let animation_status = status.clone();
    let animation = TabSelector::new(&ui, &["Snappy", "Natural", "Off"]).on_change(move |index| {
        animation_status.set_text(&format!(
            "Animation callback: {}",
            ["Snappy", "Natural", "Off"][index]
        ))
    });
    animation.accessibility_label("Animation style");
    let content = VStack::new(&ui)
        .spacing(24.0)
        .insets(Insets {
            top: 24.0,
            left: 24.0,
            right: 24.0,
            bottom: 24.0,
        })
        .push(
            HStack::new(&ui)
                .push(Label::new(&ui, "Pane style"))
                .push(Spacer::new(&ui))
                .push(pane),
        )
        .push(
            HStack::new(&ui)
                .push(Label::new(&ui, "Animation style"))
                .push(Spacer::new(&ui))
                .push(animation),
        )
        .push(status)
        .push(Spacer::new(&ui));
    let window = Window::new(&ui)
        .title("cgs Tab Selector")
        .size(CGSize::new(540.0, 180.0))
        .content(content)
        .on_close(move || native_app.stop(None));
    window.center();
    window.show();
    #[allow(deprecated)]
    app.ns_application().activateIgnoringOtherApps(true);
    app.ns_application().run();
}
