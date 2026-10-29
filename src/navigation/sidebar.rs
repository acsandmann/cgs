use objc2::rc::Retained;
use objc2_app_kit::*;
use objc2_foundation::NSArray;

use crate::Ui;
use crate::collection::{Outline, OutlineItem};
use crate::image::ImageView;
use crate::text::Label;
use crate::view::{Metrics, NativeView};

#[derive(Clone)]
pub struct SidebarItem<T> {
    pub id: T,
    pub title: String,
    pub symbol: String,
}
struct SidebarCell {
    native: Retained<NSTableCellView>,
}
impl NativeView for SidebarCell {
    fn ns_view(&self) -> &NSView { &self.native }
}

pub struct Sidebar<T: 'static> {
    outline: Outline<SidebarItem<T>>,
}
impl<T: Clone + 'static> Sidebar<T> {
    pub fn new(ui: &Ui, items: Vec<SidebarItem<T>>) -> Self {
        Self::with_children(ui, items, |_| Vec::new())
    }

    pub fn with_children(
        ui: &Ui,
        items: Vec<SidebarItem<T>>,
        children: impl Fn(&T) -> Vec<SidebarItem<T>>,
    ) -> Self {
        let ui_copy = *ui;
        let measure = Label::new(ui, "");
        let content_width = items
            .iter()
            .map(|item| {
                measure.set_text(&item.title);
                measure.ns_view().fittingSize().width + Metrics::CONTROL_SPACING * 6.0
            })
            .fold(0.0, f64::max);
        let items = items
            .into_iter()
            .map(|item| OutlineItem {
                children: children(&item.id)
                    .into_iter()
                    .map(|child| OutlineItem {
                        title: child.title.clone(),
                        id: child,
                        children: Vec::new(),
                    })
                    .collect(),
                title: item.title.clone(),
                id: item,
            })
            .collect();
        let outline = Outline::new(ui).items(items).cells(move |item, title| {
            let native = NSTableCellView::new(ui_copy.mtm());
            let label = Label::new(&ui_copy, title);
            unsafe {
                native.setTextField(Some(label.ns_text_field()));
            }
            native.addSubview(label.ns_view());
            crate::view::prepare(label.ns_view());
            let leading = if let Some(image) = ImageView::symbol(&ui_copy, &item.symbol) {
                image.width(16.0);
                image.height(16.0);
                unsafe {
                    native.setImageView(Some(image.ns_image_view()));
                }
                native.addSubview(image.ns_view());
                crate::view::prepare(image.ns_view());
                image
                    .ns_view()
                    .leadingAnchor()
                    .constraintEqualToAnchor(&native.leadingAnchor())
                    .setActive(true);
                image
                    .ns_view()
                    .centerYAnchor()
                    .constraintEqualToAnchor(&native.centerYAnchor())
                    .setActive(true);
                label
                    .ns_view()
                    .leadingAnchor()
                    .constraintEqualToAnchor_constant(&image.ns_view().trailingAnchor(), 8.0)
            } else {
                label.ns_view().leadingAnchor().constraintEqualToAnchor(&native.leadingAnchor())
            };
            NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
                leading,
                label
                    .ns_view()
                    .trailingAnchor()
                    .constraintEqualToAnchor(&native.trailingAnchor()),
                label.ns_view().centerYAnchor().constraintEqualToAnchor(&native.centerYAnchor()),
            ]));
            Box::new(SidebarCell { native })
        });
        let native = outline.ns_outline_view();
        native.setUsesAutomaticRowHeights(false);
        native.setRowSizeStyle(NSTableViewRowSizeStyle::Default);
        native.setBackgroundColor(&NSColor::clearColor());
        outline.min_width(content_width);
        outline.max_width(content_width + Metrics::PAGE_INSET * 2.0);
        Self { outline }
    }

    pub fn on_select(mut self, mut f: impl FnMut(T) + 'static) -> Self {
        let native = objc2::rc::Weak::new(self.outline.ns_outline_view());
        self.outline = self.outline.on_select(move |item| {
            if let Some(item) = item {
                unsafe {
                    if let Some(native) = native.load() {
                        if let Some(selected) = native.itemAtRow(native.selectedRow()) {
                            native.expandItem(Some(&selected));
                        }
                    }
                }
                f(item.id);
            }
        });
        self
    }

    pub fn set_items(&self, items: Vec<SidebarItem<T>>) {
        self.outline.set_items(
            items
                .into_iter()
                .map(|item| OutlineItem {
                    title: item.title.clone(),
                    id: item,
                    children: Vec::new(),
                })
                .collect(),
        );
    }

    pub fn group_parents(&self) { self.outline.group_parents(); }

    pub fn set_selected(&self, index: usize) { self.outline.set_selected(index); }

    pub fn ns_table_view(&self) -> &NSTableView { self.outline.ns_outline_view() }
}
impl<T: 'static> NativeView for Sidebar<T> {
    fn ns_view(&self) -> &NSView { self.outline.ns_view() }
}
