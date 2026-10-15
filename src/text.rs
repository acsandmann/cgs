use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::*;
use objc2_foundation::{NSAttributedString, NSDictionary, NSString, NSURL};

use crate::{NativeControl, NativeView, Ui};

pub struct Font;
impl Font {
    pub fn body() -> Retained<NSFont> { NSFont::systemFontOfSize(NSFont::systemFontSize()) }

    pub fn body_emphasized() -> Retained<NSFont> {
        NSFont::boldSystemFontOfSize(NSFont::systemFontSize())
    }

    pub fn caption() -> Retained<NSFont> { NSFont::systemFontOfSize(NSFont::smallSystemFontSize()) }

    /// A caption that marks the chosen item, such as a selected thumbnail's name.
    pub fn caption_emphasized() -> Retained<NSFont> {
        NSFont::systemFontOfSize_weight(NSFont::smallSystemFontSize(), unsafe {
            NSFontWeightSemibold
        })
    }

    /// The name of an inspected item, between a page title and a section title.
    pub fn title3() -> Retained<NSFont> {
        NSFont::systemFontOfSize_weight(15.0, unsafe { NSFontWeightSemibold })
    }

    pub fn title() -> Retained<NSFont> {
        NSFont::systemFontOfSize_weight(20.0, unsafe { NSFontWeightSemibold })
    }

    pub fn section_title() -> Retained<NSFont> {
        NSFont::systemFontOfSize_weight(NSFont::systemFontSize(), unsafe { NSFontWeightSemibold })
    }

    pub fn subsection_title() -> Retained<NSFont> {
        NSFont::systemFontOfSize_weight(NSFont::smallSystemFontSize(), unsafe {
            NSFontWeightSemibold
        })
    }

    pub fn monospaced() -> Retained<NSFont> {
        NSFont::monospacedSystemFontOfSize_weight(NSFont::systemFontSize(), unsafe {
            NSFontWeightRegular
        })
    }
}
pub struct Color;
impl Color {
    pub fn label() -> Retained<NSColor> { NSColor::labelColor() }

    pub fn secondary_label() -> Retained<NSColor> { NSColor::secondaryLabelColor() }

    pub fn tertiary_label() -> Retained<NSColor> { NSColor::tertiaryLabelColor() }

    pub fn separator() -> Retained<NSColor> { NSColor::separatorColor() }

    pub fn control_accent() -> Retained<NSColor> { NSColor::controlAccentColor() }

    pub fn window_background() -> Retained<NSColor> { NSColor::windowBackgroundColor() }

    pub fn under_page_background() -> Retained<NSColor> { NSColor::underPageBackgroundColor() }

    pub fn rgba(r: f64, g: f64, b: f64, a: f64) -> Retained<NSColor> {
        NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a)
    }
}

/// Above stack (250) and label (251) hugging, so a stack that pins its children to
/// one width sizes to its longest text rather than squeezing it to the shortest
/// sibling; above text fields, so a long value never wraps the label beside it;
/// below window-size constraints (500), so text yields in narrow windows.
pub(crate) const TEXT_COMPRESSION_RESISTANCE: f32 = 260.0;
/// Text fields grow to fit their value, taking room a settings label would
/// prefer (`LABEL_PREFERRED_WIDTH`) but not room its text needs.
pub(crate) const FIELD_COMPRESSION_RESISTANCE: f32 = 255.0;
/// Settings labels prefer half the row, but yield it to text field content.
pub(crate) const LABEL_PREFERRED_WIDTH: f32 = 253.0;

pub struct Label(Retained<NSTextField>);
impl Label {
    pub fn new(ui: &Ui, text: &str) -> Self {
        let native = NSTextField::labelWithString(&NSString::from_str(text), ui.mtm());
        native.setFont(Some(&Font::body()));
        Self(native)
    }

    pub fn ns_text_field(&self) -> &NSTextField { &self.0 }

    pub fn set_text(&self, value: &str) {
        if self.0.stringValue().to_string() != value {
            self.0.setStringValue(&NSString::from_str(value));
        }
    }

