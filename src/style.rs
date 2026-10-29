use objc2::rc::Retained;
use objc2_app_kit::*;

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

    /// Subtle detail-pane background for settings windows.
    pub fn settings_background() -> Retained<NSColor> {
        Self::window_background()
            .blendedColorWithFraction_ofColor(0.035, &Self::label())
            .unwrap_or_else(Self::window_background)
    }

    pub fn under_page_background() -> Retained<NSColor> { NSColor::underPageBackgroundColor() }

    pub fn rgba(r: f64, g: f64, b: f64, a: f64) -> Retained<NSColor> {
        NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a)
    }
}
