//! A nonactivating native glass strip. All objects are confined to the AppKit thread.
//! Geometry was inspired by ZimengXiong/tabbed's Theme, TabItemView and TabStripView
//! at https://github.com/ZimengXiong/tabbed/tree/9481b628767921bd1af1b90c4d7448282c9757b6/Sources/UI.
//! Adaptive native material and semantic colors follow the system appearance.
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, ClassType, DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::*;
use objc2_foundation::{NSAttributedString, NSDictionary, NSString};
use objc2_quartz_core::{
    CABasicAnimation, CALayer, CAMediaTiming, CAMediaTimingFunction, CATransaction,
};

use crate::{
    Button, CGPoint, CGRect, CGSize, GlassEffectView, Image, NativeView, Ui, View, VisualEffect,
};

pub struct TabBarItem {
    pub title: String,
    pub icon: Option<Image>,
}
#[derive(Clone, Copy, Debug)]
pub enum TabBarAction {
    Select(usize),
    Close(usize),
}
struct ActionState {
    callback: RefCell<Option<Box<dyn FnMut(TabBarAction)>>>,
    alive: Cell<bool>,
}
type Action = Rc<ActionState>;
struct Tab {
    select: Button,
    close: Button,
    title: String,
    icon: Option<Image>,
    visual: Retained<TabVisual>,
    index: Rc<Cell<usize>>,
}
struct Content {
    tabs: RefCell<Vec<Tab>>,
    selected: Cell<usize>,
    hovered: Cell<Option<usize>>,
    close_hovered: Cell<Option<usize>>,
    offset: Cell<f64>,
    action: Action,
    viewport: Retained<NSView>,
    highlight: Retained<CALayer>,
    retiring: Retained<NSView>,
    fonts: [Retained<NSFont>; 2],
    text_color: Retained<NSColor>,
    close_symbol: Option<Image>,
    placeholder: Image,
    close_colors: [Retained<NSColor>; 2],
    close_fill: RefCell<Retained<NSColor>>,
    clear: Retained<NSColor>,

