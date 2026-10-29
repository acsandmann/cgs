use objc2::rc::Retained;
use objc2_app_kit::*;
use objc2_foundation::NSString;

use crate::Ui;
use crate::view::NativeView;

pub struct Symbol;
impl Symbol {
    pub fn named(name: &str) -> Option<Retained<NSImage>> {
        NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None)
    }

    /// Consistent symbol metrics for toolbar menus and header buttons.
    pub fn header(name: &str) -> Option<Retained<NSImage>> {
        let configuration =
            NSImageSymbolConfiguration::configurationWithPointSize_weight(12.0, unsafe {
                NSFontWeightRegular
            });
        Self::named(name)?.imageWithSymbolConfiguration(&configuration)
    }
}
pub struct ImageView(Retained<NSImageView>);
impl ImageView {
    pub fn new(ui: &Ui, image: &NSImage) -> Self {
        let native = NSImageView::new(ui.mtm());
        native.setImage(Some(image));
        Self(native)
    }

    pub fn symbol(ui: &Ui, name: &str) -> Option<Self> {
        Symbol::named(name).map(|image| Self::new(ui, &image))
    }

    pub fn ns_image_view(&self) -> &NSImageView { &self.0 }
}
impl NativeView for ImageView {
    fn ns_view(&self) -> &NSView { &self.0 }
}

/// A native image, such as an application icon or SF Symbol.
pub type Image = Retained<NSImage>;
