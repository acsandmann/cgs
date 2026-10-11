//! A nonactivating native glass strip. All objects are confined to the AppKit thread.
//! Full-strip appearance follows ZimengXiong/tabbed's Theme, TabItemView and TabStripView
//! at https://github.com/ZimengXiong/tabbed/tree/9481b628767921bd1af1b90c4d7448282c9757b6/Sources/UI.
//! The native AppKit material replaces its SwiftUI glassEffect; no SwiftUI is hosted.
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::*;
use objc2_foundation::{NSAttributedString, NSDictionary, NSString};

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
    labels: Vec<(Retained<NSAttributedString>, CGSize)>,
    path: Option<Retained<NSBezierPath>>,
}
struct Content {
    tabs: RefCell<Vec<Tab>>,
    selected: Cell<usize>,
    hovered: Cell<Option<usize>>,
    close_hovered: Cell<Option<usize>>,
    offset: Cell<f64>,
    action: Action,
    viewport: Retained<NSView>,
    clip: RefCell<Option<Retained<NSBezierPath>>>,
    fonts: [Retained<NSFont>; 2],
    text_colors: [Retained<NSColor>; 3],
    paragraph: Retained<NSMutableParagraphStyle>,
    close_symbol: Option<Image>,
    placeholder: Image,
    close_colors: [Retained<NSColor>; 2],
    close_fill: Retained<NSColor>,
    fills: [Retained<NSColor>; 3],
    strokes: [Retained<NSColor>; 2],
    ui: Ui,
}
define_class!(
    #[unsafe(super(NSPanel))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsTabPanel"]
    struct TabPanel;
    impl TabPanel {
        #[unsafe(method(canBecomeKey))] fn key(&self) -> bool { false }
        #[unsafe(method(canBecomeMain))] fn main(&self) -> bool { false }
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
            self.setNeedsDisplay(true);
        }
        #[unsafe(method(drawRect:))] fn draw_tabs(&self, _: CGRect) {
            let s = self.ivars();
            NSGraphicsContext::saveGraphicsState_class();
            if let Some(clip) = s.clip.borrow().as_ref() { clip.addClip(); }
            let Ok(tabs) = s.tabs.try_borrow() else { NSGraphicsContext::restoreGraphicsState_class(); return; };
            for (i, tab) in tabs.iter().enumerate() {
                if tab.select.ns_view().isHidden() { continue; }
                let active = s.selected.get() == i;
                let style = if active { 2 } else if s.hovered.get() == Some(i) { 1 } else { 0 };
                if let Some(path) = &tab.path {
                    s.fills[style].setFill(); path.fill();
                    s.strokes[usize::from(active)].setStroke(); path.stroke();
                }
                let frame = tab.select.ns_view().frame();
                let Some((label, text_size)) = tab.labels.get(style) else { continue; };
                let icon_width = 20.0;
                let text_width = text_size.width.min((frame.size.width - 44.0 - icon_width).max(0.0));
                let x = frame.origin.x + (frame.size.width - text_width - icon_width) / 2.0;
                let icon = tab.icon.as_ref().unwrap_or(&s.placeholder);
                unsafe { icon.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
                    rect(x,7.0,14.0,14.0), CGRect::ZERO, NSCompositingOperation::SourceOver, 1.0, true, None); }
                label.drawWithRect_options(rect(x + icon_width, (28.0 - text_size.height)/2.0, text_width, text_size.height),
                    NSStringDrawingOptions::UsesLineFragmentOrigin | NSStringDrawingOptions::TruncatesLastVisibleLine);
            }
            NSGraphicsContext::restoreGraphicsState_class();
        }
        #[unsafe(method(isFlipped))] fn flipped(&self) -> bool { true }
        #[unsafe(method(layout))] fn layout_tabs(&self) {
            unsafe { let _: () = msg_send![super(self), layout]; }
            self.arrange(false);
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
            let changed = self.ivars().hovered.replace(hover) != hover;
            let close_changed = self.ivars().close_hovered.replace(close) != close;
            if changed || close_changed { self.arrange(false); }
        }
        #[unsafe(method(mouseExited:))] fn exited(&self, _: &NSEvent) {
            let hover = self.ivars().hovered.take();
            let close = self.ivars().close_hovered.take();
            if hover.is_some() || close.is_some() { self.arrange(false); }
        }
        #[unsafe(method(scrollWheel:))] fn scroll(&self, event: &NSEvent) {
            let s = self.ivars();
            let delta = if event.scrollingDeltaX().abs() > event.scrollingDeltaY().abs() { event.scrollingDeltaX() } else { event.scrollingDeltaY() };
            s.offset.set(s.offset.get() - delta);
            self.arrange(false);
        }
    }
);
impl TabContent {
    fn geometry(&self) -> (f64, f64) {
        let width = self.bounds().size.width.max(0.0);
        let n = self.ivars().tabs.try_borrow().map(|tabs| tabs.len().max(1)).unwrap_or(1) as f64;
        (width, ((width - 4.0 - (n - 1.0) * 3.0) / n).max(120.0))
    }

