use objc2::Message;
use objc2::rc::Retained;
use objc2_app_kit::NSView;

use crate::layout::{Divider, ScrollView, VStack};
use crate::text::{SecondaryLabel, Title};
use crate::view::{Metrics, NativeView, View};
use crate::{Insets, Ui};

pub struct SettingsPage {
    ui: Ui,
    scroll: ScrollView,
    content: VStack,
    top: Retained<objc2_app_kit::NSLayoutConstraint>,
    root: View,
    scroll_bottom: Retained<objc2_app_kit::NSLayoutConstraint>,
    width: Retained<objc2_app_kit::NSLayoutConstraint>,
    footer: Option<Box<dyn NativeView>>,
}
impl SettingsPage {
    pub fn new(ui: &Ui, title: &str) -> Self {
        let content = VStack::new(ui).spacing(Metrics::SECTION_SPACING).insets(Insets {
            top: Metrics::PAGE_TOP_INSET,
            left: Metrics::PAGE_INSET,
            bottom: Metrics::PAGE_INSET,
            right: Metrics::PAGE_INSET,
        });
        if !title.is_empty() {
            content.add(Title::new(ui, title));
        }
        let outer = View::flipped(ui);
        outer.ns_view().addSubview(content.ns_view());
        crate::view::prepare(content.ns_view());
        let parent = outer.ns_view();
        let child = content.ns_view();
        let top = child.topAnchor().constraintEqualToAnchor(&parent.topAnchor());
        top.setActive(true);
        child
            .bottomAnchor()
            .constraintEqualToAnchor(&parent.bottomAnchor())
            .setActive(true);
        child
            .centerXAnchor()
            .constraintEqualToAnchor(&parent.centerXAnchor())
            .setActive(true);
        child
            .widthAnchor()
            .constraintLessThanOrEqualToAnchor(&parent.widthAnchor())
            .setActive(true);
        let width = child.widthAnchor().constraintLessThanOrEqualToConstant(640.0);
        // Settings content follows the window unless an application opts into a cap.
        width.setActive(false);
        let fill = child.widthAnchor().constraintEqualToAnchor(&parent.widthAnchor());
        fill.setPriority(750.0);
        fill.setActive(true);
        let scroll = ScrollView::new(ui, outer);
        scroll.ns_scroll_view().setDrawsBackground(true);
        // Give the detail pane a subtle tint so its boundary with the white
        // navigation sidebar remains visible even without a prominent divider.
        scroll.ns_scroll_view().setDrawsBackground(false);
        scroll.fit_width();
        let root = View::settings_color(ui, 0);
        root.ns_view().addSubview(scroll.ns_view());
        crate::view::prepare(scroll.ns_view());
        let parent = root.ns_view();
        let child = scroll.ns_view();
        child.topAnchor().constraintEqualToAnchor(&parent.topAnchor()).setActive(true);
        child
            .leadingAnchor()
            .constraintEqualToAnchor(&parent.leadingAnchor())
            .setActive(true);
        child
            .trailingAnchor()
            .constraintEqualToAnchor(&parent.trailingAnchor())
            .setActive(true);
        let scroll_bottom = child.bottomAnchor().constraintEqualToAnchor(&parent.bottomAnchor());
        scroll_bottom.setActive(true);
        Self {
            ui: *ui,
            scroll,
            content,
            top,
            root,
            scroll_bottom,
            width,
            footer: None,
        }
    }

    /// Let a collection editor own the viewport; its table supplies scrolling.
    pub fn into_editor(self) -> EditorPage {
        self.content.ns_stack_view().setSpacing(Metrics::CONTROL_SPACING);
        let native = self.scroll.content_view().retain();
        self.scroll.ns_scroll_view().setDocumentView(None);
        native.removeFromSuperview();
        self.top.setActive(false);
        self.content
            .ns_view()
            .topAnchor()
            .constraintEqualToAnchor(&native.safeAreaLayoutGuide().topAnchor())
            .setActive(true);
        EditorPage { native, _content: self.content }
    }

    /// Adjust the rhythm of a compact visual editor without changing other panes.
    pub fn content_spacing(self, spacing: f64) -> Self {
        self.content.ns_stack_view().setSpacing(spacing);
        self
    }

    /// Collections and visual editors need more room than short preference forms.
    pub fn content_width(self, width: f64) -> Self {
        self.width.setConstant(width);
        self.width.setActive(true);
        self
    }

    pub fn subtitle(self, text: &str) -> Self {
        self.content.add(SecondaryLabel::new(&self.ui, text));
        self
    }

    /// Keep a page's title and explanation together, with the same rhythm in every pane.
    pub fn introduction(self, title: &str, description: &str) -> Self {
        self.content.add(
            VStack::new(&self.ui)
                .spacing(5.0)
                .push(Title::new(&self.ui, title))
                .push(SecondaryLabel::new(&self.ui, description)),
        );
        self
    }

    pub fn section(self, section: impl NativeView) -> Self {
        self.content.add(section);
        self
    }

    /// Keep collection actions reachable while the page content scrolls.
    pub fn bottom_bar(mut self, content: impl NativeView) -> Self {
        if let Some(old) = self.footer.take() {
            old.ns_view().removeFromSuperview();
        }
        self.scroll_bottom.setActive(false);
        let row = VStack::new(&self.ui)
            .insets(Insets {
                top: 6.0,
                left: Metrics::PAGE_INSET,
                bottom: 6.0,
                right: Metrics::PAGE_INSET,
            })
            .push(content);
        let footer = VStack::new(&self.ui).spacing(0.0).push(Divider::new(&self.ui)).push(row);
        let view = footer.ns_view();
        crate::view::prepare(view);
        self.root.ns_view().addSubview(view);
        view.bottomAnchor()
            .constraintEqualToAnchor(&self.root.ns_view().safeAreaLayoutGuide().bottomAnchor())
            .setActive(true);
        view.centerXAnchor()
            .constraintEqualToAnchor(&self.root.ns_view().centerXAnchor())
            .setActive(true);
        view.widthAnchor()
            .constraintLessThanOrEqualToAnchor(&self.root.ns_view().widthAnchor())
            .setActive(true);
        let fill = view.widthAnchor().constraintEqualToAnchor(&self.root.ns_view().widthAnchor());
        fill.setPriority(750.0);
        fill.setActive(true);
        self.scroll
            .ns_view()
            .bottomAnchor()
            .constraintEqualToAnchor(&view.topAnchor())
            .setActive(true);
        self.footer = Some(Box::new(footer));
        self
    }

    pub fn ns_scroll_view(&self) -> &objc2_app_kit::NSScrollView { self.scroll.ns_scroll_view() }
}
impl NativeView for SettingsPage {
    fn ns_view(&self) -> &NSView { self.root.ns_view() }
}

pub struct EditorPage {
    native: Retained<NSView>,
    _content: VStack,
}
impl NativeView for EditorPage {
    fn ns_view(&self) -> &NSView { &self.native }
}
