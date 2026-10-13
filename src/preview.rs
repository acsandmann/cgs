use std::cell::{Cell, RefCell};
use std::time::Duration;

use objc2::rc::Retained;
use objc2::{MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAnimationContext, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSColor, NSView, NSWorkspace,
};
use objc2_foundation::NSArray;
use objc2_quartz_core::{CALayer, CATransaction};

use crate::{CGPoint, CGRect, CGSize, LayerHost, NativeView, Ui};

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsLayoutPreview"]
    struct PreviewView;
    impl PreviewView {
        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn appearance_changed(&self) {
            unsafe { let _: () = msg_send![super(self), viewDidChangeEffectiveAppearance]; }
            self.update_palette();
        }
    }
);

impl NativeView for PreviewView {
    fn ns_view(&self) -> &NSView { self }
}

impl PreviewView {
    fn update_palette(&self) {
        let Some(layer) = self.layer() else { return };
        let names = NSArray::from_slice(&[unsafe { NSAppearanceNameAqua }, unsafe {
            NSAppearanceNameDarkAqua
        }]);
        let dark = self
            .effectiveAppearance()
            .bestMatchFromAppearancesWithNames(&names)
            .is_some_and(|name| &*name == unsafe { NSAppearanceNameDarkAqua });
        let background = NSColor::colorWithWhite_alpha(if dark { 0.13 } else { 0.96 }, 1.0);
        let border = NSColor::colorWithWhite_alpha(if dark { 0.65 } else { 0.40 }, 1.0);
        let tile = if dark {
            NSColor::colorWithSRGBRed_green_blue_alpha(0.57, 0.72, 0.88, 1.0)
        } else {
            NSColor::colorWithSRGBRed_green_blue_alpha(0.29, 0.53, 0.78, 1.0)
        };
        // CGColors don't follow appearance changes; refresh the native layers
        // explicitly without introducing color animations during a theme switch.
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        layer.setBackgroundColor(Some(&background.CGColor()));
        layer.setBorderColor(Some(&border.CGColor()));
        for view in self.subviews() {
            if let Some(layer) = view.layer() {
                layer.setBackgroundColor(Some(&tile.CGColor()));
            }
        }
        CATransaction::commit();
    }
}

/// A window in a layout illustration. Stable IDs preserve native views across
/// rearrangements. Frames use canvas points and a top-left origin; slice order
/// determines back-to-front stacking, so overlapping layouts work too.
#[derive(Clone, Copy, Debug)]
pub struct PreviewWindow {
    pub id: u64,
    pub frame: CGRect,
}
impl PreviewWindow {
    pub fn new(id: u64, frame: CGRect) -> Self { Self { id, frame } }
}

/// Native animation policy for a preview update. Reduce Motion always wins.
#[derive(Clone, Copy, Debug)]
pub enum PreviewAnimation {
    Immediate,
    Animated(Duration),
}
impl Default for PreviewAnimation {
    fn default() -> Self { Self::Animated(Duration::from_millis(160)) }
}

struct WindowView {
    id: u64,
    view: LayerHost,
    frame: CGRect,
}

/// An appearance-aware layout canvas with blue tiles and a rounded screen outline.
/// Window geometry animates natively.
/// No capture, timers, shadows, or background rendering run while it is idle.
/// Callers supply layout geometry; this component owns all rendering and motion.
pub struct LayoutPreview {
    host: Retained<PreviewView>,
    ui: Ui,
    size: CGSize,
    windows: RefCell<Vec<WindowView>>,
    initialized: Cell<bool>,
}
impl LayoutPreview {
    pub fn new(ui: &Ui, size: CGSize) -> Self {
        assert!(
            size.width.is_finite()
                && size.width > 0.0
                && size.height.is_finite()
                && size.height > 0.0
        );
        let root = CALayer::layer();
        root.setCornerRadius(9.0);
        root.setBorderWidth(2.0);
        root.setMasksToBounds(true);
        let host: Retained<PreviewView> = unsafe { msg_send![PreviewView::alloc(ui.mtm()), init] };
        host.setWantsLayer(true);
        host.setLayer(Some(&root));
        host.update_palette();
        host.width(size.width);
        host.height(size.height);
        Self {
            host,
            ui: *ui,
            size,
            windows: RefCell::new(Vec::new()),
            initialized: Cell::new(false),
        }
    }

    pub fn canvas_size(&self) -> CGSize { self.size }

    /// Apply an entire layout in one animation transaction. Newly added windows
    /// appear at their supplied frame; retained windows move and resize together.
    /// The first layout is immediate. Identical geometry causes no animation.
    pub fn set_windows(&self, layout: &[PreviewWindow], animation: PreviewAnimation) {
        assert!(
            layout.iter().enumerate().all(|(i, w)| layout[..i].iter().all(|p| p.id != w.id)),
            "preview window IDs must be unique"
        );
        let mut windows = self.windows.borrow_mut();
        let same_order = windows.iter().map(|w| w.id).eq(layout.iter().map(|w| w.id));
        if same_order && windows.iter().zip(layout).all(|(a, b)| a.frame == b.frame) {
            return;
        }
        windows.retain(|window| {
            if layout.iter().any(|w| w.id == window.id) {
                true
            } else {
                window.view.ns_view().removeFromSuperview();
                false
            }
        });
        let duration = match animation {
            PreviewAnimation::Animated(duration)
                if self.initialized.get()
                    && !NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion() =>
            {
                duration.as_secs_f64()
            }
            _ => 0.0,
        };
        NSAnimationContext::beginGrouping();
        NSAnimationContext::currentContext().setDuration(duration);
        for window in layout {
            let frame = CGRect::new(
                CGPoint::new(
                    window.frame.origin.x,
                    self.size.height - window.frame.origin.y - window.frame.size.height,
                ),
                window.frame.size,
            );
            let index = windows.iter().position(|w| w.id == window.id);
            let existing = index.is_some();
            let index = index.unwrap_or_else(|| {
                let layer = CALayer::layer();
                layer.setCornerRadius(3.0);
                layer.setMasksToBounds(true);
                let view = LayerHost::new(&self.ui, &layer);
                view.ns_view().setTranslatesAutoresizingMaskIntoConstraints(true);
                self.host.addSubview(view.ns_view());
                windows.push(WindowView {
                    id: window.id,
                    view,
                    frame: window.frame,
                });
                windows.len() - 1
            });
            let pane = &mut windows[index];
            if existing && duration > 0.0 && pane.frame != window.frame {
                unsafe {
                    let animator: objc2::rc::Retained<objc2::runtime::AnyObject> =
                        objc2::msg_send![pane.view.ns_view(), animator];
                    let _: () = objc2::msg_send![&*animator, setFrame: frame];
                }
            } else {
                pane.view.ns_view().setFrame(frame);
            }
            pane.frame = window.frame;
            if !same_order {
                // Move to the front in supplied order without recreating the view.
                pane.view.ns_view().removeFromSuperview();
                self.host.addSubview(pane.view.ns_view());
            }
        }
        NSAnimationContext::endGrouping();
        self.host.update_palette();
        self.initialized.set(true);
    }

    /// Convenience for layouts whose window identity is their slice index.
    pub fn set_panes(&self, frames: &[CGRect]) {
        let windows: Vec<_> = frames
            .iter()
            .enumerate()
            .map(|(id, frame)| PreviewWindow::new(id as u64, *frame))
            .collect();
        self.set_windows(&windows, PreviewAnimation::default());
    }
}
impl NativeView for LayoutPreview {
    fn ns_view(&self) -> &objc2_app_kit::NSView { &self.host }
}
