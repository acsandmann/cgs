use std::cell::{Cell, RefCell};
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSAnimationContext, NSAppearanceCustomization, NSColor, NSView, NSWorkspace,
};
use objc2_quartz_core::{CALayer, CATransaction};

use crate::{CGPoint, CGRect, CGSize, LayerHost, NativeView, Ui};

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsLayoutPreview"]
    #[ivars = Cell<bool>]
    struct PreviewView;
    impl PreviewView {
        // Illustrations never intercept their enclosing native control’s clicks.
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: CGPoint) -> *mut NSView { std::ptr::null_mut() }
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
        // Resolve semantic colors using this view’s appearance before assigning CGColors.
        self.effectiveAppearance().performAsCurrentDrawingAppearance(&RcBlock::new(|| {
            // A desktop tinted with the accent color, and windows as solid cards with a
            // defined edge, so overlapping windows layer cleanly at thumbnail size.
            // Selection deepens the tint without an outline.
            let selected = self.ivars().get();
            let surface = grouped_surface();
            let accent = NSColor::controlAccentColor();
            let label = NSColor::labelColor();
            let mix = |color: &NSColor, fraction: f64, ink: &NSColor| {
                color
                    .blendedColorWithFraction_ofColor(fraction, ink)
                    .unwrap_or_else(|| ink.colorWithAlphaComponent(fraction))
            };
            // Light text means a dark appearance, including its high-contrast variant.
            let dark = label
                .colorUsingColorSpace(&objc2_app_kit::NSColorSpace::sRGBColorSpace())
                .is_some_and(|color| color.redComponent() > 0.5);
            let background = mix(&surface, if selected { 0.30 } else { 0.20 }, &accent);
            let tile = if dark {
                mix(&surface, 0.18, &label)
            } else {
                NSColor::controlBackgroundColor()
            };
            let stroke = mix(&background, 0.35, &label);
            CATransaction::begin();
            CATransaction::setDisableActions(true);
            layer.setBackgroundColor(Some(&background.CGColor()));
            layer.setBorderColor(Some(&accent.CGColor()));
            layer.setBorderWidth(0.0);
            for view in self.subviews() {
                if let Some(layer) = view.layer() {
                    layer.setBackgroundColor(Some(&tile.CGColor()));
                    layer.setBorderColor(Some(&stroke.CGColor()));
                }
            }
            CATransaction::commit();
        }));
    }
}

/// The opaque color of a grouped settings surface (`GroupBox`) over the window
/// background, so previews blend into it instead of reading as darker panels.
fn grouped_surface() -> objc2::rc::Retained<NSColor> {
    let srgb = objc2_app_kit::NSColorSpace::sRGBColorSpace();
    let base = NSColor::windowBackgroundColor();
    let fill = NSColor::quaternarySystemFillColor();
    match (
        base.colorUsingColorSpace(&srgb),
        fill.colorUsingColorSpace(&srgb),
    ) {
        (Some(base), Some(fill)) => {
            let alpha = fill.alphaComponent();
            let over = |top: f64, bottom: f64| top * alpha + bottom * (1.0 - alpha);
            NSColor::colorWithSRGBRed_green_blue_alpha(
                over(fill.redComponent(), base.redComponent()),
                over(fill.greenComponent(), base.greenComponent()),
                over(fill.blueComponent(), base.blueComponent()),
                1.0,
            )
        }
        _ => base,
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

/// An appearance-aware layout canvas with neutral windows and a restrained screen outline.
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
    /// Mark the inspected choice using the system accent without changing its geometry.
    pub fn set_selected(&self, selected: bool) {
        if self.host.ivars().replace(selected) != selected {
            self.host.update_palette();
        }
    }

    pub fn new(ui: &Ui, size: CGSize) -> Self {
        assert!(
            size.width.is_finite()
                && size.width > 0.0
                && size.height.is_finite()
                && size.height > 0.0
        );
        let root = CALayer::layer();
        root.setCornerRadius((size.height * 0.1).min(9.0));
        root.setBorderWidth(1.0);
        root.setMasksToBounds(true);
        let host: Retained<PreviewView> = unsafe {
            msg_send![
                super(PreviewView::alloc(ui.mtm()).set_ivars(Cell::new(false))),
                init
            ]
        };
        host.setAccessibilityElement(true);
        host.setAccessibilityRole(Some(unsafe { objc2_app_kit::NSAccessibilityImageRole }));
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
                layer.setBorderWidth(1.0);
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
            pane.view
                .ns_view()
                .setHidden(frame.size.width <= 0.0 || frame.size.height <= 0.0);
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
