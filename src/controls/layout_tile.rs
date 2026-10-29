use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::*;
use objc2_foundation::NSString;

use crate::Ui;
use crate::controls::Button;
use crate::view::NativeView;

/// One choice in a gallery of layouts: a live illustration above its name, and an
/// optional quiet marker, such as “Default”, on a second line. Selection rings the
/// illustration with the accent color, as thumbnails do in System Settings, and
/// assistive apps hear each tile as a radio button with its marker in its name.
/// Whether a tile is selected, and its marker.
type TileState = std::rc::Rc<std::cell::RefCell<Option<(bool, Option<String>)>>>;

pub struct LayoutTile {
    button: Button,
    preview: crate::controls::LayoutPreview,
    title: String,
    state: TileState,
}
impl LayoutTile {
    pub fn new(ui: &Ui, title: &str, canvas: crate::CGSize) -> Self {
        let preview = crate::controls::LayoutPreview::selectable(ui, canvas);
        let native = NSButton::new(ui.mtm());
        native.setButtonType(NSButtonType::PushOnPushOff);
        #[allow(deprecated)] // AppKit’s flexible native image-and-title button bezel.
        native.setBezelStyle(NSBezelStyle::RegularSquare);
        native.setBordered(false);
        native.setContentTintColor(Some(&NSColor::labelColor()));
        // An empty image reserves the illustration’s room, with its ring, above the title.
        let outset = crate::controls::LayoutPreview::RING_OUTSET;
        let room = crate::CGSize::new(canvas.width, canvas.height + 2.0 * outset);
        native.setImage(Some(&NSImage::initWithSize(NSImage::alloc(), room)));
        native.setImagePosition(NSCellImagePosition::ImageAbove);
        native.addSubview(preview.ns_view());
        native.setAccessibilityRole(Some(unsafe { NSAccessibilityRadioButtonRole }));
        let view = preview.ns_view();
        view.setAccessibilityElement(false);
        view.centerXAnchor()
            .constraintEqualToAnchor(&native.centerXAnchor())
            .setActive(true);
        view.topAnchor()
            .constraintEqualToAnchor_constant(&native.topAnchor(), outset + 2.0)
            .setActive(true);
        let button = Button::from_native(ui, native);
        // The illustration, then two caption lines: the name and its marker.
        button.height(room.height + 2.0 + 2.0 * NSFont::smallSystemFontSize() + 16.0);
        let tile = Self {
            button,
            preview,
            title: title.into(),
            state: Default::default(),
        };
        tile.set_state(false, None);
        tile
    }

    pub fn on_click(mut self, mut f: impl FnMut() + 'static) -> Self {
        let native = objc2::rc::Weak::new(self.button.ns_button());
        let state = self.state.clone();
        self.button = self.button.on_click(move || {
            f();
            // A click toggles the native state; selection is only ever set by `set_state`.
            if let (Some(native), Some((selected, _))) = (native.load(), &*state.borrow()) {
                native.setState(if *selected {
                    NSControlStateValueOn
                } else {
                    NSControlStateValueOff
                });
            }
        });
        self
    }

    pub fn preview(&self) -> &crate::controls::LayoutPreview { &self.preview }

    /// Whether this tile is the chosen one, and its marker, such as “Default”.
    pub fn set_state(&self, selected: bool, marker: Option<&str>) {
        let state = Some((selected, marker.map(str::to_owned)));
        if *self.state.borrow() == state {
            return;
        }
        *self.state.borrow_mut() = state;
        let native = self.button.ns_button();
        native.setState(if selected {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        self.preview.set_selected(selected);
        native.setAttributedTitle(&two_line_title(&self.title, marker, selected));
        let name = match marker {
            Some(marker) => format!("{}, {}", self.title, marker.to_lowercase()),
            None => self.title.clone(),
        };
        native.setAccessibilityLabel(Some(&NSString::from_str(&name)));
    }

    pub fn ns_button(&self) -> &NSButton { self.button.ns_button() }
}
impl NativeView for LayoutTile {
    fn ns_view(&self) -> &NSView { self.button.ns_view() }
}

/// A centered caption, emphasized while selected, over a quieter marker line. Every tile
/// reserves the second line, so names stay aligned across the gallery.
fn two_line_title(
    title: &str,
    marker: Option<&str>,
    selected: bool,
) -> Retained<objc2_foundation::NSAttributedString> {
    use objc2_foundation::{NSAttributedString, NSDictionary, NSMutableAttributedString};
    let paragraph = NSMutableParagraphStyle::new();
    paragraph.setAlignment(NSTextAlignment::Center);
    paragraph.setParagraphSpacing(1.0);
    let line = |text: &str, font: &NSFont, color: &NSColor| unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(text),
            Some(&NSDictionary::from_slices(
                &[
                    NSFontAttributeName,
                    NSForegroundColorAttributeName,
                    NSParagraphStyleAttributeName,
                ],
                &[
                    font as &AnyObject,
                    color as &AnyObject,
                    &*paragraph as &AnyObject,
                ],
            )),
        )
    };
    let text = NSMutableAttributedString::new();
    let name = if selected {
        crate::style::Font::caption_emphasized()
    } else {
        crate::style::Font::caption()
    };
    text.appendAttributedString(&line(&format!("{title}\n"), &name, &NSColor::labelColor()));
    text.appendAttributedString(&line(
        marker.unwrap_or(" "),
        &NSFont::systemFontOfSize_weight(NSFont::labelFontSize() - 1.0, unsafe {
            NSFontWeightMedium
        }),
        &NSColor::secondaryLabelColor(),
    ));
    Retained::into_super(text)
}