    ui: Ui,
}
define_class!(
    #[unsafe(super(NSPanel))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsTabPanel"]
    struct TabPanel;
    impl TabPanel {
        #[unsafe(method(canBecomeKeyWindow))] fn key(&self) -> bool { false }
        #[unsafe(method(canBecomeMainWindow))] fn main(&self) -> bool { false }
        // Private AppKit appearance hook. This changes material rendering only;
        // actual key/main eligibility and focus routing remain public AppKit state.
        // Undocumented behavior may change in future macOS releases.
        #[unsafe(method(_hasActiveAppearance))] fn active_appearance(&self) -> bool { true }
    }
);
define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsTabContent"]
    #[ivars = Content]
    struct TabContent;
    impl TabContent {
        #[unsafe(method(viewDidChangeEffectiveAppearance))] fn appearance_changed(&self) {
            unsafe { let _: () = msg_send![super(self), viewDidChangeEffectiveAppearance]; }
            self.update_material();
            for tab in self.ivars().tabs.borrow().iter() { tab.visual.setNeedsDisplay(true); }
            self.setNeedsDisplay(true);
        }
        #[unsafe(method(accessibilityChanged:))] fn accessibility_changed(&self, _: &objc2_foundation::NSNotification) {
            if NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion() { self.stop_motion(); }
            self.update_material();
            self.setNeedsDisplay(true);
        }
        #[unsafe(method(drawRect:))] fn draw_tabs(&self, dirty: CGRect) {
            let workspace = NSWorkspace::sharedWorkspace();
            let opaque = workspace.accessibilityDisplayShouldReduceTransparency();
            NSGraphicsContext::saveGraphicsState_class();
            // Leave the native material uncovered except when accessibility
            // explicitly requests an opaque surface.
            if opaque {
                NSColor::windowBackgroundColor().setFill();
                NSBezierPath::fillRect(dirty);
            }
            NSGraphicsContext::restoreGraphicsState_class();
        }
        #[unsafe(method(isFlipped))] fn flipped(&self) -> bool { true }
        #[unsafe(method(layout))] fn layout_tabs(&self) {
            unsafe { let _: () = msg_send![super(self), layout]; }
            self.arrange(false, true, false);
        }
        #[unsafe(method(updateTrackingAreas))] fn tracking(&self) {
            unsafe { let _: () = msg_send![super(self), updateTrackingAreas]; }
            for area in self.trackingAreas() { self.removeTrackingArea(&area); }
            let area = unsafe { NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(), self.bounds(),
                NSTrackingAreaOptions::MouseEnteredAndExited | NSTrackingAreaOptions::MouseMoved |
                NSTrackingAreaOptions::ActiveAlways | NSTrackingAreaOptions::InVisibleRect,
                Some(self), None) };
            self.addTrackingArea(&area);
        }
        #[unsafe(method(mouseEntered:))] fn entered(&self, event: &NSEvent) { self.mouseMoved(event); }
        #[unsafe(method(mouseMoved:))] fn moved(&self, event: &NSEvent) {
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            let hover = self.index_at(p);
            let close = hover.filter(|&i| {
                let tabs = self.ivars().tabs.borrow();
                let button = tabs[i].close.ns_view();
                p.x >= button.frame().origin.x && p.x < button.frame().origin.x + 15.0 && p.y >= 6.5 && p.y < 21.5
            });
            let previous = self.ivars().hovered.replace(hover);
            let changed = previous != hover;
            let close_changed = self.ivars().close_hovered.replace(close) != close;
            if changed || close_changed { self.refresh_hover(previous); }
        }
        #[unsafe(method(mouseExited:))] fn exited(&self, _: &NSEvent) {
            let hover = self.ivars().hovered.take();
            let close = self.ivars().close_hovered.take();
            if hover.is_some() || close.is_some() { self.refresh_hover(hover); }
        }
        #[unsafe(method(scrollWheel:))] fn scroll(&self, event: &NSEvent) {
            let s = self.ivars();
            let delta = if event.scrollingDeltaX().abs() > event.scrollingDeltaY().abs() { event.scrollingDeltaX() } else { event.scrollingDeltaY() };
            self.stop_motion();
            s.offset.set(s.offset.get() - delta);
            self.arrange(false, true, false);
        }
    }
);
impl TabContent {
    // Hover changes neither geometry nor labels. Invalidate only the two affected capsules.
    fn refresh_hover(&self, previous: Option<usize>) {
        let s = self.ivars();
        let tabs = s.tabs.borrow();
        let close_fill = s.close_fill.borrow();
        for index in [previous, s.hovered.get()].into_iter().flatten() {
            let Some(tab) = tabs.get(index) else {
                continue;
            };
            let close_hover = s.close_hovered.get() == Some(index);
            tab.close
                .ns_button()
                .setContentTintColor(Some(&s.close_colors[usize::from(close_hover)]));
            if let Some(layer) = tab.close.ns_view().layer() {
                layer.setBackgroundColor(Some(
                    &if close_hover { &*close_fill } else { &s.clear }.CGColor(),
                ));
            }
            let frame = tab.close.ns_view().frame();
            tab.close.set_hidden(
                s.hovered.get() != Some(index)
                    || frame.origin.x < 0.0
                    || frame.origin.x + frame.size.width > self.bounds().size.width,
            );
            tab.visual.ivars().style.set(if s.selected.get() == index {
                2
            } else if s.hovered.get() == Some(index) {
                1
            } else {
                0
            });
            tab.visual.setNeedsDisplay(true);
        }
    }

    fn geometry(&self) -> (f64, f64) {
        let width = self.bounds().size.width.max(0.0);
        let n = self.ivars().tabs.try_borrow().map(|tabs| tabs.len().max(1)).unwrap_or(1) as f64;
        (width, ((width - 4.0 - (n - 1.0) * 2.0) / n).max(120.0))
    }

