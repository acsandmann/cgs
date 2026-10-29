use objc2::rc::Retained;
use objc2_app_kit::*;
use objc2_foundation::NSObjectProtocol;

use crate::Ui;
use crate::bridge::ActionTarget;
use crate::view::{NativeControl, NativeView};

pub struct ColorWell {
    native: Retained<NSColorWell>,
    target: Retained<ActionTarget>,
}
impl ColorWell {
    pub fn new(ui: &Ui) -> Self {
        let native = NSColorWell::new(ui.mtm());
        native.setContinuous(false);
        let target = ActionTarget::new(ui);
        target.attach(&native);
        Self { native, target }
    }

    /// Show an sRGB `[red, green, blue, alpha]` color.
    pub fn set_value(&self, [r, g, b, a]: [f64; 4]) {
        let color = NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, a);
        if !self.native.color().isEqual(Some(&color)) {
            self.native.setColor(&color);
        }
    }

    /// Receive the chosen color as sRGB `[red, green, blue, alpha]`.
    pub fn on_change(self, mut f: impl FnMut([f64; 4]) + 'static) -> Self {
        self.target.set(move |sender| {
            let color = sender.downcast_ref::<NSColorWell>().and_then(|control| {
                control
                    .color()
                    .colorUsingColorSpace(&objc2_app_kit::NSColorSpace::sRGBColorSpace())
            });
            if let Some(c) = color {
                f([
                    c.redComponent(),
                    c.greenComponent(),
                    c.blueComponent(),
                    c.alphaComponent(),
                ]);
            }
        });
        self
    }
}
native_control!(ColorWell, NSColorWell, ns_color_well);
