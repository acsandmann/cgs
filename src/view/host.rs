use std::cell::RefCell;
use std::rc::Rc;

use objc2::Message;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSControl, NSLayoutConstraint, NSView, NSViewController,
};

use super::prepare;
use crate::Ui;
use crate::view::{NativeView, View};

pub struct PageHost {
    view: View,
    controller: Retained<NSViewController>,
    page: RefCell<Option<Box<dyn NativeView>>>,
    /// Owner of the current page when it was shown with `set_cached_page`.
    cached: RefCell<Option<std::rc::Weak<dyn NativeView>>>,
    parked: RefCell<Vec<Parked>>,
    content_constraints: Option<RefCell<Vec<Retained<NSLayoutConstraint>>>>,
}
/// A caller-cached page left mounted but hidden. Detaching a page from its window discards
/// its solved constraints, so revisiting it would cost a full layout pass; parked pages also
/// stop autoresizing so window resizes skip them. Unmounted once the caller drops the page.
struct Parked {
    owner: std::rc::Weak<dyn NativeView>,
    view: Retained<NSView>,
    controller: Option<Retained<NSViewController>>,
}
impl PageHost {
    pub fn new(ui: &Ui) -> Self {
        let view = View::new(ui);
        let controller = NSViewController::new(ui.mtm());
        controller.setView(view.ns_view());
        Self {
            view,
            controller,
            page: RefCell::new(None),
            cached: RefCell::new(None),
            parked: RefCell::new(Vec::new()),
            content_constraints: None,
        }
    }

    /// An inline host whose current content determines its Auto Layout height.
    /// Cached content is retained, but only the visible content constrains the host.
    pub fn content_sized(ui: &Ui) -> Self {
        let mut host = Self::new(ui);
        host.content_constraints = Some(RefCell::new(Vec::new()));
        host
    }

    pub fn ns_view_controller(&self) -> &NSViewController { &self.controller }

    fn is_current(&self, view: &NSView) -> bool {
        self.page
            .borrow()
            .as_ref()
            .is_some_and(|current| std::ptr::eq(current.ns_view(), view))
    }

    /// Show page-owned content; it is unmounted and released when replaced.
    pub fn set_page(&self, page: impl NativeView) {
        if self.is_current(page.ns_view()) {
            return;
        }
        self.leave();
        self.mount(page.ns_view(), page.view_controller());
        *self.page.borrow_mut() = Some(Box::new(page));
        self.prune();
    }

    /// Show a page the caller keeps cached. Leaving it parks it hidden, so returning only
    /// unhides it and preserves its layout, scroll position and editing state.
    pub fn set_cached_page(&self, page: Rc<dyn NativeView>) {
        if self.is_current(page.ns_view()) {
            return;
        }
        self.leave();
        let index =
            self.parked.borrow().iter().position(|p| std::ptr::eq(&*p.view, page.ns_view()));
        if let Some(index) = index {
            self.parked.borrow_mut().remove(index);
            self.fill(page.ns_view());
            page.ns_view().setHidden(false);
        } else {
            self.mount(page.ns_view(), page.view_controller());
        }
        *self.cached.borrow_mut() = Some(Rc::downgrade(&page));
        *self.page.borrow_mut() = Some(Box::new(page));
        self.prune();
    }

    fn fill(&self, view: &NSView) {
        if let Some(constraints) = &self.content_constraints {
            prepare(view);
            let parent = self.view.ns_view();
            let edges = vec![
                view.topAnchor().constraintEqualToAnchor(&parent.topAnchor()),
                view.bottomAnchor().constraintEqualToAnchor(&parent.bottomAnchor()),
                view.leadingAnchor().constraintEqualToAnchor(&parent.leadingAnchor()),
                view.trailingAnchor().constraintEqualToAnchor(&parent.trailingAnchor()),
            ];
            for edge in &edges {
                edge.setActive(true);
            }
            *constraints.borrow_mut() = edges;
            return;
        }
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        view.setFrame(self.view.ns_view().bounds());
    }

    fn mount(&self, view: &NSView, controller: Option<&NSViewController>) {
        if let Some(controller) = controller {
            self.controller.addChildViewController(controller);
        }
        view.setTranslatesAutoresizingMaskIntoConstraints(true);
        self.view.ns_view().addSubview(view);
        self.fill(view);
    }

    fn unmount(view: &NSView, controller: Option<&NSViewController>) {
        view.removeFromSuperview();
        if let Some(controller) = controller {
            controller.removeFromParentViewController();
        }
    }

