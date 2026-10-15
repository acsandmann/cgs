//! A row of named thumbnails that behaves as one native radio group, as in the
//! appearance choices of System Settings.
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSApplication, NSBezierPath, NSColor, NSEvent, NSFocusRingType,
    NSLayoutAttribute, NSLayoutConstraintOrientation, NSMenu, NSStackViewDistribution,
    NSTextAlignment, NSView,
};
use objc2_foundation::NSNumber;

use crate::bridge::callback;
use crate::{CGPoint, CGRect, Caption, Font, HStack, Label, Menu, NativeView, Ui, VStack};

/// Width of the selection ring, and the gap between it and the thumbnail.
const RING: f64 = 3.0;
const RING_GAP: f64 = 1.0;
const RING_INSET: f64 = RING + RING_GAP;

define_class!(
    // The accent ring around a selected thumbnail; it resolves the accent color whenever
    // AppKit redisplays it, including after appearance and accent changes.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsThumbnailRing"]
    #[ivars = Cell<bool>]
    struct RingView;
    impl RingView {
        #[unsafe(method(wantsUpdateLayer))]
        fn wants_update_layer(&self) -> bool { true }
        #[unsafe(method(updateLayer))]
        fn update_layer(&self) {
            if let Some(layer) = self.layer() {
                let selected = self.ivars().get();
                layer.setBorderWidth(if selected { RING } else { 0.0 });
                layer.setBorderColor(Some(&NSColor::controlAccentColor().CGColor()));
            }
        }
    }
);

struct TileIvars {
    owner: Weak<Inner>,
    index: usize,
}
define_class!(
    // One choice: the native target for clicks, its context menu, and its radio button
    // accessibility element. Its thumbnail and labels are presentation only.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsThumbnailTile"]
    #[ivars = TileIvars]
    struct TileView;
    impl TileView {
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: CGPoint) -> *mut NSView {
            let hit: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
            if hit.is_some() { (self as *const Self).cast_mut().cast() } else { std::ptr::null_mut() }
        }
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let _keep_alive = self.retain();
            if let Some(owner) = self.ivars().owner.upgrade() {
                let index = self.ivars().index;
                callback(|| {
                    owner.choose(index);
                    if event.clickCount() == 2 {
                        owner.activate(index);
                    }
                });
            }
        }
        #[unsafe(method_id(menuForEvent:))]
        fn menu_for_event(&self, _event: &NSEvent) -> Option<Retained<NSMenu>> {
            let owner = self.ivars().owner.upgrade();
            let menus = owner.as_ref().map(|owner| owner.menus.borrow());
            menus
                .and_then(|menus| menus.get(self.ivars().index)?.as_ref().map(|menu| menu.ns_menu().retain()))
        }
        #[unsafe(method(accessibilityPerformPress))]
        fn accessibility_press(&self) -> bool {
            let _keep_alive = self.retain();
            if let Some(owner) = self.ivars().owner.upgrade() {
                let index = self.ivars().index;
                callback(|| owner.choose(index));
            }
            true
        }
    }
);

define_class!(
    // The group: one key view whose arrow keys move the selection, with the focus ring
    // drawn around the selected thumbnail.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "CgsThumbnailPicker"]
    #[ivars = RefCell<Weak<Inner>>]
    struct PickerView;
    impl PickerView {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool { true }
        #[unsafe(method(canBecomeKeyView))]
        fn can_become_key_view(&self) -> bool {
            NSApplication::sharedApplication(self.mtm()).isFullKeyboardAccessEnabled()
        }
        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let _keep_alive = self.retain();
            let owner = self.ivars().borrow().upgrade();
            // Left, right, down, and up arrows.
            let step = match event.keyCode() {
                123 | 126 => Some(-1),
                124 | 125 => Some(1),
                _ => None,
            };
            match (owner, step) {
                (Some(owner), Some(step)) => callback(|| owner.step(step)),
                _ => unsafe { let _: () = msg_send![super(self), keyDown: event]; },
            }
        }
        #[unsafe(method(focusRingMaskBounds))]
        fn focus_ring_bounds(&self) -> CGRect {
            self.ivars().borrow().upgrade().map_or(CGRect::ZERO, |owner| owner.ring_frame())
        }
        #[unsafe(method(drawFocusRingMask))]
        fn focus_ring_mask(&self) {
            if let Some(owner) = self.ivars().borrow().upgrade() {
                let frame = owner.ring_frame();
                let radius = owner.radius + RING_INSET;
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(frame, radius, radius).fill();
            }
        }
    }
);