    fn index_at(&self, p: CGPoint) -> Option<usize> {
        let (width, tab_width) = self.geometry();
        if p.x < 2.0 || p.x >= width || p.y < 2.0 || p.y >= 26.0 {
            return None;
        }
        let x = p.x - 2.0 + self.ivars().offset.get();
        let i = (x / (tab_width + 3.0)) as usize;
        (i < self.ivars().tabs.borrow().len() && x % (tab_width + 3.0) < tab_width).then_some(i)
    }

    fn arrange(&self, reveal: bool) {
        let s = self.ivars();
        let (width, tab_width) = self.geometry();
        let Ok(mut tabs) = s.tabs.try_borrow_mut() else {
            return;
        };
        let max = (tabs.len() as f64 * (tab_width + 3.0) + 1.0 - width).max(0.0);
        let mut offset = s.offset.get().clamp(0.0, max);
        if reveal && s.selected.get() < tabs.len() {
            let left = s.selected.get() as f64 * (tab_width + 3.0);
            offset = offset.min(left).max(left + tab_width + 4.0 - width).clamp(0.0, max);
        }
        s.offset.set(offset);
        let viewport_frame = rect(0.0, 0.0, width, 28.0);
        if s.viewport.frame() != viewport_frame || s.clip.borrow().is_none() {
            s.viewport.setFrame(viewport_frame);
            *s.clip.borrow_mut() = Some(NSBezierPath::bezierPathWithRect(viewport_frame));
        }
        for (i, tab) in tabs.iter_mut().enumerate() {
            let active = i == s.selected.get();
            let hover = s.hovered.get() == Some(i);
            let x = 2.0 + i as f64 * (tab_width + 3.0) - offset;
            let button = tab.select.ns_button();
            let frame = rect(x, 2.0, tab_width, 24.0);
            if button.frame() != frame || tab.path.is_none() {
                button.setFrame(frame);
                let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    rect(x, 2.0, tab_width, 24.0),
                    12.0,
                    12.0,
                );
                path.setLineWidth(1.0);
                tab.path = Some(path);
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
                        &s.close_fill
                    } else {
                        &s.fills[0]
                    }
                    .CGColor(),
                ));
            }
            tab.close.ns_button().setContentTintColor(Some(
                &s.close_colors[usize::from(s.close_hovered.get() == Some(i))],
            ));
            tab.close.ns_button().setFrame(rect(x + 8.0, 6.5, 15.0, 15.0));
            tab.close.set_hidden(!hover || x + 8.0 < 0.0 || x + 23.0 > width);
            // Only visible tabs participate in native hit testing.
            tab.select.set_hidden(x + tab_width <= 0.0 || x >= width);
        }
        self.setNeedsDisplay(true);
    }
}
#[allow(deprecated)]
fn placeholder() -> Image {
    let image = NSImage::initWithSize(NSImage::alloc(), CGSize::new(14.0, 14.0));
    image.lockFocusFlipped(true);
    NSColor::colorWithWhite_alpha(1.0, 0.20).setFill();
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
        let paragraph = NSMutableParagraphStyle::new();
        paragraph.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
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
                    clip: RefCell::new(None),
                    fonts: {
                        [
                            NSFont::systemFontOfSize_weight(12.0, NSFontWeightMedium),
                            NSFont::systemFontOfSize_weight(12.0, NSFontWeightSemibold),
                        ]
                    },
                    text_colors: [0.55, 0.80, 0.95].map(|a| NSColor::colorWithWhite_alpha(1.0, a)),
                    paragraph,
                    close_symbol: crate::Symbol::named("xmark").and_then(|image| {
                        image.imageWithSymbolConfiguration(
                            &NSImageSymbolConfiguration::configurationWithPointSize_weight(
                                8.0,
                                NSFontWeightBold,
                            ),
                        )
                    }),
                    placeholder: placeholder(),
                    close_colors: [0.60, 0.95].map(|a| NSColor::colorWithWhite_alpha(1.0, a)),
                    close_fill: NSColor::colorWithWhite_alpha(1.0, 0.18),
                    fills: [0.0, 0.10, 0.16].map(|a| NSColor::colorWithWhite_alpha(1.0, a)),
                    strokes: [0.08, 0.22].map(|a| NSColor::colorWithWhite_alpha(1.0, a)),
                    ui: *ui,
                })),
                init
            ]
        };
        content.setAccessibilityElement(true);
        content.setAccessibilityRole(Some(unsafe { NSAccessibilityTabGroupRole }));
        content.setAccessibilityLabel(Some(&NSString::from_str("Window tabs")));
        content.addSubview(&content.ivars().viewport);
        content.setWantsLayer(true);
        if let Some(layer) = content.layer() {
            layer.setMasksToBounds(true);
        }
        let host = View::new(ui).content(content.retain().into_super(), crate::uniform_insets(0.0));
        // Tabbed puts black backing inside the regular glass, rather than tinting
        // the material. Keep these separate: NSGlassEffectView renders live glass.
        let backing = host.ns_view().retain();
        backing.setWantsLayer(true);
        let glass = GlassEffectView::new(ui, host)
            .corner_radius(14.0)
            .style(crate::GlassStyle::Regular);
        let has_glass = glass.ns_glass_effect_view().is_some();
        if let Some(layer) = backing.layer() {
            layer.setCornerRadius(14.0);
            layer.setMasksToBounds(true);
            layer.setBackgroundColor(Some(
                &NSColor::colorWithWhite_alpha(0.0, if has_glass { 0.45 } else { 0.18 }).CGColor(),
            ));
        }
        let surface: Box<dyn NativeView> = if has_glass {
            Box::new(glass)
        } else {
            let fallback = VisualEffect::new(ui, NSVisualEffectMaterial::HUDWindow, glass);
            fallback.ns_visual_effect_view().setState(NSVisualEffectState::Active);
            fallback
                .ns_visual_effect_view()
                .setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            fallback.ns_view().setWantsLayer(true);
            if let Some(layer) = fallback.ns_view().layer() {
                layer.setCornerRadius(14.0);
                layer.setMasksToBounds(true);
                layer.setBorderWidth(1.0);
                layer.setBorderColor(Some(&NSColor::colorWithWhite_alpha(1.0, 0.10).CGColor()));
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
        panel.setHasShadow(true);
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setLevel(NSNormalWindowLevel);
        panel.setHidesOnDeactivate(false);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        panel.setAppearance(
            NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }).as_deref(),
        );
        panel.setContentView(Some(surface.ns_view()));
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
        if s.selected.get() == selected {
            let tabs = s.tabs.borrow();
            if tabs.len() == items.len()
                && tabs
                    .iter()
                    .zip(&items)
                    .all(|(tab, item)| tab.title == item.title && tab.icon == item.icon)
            {
                return;
            }
        }
        let mut tabs = s.tabs.borrow_mut();
        while tabs.len() > items.len() {
            let tab = tabs.pop().unwrap();
            tab.select.ns_view().removeFromSuperview();
            tab.close.ns_view().removeFromSuperview();
        }
        for (i, item) in items.into_iter().enumerate() {
            if i == tabs.len() {
                let action = s.action.clone();
                let select = Button::new(&s.ui, "")
                    .borderless()
                    .on_click(move || emit(&action, TabBarAction::Select(i)));
                select.ns_button().setTransparent(true);
                select
                    .ns_button()
                    .setAccessibilityRole(Some(unsafe { NSAccessibilityRadioButtonRole }));
                let action = s.action.clone();
                let close = Button::new(&s.ui, "")
                    .borderless()
                    .symbol("xmark")
                    .on_click(move || emit(&action, TabBarAction::Close(i)));
                close.accessibility_label("Close window");
                close.ns_button().setBezelStyle(NSBezelStyle::FlexiblePush);
                close.ns_button().setImage(s.close_symbol.as_deref());
                close.ns_button().setContentTintColor(Some(&s.close_colors[0]));
                close.ns_view().setWantsLayer(true);
                if let Some(layer) = close.ns_view().layer() {
                    layer.setCornerRadius(7.5);
                }
                s.viewport.addSubview(select.ns_view());
                s.viewport.addSubview(close.ns_view());
                tabs.push(Tab {
                    select,
                    close,
                    title: String::new(),
                    icon: None,
                    labels: Vec::new(),
                    path: None,
                });
            }
            let tab = &mut tabs[i];
            if tab.title != item.title || tab.labels.is_empty() {
                tab.select.set_title(&item.title);
                tab.labels = (0..3)
                    .map(|style| {
                        let attrs = NSDictionary::from_slices(
                            &[
                                unsafe { NSFontAttributeName },
                                unsafe { NSForegroundColorAttributeName },
                                unsafe { NSParagraphStyleAttributeName },
                            ],
                            &[
                                &*s.fonts[usize::from(style == 2)] as &AnyObject,
                                &*s.text_colors[style] as &AnyObject,
                                &*s.paragraph as &AnyObject,
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
                tab.title = item.title;
            }
            if tab.icon != item.icon {
                tab.icon = item.icon;
            }
        }
        let count = tabs.len();
        drop(tabs);
        s.selected.set(selected);
        if s.hovered.get().is_some_and(|index| index >= count) {
            s.hovered.set(None);
            s.close_hovered.set(None);
        }
        self.content.arrange(true);
    }

    pub fn set_frame(&self, frame: CGRect) {
        if self.panel.frame() != frame {
            self.panel.setFrame_display(frame, true);
            self.content.arrange(true);
        }
    }

    /// Reassert relative order: the anchor may have been raised since the previous call.
    pub fn show_above(&self, number: isize) {
        if number > 0 {
            self.panel.orderWindow_relativeTo(NSWindowOrderingMode::Above, number);
        }
    }

    pub fn hide(&self) {
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
        let action = &self.content.ivars().action;
        action.alive.set(false);
        if let Ok(mut callback) = action.callback.try_borrow_mut() {
            callback.take();
        }
        self.panel.setContentView(None);
        self.panel.close();
    }
}
