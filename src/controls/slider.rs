use objc2::rc::Retained;
use objc2::{MainThreadOnly, define_class, msg_send};
use objc2_app_kit::*;

use crate::Ui;
use crate::bridge::ActionTarget;
use crate::controls::NumberField;
use crate::layout::HStack;
use crate::view::{NativeControl, NativeView};

define_class!(
    #[unsafe(super(NSSlider))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsSlider"]
    struct NaturalSlider;
    impl NaturalSlider {
        #[unsafe(method(intrinsicContentSize))]
        fn intrinsic_size(&self) -> crate::CGSize {
            let mut size: crate::CGSize = unsafe { msg_send![super(self), intrinsicContentSize] };
            // AppKit sliders otherwise have no intrinsic width. This is a
            // compressible preference, not a constraint on the containing window.
            size.width = size.width.max(NSFont::systemFontSize() * 6.0);
            size
        }
    }
);

/// An accent-colored native slider, with optional snapping and tactile detents.
/// Native tracking preserves keyboard, accessibility, and appearance support.
pub struct Slider {
    native: Retained<NSSlider>,
    target: Retained<ActionTarget>,
    last_value: std::rc::Rc<std::cell::Cell<f64>>,
    step: std::rc::Rc<std::cell::Cell<Option<f64>>>,
}
impl Slider {
    pub fn new(ui: &Ui) -> Self {
        let native: Retained<NaturalSlider> =
            unsafe { msg_send![NaturalSlider::alloc(ui.mtm()), init] };
        let native = native.into_super();
        native.setTrackFillColor(Some(&NSColor::controlAccentColor()));
        let target = ActionTarget::new(ui);
        target.attach(&native);
        Self {
            native,
            target,
            last_value: std::rc::Rc::new(std::cell::Cell::new(0.0)),
            step: Default::default(),
        }
    }

    pub fn range(self, min: f64, max: f64) -> Self {
        self.set_range(min, max);
        self
    }

    pub fn set_range(&self, min: f64, max: f64) {
        assert!(min.is_finite() && max.is_finite() && min <= max);
        if self.native.minValue() != min {
            self.native.setMinValue(min);
        }
        if self.native.maxValue() != max {
            self.native.setMaxValue(max);
        }
        self.update_ticks();
        self.last_value.set(self.native.doubleValue());
    }

    /// Move in multiples of `step`, such as 5 for percentages, with an alignment haptic
    /// at each one while dragging. Range endpoints stay reachable when they fall between
    /// steps. Tick marks appear when the range spans whole steps and a few of them.
    pub fn step(self, step: f64) -> Self {
        self.set_step(step);
        self
    }

    pub fn set_step(&self, step: f64) {
        assert!(step.is_finite() && step > 0.0);
        self.step.set(Some(step));
        self.update_ticks();
    }

    fn update_ticks(&self) {
        let Some(step) = self.step.get() else { return };
        let (min, max) = (self.native.minValue(), self.native.maxValue());
        let whole = |value: f64| ((value / step).round() * step - value).abs() < 1e-6;
        let count = ((max - min) / step).round() as isize + 1;
        let ticks = if whole(min) && whole(max) && (2..=21).contains(&count) {
            count
        } else {
            0
        };
        if self.native.numberOfTickMarks() != ticks {
            self.native.setNumberOfTickMarks(ticks);
            self.native.setTickMarkPosition(NSTickMarkPosition::Below);
        }
    }

    /// Equally spaced notches, including both endpoints. Each changed detent
    /// produces a subtle alignment haptic on supported hardware during tracking.
    /// Programmatic updates never produce feedback.
    pub fn notches(self, count: usize) -> Self {
        assert!(count >= 2 && count <= isize::MAX as usize);
        self.native.setNumberOfTickMarks(count as isize);
        self.native.setAllowsTickMarkValuesOnly(true);
        self.native.setTickMarkPosition(NSTickMarkPosition::Below);
        self.native.setContinuous(true);
        self
    }

    pub fn continuous(self, value: bool) -> Self {
        self.native.setContinuous(value);
        self
    }

    pub fn value(self, value: f64) -> Self {
        self.set_value(value);
        self
    }

    pub fn set_value(&self, value: f64) {
        if self.native.doubleValue() != value {
            self.native.setDoubleValue(value);
        }
        self.last_value.set(self.native.doubleValue());
    }

    pub fn get_value(&self) -> f64 { self.native.doubleValue() }

    pub fn on_change(self, mut f: impl FnMut(f64) + 'static) -> Self {
        let last = self.last_value.clone();
        let step = self.step.clone();
        self.target.set(move |sender| {
            let Some(control) = sender.downcast_ref::<NSSlider>() else {
                return;
            };
            let mut value = control.doubleValue();
            let snapped = if control.allowsTickMarkValuesOnly() {
                Some(control.closestTickMarkValueToValue(value))
            } else {
                step.get().map(|step| {
                    let (min, max) = (control.minValue(), control.maxValue());
                    let nearest = ((value / step).round() * step).clamp(min, max);
                    // An endpoint between steps wins when it’s closer than the step.
                    [min, max]
                        .into_iter()
                        .find(|bound| (value - bound).abs() < (value - nearest).abs())
                        .unwrap_or(nearest)
                })
            };
            if let Some(snapped) = snapped {
                value = snapped;
                control.setDoubleValue(value);
                let changed = last.replace(value) != value;
                let tracking = NSApplication::sharedApplication(control.mtm())
                    .currentEvent()
                    .is_some_and(|event| {
                        matches!(
                            event.r#type(),
                            NSEventType::LeftMouseDown | NSEventType::LeftMouseDragged
                        )
                    });
                if changed && tracking {
                    NSHapticFeedbackManager::defaultPerformer()
                        .performFeedbackPattern_performanceTime(
                            NSHapticFeedbackPattern::Alignment,
                            NSHapticFeedbackPerformanceTime::Now,
                        );
                }
            } else {
                last.set(value);
            }
            f(value);
        });
        self
    }
}
native_control!(Slider, NSSlider, ns_slider);

/// A native slider paired with a field for exact values.
/// The caller binds both controls to its model; the field can accept values
/// outside the slider's convenient range without losing precision.
pub struct NumberSlider {
    stack: HStack,
}
impl NumberSlider {
    pub fn new(ui: &Ui, slider: std::rc::Rc<Slider>, field: std::rc::Rc<NumberField>) -> Self {
        field.hide_stepper();
        Self {
            stack: HStack::new(ui).spacing(12.0).push(slider).push(field),
        }
    }

    /// A standard-length slider with an external unit and no redundant stepper.
    pub fn with_unit(
        ui: &Ui,
        slider: std::rc::Rc<Slider>,
        field: std::rc::Rc<NumberField>,
        unit: &str,
    ) -> Self {
        slider.width(crate::view::Metrics::SLIDER_WIDTH);
        field.hide_stepper();
        let value = field.unit_field(ui, unit);
        field.min_width(crate::view::Metrics::VALUE_FIELD_WIDTH);
        Self {
            // The bezel hosts the field's view; keep its Rust owner with the row.
            stack: HStack::new(ui).spacing(8.0).push(slider).push(value).keep(field),
        }
    }
}
impl NativeView for NumberSlider {
    fn ns_view(&self) -> &NSView { self.stack.ns_view() }
}
