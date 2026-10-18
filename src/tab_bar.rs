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
    CABasicAnimation, CALayer, CAMediaTiming, CAMediaTimingFunction, CASpringAnimation,
    CATransaction,
};

use crate::{
    Button, CGPoint, CGRect, CGSize, GlassEffectView, Image, NativeView, Ui, View, VisualEffect,
};

pub struct TabBarItem {
    pub title: String,
    pub icon: Option<Image>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TabBarEdge {
    #[default]
    Top,
    Bottom,
    Left,
    Right,
}
#[derive(Clone, Copy, Debug)]
pub enum TabBarAction {
    Select(usize),
    Close(usize),
    /// The tab at `from` was dragged to `to`. The strip already shows the new order.
    Move {
        from: usize,
        to: usize,
    },
}
/// A press on a tab that may become a drag.
#[derive(Clone, Copy)]
struct Press {
    index: usize,
    origin: CGPoint,
    /// Pointer offset from the tab's leading edge along the tab axis.
    grab: f64,
}
/// A tab following the pointer. Other tabs make room for it at `slot`.
#[derive(Clone, Copy)]
struct Drag {
    from: usize,
    slot: usize,
    /// Leading edge of the dragged tab along the tab axis.
    main: f64,
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
    vertical: Cell<bool>,
    upward: Cell<bool>,
    select_on_hover: Cell<bool>,
    action: Action,
    viewport: Retained<NSView>,
    highlight: Retained<CALayer>,
    press: Cell<Option<Press>>,
    drag: Cell<Option<Drag>>,
    /// A raised thumb carried under the dragged tab, then settled into its slot.
    lift: Retained<CALayer>,
    /// The tab riding the lift, while dragged and until it lands.
    lifting: Cell<Option<usize>>,
    /// Invalidates the landing of an earlier drag.
    lift_epoch: Cell<u64>,
    /// Use springs, as for tabs moving under the pointer, rather than the eased default.
    springy: Cell<bool>,
    retiring: Retained<NSView>,
    fonts: [Retained<NSFont>; 2],
    text_colors: [Retained<NSColor>; 2],
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
    // Tabs are positioned top-down, matching the content view and its highlight layer.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsTabViewport"]
    struct TabViewport;
    impl TabViewport {
        #[unsafe(method(isFlipped))] fn flipped(&self) -> bool { true }
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
            // Glass alone takes on the wallpaper behind it. A translucent window-colored
            // wash keeps labels legible over busy imagery while preserving the material.
            if opaque {
                NSColor::windowBackgroundColor().setFill();
            } else {
                if dark(self) {
                    NSColor::windowBackgroundColor().colorWithAlphaComponent(0.72).setFill();
                } else {
                    // A neutral gray track, as in a light segmented control, lets the white
                    // thumb read. Kept near-opaque so the wallpaper's hue doesn't muddy it.
                    NSColor::colorWithWhite_alpha(0.89, 0.94).setFill();
                }
            }
            NSBezierPath::fillRect(dirty);
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
        // Tabs track their own presses so they can be dragged; close buttons stay controls.
        // The views returned are owned by the view hierarchy, which outlives the call.
        #[unsafe(method(hitTest:))] fn hit(&self, point: CGPoint) -> *mut NSView {
            let hit: *mut NSView = unsafe { msg_send![super(self), hitTest: point] };
            let s = self.ivars();
            let tab = std::ptr::eq(hit, &*s.viewport)
                || s.tabs.try_borrow().is_ok_and(|tabs| {
                    tabs.iter().any(|tab| std::ptr::eq(hit, tab.select.ns_view()))
                });
            if tab { (self as *const Self).cast_mut().cast() } else { hit }
        }
        // The strip sits over other apps' windows; the first click must act.
        #[unsafe(method(acceptsFirstMouse:))] fn first_mouse(&self, _: Option<&NSEvent>) -> bool { true }
        #[unsafe(method(mouseDownCanMoveWindow))] fn moves_window(&self) -> bool { false }
        #[unsafe(method(mouseDown:))] fn down(&self, event: &NSEvent) {
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            self.cancel_drag();
            let Some(index) = self.index_at(p) else { return; };
            let start = self.slot_start(index);
            self.ivars().press.set(Some(Press { index, origin: p, grab: self.main_of(p) - start }));
        }
        #[unsafe(method(mouseDragged:))] fn dragged(&self, event: &NSEvent) {
            let s = self.ivars();
            let Some(press) = s.press.get() else { return; };
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            if s.drag.get().is_none() {
                let moved = (p.x - press.origin.x).hypot(p.y - press.origin.y);
                if moved < DRAG_THRESHOLD || s.tabs.borrow().len() < 2 { return; }
                self.begin_drag(press);
            }
            self.drag_to(self.main_of(p) - press.grab);
        }
        #[unsafe(method(mouseUp:))] fn up(&self, event: &NSEvent) {
            let s = self.ivars();
            let Some(press) = s.press.take() else { return; };
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            if s.drag.get().is_some() {
                self.end_drag(p);
            } else if self.index_at(p) == Some(press.index) {
                emit(&s.action, TabBarAction::Select(press.index));
            }
        }
        #[unsafe(method(mouseEntered:))] fn entered(&self, event: &NSEvent) { self.mouseMoved(event); }
        #[unsafe(method(mouseMoved:))] fn moved(&self, event: &NSEvent) {
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            let hover = self.index_at(p);
            let close = hover.filter(|&i| {
                let tabs = self.ivars().tabs.borrow();
                let f = tabs[i].close.ns_view().frame();
                p.x >= f.origin.x && p.x < f.origin.x + f.size.width && p.y >= f.origin.y && p.y < f.origin.y + f.size.height
            });
            let previous = self.ivars().hovered.replace(hover);
            let changed = previous != hover;
            let close_changed = self.ivars().close_hovered.replace(close) != close;
            if changed || close_changed { self.refresh_hover(previous); }
            // Entering a tab selects it, saving the click; the close button stays a target.
            if let Some(i) = hover.filter(|&i| {
                changed && close.is_none() && self.ivars().select_on_hover.get() && i != self.ivars().selected.get()
            }) {
                emit(&self.ivars().action, TabBarAction::Select(i));
            }
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
            if let Some(next) = tabs.get(index + 1) {
                next.visual.ivars().separator.set(self.separated(index + 1));
                next.visual.setNeedsDisplay(true);
            }
            let Some(tab) = tabs.get(index) else {
                continue;
            };
            tab.visual.ivars().separator.set(self.separated(index));
            let close_hover = s.close_hovered.get() == Some(index);
            tab.close
                .ns_button()
                .setContentTintColor(Some(&s.close_colors[usize::from(close_hover)]));
            if let Some(layer) = tab.close.ns_view().layer() {
                layer.setBackgroundColor(Some(
                    &if close_hover { &*close_fill } else { &s.clear }.CGColor(),
                ));
            }
            tab.close
                .set_hidden(s.hovered.get() != Some(index) || self.clipped(tab.close.ns_view()));
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

    /// Hairlines divide resting tabs only; a capsule already delimits its neighbours.
    fn separated(&self, index: usize) -> bool {
        let s = self.ivars();
        let plain = |i: usize| s.selected.get() != i && s.hovered.get() != Some(i);
        // Hairlines would slide between moving tabs; leave them out until the drop lands.
        s.lifting.get().is_none() && index > 0 && plain(index) && plain(index - 1)
    }

    fn main_of(&self, p: CGPoint) -> f64 {
        if self.ivars().vertical.get() {
            p.y
        } else {
            p.x
        }
    }

    /// Leading edge of a slot along the tab axis, in view coordinates.
    fn slot_start(&self, slot: usize) -> f64 {
        let (_, _, tab_width) = self.geometry();
        INSET + slot as f64 * (tab_width + GAP) - self.ivars().offset.get()
    }

    /// Where a tab sits while another is dragged: the dragged tab takes its slot and
    /// those it passed over close the gap it left.
    fn display_slot(&self, index: usize) -> usize {
        match self.ivars().drag.get() {
            Some(Drag { from, slot, .. }) if index == from => slot,
            Some(Drag { from, slot, .. }) if from < slot && (from + 1..=slot).contains(&index) => {
                index - 1
            }
            Some(Drag { from, slot, .. }) if slot < from && (slot..from).contains(&index) => {
                index + 1
            }
            _ => index,
        }
    }

    fn motion_enabled(&self) -> bool {
        self.window().is_some_and(|window| window.isVisible())
            && !NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
    }

    /// Lift the pressed tab out of the strip once the pointer has moved far enough.
    fn begin_drag(&self, press: Press) {
        let s = self.ivars();
        s.drag.set(Some(Drag {
            from: press.index,
            slot: press.index,
            main: self.slot_start(press.index),
        }));
        s.lift_epoch.set(s.lift_epoch.get() + 1);
        s.lifting.set(Some(press.index));
        s.hovered.set(None);
        s.close_hovered.set(None);
        haptic(NSHapticFeedbackPattern::Generic);
        let motion = self.motion_enabled();
        let selected = press.index == s.selected.get();
        let lift = &s.lift;
        let rest = self.lift_shadow(false);
        without_actions(|| {
            lift.removeAllAnimations();
            lift.setHidden(false);
            lift.setOpacity(1.0);
            self.set_lift_shadow(self.lift_shadow(true));
            if let Some(layer) = s.tabs.borrow()[press.index].visual.layer() {
                layer.setZPosition(2.0);
            }
        });
        if motion {
            // The selected thumb rises in place; any other tab gains one as it lifts.
            if !selected {
                let zero = objc2_foundation::NSNumber::new_f32(0.0);
                animate_value(lift, "opacity", Some(&zero), 0.12);
            }
            self.animate_lift_shadow(rest, 0.16, false);
        }
        self.arrange(false, false, motion);
    }

    fn drag_to(&self, main: f64) {
        let s = self.ivars();
        let Some(mut drag) = s.drag.get() else {
            return;
        };
        let (length, _, tab_width) = self.geometry();
        let count = s.tabs.borrow().len();
        // Scroll an overflowing strip while the tab is held against one of its ends.
        let max = (count as f64 * (tab_width + GAP) - GAP + 2.0 * INSET - length).max(0.0);
        let before = s.offset.get();
        if main < INSET {
            s.offset.set((before - (INSET - main) * 0.5).max(0.0));
        } else if main + tab_width > length - INSET {
            s.offset.set((before + (main + tab_width - length + INSET) * 0.5).min(max));
        }
        let first = self.slot_start(0);
        let last = self.slot_start(count - 1);
        // Resist travel past either end rather than stopping dead, as a scroll view does.
        drag.main = if main < first {
            first - rubber_band(first - main)
        } else if main > last {
            last + rubber_band(main - last)
        } else {
            main
        };
        let slot = ((drag.main - first) / (tab_width + GAP)).round().clamp(0.0, (count - 1) as f64)
            as usize;
        if slot != drag.slot {
            drag.slot = slot;
            haptic(NSHapticFeedbackPattern::Alignment);
        }
        s.drag.set(Some(drag));
        self.arrange(false, false, self.motion_enabled());
    }

    /// Drop the dragged tab into its slot, commit the order, and report the move.
    fn end_drag(&self, p: CGPoint) {
        let s = self.ivars();
        let Some(Drag { from, slot: to, .. }) = s.drag.take() else {
            return;
        };
        if from != to {
            let mut tabs = s.tabs.borrow_mut();
            let tab = tabs.remove(from);
            tabs.insert(to, tab);
            for (i, tab) in tabs.iter().enumerate() {
                tab.index.set(i);
            }
            s.selected.set(moved_index(s.selected.get(), from, to));
        }
        s.lifting.set(Some(to));
        s.hovered.set(self.index_at(p));
        let motion = self.motion_enabled();
        let epoch = s.lift_epoch.get();
        let content = self.retain();
        CATransaction::begin();
        let land = block2::RcBlock::new(move || content.land(epoch));
        unsafe {
            CATransaction::setCompletionBlock(Some(&land));
        }
        s.springy.set(true);
        self.arrange(false, true, motion);
        s.springy.set(false);
        if motion {
            let to_selected = s.selected.get() == to;
            // The selected thumb settles back into the strip; another tab's dissolves.
            self.animate_lift_shadow(self.lift_shadow(true), 0.24, true);
            if !to_selected {
                let one = objc2_foundation::NSNumber::new_f32(1.0);
                without_actions(|| s.lift.setOpacity(0.0));
                animate_value(&s.lift, "opacity", Some(&one), 0.24);
            }
        }
        CATransaction::commit();
        if from != to {
            haptic(NSHapticFeedbackPattern::LevelChange);
            emit(&s.action, TabBarAction::Move { from, to });
        }
    }

    /// The dropped tab has landed: hand the selection back to the strip's own thumb.
    fn land(&self, epoch: u64) {
        let s = self.ivars();
        if s.lift_epoch.get() != epoch || s.drag.get().is_some() {
            return;
        }
        self.clear_lift();
        self.arrange(false, false, false);
    }

    fn cancel_drag(&self) {
        let s = self.ivars();
        s.press.set(None);
        if s.drag.take().is_some() || s.lifting.get().is_some() {
            s.lift_epoch.set(s.lift_epoch.get() + 1);
            self.clear_lift();
            self.arrange(false, false, false);
        }
    }

    fn clear_lift(&self) {
        let s = self.ivars();
        s.lifting.set(None);
        without_actions(|| {
            s.lift.removeAllAnimations();
            s.lift.setHidden(true);
            if let Ok(tabs) = s.tabs.try_borrow() {
                for tab in tabs.iter() {
                    if let Some(layer) = tab.visual.layer() {
                        layer.setZPosition(0.0);
                    }
                }
            }
        });
    }

    /// Shadow opacity, radius, and downward offset of the thumb, raised or at rest.
    fn lift_shadow(&self, lifted: bool) -> (f32, f64, f64) {
        let opaque = NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceTransparency();
        let dark = dark(self);
        let opacity = match (opaque, dark, lifted) {
            (true, ..) => 0.0,
            (false, true, true) => 0.5,
            (false, false, true) => 0.22,
            (false, true, false) => 0.3,
            (false, false, false) => 0.2,
        };
        match (dark, lifted) {
            (_, true) => (opacity, 6.0, 3.0),
            (true, false) => (opacity, 2.0, 1.0),
            (false, false) => (opacity, 1.5, 1.0),
        }
    }

    fn set_lift_shadow(&self, (opacity, radius, offset): (f32, f64, f64)) {
        let lift = &self.ivars().lift;
        lift.setShadowOpacity(opacity);
        lift.setShadowRadius(radius);
        lift.setShadowOffset(CGSize::new(0.0, offset));
    }

    /// Animate the shadow from `from` to its raised (or, when `settling`, resting) model.
    fn animate_lift_shadow(&self, from: (f32, f64, f64), duration: f64, settling: bool) {
        let lift = &self.ivars().lift;
        without_actions(|| self.set_lift_shadow(self.lift_shadow(!settling)));
        let (opacity, radius, offset) = from;
        let opacity = objc2_foundation::NSNumber::new_f32(opacity);
        let radius = objc2_foundation::NSNumber::new_f64(radius);
        let offset = objc2_foundation::NSValue::new::<CGSize>(CGSize::new(0.0, offset));
        animate_value(lift, "shadowOpacity", Some(&opacity), duration);
        animate_value(lift, "shadowRadius", Some(&radius), duration);
        animate_value(lift, "shadowOffset", Some(&offset), duration);
    }

    /// Bar length along the tab axis, bar thickness across it, and the length of one tab.
    /// Tabs run left to right in a horizontal bar and top to bottom in a vertical one.
    fn geometry(&self) -> (f64, f64, f64) {
        let size = self.bounds().size;
        let (length, thickness) = if self.ivars().vertical.get() {
            (size.height, size.width)
        } else {
            (size.width, size.height)
        };
        let length = length.max(0.0);
        let n = self.ivars().tabs.try_borrow().map(|tabs| tabs.len().max(1)).unwrap_or(1) as f64;
        (
            length,
            thickness.max(0.0),
            ((length - 2.0 * INSET - (n - 1.0) * GAP) / n).max(120.0),
        )
    }

    /// A rectangle given along (`main`) and across (`cross`) the tab axis.
    fn place(&self, main: f64, cross: f64, main_len: f64, cross_len: f64) -> CGRect {
        if self.ivars().vertical.get() {
            rect(cross, main, cross_len, main_len)
        } else {
            rect(main, cross, main_len, cross_len)
        }
    }

    fn clipped(&self, view: &NSView) -> bool {
        let f = view.frame();
        let (length, ..) = self.geometry();
        let (start, len) = if self.ivars().vertical.get() {
            (f.origin.y, f.size.height)
        } else {
            (f.origin.x, f.size.width)
        };
        start < 0.0 || start + len > length
    }

    fn index_at(&self, p: CGPoint) -> Option<usize> {
        let (width, thickness, tab_width) = self.geometry();
        let p = if self.ivars().vertical.get() {
            CGPoint::new(p.y, p.x)
        } else {
            p
        };
        if p.x < INSET || p.x >= width || p.y < INSET || p.y >= thickness - INSET {
            return None;
        }
        let x = p.x - INSET + self.ivars().offset.get();
        let i = (x / (tab_width + GAP)) as usize;
        (i < self.ivars().tabs.borrow().len() && x % (tab_width + GAP) < tab_width).then_some(i)
    }

    fn arrange(&self, reveal: bool, mut redraw: bool, animate: bool) {
        let s = self.ivars();
        let (width, thickness, tab_width) = self.geometry();
        let vertical = s.vertical.get();
        let Ok(mut tabs) = s.tabs.try_borrow_mut() else {
            return;
        };
        // Commit model geometry and its explicit animations together.
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        let max = (tabs.len() as f64 * (tab_width + GAP) - GAP + 2.0 * INSET - width).max(0.0);
        let mut offset = s.offset.get().clamp(0.0, max);
        if reveal && s.selected.get() < tabs.len() {
            let left = s.selected.get() as f64 * (tab_width + GAP);
            offset = offset.min(left).max(left + tab_width + 2.0 * INSET - width).clamp(0.0, max);
        }
        s.offset.set(offset);
        let viewport_frame = self.bounds();
        if s.viewport.frame() != viewport_frame {
            redraw = true;
            s.viewport.setFrame(viewport_frame);
            s.retiring.setFrame(viewport_frame);
        }
        let close_fill = s.close_fill.borrow();
        let drag = s.drag.get();
        let springy = s.springy.get() || drag.is_some();
        let mut lift_target = None;
        for (i, tab) in tabs.iter_mut().enumerate() {
            let active = i == s.selected.get();
            let hover = s.hovered.get() == Some(i);
            let lifted = s.lifting.get() == Some(i);
            let x = INSET + self.display_slot(i) as f64 * (tab_width + GAP) - offset;
            // The dragged tab follows the pointer rather than its slot.
            let held = drag.filter(|drag| drag.from == i).map(|drag| drag.main);
            let visual_x = held.unwrap_or(x);
            let button = tab.select.ns_button();
            let frame = self.place(x, INSET, tab_width, thickness - 2.0 * INSET);
            if button.frame() != frame {
                button.setFrame(frame);
            }
            let visual_frame = self.place(visual_x, 0.0, tab_width, thickness);
            if lifted {
                lift_target = Some(self.place(visual_x, INSET, tab_width, thickness - 2.0 * INSET));
            }
            let upward = s.upward.get();
            let vertical_changed = tab.visual.ivars().vertical.replace(vertical) != vertical;
            let upward_changed = tab.visual.ivars().upward.replace(upward) != upward;
            if vertical_changed || upward_changed {
                tab.visual.setNeedsDisplay(true);
            }
            if tab.visual.frame() != visual_frame {
                let layer = tab.visual.layer().unwrap();
                let animate = animate && held.is_none();
                let animation = animate.then(|| {
                    let from =
                        unsafe { layer.presentationLayer() }.unwrap_or_else(|| layer.retain());
                    (
                        from.bounds().size.width > 0.0 && from.bounds().size.height > 0.0,
                        layer_value(&from, "position"),
                        layer_value(&from, "bounds"),
                    )
                });
                without_actions(|| tab.visual.setFrame(visual_frame));
                if let Some((true, old_position, old_bounds)) = animation {
                    if springy {
                        spring_value(&layer, "position", old_position.as_deref());
                        spring_value(&layer, "bounds", old_bounds.as_deref());
                    } else {
                        animate_value(&layer, "position", old_position.as_deref(), 0.20);
                        animate_value(&layer, "bounds", old_bounds.as_deref(), 0.20);
                    }
                } else if !animate {
                    layer.removeAllAnimations();
                }
                tab.visual.setNeedsDisplay(true);
            }
            let style = if active || lifted {
                2
            } else if hover {
                1
            } else {
                0
            };
            let separator = self.separated(i);
            let style_changed = tab.visual.ivars().style.replace(style) != style;
            let separator_changed = tab.visual.ivars().separator.replace(separator) != separator;
            if style_changed || separator_changed {
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
            let close = CLOSE.min(thickness - 2.0 * INSET);
            // The close button leads the tab in reading order: left, top, or bottom.
            let lead = (thickness - close) / 2.0 - INSET + 2.0;
            let close_main = if vertical && s.upward.get() {
                x + tab_width - lead - close
            } else {
                x + lead
            };
            tab.close.ns_button().setFrame(self.place(
                close_main,
                (thickness - close) / 2.0,
                close,
                close,
            ));
            if let Some(layer) = tab.close.ns_view().layer() {
                layer.setCornerRadius(close / 2.0);
            }
            tab.close.set_hidden(!hover || self.clipped(tab.close.ns_view()));
            // Only visible tabs participate in native hit testing.
            tab.select.set_hidden(x + tab_width <= 0.0 || x >= width);
        }
        let target = tabs.get(s.selected.get()).map(|tab| tab.select.ns_view().frame());
        let layer = &s.highlight;
        let radius = (thickness / 2.0 - INSET).max(0.0);
        if layer.cornerRadius() != radius {
            without_actions(|| {
                layer.setCornerRadius(radius);
                s.lift.setCornerRadius(radius);
            });
        }
        if let Some(target) = lift_target.filter(|&target| target != s.lift.frame()) {
            let from = (animate && drag.is_none()).then(|| {
                let from = unsafe { s.lift.presentationLayer() }.unwrap_or_else(|| s.lift.retain());
                (layer_value(&from, "position"), layer_value(&from, "bounds"))
            });
            without_actions(|| s.lift.setFrame(target));
            if let Some((position, bounds)) = from {
                spring_value(&s.lift, "position", position.as_deref());
                spring_value(&s.lift, "bounds", bounds.as_deref());
            } else {
                s.lift.removeAnimationForKey(&NSString::from_str("position"));
                s.lift.removeAnimationForKey(&NSString::from_str("bounds"));
            }
        }
        // Concentric with the outer capsule, whose radius follows the bar's thickness.
        if let Some(outer) = self.layer() {
            outer.setCornerRadius(thickness / 2.0);
        }
        // The lift stands in for the thumb while the selected tab is carried; the thumb
        // waits, hidden, in the slot where it lands.
        let carried = s.lifting.get().is_some_and(|i| i == s.selected.get());
        let hidden = target.is_none() || carried;
        let was_hidden = layer.isHidden();
        let changed = target.is_some_and(|target| target != layer.frame());
        let animation = (animate && !was_hidden && !hidden && changed).then(|| {
            let from = unsafe { layer.presentationLayer() }.unwrap_or_else(|| layer.retain());
            (layer_value(&from, "position"), layer_value(&from, "bounds"))
        });
        without_actions(|| {
            if was_hidden != hidden {
                layer.setHidden(hidden);
            }
            if let Some(target) = target.filter(|_| changed) {
                layer.setFrame(target);
            }
        });
        if let Some((position, bounds)) = animation {
            if springy {
                spring_value(layer, "position", position.as_deref());
                spring_value(layer, "bounds", bounds.as_deref());
            } else {
                animate_value(layer, "position", position.as_deref(), 0.18);
                animate_value(layer, "bounds", bounds.as_deref(), 0.18);
            }
        } else if (!animate || hidden) && changed {
            layer.removeAllAnimations();
        }
        if redraw {
            self.setNeedsDisplay(true);
        }
        CATransaction::commit();
    }
}
// Concentric with the capsule: the outer radius equals the tab radius plus this inset.
const INSET: f64 = 3.0;
const GAP: f64 = 2.0;
const CLOSE: f64 = 15.0;
/// How far a pressed tab must travel before it lifts, as for AppKit drags.
const DRAG_THRESHOLD: f64 = 4.0;
struct Visual {
    labels: RefCell<Vec<(Retained<NSAttributedString>, CGSize)>>,
    icon: RefCell<Image>,
    style: Cell<usize>,
    separator: Cell<bool>,
    vertical: Cell<bool>,
    /// A vertical title reads bottom to top rather than top to bottom.
    upward: Cell<bool>,
}
define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsTabVisual"]
    #[ivars = Visual]
    struct TabVisual;
    impl TabVisual {
        #[unsafe(method(isFlipped))] fn flipped(&self) -> bool { true }
        // Vibrant labels pick up the wallpaper's hue; plain semantic colors stay neutral.
        #[unsafe(method(allowsVibrancy))] fn vibrant(&self) -> bool { false }
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
            let vertical = s.vertical.get();
            let size = self.bounds().size;
            // Work along the tab axis; a vertical bar swaps the two dimensions.
            let (length, thickness) = if vertical { (size.height, size.width) } else { (size.width, size.height) };
            let place = |main: f64, cross: f64, main_len: f64, cross_len: f64| {
                if vertical { rect(cross, main, cross_len, main_len) } else { rect(main, cross, main_len, cross_len) }
            };
            let radius = (thickness / 2.0 - INSET).max(0.0);
            if style == 1 {
                NSColor::labelColor().colorWithAlphaComponent(0.07).setFill();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(place(0.0, INSET, length, thickness - 2.0 * INSET), radius, radius).fill();
            }
            let scale = self.window().map_or(2.0, |window| window.backingScaleFactor()).max(1.0);
            let snap = |value: f64| (value * scale).round() / scale;
            if s.separator.get() {
                NSColor::separatorColor().setFill();
                let inset = (thickness / 2.0 - 6.0).max(INSET);
                NSBezierPath::fillRect(place(0.0, inset, 1.0 / scale, thickness - 2.0 * inset));
            }
            let icon = (thickness - 2.0 * INSET - 8.0).clamp(10.0, 14.0);
            // Resting tabs recede slightly so the selection reads at a glance.
            let fraction = if style == 2 { 1.0 } else { 0.85 };
            let icon_width = icon + 8.0;
            let text_width = labels[2].1.width.min((length - 44.0 - icon_width).max(0.0));
            let start = snap((length - text_width - icon_width) / 2.0);
            let options = NSStringDrawingOptions::UsesLineFragmentOrigin | NSStringDrawingOptions::TruncatesLastVisibleLine;
            // The icon stays upright and leads the title in reading order; the block is centered.
            let upward = vertical && s.upward.get();
            let icon_main = if upward { start + text_width + icon_width - icon } else { start };
            unsafe { s.icon.borrow().drawInRect_fromRect_operation_fraction_respectFlipped_hints(
                place(icon_main, snap((thickness - icon) / 2.0), icon, icon), CGRect::ZERO,
                NSCompositingOperation::SourceOver, fraction, true, None); }
            if !vertical {
                let text_y = snap((thickness - text_size.height) / 2.0);
                label.drawWithRect_options(rect(start + icon_width, text_y, text_width, text_size.height), options);
                return;
            }
            // Side titles turn so their tops face the screen edge: a left bar reads bottom
            // to top, a right bar top to bottom, as in IDE tool-window strips.
            NSGraphicsContext::saveGraphicsState_class();
            if let Some(context) = NSGraphicsContext::currentContext() {
                let cg = context.CGContext();
                let (origin, angle) = if upward {
                    (start + text_width, -std::f64::consts::FRAC_PI_2)
                } else {
                    (start + icon_width, std::f64::consts::FRAC_PI_2)
                };
                objc2_core_graphics::CGContext::translate_ctm(Some(&cg), snap(thickness / 2.0), origin);
                objc2_core_graphics::CGContext::rotate_ctm(Some(&cg), angle);
            }
            label.drawWithRect_options(rect(0.0, -text_size.height / 2.0, text_width, text_size.height), options);
            NSGraphicsContext::restoreGraphicsState_class();
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
        let to: Option<Retained<AnyObject>> = msg_send![layer, valueForKey: &*key];
        animation.setToValue(to.as_deref());
    }
    animation.setDuration(duration);
    animation.setTimingFunction(Some(&CAMediaTimingFunction::functionWithControlPoints(
        0.42, 0.0, 0.58, 1.0,
    )));
    layer.addAnimation_forKey(&animation, Some(&key));
}
/// A critically damped spring, as AppKit uses for content that follows the pointer.
fn spring_value(layer: &CALayer, key: &str, from: Option<&AnyObject>) {
    let key = NSString::from_str(key);
    let animation = CASpringAnimation::animationWithKeyPath(Some(&key));
    unsafe {
        animation.setFromValue(from);
        let to: Option<Retained<AnyObject>> = msg_send![layer, valueForKey: &*key];
        animation.setToValue(to.as_deref());
    }
    animation.setMass(1.0);
    animation.setStiffness(380.0);
    animation.setDamping(39.0);
    animation.setDuration(animation.settlingDuration());
    layer.addAnimation_forKey(&animation, Some(&key));
}
/// Force Touch trackpads tap; other hardware ignores the request.
fn haptic(pattern: NSHapticFeedbackPattern) {
    NSHapticFeedbackManager::defaultPerformer()
        .performFeedbackPattern_performanceTime(pattern, NSHapticFeedbackPerformanceTime::Now);
}
/// AppKit's scroll-edge resistance: travel grows ever more slowly toward a limit.
fn rubber_band(distance: f64) -> f64 {
    const LIMIT: f64 = 18.0;
    LIMIT * (1.0 - 1.0 / (distance * 0.55 / LIMIT + 1.0))
}
/// Where the tab at `index` ends up once the tab at `from` moves to `to`.
fn moved_index(index: usize, from: usize, to: usize) -> usize {
    if index == from {
        to
    } else if from < to && (from + 1..=to).contains(&index) {
        index - 1
    } else if to < from && (to..from).contains(&index) {
        index + 1
    } else {
        index
    }
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
                    // The selection is a raised thumb, as in a native segmented control:
                    // near-opaque in light, a lifted tone in dark, with a soft shadow.
                    let dark = dark(self);
                    s.highlight.setBackgroundColor(Some(
                        &if dark {
                            NSColor::colorWithWhite_alpha(1.0, if contrast { 0.30 } else { 0.20 })
                        } else {
                            NSColor::whiteColor()
                        }
                        .CGColor(),
                    ));
                    s.highlight.setBorderWidth(if contrast { 1.0 } else { 0.5 });
                    s.highlight.setBorderColor(Some(
                        &if contrast {
                            NSColor::labelColor()
                        } else if dark {
                            NSColor::colorWithWhite_alpha(1.0, 0.10)
                        } else {
                            NSColor::colorWithWhite_alpha(0.0, 0.10)
                        }
                        .CGColor(),
                    ));
                    s.highlight.setShadowColor(Some(&NSColor::blackColor().CGColor()));
                    s.highlight.setShadowOpacity(if opaque {
                        0.0
                    } else if dark {
                        0.3
                    } else {
                        0.2
                    });
                    s.highlight.setShadowRadius(if dark { 2.0 } else { 1.5 });
                    s.highlight.setShadowOffset(CGSize::new(0.0, 1.0));
                    s.lift.setBackgroundColor(s.highlight.backgroundColor().as_deref());
                    s.lift.setBorderWidth(s.highlight.borderWidth());
                    s.lift.setBorderColor(s.highlight.borderColor().as_deref());
                    s.lift.setShadowColor(s.highlight.shadowColor().as_deref());
                    self.set_lift_shadow(self.lift_shadow(s.lifting.get().is_some()));
                })
            }));
    }

    fn stop_motion(&self) {
        self.cancel_drag();
        self.ivars().highlight.removeAllAnimations();
        for tab in self.ivars().tabs.borrow().iter() {
            tab.visual.layer().unwrap().removeAllAnimations();
        }
        for view in self.ivars().retiring.subviews() {
            view.removeFromSuperview();
        }
    }
}
fn dark(view: &NSView) -> bool {
    let names = objc2_foundation::NSArray::from_slice(&[unsafe { NSAppearanceNameAqua }, unsafe {
        NSAppearanceNameDarkAqua
    }]);
    view.effectiveAppearance()
        .bestMatchFromAppearancesWithNames(&names)
        .is_some_and(|name| &*name == unsafe { NSAppearanceNameDarkAqua })
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
/// One native tab strip, optionally hosted in a panel. Updates preserve unchanged controls.
pub struct TabBar {
    panel: Option<Retained<TabPanel>>,
    content: Retained<TabContent>,
    surface: Box<dyn NativeView>,
}
impl TabBar {
    /// Default thickness; any thickness from about 20 points works.
    pub const HEIGHT: f64 = 28.0;

    pub fn new(ui: &Ui) -> Self { Self::build(ui, false) }

    /// The same native tab strip hosted inside an existing view hierarchy.
    /// Allocates no panel or window; use NativeView and set_frame for layout.
    pub fn embedded(ui: &Ui) -> Self { Self::build(ui, true) }

    fn build(ui: &Ui, embedded: bool) -> Self {
        let action: Action = Rc::new(ActionState {
            callback: RefCell::new(None),
            alive: Cell::new(true),
        });
        let viewport: Retained<TabViewport> =
            unsafe { msg_send![super(TabViewport::alloc(ui.mtm()).set_ivars(())), init] };
        let viewport = viewport.into_super();
        viewport.setWantsLayer(true);
        if let Some(layer) = viewport.layer() {
            layer.setMasksToBounds(true);
        }
        let highlight = CALayer::new();
        highlight.setCornerRadius(TabBar::HEIGHT / 2.0 - INSET);
        highlight.setCornerCurve(unsafe { objc2_quartz_core::kCACornerCurveContinuous });
        highlight.setHidden(true);
        let lift = CALayer::new();
        lift.setCornerRadius(TabBar::HEIGHT / 2.0 - INSET);
        lift.setCornerCurve(unsafe { objc2_quartz_core::kCACornerCurveContinuous });
        lift.setZPosition(1.0);
        lift.setHidden(true);
        if let Some(layer) = viewport.layer() {
            layer.addSublayer(&lift);
        }
        let retiring: Retained<TabVisual> = unsafe {
            msg_send![
                super(TabVisual::alloc(ui.mtm()).set_ivars(Visual {
                    labels: RefCell::new(Vec::new()),
                    icon: RefCell::new(placeholder()),
                    style: Cell::new(0),
                    separator: Cell::new(false),
                    vertical: Cell::new(false),
                    upward: Cell::new(false),
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
                    vertical: Cell::new(false),
                    upward: Cell::new(false),
                    select_on_hover: Cell::new(false),
                    action,
                    viewport,
                    highlight,
                    press: Cell::new(None),
                    drag: Cell::new(None),
                    lift,
                    lifting: Cell::new(None),
                    lift_epoch: Cell::new(0),
                    springy: Cell::new(false),
                    retiring,
                    fonts: {
                        [
                            NSFont::systemFontOfSize_weight(12.0, NSFontWeightRegular),
                            NSFont::systemFontOfSize_weight(12.0, NSFontWeightMedium),
                        ]
                    },
                    text_colors: [NSColor::secondaryLabelColor(), NSColor::labelColor()],
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
        let panel = (!embedded).then(|| {
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
            panel
        });
        content.update_material();
        Self { panel, content, surface }
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
                drop(tabs);
                self.set_selected_index(selected);
                return;
            }
        }
        // A held or landing tab keeps its place through relabelling, such as a terminal's
        // animated title. Only a change in the tabs themselves ends the drag.
        let held = s.drag.get().is_some() || s.lifting.get().is_some();
        if held && s.tabs.borrow().len() == items.len() {
            let mut tabs = s.tabs.borrow_mut();
            for (tab, item) in tabs.iter_mut().zip(items) {
                self.update_item(tab, item);
            }
            drop(tabs);
            self.set_selected_index(selected);
            return;
        }
        self.content.cancel_drag();
        let animate = self.motion_enabled();
        let count_changed = s.tabs.borrow().len() != items.len();
        for view in s.retiring.subviews() {
            view.removeFromSuperview();
        }
        let mut tabs = s.tabs.borrow_mut();
        let mut old: Vec<_> = tabs.drain(..).map(Some).collect();
        let mut first_remaining = 0;
        for (i, item) in items.into_iter().enumerate() {
            while first_remaining < old.len() && old[first_remaining].is_none() {
                first_remaining += 1;
            }
            let existing = old[first_remaining..]
                .iter()
                .position(|tab| {
                    tab.as_ref().is_some_and(|tab| tab.title == item.title && tab.icon == item.icon)
                })
                .map(|index| first_remaining + index);
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
                            separator: Cell::new(false),
                            vertical: Cell::new(false),
                            upward: Cell::new(false),
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
            self.update_item(tab, item);
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

    /// Bring a tab's label, tooltip, accessibility and icon up to date with its item.
    fn update_item(&self, tab: &mut Tab, item: TabBarItem) {
        let s = self.content.ivars();
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
                            &*s.text_colors[usize::from(style != 0)] as &AnyObject,
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

    /// Update selection without replacing or cloning the tab items.
    /// An index outside the tab list leaves no tab selected.
    pub fn set_selected_index(&self, selected: usize) {
        let s = self.content.ivars();
        if s.selected.replace(selected) == selected {
            return;
        }
        // arrange updates selection styling and separators, and reveals overflow.
        self.content.arrange(true, false, self.motion_enabled());
    }

    fn motion_enabled(&self) -> bool { self.content.motion_enabled() }

    /// Select a tab as soon as the pointer enters it, rather than on click.
    pub fn set_select_on_hover(&self, enabled: bool) {
        self.content.ivars().select_on_hover.set(enabled);
    }

    /// Which side of its windows the bar sits on. Left and right bars run top to bottom.
    pub fn set_edge(&self, edge: TabBarEdge) {
        let s = self.content.ivars();
        let vertical = matches!(edge, TabBarEdge::Left | TabBarEdge::Right);
        let upward = edge == TabBarEdge::Left;
        let turned = s.vertical.replace(vertical) != vertical;
        if s.upward.replace(upward) != upward || turned {
            self.content.stop_motion();
            s.offset.set(0.0);
            self.content.arrange(true, true, false);
        }
    }

    pub fn set_frame(&self, frame: CGRect) {
        let previous = self
            .panel
            .as_ref()
            .map_or_else(|| self.surface.ns_view().frame(), |panel| panel.frame());
        if previous == frame {
            return;
        }
        let resized = previous.size != frame.size;
        if resized {
            self.content.stop_motion();
            self.set_corner_radius(frame.size.width.min(frame.size.height) / 2.0);
        }
        if let Some(panel) = &self.panel {
            panel.setFrame_display(frame, true);
        } else {
            self.surface.ns_view().setFrame(frame);
            self.surface.ns_view().layoutSubtreeIfNeeded();
        }
        if resized {
            self.content.arrange(true, true, false);
        }
    }

    fn set_corner_radius(&self, radius: f64) {
        let view = self.surface.ns_view();
        if objc2::runtime::AnyClass::get(c"NSGlassEffectView").is_some()
            && let Some(glass) = view.downcast_ref::<NSGlassEffectView>()
        {
            glass.setCornerRadius(radius);
        } else if let Some(layer) = view.layer() {
            layer.setCornerRadius(radius);
        }
    }

    /// Reassert relative order: the anchor may have been raised since the previous call.
    pub fn show_above(&self, number: isize) {
        if number > 0
            && let Some(panel) = &self.panel
        {
            panel.orderWindow_relativeTo(NSWindowOrderingMode::Above, number);
        }
    }

    pub fn hide(&self) {
        self.content.stop_motion();
        if let Some(panel) = &self.panel {
            if panel.isVisible() {
                panel.orderOut(None);
            }
        } else {
            self.surface.set_hidden(true);
        }
    }

    pub fn is_visible(&self) -> bool {
        self.panel.as_ref().map_or_else(
            || {
                !self.surface.ns_view().isHiddenOrHasHiddenAncestor()
                    && self.surface.ns_view().window().is_some_and(|window| window.isVisible())
            },
            |panel| panel.isVisible(),
        )
    }

    /// The owner of a windowed strip. Embedded strips have no panel.
    pub fn ns_panel(&self) -> &NSPanel {
        self.panel.as_deref().expect("embedded tab strips have no panel")
    }
}
impl NativeView for TabBar {
    fn ns_view(&self) -> &NSView { self.surface.ns_view() }
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
        if let Some(panel) = &self.panel {
            panel.setContentView(None);
            panel.close();
        }
    }
}