    fn index_at(&self, p: CGPoint) -> Option<usize> {
        let (width, tab_width) = self.geometry();
        if p.x < 2.0 || p.x >= width || p.y < 2.0 || p.y >= 26.0 {
            return None;
        }
        let x = p.x - 2.0 + self.ivars().offset.get();
        let i = (x / (tab_width + 2.0)) as usize;
        (i < self.ivars().tabs.borrow().len() && x % (tab_width + 2.0) < tab_width).then_some(i)
    }

    fn arrange(&self, reveal: bool, mut redraw: bool, animate: bool) {
        let s = self.ivars();
        let (width, tab_width) = self.geometry();
        let Ok(mut tabs) = s.tabs.try_borrow_mut() else {
            return;
        };
        let max = (tabs.len() as f64 * (tab_width + 2.0) + 2.0 - width).max(0.0);
        let mut offset = s.offset.get().clamp(0.0, max);
        if reveal && s.selected.get() < tabs.len() {
            let left = s.selected.get() as f64 * (tab_width + 2.0);
            offset = offset.min(left).max(left + tab_width + 4.0 - width).clamp(0.0, max);
        }
        s.offset.set(offset);
        let viewport_frame = rect(0.0, 0.0, width, 28.0);
        if s.viewport.frame() != viewport_frame {
            redraw = true;
            s.viewport.setFrame(viewport_frame);
            s.retiring.setFrame(viewport_frame);
        }
        let close_fill = s.close_fill.borrow();
        for (i, tab) in tabs.iter_mut().enumerate() {
            let active = i == s.selected.get();
            let hover = s.hovered.get() == Some(i);
            let x = 2.0 + i as f64 * (tab_width + 2.0) - offset;
            let button = tab.select.ns_button();
            let frame = rect(x, 3.0, tab_width, 22.0);
            button.setFrame(frame);
            let visual_frame = rect(x, 0.0, tab_width, 28.0);
            if tab.visual.frame() != visual_frame {
                let layer = tab.visual.layer().unwrap();
                let from = unsafe { layer.presentationLayer() }.unwrap_or_else(|| layer.retain());
                let had_size = from.bounds().size.width > 0.0;
                let old_position = layer_value(&from, "position");
                let old_bounds = layer_value(&from, "bounds");
                without_actions(|| tab.visual.setFrame(visual_frame));
                if animate && had_size {
                    animate_value(&layer, "position", old_position.as_deref(), 0.20);
                    animate_value(&layer, "bounds", old_bounds.as_deref(), 0.20);
                } else if !animate {
                    layer.removeAllAnimations();
                }
                tab.visual.setNeedsDisplay(true);
            }
            let style = if active {
                2
            } else if hover {
                1
            } else {
                0
            };
            if tab.visual.ivars().style.replace(style) != style {
                tab.visual.setNeedsDisplay(true);
            }
            button.setAccessibilitySelected(active);
            button.setState(if active {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            if let Some(layer) = tab.close.ns_view().layer() {
                layer.setBackgroundColor(Some(
                    &if s.close_hovered.get() == Some(i) {
                        &*close_fill
                    } else {
                        &s.clear
                    }
                    .CGColor(),
                ));
            }
            tab.close.ns_button().setContentTintColor(Some(
                &s.close_colors[usize::from(s.close_hovered.get() == Some(i))],
            ));
            tab.close.ns_button().setFrame(rect(x + 8.0, 6.5, 15.0, 15.0));
            let close_x = tab.close.ns_view().frame().origin.x;
            tab.close.set_hidden(!hover || close_x < 0.0 || close_x + 15.0 > width);
            // Only visible tabs participate in native hit testing.
            tab.select.set_hidden(x + tab_width <= 0.0 || x >= width);
        }
        let target = tabs.get(s.selected.get()).map(|tab| tab.select.ns_view().frame());
        let layer = &s.highlight;
        let was_hidden = layer.isHidden();
        let from = unsafe { layer.presentationLayer() }.unwrap_or_else(|| layer.retain());
        let position = layer_value(&from, "position");
        let bounds = layer_value(&from, "bounds");
        let changed = target.is_some_and(|target| target != layer.frame());
        without_actions(|| {
            layer.setHidden(target.is_none());
            if let Some(target) = target {
                layer.setFrame(target);
            }
        });
        if animate && !was_hidden && changed {
            animate_value(layer, "position", position.as_deref(), 0.18);
            animate_value(layer, "bounds", bounds.as_deref(), 0.18);
        } else if !animate && changed {
            layer.removeAllAnimations();
        }
        if redraw {
            self.setNeedsDisplay(true);
        }
    }
}
struct Visual {
    labels: RefCell<Vec<(Retained<NSAttributedString>, CGSize)>>,
    icon: RefCell<Image>,
    style: Cell<usize>,
}
define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsTabVisual"]
    #[ivars = Visual]
    struct TabVisual;
    impl TabVisual {
        #[unsafe(method(isFlipped))] fn flipped(&self) -> bool { true }
        // Let AppKit composite semantic label colors against the native material.
        #[unsafe(method(allowsVibrancy))] fn vibrant(&self) -> bool { true }
        #[unsafe(method(hitTest:))] fn hit(&self, _: CGPoint) -> Option<&NSView> { None }
        #[unsafe(method(viewDidChangeEffectiveAppearance))] fn appearance(&self) {
            unsafe { let _: () = msg_send![super(self), viewDidChangeEffectiveAppearance]; }
            self.setNeedsDisplay(true);
        }
        #[unsafe(method(drawRect:))] fn draw(&self, _: CGRect) {
            let s = self.ivars();
            let labels = s.labels.borrow();
            let style = s.style.get();
            let Some((label, text_size)) = labels.get(style) else { return; };
            let width = self.bounds().size.width;
            if style == 1 {
                NSColor::labelColor().colorWithAlphaComponent(0.035).setFill();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect(0.0, 3.0, width, 22.0), 11.0, 11.0).fill();
            }
            let icon_width = 22.0;
            let text_width = labels[2].1.width.min((width - 44.0 - icon_width).max(0.0));
            let scale = self.window().map_or(1.0, |window| window.backingScaleFactor()).max(1.0);
            let x = (((width - text_width - icon_width) / 2.0) * scale).round() / scale;
            let text_y = (((28.0 - text_size.height) / 2.0) * scale).round() / scale;
            unsafe { s.icon.borrow().drawInRect_fromRect_operation_fraction_respectFlipped_hints(
                rect(x, 7.0, 14.0, 14.0), CGRect::ZERO, NSCompositingOperation::SourceOver, 1.0, true, None); }
            label.drawWithRect_options(rect(x + icon_width, text_y, text_width, text_size.height),
                NSStringDrawingOptions::UsesLineFragmentOrigin | NSStringDrawingOptions::TruncatesLastVisibleLine);
        }
    }
);
fn without_actions(f: impl FnOnce()) {
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    f();
    CATransaction::commit();
}
fn layer_value(layer: &CALayer, key: &str) -> Option<Retained<AnyObject>> {
    unsafe { msg_send![layer, valueForKey: &*NSString::from_str(key)] }
}
fn animate_value(layer: &CALayer, key: &str, from: Option<&AnyObject>, duration: f64) {
    let key = NSString::from_str(key);
    let animation = CABasicAnimation::animationWithKeyPath(Some(&key));
    unsafe {
        animation.setFromValue(from);
    }
    animation.setDuration(duration);
    animation.setTimingFunction(Some(&CAMediaTimingFunction::functionWithControlPoints(
        0.42, 0.0, 0.58, 1.0,
    )));
    layer.addAnimation_forKey(&animation, Some(&key));
}
impl TabContent {
    fn update_material(&self) {
        let s = self.ivars();
        let workspace = NSWorkspace::sharedWorkspace();
        let contrast = workspace.accessibilityDisplayShouldIncreaseContrast();
        let opaque = workspace.accessibilityDisplayShouldReduceTransparency();
        self.effectiveAppearance()
            .performAsCurrentDrawingAppearance(&block2::RcBlock::new(|| {
                without_actions(|| {
                    *s.close_fill.borrow_mut() =
                        NSColor::labelColor().colorWithAlphaComponent(0.08);
                    s.highlight.setBackgroundColor(Some(
                        &if opaque || contrast {
                            NSColor::labelColor().colorWithAlphaComponent(0.24)
                        } else {
                            // A selected segment needs a stronger tonal step than hover.
                            NSColor::labelColor().colorWithAlphaComponent(0.18)
                        }
                        .CGColor(),
                    ));
                    s.highlight.setBorderWidth(if contrast { 1.0 } else { 0.0 });
                    s.highlight.setBorderColor(
                        contrast.then(|| NSColor::labelColor().CGColor()).as_deref(),
                    );
                })
            }));
    }