struct Tile {
    view: Retained<TileView>,
    ring: Retained<RingView>,
    name: String,
    title: Label,
    marker: Caption,
    _thumbnail: Box<dyn NativeView>,
    _content: VStack,
}

type Handler = RefCell<Option<Rc<dyn Fn(usize)>>>;

struct Inner {
    view: Retained<PickerView>,
    tiles: Vec<Tile>,
    radius: f64,
    selected: Cell<usize>,
    /// The marked choice, its visible marker, and how VoiceOver names it.
    marker: RefCell<Option<(usize, String, String)>>,
    menus: RefCell<Vec<Option<Menu>>>,
    on_select: Handler,
    on_activate: Handler,
    _row: HStack,
}
impl Inner {
    /// Select `index` as the user did, then tell the owner.
    fn choose(&self, index: usize) {
        if index >= self.tiles.len() {
            return;
        }
        self.select(index);
        let handler = self.on_select.borrow().clone();
        if let Some(handler) = handler {
            handler(index);
        }
    }

    fn activate(&self, index: usize) {
        let handler = self.on_activate.borrow().clone();
        if let Some(handler) = handler {
            handler(index);
        }
    }

    fn step(&self, step: isize) {
        let next = self.selected.get() as isize + step;
        if (0..self.tiles.len() as isize).contains(&next) {
            self.choose(next as usize);
        } else {
            beep();
        }
    }

    fn select(&self, index: usize) {
        let previous = self.selected.replace(index);
        if previous != index || !self.tiles[index].ring.ivars().get() {
            self.refresh();
            self.view.noteFocusRingMaskChanged();
        }
    }

    fn refresh(&self) {
        let marker = self.marker.borrow();
        for (index, tile) in self.tiles.iter().enumerate() {
            let selected = index == self.selected.get();
            if tile.ring.ivars().replace(selected) != selected {
                tile.ring.setNeedsDisplay(true);
            }
            tile.title.ns_text_field().setFont(Some(&*if selected {
                Font::caption_emphasized()
            } else {
                Font::caption()
            }));
            let mark = marker.as_ref().filter(|(marked, ..)| *marked == index);
            // Every tile reserves the marker's line so names stay aligned.
            tile.marker.set_text(mark.map_or(" ", |(_, text, _)| text.as_str()));
            let spoken = match mark {
                Some((_, _, spoken)) => format!("{}, {spoken}", tile.name),
                None => tile.name.clone(),
            };
            tile.view
                .setAccessibilityLabel(Some(&objc2_foundation::NSString::from_str(&spoken)));
            // SAFETY: a radio button's accessibility value is an NSNumber.
            unsafe { tile.view.setAccessibilityValue(Some(&NSNumber::new_bool(selected))) };
        }
    }

    /// The selected ring, in the picker's coordinates.
    fn ring_frame(&self) -> CGRect {
        let ring = &self.tiles[self.selected.get()].ring;
        ring.convertRect_toView(ring.bounds(), Some(&self.view))
    }
}

fn beep() {
    unsafe extern "C" {
        fn NSBeep();
    }
    unsafe { NSBeep() }
}

/// Named thumbnails in one row, chosen like radio buttons. Selection draws an accent ring
/// around the thumbnail and emphasizes its name; an optional marker, such as “Default”,
/// sits on a second line every tile reserves. Arrow keys move the selection, Tab enters
/// and leaves the group, and each choice is a VoiceOver radio button. Thumbnails are
/// presentation only; the owner keeps and updates them.
pub struct ThumbnailPicker(Rc<Inner>);
impl ThumbnailPicker {
    /// `label` names the group for accessibility. Each thumbnail keeps its own size;
    /// `corner_radius` is the thumbnails' radius, which the ring follows.
    pub fn new(
        ui: &Ui,
        label: &str,
        corner_radius: f64,
        items: Vec<(String, Box<dyn NativeView>)>,
    ) -> Self {
        assert!(!items.is_empty());
        let picker = Self(Rc::new_cyclic(|weak: &Weak<Inner>| {
            let view: Retained<PickerView> = unsafe {
                msg_send![
                    super(PickerView::alloc(ui.mtm()).set_ivars(RefCell::new(weak.clone()))),
                    init
                ]
            };
            crate::view::prepare(&view);
            view.setFocusRingType(NSFocusRingType::Exterior);
            view.setAccessibilityElement(true);
            view.setAccessibilityRole(Some(unsafe {
                objc2_app_kit::NSAccessibilityRadioGroupRole
            }));
            view.setAccessibilityLabel(Some(&objc2_foundation::NSString::from_str(label)));
            let row = HStack::new(ui)
                .spacing(4.0)
                .alignment(NSLayoutAttribute::Top)
                .distribution(NSStackViewDistribution::FillEqually);
            let tiles: Vec<Tile> = items
                .into_iter()
                .enumerate()
                .map(|(index, (name, thumbnail))| {
                    let tile = Tile::new(ui, weak.clone(), index, name, thumbnail, corner_radius);
                    row.add(tile.view.clone().into_super());
                    tile
                })
                .collect();
            view.addSubview(row.ns_view());
            crate::view::pin(&view, row.ns_view(), crate::uniform_insets(0.0));
            let menus = RefCell::new(tiles.iter().map(|_| None).collect());
            Inner {
                view,
                tiles,
                radius: corner_radius,
                selected: Cell::new(0),
                marker: RefCell::new(None),
                menus,
                on_select: RefCell::new(None),
                on_activate: RefCell::new(None),
                _row: row,
            }
        }));
        picker.0.refresh();
        picker
    }

