use std::cell::RefCell;
use std::time::Duration;

use objc2::rc::{Retained, Weak};
use objc2_app_kit::*;
use objc2_foundation::NSTimer;

use crate::Ui;
use crate::layout::VStack;
use crate::text::{Label, SecondaryLabel};
use crate::view::NativeView;

pub struct ProgressIndicator(Retained<NSProgressIndicator>);
impl ProgressIndicator {
    pub fn new(ui: &Ui) -> Self {
        let native = NSProgressIndicator::new(ui.mtm());
        native.setStyle(NSProgressIndicatorStyle::Spinning);
        native.setIndeterminate(true);
        Self(native)
    }

    pub fn start(&self) {
        unsafe {
            self.0.startAnimation(None);
        }
    }

    pub fn stop(&self) {
        unsafe {
            self.0.stopAnimation(None);
        }
    }

    pub fn fraction(&self, value: f64) {
        self.0.setIndeterminate(false);
        self.0.setMinValue(0.0);
        self.0.setMaxValue(1.0);
        self.0.setDoubleValue(value);
    }

    pub fn ns_progress_indicator(&self) -> &NSProgressIndicator { &self.0 }
}
impl NativeView for ProgressIndicator {
    fn ns_view(&self) -> &NSView { &self.0 }
}
pub struct EmptyState(VStack);
impl EmptyState {
    pub fn new(ui: &Ui, title: &str, description: &str) -> Self {
        Self(
            VStack::new(ui)
                .push(crate::text::SectionTitle::new(ui, title))
                .push(SecondaryLabel::new(ui, description)),
        )
    }

    pub fn action(self, button: crate::controls::Button) -> Self { Self(self.0.push(button)) }
}
impl NativeView for EmptyState {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }
}
pub struct LoadingView(VStack);
impl LoadingView {
    pub fn new(ui: &Ui, title: &str) -> Self {
        let progress = ProgressIndicator::new(ui);
        progress.start();
        Self(VStack::new(ui).push(progress).push(Label::new(ui, title)))
    }
}
impl NativeView for LoadingView {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }
}
pub type StatusLabel = SecondaryLabel;
pub struct TransientStatus {
    label: StatusLabel,
    timer: RefCell<Option<Retained<NSTimer>>>,
    target: Retained<crate::bridge::ActionTarget>,
}
impl TransientStatus {
    pub fn new(ui: &Ui) -> Self {
        let label = StatusLabel::new(ui, "");
        label.set_hidden(true);
        let target = crate::bridge::ActionTarget::new(ui);
        let weak = Weak::new(label.ns_view());
        target.set(move |_| {
            if let Some(view) = weak.load() {
                view.setHidden(true);
            }
        });
        Self {
            label,
            timer: RefCell::new(None),
            target,
        }
    }

    pub fn show(&self, text: &str, duration: Duration) {
        self.clear();
        self.label.set_text(text);
        self.label.set_hidden(false);
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                duration.as_secs_f64(),
                &self.target,
                objc2::sel!(invoke:),
                None,
                false,
            )
        };
        *self.timer.borrow_mut() = Some(timer);
    }

    pub fn clear(&self) {
        if let Some(timer) = self.timer.borrow_mut().take() {
            timer.invalidate();
        }
        self.label.set_hidden(true);
    }

    pub fn ns_text_field(&self) -> &NSTextField { self.label.ns_text_field() }
}
impl NativeView for TransientStatus {
    fn ns_view(&self) -> &NSView { self.label.ns_view() }
}
impl Drop for TransientStatus {
    fn drop(&mut self) { self.clear(); }
}