    fn stop_motion(&self) {
        self.ivars().highlight.removeAllAnimations();
        for tab in self.ivars().tabs.borrow().iter() {
            tab.visual.layer().unwrap().removeAllAnimations();
        }
        for view in self.ivars().retiring.subviews() {
            view.removeFromSuperview();
        }
    }
}
#[allow(deprecated)]
fn placeholder() -> Image {
    let image = NSImage::initWithSize(NSImage::alloc(), CGSize::new(14.0, 14.0));
    image.lockFocusFlipped(true);
    NSColor::secondaryLabelColor().setFill();
    NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect(0.0, 0.0, 14.0, 14.0), 3.0, 3.0)
        .fill();
    image.unlockFocus();
    image
}
fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
    CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
}
fn emit(action: &Action, event: TabBarAction) {
    if !action.alive.get() {
        return;
    }
    if let Ok(mut f) = action.callback.try_borrow_mut() {
        if let Some(f) = f.as_mut() {
            crate::bridge::callback(|| f(event));
        }
    }
}
/// One panel and one glass surface; updates preserve the native window and unchanged controls.
pub struct TabBar {
    panel: Retained<TabPanel>,
    content: Retained<TabContent>,
    _surface: Box<dyn NativeView>,
}
impl TabBar {
    pub const HEIGHT: f64 = 28.0;