    pub fn on_select(self, f: impl Fn(usize) + 'static) -> Self {
        *self.0.on_select.borrow_mut() = Some(Rc::new(f));
        self
    }

    /// Double-clicking a thumbnail, after it is selected.
    pub fn on_double_click(self, f: impl Fn(usize) + 'static) -> Self {
        *self.0.on_activate.borrow_mut() = Some(Rc::new(f));
        self
    }

    /// The context menu for one thumbnail.
    pub fn menu(self, index: usize, menu: Menu) -> Self {
        if let Some(slot) = self.0.menus.borrow_mut().get_mut(index) {
            *slot = Some(menu);
        }
        self
    }

    pub fn selected(&self) -> usize { self.0.selected.get() }

    pub fn set_selected(&self, index: usize) {
        if index < self.0.tiles.len() {
            self.0.select(index);
        }
    }

    /// Mark one choice with `text` on its second line, which VoiceOver reads as `spoken`.
    pub fn set_marker(&self, index: Option<usize>, text: &str, spoken: &str) {
        let marker = index.map(|index| (index, text.to_owned(), spoken.to_owned()));
        if *self.0.marker.borrow() != marker {
            *self.0.marker.borrow_mut() = marker;
            self.0.refresh();
        }
    }
}
impl NativeView for ThumbnailPicker {
    fn ns_view(&self) -> &NSView { &self.0.view }
}

impl Tile {
    fn new(
        ui: &Ui,
        owner: Weak<Inner>,
        index: usize,
        name: String,
        thumbnail: Box<dyn NativeView>,
        radius: f64,
    ) -> Self {
        let view: Retained<TileView> = unsafe {
            msg_send![
                super(TileView::alloc(ui.mtm()).set_ivars(TileIvars { owner, index })),
                init
            ]
        };
        crate::view::prepare(&view);
        view.setAccessibilityElement(true);
        view.setAccessibilityRole(Some(unsafe { objc2_app_kit::NSAccessibilityRadioButtonRole }));
        let ring: Retained<RingView> = unsafe {
            msg_send![
                super(RingView::alloc(ui.mtm()).set_ivars(Cell::new(false))),
                init
            ]
        };
        ring.setWantsLayer(true);
        if let Some(layer) = ring.layer() {
            layer.setCornerRadius(radius + RING_INSET);
        }
        thumbnail.ns_view().setAccessibilityElement(false);
        ring.addSubview(thumbnail.ns_view());
        crate::view::pin(&ring, thumbnail.ns_view(), crate::uniform_insets(RING_INSET));
        let title = Label::new(ui, &name).font(&Font::caption()).truncating();
        let marker = Caption::single_line(ui, " ");
        for label in [title.ns_text_field(), marker.ns_text_field()] {
            label.setAlignment(NSTextAlignment::Center);
            label.setAccessibilityElement(false);
            label.setContentCompressionResistancePriority_forOrientation(
                1.0,
                NSLayoutConstraintOrientation::Horizontal,
            );
        }
        let content = VStack::new(ui)
            .spacing(2.0)
            .alignment(NSLayoutAttribute::CenterX)
            .push(title.ns_view().retain())
            .push(marker.ns_view().retain());
        // A vertical stack pins what it adds to its width; the ring keeps its own.
        crate::view::prepare(&ring);
        content.ns_stack_view().insertArrangedSubview_atIndex(&ring, 0);
        content.ns_stack_view().setCustomSpacing_afterView(6.0, &ring);
        view.addSubview(content.ns_view());
        crate::view::pin(&view, content.ns_view(), crate::uniform_insets(0.0));
        Self {
            view,
            ring,
            name,
            title,
            marker,
            _thumbnail: thumbnail,
            _content: content,
        }
    }
}