    pub fn font(self, value: &NSFont) -> Self {
        self.0.setFont(Some(value));
        self
    }

    pub fn color(self, value: &NSColor) -> Self {
        self.0.setTextColor(Some(value));
        self
    }

    pub fn wrapping(self) -> Self {
        if let Some(cell) = self.0.cell() {
            cell.setWraps(true);
            cell.setScrollable(false);
        }
        self.0.setUsesSingleLineMode(false);
        self.0.setMaximumNumberOfLines(0);
        self.0.setContentCompressionResistancePriority_forOrientation(
            TEXT_COMPRESSION_RESISTANCE,
            NSLayoutConstraintOrientation::Horizontal,
        );
        self
    }

    /// Truncate the tail when space runs out. Keeps the single line of `new`;
    /// unlike `wrapping`, the natural width is the full line, so stacks show it whole.
    pub fn truncating(self) -> Self {
        self.0.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        self.0.setContentCompressionResistancePriority_forOrientation(
            TEXT_COMPRESSION_RESISTANCE,
            NSLayoutConstraintOrientation::Horizontal,
        );
        self
    }

    /// Limit wrapping text to `lines` (0 is unlimited).
    pub fn max_lines(self, lines: isize) -> Self {
        self.0.setMaximumNumberOfLines(lines);
        self
    }

    /// Allow selecting and copying the text.
    pub fn selectable(self) -> Self {
        self.0.setSelectable(true);
        self
    }
}
impl NativeView for Label {
    fn ns_view(&self) -> &NSView { &self.0 }
}
impl NativeControl for Label {
    fn ns_control(&self) -> &NSControl { &self.0 }
}
macro_rules! text_style {
    ($name:ident, $font:ident, $color:ident) => {
        pub struct $name(Label);
        impl $name {
            pub fn new(ui: &Ui, text: &str) -> Self {
                Self(
                    Label::new(ui, text)
                        .font(&Font::$font())
                        .color(&Color::$color())
                        .wrapping(),
                )
            }

            pub fn set_text(&self, text: &str) { self.0.set_text(text); }

            pub fn max_lines(self, lines: isize) -> Self { Self(self.0.max_lines(lines)) }

            /// One line that truncates its tail instead of wrapping.
            pub fn single_line(ui: &Ui, text: &str) -> Self {
                Self(
                    Label::new(ui, text)
                        .font(&Font::$font())
                        .color(&Color::$color())
                        .truncating(),
                )
            }

            pub fn ns_text_field(&self) -> &NSTextField { self.0.ns_text_field() }
        }
        impl NativeView for $name {
            fn ns_view(&self) -> &NSView { self.0.ns_view() }
        }
    };
}
text_style!(SecondaryLabel, body, secondary_label);
text_style!(Title, title, label);
text_style!(SectionTitle, section_title, label);
text_style!(SubsectionTitle, subsection_title, secondary_label);
text_style!(Caption, caption, secondary_label);
text_style!(MonospaceLabel, monospaced, label);
text_style!(WrappingLabel, body, label);

pub struct Link(Label);
impl Link {
    pub fn new(ui: &Ui, text: &str, url: &str) -> Self {
        let label = Label::new(ui, text);
        if let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) {
            let attrs =
                NSDictionary::from_slices(
                    &[unsafe { NSLinkAttributeName }],
                    &[&*url as &AnyObject],
                );
            let text = unsafe {
                NSAttributedString::initWithString_attributes(
                    NSAttributedString::alloc(),
                    &NSString::from_str(text),
                    Some(&attrs),
                )
            };
            label.ns_text_field().setAttributedStringValue(&text);
            label.ns_text_field().setAllowsEditingTextAttributes(true);
            label.ns_text_field().setSelectable(true);
        }
        Self(label)
    }

    pub fn ns_text_field(&self) -> &NSTextField { self.0.ns_text_field() }
}
impl NativeView for Link {
    fn ns_view(&self) -> &NSView { self.0.ns_view() }
}

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