    pub fn new(ui: &Ui) -> Self {
        let action: Action = Rc::new(ActionState {
            callback: RefCell::new(None),
            alive: Cell::new(true),
        });
        let viewport = NSView::new(ui.mtm());
        viewport.setWantsLayer(true);
        if let Some(layer) = viewport.layer() {
            layer.setMasksToBounds(true);
        }
        let highlight = CALayer::new();
        highlight.setCornerRadius(11.0);
        highlight.setCornerCurve(unsafe { objc2_quartz_core::kCACornerCurveContinuous });
        highlight.setHidden(true);
        let retiring: Retained<TabVisual> = unsafe {
            msg_send![
                super(TabVisual::alloc(ui.mtm()).set_ivars(Visual {
                    labels: RefCell::new(Vec::new()),
                    icon: RefCell::new(placeholder()),
                    style: Cell::new(0),
                })),
                init
            ]
        };
        let retiring = retiring.into_super();
        let content: Retained<TabContent> = unsafe {
            msg_send![
                super(TabContent::alloc(ui.mtm()).set_ivars(Content {
                    tabs: RefCell::new(Vec::new()),
                    selected: Cell::new(0),
                    hovered: Cell::new(None),
                    close_hovered: Cell::new(None),
                    offset: Cell::new(0.0),
                    action,
                    viewport,
                    highlight,
                    retiring,
                    fonts: {
                        [
                            NSFont::systemFontOfSize_weight(12.0, NSFontWeightRegular),
                            NSFont::systemFontOfSize_weight(12.0, NSFontWeightMedium),
                        ]
                    },
                    text_color: NSColor::labelColor(),
                    close_symbol: crate::Symbol::named("xmark").and_then(|image| {
                        image.imageWithSymbolConfiguration(
                            &NSImageSymbolConfiguration::configurationWithPointSize_weight(
                                8.0,
                                NSFontWeightBold,
                            ),
                        )
                    }),
                    placeholder: placeholder(),
                    close_colors: [NSColor::secondaryLabelColor(), NSColor::labelColor()],
                    close_fill: RefCell::new(NSColor::labelColor().colorWithAlphaComponent(0.08)),
                    clear: NSColor::clearColor(),
                    ui: *ui,
                })),
                init
            ]
        };
        unsafe {
            NSWorkspace::sharedWorkspace()
                .notificationCenter()
                .addObserver_selector_name_object(
                    &content,
                    objc2::sel!(accessibilityChanged:),
                    Some(&NSString::from_str(
                        "NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification",
                    )),
                    None,
                );
        }
        content.setAccessibilityElement(true);
        content.setAccessibilityRole(Some(unsafe { NSAccessibilityTabGroupRole }));
        content.setAccessibilityLabel(Some(&NSString::from_str("Window tabs")));
        content.addSubview(&content.ivars().viewport);
        content.addSubview(&content.ivars().retiring);
        content.setWantsLayer(true);
        if let Some(layer) = content.layer() {
            layer.setCornerRadius(14.0);
            layer.setCornerCurve(unsafe { objc2_quartz_core::kCACornerCurveContinuous });
            layer.setMasksToBounds(true);
            layer.insertSublayer_atIndex(&content.ivars().highlight, 0);
        }
        let host = View::new(ui).content(content.retain().into_super(), crate::uniform_insets(0.0));
        // The appearance hook must belong to AppKit itself, not just our subclass.
        // If Apple removes it, use public active vibrancy rather than subdued glass.
        let active_glass = objc2::runtime::AnyClass::get(c"NSGlassEffectView").is_some()
            && NSPanel::class().instance_method(objc2::sel!(_hasActiveAppearance)).is_some();
        let surface: Box<dyn NativeView> = if active_glass {
            let glass = GlassEffectView::new(ui, host)
                .corner_radius(14.0)
                .style(crate::GlassStyle::Regular);
            Box::new(glass)
        } else {
            // Public-API fallback: active vibrancy, not Liquid Glass. Keep one material.
            let fallback = VisualEffect::new(ui, NSVisualEffectMaterial::Popover, host);
            fallback.ns_visual_effect_view().setState(NSVisualEffectState::Active);
            fallback
                .ns_visual_effect_view()
                .setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            fallback.ns_view().setWantsLayer(true);
            if let Some(layer) = fallback.ns_view().layer() {
                layer.setCornerRadius(14.0);
                layer.setMasksToBounds(true);
            }
            Box::new(fallback)
        };
        let panel: Retained<TabPanel> = unsafe {
            msg_send![super(TabPanel::alloc(ui.mtm()).set_ivars(())), initWithContentRect: rect(0.0,0.0,400.0,28.0), styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel, backing: NSBackingStoreType::Buffered, defer: false]
        };
        unsafe {
            panel.setReleasedWhenClosed(false);
        }
        panel.setAcceptsMouseMovedEvents(true);
        panel.setFloatingPanel(false);
        panel.setHasShadow(false);
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setLevel(NSNormalWindowLevel);
        panel.setHidesOnDeactivate(false);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        panel.setContentView(Some(surface.ns_view()));
        content.update_material();
        Self {
            panel,
            content,
            _surface: surface,
        }
    }