    fn leave(&self) {
        self.release_content_constraints();
        let Some(page) = self.page.borrow_mut().take() else {
            return;
        };
        match self.cached.borrow_mut().take() {
            Some(owner) => {
                let view = page.ns_view();
                view.setHidden(true);
                view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewNotSizable);
                self.parked.borrow_mut().push(Parked {
                    owner,
                    view: view.retain(),
                    controller: page.view_controller().map(|controller| controller.retain()),
                });
            }
            None => Self::unmount(page.ns_view(), page.view_controller()),
        }
    }

    fn prune(&self) {
        self.parked.borrow_mut().retain(|page| {
            let live = page.owner.strong_count() > 0;
            if !live {
                Self::unmount(&page.view, page.controller.as_deref());
            }
            live
        });
    }

    /// Scroll to and focus the visible control labelled `title`, preferring one inside the
    /// section headed `section`. Parked pages are hidden and never match.
    pub fn reveal(&self, section: &str, title: &str) {
        use objc2_app_kit::{NSAccessibility, NSTextField};
        fn find(view: &NSView, title: &str, labels: bool) -> Option<Retained<NSView>> {
            if view.isHidden() {
                return None;
            }
            if let Some(control) = view.downcast_ref::<NSControl>() {
                let name = control.accessibilityLabel().map(|value| value.to_string());
                let matches = name
                    .as_deref()
                    .is_some_and(|name| name == title || name.starts_with(&format!("{title} (")))
                    || (labels
                        && view
                            .downcast_ref::<NSTextField>()
                            .is_some_and(|field| field.stringValue().to_string() == title));
                if matches {
                    return Some(view.retain());
                }
            }
            view.subviews().iter().find_map(|child| find(&child, title, labels))
        }
        let root = self.view.ns_view();
        root.layoutSubtreeIfNeeded();
        // A disclosure or composite header may wrap its title separately from
        // its controls. Walk out to the nearest containing section before falling
        // back to the whole visible page.
        let heading = find(root, section, true);
        let mut scope = heading.as_ref().and_then(|heading| unsafe { heading.superview() });
        let mut target = None;
        while let Some(parent) = scope {
            crate::layout::reveal_disclosures(&parent, title);
            target = find(&parent, title, false).or_else(|| find(&parent, title, true));
            if target.is_some() || std::ptr::eq(&*parent, root) {
                break;
            }
            scope = unsafe { parent.superview() };
        }
        if target.is_none() {
            crate::layout::reveal_disclosures(root, title);
        }
        root.layoutSubtreeIfNeeded();
        let Some(view) =
            target.or_else(|| find(root, title, false)).or_else(|| find(root, title, true))
        else {
            // A setting edited elsewhere, such as in a sheet: show the section that opens it.
            if let Some(heading) = heading {
                heading.scrollRectToVisible(heading.bounds());
            }
            return;
        };
        view.scrollRectToVisible(view.bounds());
        let focusable = view.downcast_ref::<NSControl>().is_some_and(|control| {
            control.isEnabled()
                && view.downcast_ref::<NSTextField>().is_none_or(|field| field.isEditable())
        });
        if focusable && let Some(window) = view.window() {
            window.makeFirstResponder(Some(&view));
        }
    }

    fn release_content_constraints(&self) {
        if let Some(constraints) = &self.content_constraints {
            for constraint in constraints.borrow_mut().drain(..) {
                constraint.setActive(false);
            }
        }
    }

    /// Unmount the current and all parked pages.
    pub fn clear(&self) {
        self.release_content_constraints();
        if let Some(page) = self.page.borrow_mut().take() {
            Self::unmount(page.ns_view(), page.view_controller());
        }
        self.cached.borrow_mut().take();
        for page in self.parked.take() {
            Self::unmount(&page.view, page.controller.as_deref());
        }
    }
}
impl Drop for PageHost {
    fn drop(&mut self) { self.clear(); }
}
impl NativeView for PageHost {
    fn view_controller(&self) -> Option<&NSViewController> { Some(&self.controller) }

    fn ns_view(&self) -> &NSView { self.view.ns_view() }
}

/// Shallow navigation that keeps visited pages and their scroll positions alive.
pub struct NavigationHost {
    host: PageHost,
    root: RefCell<Option<Rc<dyn NativeView>>>,
}
impl NavigationHost {
    pub fn new(ui: &Ui) -> Self {
        Self {
            host: PageHost::new(ui),
            root: RefCell::new(None),
        }
    }

    pub fn set_root(&self, page: Rc<dyn NativeView>) {
        self.host.set_cached_page(page.clone());
        *self.root.borrow_mut() = Some(page);
    }

    pub fn push(&self, page: Rc<dyn NativeView>) { self.host.set_cached_page(page); }

    pub fn pop(&self) {
        if let Some(root) = self.root.borrow().as_ref() {
            self.host.set_cached_page(root.clone());
        }
    }
}
impl NativeView for NavigationHost {
    fn ns_view(&self) -> &NSView { self.host.ns_view() }

    fn view_controller(&self) -> Option<&NSViewController> { self.host.view_controller() }
}
