//! A compact native AppKit tab strip used to choose a settings value.
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::*;
use objc2_foundation::NSString;

use crate::bridge::ActionTarget;
use crate::*;

/// Native glass tabs with labels, optional SF Symbols, drag tracking, keyboard
/// navigation and accessibility supplied by NSTabView.
pub struct TabSelector {
    native: Retained<NSSegmentedControl>,
    _tabs: Retained<NSTabView>,
    target: Retained<ActionTarget>,
}
impl TabSelector {
    pub fn new(ui: &Ui, labels: &[&str]) -> Self {
        Self::choices(
            ui,
            &labels.iter().map(|label| (*label, None)).collect::<Vec<_>>(),
        )
    }

    pub fn choices(ui: &Ui, choices: &[(&str, Option<&str>)]) -> Self {
        let tabs = NSTabView::new(ui.mtm());
        tabs.setControlSize(NSControlSize::Large);
        tabs.setDrawsBackground(false);
        tabs.setTabViewBorderType(NSTabViewBorderType::None);
        tabs.setFont(&Font::body());
        tabs.setAllowsTruncatedLabels(false);
        for (label, _) in choices {
            let item = unsafe { NSTabViewItem::initWithIdentifier(NSTabViewItem::alloc(), None) };
            item.setLabel(&NSString::from_str(label));
            tabs.addTabViewItem(&item);
        }
        tabs.setFrameSize(CGSize::new(tabs.minimumSize().width.max(1.0), 80.0));
        tabs.layoutSubtreeIfNeeded();
        // AppKit supplies a segmented tab strip as a public NSSegmentedControl
        // child. Retain its tab-view owner (target and selection model), and host
        // only the strip so there is no unused page frame or clipping.
        let native = tabs
            .subviews()
            .iter()
            .find_map(|view| view.downcast::<NSSegmentedControl>().ok())
            .unwrap_or_else(|| {
                // Older AppKit versions may draw tabs themselves. Keep the same
                // API functional with the standard native rounded control.
                let control = NSSegmentedControl::new(ui.mtm());
                control.setControlSize(NSControlSize::Large);
                control.setSegmentStyle(NSSegmentStyle::Rounded);
                control.setTrackingMode(NSSegmentSwitchTracking::SelectOne);
                control.setSegmentCount(choices.len() as isize);
                for (index, (label, _)) in choices.iter().enumerate() {
                    control.setLabel_forSegment(&NSString::from_str(label), index as isize);
                }
                if !choices.is_empty() {
                    control.setSelectedSegment(0);
                }
                control
            });
        native.removeFromSuperview();
        native.setTranslatesAutoresizingMaskIntoConstraints(false);
        for (index, (_, symbol)) in choices.iter().enumerate() {
            if let Some(symbol) = symbol {
                native.setImage_forSegment(Symbol::named(symbol).as_deref(), index as isize);
            }
        }
        let target = ActionTarget::new(ui);
        target.attach(&native);
        Self { native, _tabs: tabs, target }
    }

    pub fn ns_segmented_control(&self) -> &NSSegmentedControl { &self.native }

    pub fn set_selected(&self, index: usize) {
        if index < self.native.segmentCount() as usize && self.selected() != Some(index) {
            self.native.setSelectedSegment(index as isize);
        }
    }

    pub fn selected(&self) -> Option<usize> { usize::try_from(self.native.selectedSegment()).ok() }

    pub fn on_change(self, mut change: impl FnMut(usize) + 'static) -> Self {
        self.target.set(move |sender| {
            if let Some(control) = sender.downcast_ref::<NSSegmentedControl>() {
                if let Ok(index) = usize::try_from(control.selectedSegment()) {
                    change(index);
                }
            }
        });
        self
    }
}
impl NativeView for TabSelector {
    fn ns_view(&self) -> &NSView { &self.native }
}

impl NativeControl for TabSelector {
    fn ns_control(&self) -> &NSControl { &self.native }
}