    pub fn on_action(self, f: impl FnMut(TabBarAction) + 'static) -> Self {
        *self.content.ivars().action.callback.borrow_mut() = Some(Box::new(f));
        self
    }

    pub fn set_tabs(&self, items: Vec<TabBarItem>, selected: usize) {
        let s = self.content.ivars();
        {
            let tabs = s.tabs.borrow();
            if tabs.len() == items.len()
                && tabs
                    .iter()
                    .zip(&items)
                    .all(|(tab, item)| tab.title == item.title && tab.icon == item.icon)
            {
                let previous = s.selected.replace(selected);
                if previous == selected {
                    return;
                }
                drop(tabs);
                // Reveal may change every frame during overflow. Otherwise only two
                // cached labels and capsules need redrawing; controls remain unchanged.
                self.content.arrange(true, false, self.motion_enabled());
                let tabs = s.tabs.borrow();
                for index in [previous, selected] {
                    if let Some(tab) = tabs.get(index) {
                        tab.visual.setNeedsDisplay(true);
                    }
                }
                return;
            }
        }
        let animate = self.motion_enabled();
        let count_changed = s.tabs.borrow().len() != items.len();
        for view in s.retiring.subviews() {
            view.removeFromSuperview();
        }
        let mut tabs = s.tabs.borrow_mut();
        let mut old: Vec<_> = tabs.drain(..).map(Some).collect();
        for (i, item) in items.into_iter().enumerate() {
            let existing = old.iter().position(|tab| {
                tab.as_ref().is_some_and(|tab| tab.title == item.title && tab.icon == item.icon)
            });
            if let Some(tab) = existing.and_then(|index| old[index].take()).or_else(|| {
                if !count_changed {
                    old.get_mut(i).and_then(Option::take)
                } else {
                    None
                }
            }) {
                tabs.push(tab);
            }
            if i == tabs.len() {
                let index = Rc::new(Cell::new(i));
                let select_index = index.clone();
                let close_index = index.clone();
                let action = s.action.clone();
                let select = Button::new(&s.ui, "")
                    .borderless()
                    .on_click(move || emit(&action, TabBarAction::Select(select_index.get())));
                select.ns_button().setTransparent(true);
                select
                    .ns_button()
                    .setAccessibilityRole(Some(unsafe { NSAccessibilityRadioButtonRole }));
                let action = s.action.clone();
                let close = Button::new(&s.ui, "")
                    .borderless()
                    .symbol("xmark")
                    .on_click(move || emit(&action, TabBarAction::Close(close_index.get())));
                close.accessibility_label("Close window");

                close.ns_button().setImage(s.close_symbol.as_deref());
                close.ns_button().setContentTintColor(Some(&s.close_colors[0]));
                close.ns_view().setWantsLayer(true);
                if let Some(layer) = close.ns_view().layer() {
                    layer.setCornerRadius(7.5);
                }
                let visual: Retained<TabVisual> = unsafe {
                    msg_send![
                        super(TabVisual::alloc(s.ui.mtm()).set_ivars(Visual {
                            labels: RefCell::new(Vec::new()),
                            icon: RefCell::new(s.placeholder.retain()),
                            style: Cell::new(0),
                        })),
                        init
                    ]
                };
                visual.setWantsLayer(true);
                // Preserve glyph/icon scale while the compositor resizes the tab.
                visual.setLayerContentsPlacement(NSViewLayerContentsPlacement::Center);
                visual.setAccessibilityElement(false);
                s.viewport.addSubview(&visual);
                if animate {
                    let layer = visual.layer().unwrap();
                    without_actions(|| layer.setOpacity(1.0));
                    let zero = objc2_foundation::NSNumber::new_f32(0.0);
                    animate_value(&layer, "opacity", Some(&zero), 0.14);
                }
                s.viewport.addSubview(select.ns_view());
                s.viewport.addSubview(close.ns_view());
                tabs.push(Tab {
                    select,
                    close,
                    title: String::new(),
                    icon: None,
                    visual,
                    index,
                });
            }
            let tab = &mut tabs[i];
            tab.index.set(i);
            if tab.title != item.title || tab.visual.ivars().labels.borrow().is_empty() {
                tab.select.set_title(&item.title);
                let paragraph = NSMutableParagraphStyle::new();
                paragraph.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
                if item.title.contains('/') || item.title.contains('\\') {
                    paragraph.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
                }
                let labels = (0..3)
                    .map(|style| {
                        let attrs = NSDictionary::from_slices(
                            &[
                                unsafe { NSFontAttributeName },
                                unsafe { NSForegroundColorAttributeName },
                                unsafe { NSParagraphStyleAttributeName },
                            ],
                            &[
                                &*s.fonts[usize::from(style == 2)] as &AnyObject,
                                &*s.text_color as &AnyObject,
                                &*paragraph as &AnyObject,
                            ],
                        );
                        let label = unsafe {
                            NSAttributedString::initWithString_attributes(
                                NSAttributedString::alloc(),
                                &NSString::from_str(&item.title),
                                Some(&attrs),
                            )
                        };
                        let size = label.size();
                        (label, size)
                    })
                    .collect();
                tab.select.accessibility_label(&item.title);
                tab.select.tooltip(&item.title);
                tab.close.accessibility_label(&format!("Close {}", item.title));
                *tab.visual.ivars().labels.borrow_mut() = labels;
                tab.visual.setNeedsDisplay(true);
                tab.title = item.title;
            }
            if tab.icon != item.icon {
                *tab.visual.ivars().icon.borrow_mut() =
                    item.icon.as_ref().unwrap_or(&s.placeholder).retain();
                tab.visual.setNeedsDisplay(true);
                tab.icon = item.icon;
            }
        }
        for tab in old.into_iter().flatten() {
            tab.select.ns_view().removeFromSuperview();
            tab.close.ns_view().removeFromSuperview();
            if animate {
                s.retiring.addSubview(&tab.visual);
                let layer = tab.visual.layer().unwrap();
                let from = layer_value(
                    &unsafe { layer.presentationLayer() }.unwrap_or_else(|| layer.retain()),
                    "opacity",
                );
                CATransaction::begin();
                CATransaction::setDisableActions(true);
                layer.setOpacity(0.0);
                let visual = tab.visual.retain();
                let done = block2::RcBlock::new(move || visual.removeFromSuperview());
                unsafe {
                    CATransaction::setCompletionBlock(Some(&done));
                }
                animate_value(&layer, "opacity", from.as_deref(), 0.14);
                CATransaction::commit();
            } else {
                tab.visual.removeFromSuperview();
            }
        }
        let count = tabs.len();
        drop(tabs);
        s.selected.set(selected);
        if count_changed || s.hovered.get().is_some_and(|index| index >= count) {
            s.hovered.set(None);
            s.close_hovered.set(None);
        }
        self.content.arrange(true, true, animate);
    }

    fn motion_enabled(&self) -> bool {
        self.content.window().is_some_and(|window| window.isVisible())
            && !NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
    }

    pub fn set_frame(&self, frame: CGRect) {
        if self.panel.frame() != frame {
            self.content.stop_motion();
            self.panel.setFrame_display(frame, true);
            self.content.arrange(true, true, false);
        }
    }

    /// Reassert relative order: the anchor may have been raised since the previous call.
    pub fn show_above(&self, number: isize) {
        if number > 0 {
            self.panel.orderWindow_relativeTo(NSWindowOrderingMode::Above, number);
        }
    }

    pub fn hide(&self) {
        self.content.stop_motion();
        if self.is_visible() {
            self.panel.orderOut(None);
        }
    }

    pub fn is_visible(&self) -> bool { self.panel.isVisible() }

    pub fn ns_panel(&self) -> &NSPanel { &self.panel }
}
impl Drop for TabBar {
    fn drop(&mut self) {
        self.hide();
        unsafe {
            NSWorkspace::sharedWorkspace()
                .notificationCenter()
                .removeObserver(&self.content);
        }
        let action = &self.content.ivars().action;
        action.alive.set(false);
        if let Ok(mut callback) = action.callback.try_borrow_mut() {
            callback.take();
        }
        self.panel.setContentView(None);
        self.panel.close();
    }
}
