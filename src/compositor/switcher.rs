//! A held-open window switcher: carousel, stack and grid arrangements of live window previews.
//!
//! Like Mission Control, every preview is its own window. WindowServer places each one in 3D
//! (`SLSTransactionSetWindowTransform3D`, with perspective), so moving a card never re-renders
//! its content: one transaction per display frame carries every card's transform, alpha and
//! stacking, driven by critically damped springs that keep their velocity when retargeted.
//! A full-screen backdrop window below them holds the blurred desktop, floor and title.
//! Callers supply window descriptors with an opaque payload, forward semantic commands, decide
//! what activation means, and call `pump` on each display frame while `animating` is true.
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSFont, NSRunningApplication};
use objc2_core_foundation::{CFRetained, CFString, CFType, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGColor, CGImage, CGWindowListCopyWindowInfo, CGWindowListOption, kCGDesktopWindowLevel,
    kCGNullWindowID,
};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, ns_string};
use objc2_quartz_core::{
    CABasicAnimation, CAGradientLayer, CALayer, CAMediaTiming, CAMediaTimingFunction, CATextLayer,
    CATransform3D,
};

use super::{
    CaptureMode, CaptureResolution, CgsWindow, LayerTransaction, PreviewCompositor, PreviewFrame,
    PreviewLayer, PreviewSource, WindowSurface, WindowTransaction, backdrop_blur, blur_filters,
    release_layer_tree,
};

/// Spring response in seconds: how long a card takes to reach a new target.
const RESPONSE: f64 = 0.3;
/// Backdrop, title and reflection fades.
const FADE: f64 = 0.3;
const CORNER: f64 = 10.;
/// Margin around a card inside its window, so transformed edges stay antialiased.
const PAD: f64 = 2.;
/// Height of the floor reflection, as a fraction of the card.
const REFLECT: f64 = 0.45;
/// Swipe travel per window, in working-area widths.
const SCRUB: f64 = 0.3;
/// A release this soon after opening, before any navigation, picks the previous window.
const QUICK: Duration = Duration::from_millis(250);
/// Above the menu bar and Dock, like the system switchers.
const LEVEL: i32 = 101;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SwitcherLayout {
    /// A concave ring of windows on a reflective floor.
    #[default]
    Carousel,
    /// A ring of upright windows seen from above, the selection in front.
    Stack,
    /// Every window side by side, keeping relative sizes.
    Grid,
}
impl SwitcherLayout {
    pub fn next(self, forward: bool) -> Self {
        const ALL: [SwitcherLayout; 3] = [
            SwitcherLayout::Carousel,
            SwitcherLayout::Stack,
            SwitcherLayout::Grid,
        ];
        let i = ALL.iter().position(|l| *l == self).unwrap();
        ALL[(i + if forward { 1 } else { 2 }) % 3]
    }
}

/// One window offered by the switcher. Items are ordered most recent first; the first is selected.
pub struct SwitcherItem<T> {
    pub window_id: u32,
    pub process_id: i32,
    /// Where the window lives, local to its own display (top-left origin).
    pub frame: CGRect,
    /// The window is on screen at `frame` on the switcher's display right now.
    pub on_screen: bool,
    /// The window belongs to the switcher's display; others fade instead of flying off-screen.
    pub local: bool,
    pub title: String,
    pub data: T,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SwitcherCommand {
    Step(i64),
    Layout(bool),
    /// Select under a global point; grid only, where hovering never moves cards.
    Hover(CGPoint),
    /// Activate under a global point, or dismiss when nothing is there.
    Click(CGPoint),
    Scroll(CGPoint),
    /// `release` is a held modifier or swipe ending: a quick one picks the previous window.
    Commit {
        release: bool,
    },
    Dismiss,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitcherOutcome {
    /// Close immediately; nothing was shown or chosen.
    Close,
    /// Activate `selected()`. When `activation_stamp` is set, keep the switcher until handoff.
    Activate,
}

/// A warm capture host kept between invocations.
pub struct SwitcherCache {
    key: (u32, CGRect, f64),
    captures: PreviewCompositor,
}

/// A card's place on the stage, in display-local points. `key` is its stacking order.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pose {
    /// Signed carousel slot, to tell wrapping cards from ones sweeping with the turn.
    slot: f64,
    /// Rotation about the card's own vertical axis.
    turn: f64,
    z: f64,
    key: f64,
    center: CGPoint,
    /// Relative to the real window's size.
    scale: f64,
    opacity: f32,
    reflect: f32,
    border: bool,
}
fn flat(center: CGPoint, scale: f64, z: f64, key: f64) -> Pose {
    Pose {
        slot: 0.,
        turn: 0.,
        z,
        key,
        center,
        scale,
        opacity: 1.,
        reflect: 0.,
        border: false,
    }
}

/// Display-local, top-left perspective stage.
#[derive(Clone, Copy, Debug)]
struct Stage {
    size: CGSize,
    unit: f64,
    /// Height of the vanishing point: just above the floor for the carousel, so card bottoms
    /// line up and nearer edges rise; mid-screen for the stack, seen from above.
    eye: f64,
}
/// Where the front carousel card meets the floor, and the camera height above it.
const FLOOR: f64 = 0.74;
const EYE: f64 = 0.03;
impl Stage {
    fn new(size: CGSize, layout: SwitcherLayout) -> Self {
        Self {
            size,
            unit: size.width.min(size.height * 1.6),
            eye: size.height
                * match layout {
                    SwitcherLayout::Carousel => FLOOR - EYE,
                    _ => 0.5,
                },
        }
    }

    fn distance(self) -> f64 { 1.1 * self.unit }

    /// On-screen magnification at depth `z`.
    fn depth_scale(self, z: f64) -> f64 { self.distance() / (self.distance() - z) }

    /// Forward map from card-centred points, in units of `content` times the real window size,
    /// to display-local screen points.
    fn place(self, p: &Pose, content: f64) -> CATransform3D {
        let v = CGPoint::new(self.size.width / 2., self.eye);
        let mut perspective = CATransform3D::new_scale(1., 1., 1.);
        perspective.m34 = -1. / self.distance();
        CATransform3D::new_scale(p.scale / content, p.scale / content, 1.)
            .concat(CATransform3D::new_rotation(p.turn, 0., 1., 0.))
            .concat(CATransform3D::new_translation(
                p.center.x - v.x,
                p.center.y - v.y,
                p.z,
            ))
            .concat(perspective)
            .concat(CATransform3D::new_translation(v.x, v.y, 0.))
    }

    /// Screen position of a card-centred point at real size.
    fn point(self, p: &Pose, x: f64, y: f64) -> Option<CGPoint> { map(&self.place(p, 1.), x, y) }

    /// Screen bounds of a posed card, through the same matrix WindowServer applies.
    fn project(self, p: &Pose, size: CGSize) -> Option<CGRect> {
        let (mut min, mut max) = (
            CGPoint::new(f64::MAX, f64::MAX),
            CGPoint::new(f64::MIN, f64::MIN),
        );
        for (x, y) in [(-1., -1.), (1., -1.), (-1., 1.), (1., 1.)] {
            let q = self.point(p, x * size.width / 2., y * size.height / 2.)?;
            min = CGPoint::new(min.x.min(q.x), min.y.min(q.y));
            max = CGPoint::new(max.x.max(q.x), max.y.max(q.y));
        }
        Some(rect(min.x, min.y, max.x - min.x, max.y - min.y))
    }
}
/// Row-vector projection with the homogeneous divide.
fn map(m: &CATransform3D, x: f64, y: f64) -> Option<CGPoint> {
    let w = x * m.m14 + y * m.m24 + m.m44;
    (w > 0.01).then(|| {
        CGPoint::new(
            (x * m.m11 + y * m.m21 + m.m41) / w,
            (x * m.m12 + y * m.m22 + m.m42) / w,
        )
    })
}

/// Cover Flow on a reflective floor: the selection faces the viewer, neighbours turn inward at
/// nearly the same depth with their outer edges nearer and inner edges tucked behind.
fn carousel(st: Stage, sizes: &[CGSize], sel: usize) -> Vec<Pose> {
    const TURN: f64 = 0.67;
    let (w, h, u, n) = (st.size.width, st.size.height, st.unit, sizes.len());
    let (front, side) = (-0.2 * u, -0.24 * u);
    let (f0, fs) = (st.depth_scale(front), st.depth_scale(side));
    let floor = st.eye + (FLOOR * h - st.eye) / f0;
    let fit: Vec<f64> = sizes.iter().map(|c| carousel_fit(st, *c)).collect();
    // On-screen half widths, facing or turned.
    let half = |i: usize, f: f64| sizes[i].width * fit[i] * f / 2.;
    let mut poses: Vec<Pose> = (0..n)
        .map(|i| Pose {
            center: CGPoint::new(w / 2., floor - sizes[i].height * fit[i] / 2.),
            scale: fit[i],
            reflect: 1.,
            border: i == sel,
            ..flat(CGPoint::ZERO, 1., front, 0.)
        })
        .collect();
    for (dir, count) in [(1., n / 2), (-1., (n - 1) / 2)] {
        let (mut x, mut prev) = (half(sel, f0), sel);
        for m in 1..=count {
            let i = (sel as i64 + dir as i64 * m as i64).rem_euclid(n as i64) as usize;
            x += if m == 1 {
                half(i, fs * TURN.cos()) - 0.005 * w
            } else {
                0.6 * (half(prev, fs * TURN.cos()) + half(i, fs * TURN.cos()))
            };
            let p = &mut poses[i];
            p.slot = dir * m as f64;
            p.turn = -dir * TURN;
            p.z = side;
            p.key = -(m as f64);
            p.center.x = w / 2. + dir * x / fs;
            p.opacity = if m <= 3 { 1. } else { 0. };
            p.border = false;
            prev = i;
        }
    }
    poses
}
fn carousel_fit(st: Stage, size: CGSize) -> f64 {
    (0.59 * st.unit / size.width).min(0.67 * st.size.height / size.height).min(1.)
}
fn stack_fit(st: Stage, size: CGSize) -> f64 {
    (0.45 * st.size.width / size.width)
        .min(0.5 * st.size.height / size.height)
        .min(1.)
}

/// An upright ring seen from above: neighbours alternate to the sides, stepping back and up;
/// spacing tops out at a fifth of the circle.
fn stack(st: Stage, sizes: &[CGSize], sel: usize) -> Vec<Pose> {
    let (w, h, u, n) = (st.size.width, st.size.height, st.unit, sizes.len());
    let step = (std::f64::consts::TAU / n as f64).min(1.2566);
    (0..n)
        .map(|i| {
            let k = ((i + n - sel) % n) as i64;
            let k = if k > n as i64 / 2 { k - n as i64 } else { k };
            let theta = k as f64 * step;
            let c = 1. - theta.cos();
            let z = -0.62 * u * c;
            let center = CGPoint::new(
                w / 2. + 0.46 * w * theta.sin(),
                0.67 * h - h * (0.122 * c + 0.077 * c * c),
            );
            Pose {
                opacity: 1. - 0.12 * c as f32,
                border: k == 0,
                ..flat(center, stack_fit(st, sizes[i]), z, z)
            }
        })
        .collect()
}

/// Rows keep the windows' native reading order and a common scale preserves relative size.
fn grid(homes: &[CGRect], area: CGRect, gap: f64) -> Vec<(CGPoint, f64)> {
    let n = homes.len();
    let fit = |rows: usize| {
        let cols = n.div_ceil(rows);
        let (w, h) = (area.size.width / cols as f64, area.size.height / rows as f64);
        homes
            .iter()
            .map(|r| ((w - gap) / r.size.width).min((h - gap) / r.size.height))
            .fold(1., f64::min)
    };
    let rows = (1..=n).max_by(|a, b| fit(*a).total_cmp(&fit(*b))).unwrap_or(1);
    let (scale, cols) = (fit(rows), n.div_ceil(rows));
    let (w, h) = (area.size.width / cols as f64, area.size.height / rows as f64);
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|a, b| homes[*a].mid().y.total_cmp(&homes[*b].mid().y));
    let used = n.div_ceil(cols) as f64;
    let top = area.origin.y + (area.size.height - used * h) / 2.;
    let mut out = vec![(CGPoint::ZERO, scale); n];
    for (row, chunk) in order.chunks_mut(cols).enumerate() {
        chunk.sort_by(|a, b| homes[*a].mid().x.total_cmp(&homes[*b].mid().x));
        let left = area.origin.x + (area.size.width - chunk.len() as f64 * w) / 2.;
        for (col, &i) in chunk.iter().enumerate() {
            out[i].0 = CGPoint::new(left + (col as f64 + 0.5) * w, top + (row as f64 + 0.5) * h);
        }
    }
    out
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
    CGRect::new(CGPoint::new(x, y), CGSize::new(w, h))
}
fn contains(r: CGRect, p: CGPoint) -> bool {
    p.x >= r.min().x && p.y >= r.min().y && p.x < r.max().x && p.y < r.max().y
}
fn color(r: f64, g: f64, b: f64, a: f64) -> CFRetained<CGColor> {
    CGColor::new_generic_rgb(r, g, b, a)
}
fn layer(frame: CGRect, scale: f64) -> Retained<CALayer> {
    let l = CALayer::layer();
    l.setFrame(frame);
    l.setContentsScale(scale);
    l
}
fn obj<T>(value: &T) -> &AnyObject { unsafe { &*(value as *const T as *const AnyObject) } }
/// Fade from the on-screen opacity, so interrupted fades never jump.
fn fade(l: &CALayer, to: f32, duration: f64) {
    if l.opacity() == to {
        return;
    }
    let from = unsafe { l.presentationLayer() }.map_or(l.opacity(), |p| p.opacity());
    l.setOpacity(to);
    let key = NSString::from_str("opacity");
    if duration <= 0. {
        l.removeAnimationForKey(&key);
        return;
    }
    let a = CABasicAnimation::animationWithKeyPath(Some(&key));
    unsafe {
        a.setFromValue(Some(obj(&*NSNumber::new_f32(from))));
        a.setToValue(Some(obj(&*NSNumber::new_f32(to))));
    }
    a.setDuration(duration);
    a.setTimingFunction(Some(&CAMediaTimingFunction::functionWithControlPoints(
        0.2, 0.85, 0.25, 1.,
    )));
    l.addAnimation_forKey(&a, Some(&key));
}
fn gradient(frame: CGRect, stops: &[(f64, f64)], white: f64) -> Retained<CAGradientLayer> {
    let g = CAGradientLayer::layer();
    g.setFrame(frame);
    let colors: Vec<_> = stops.iter().map(|(_, a)| color(white, white, white, *a)).collect();
    let colors: Vec<&AnyObject> = colors.iter().map(|c| obj(&**c)).collect();
    let locations: Vec<_> = stops.iter().map(|(l, _)| NSNumber::new_f64(*l)).collect();
    unsafe { g.setColors(Some(&NSArray::from_slice(&colors))) };
    g.setLocations(Some(&NSArray::from_retained_slice(&locations)));
    g.setStartPoint(CGPoint::new(0.5, 0.));
    g.setEndPoint(CGPoint::new(0.5, 1.));
    g
}
fn app_icon(pid: i32) -> Option<Retained<CGImage>> {
    let image = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?.icon()?;
    unsafe { image.CGImageForProposedRect_context_hints(std::ptr::null_mut(), None, None) }
}
/// The Dock's desktop-picture window covering `bounds` (global, top-left).
fn wallpaper(bounds: CGRect) -> Option<(u32, i32)> {
    let list = CGWindowListCopyWindowInfo(CGWindowListOption::OptionOnScreenOnly, kCGNullWindowID)?;
    // CFArray/CFDictionary are toll-free bridged; the retained list owns every entry.
    let list =
        unsafe { &*(&*list as *const _ as *const NSArray<NSDictionary<NSString, AnyObject>>) };
    let number = |d: &NSDictionary<NSString, AnyObject>, key: &NSString| {
        d.objectForKey(key)?.downcast::<NSNumber>().ok().map(|n| n.doubleValue())
    };
    list.iter().find_map(|entry| {
        if number(&entry, ns_string!("kCGWindowLayer"))? != kCGDesktopWindowLevel as f64 {
            return None;
        }
        let frame = entry.objectForKey(ns_string!("kCGWindowBounds"))?;
        let frame = frame.downcast::<NSDictionary>().ok()?;
        let frame = unsafe {
            &*(&*frame as *const NSDictionary as *const NSDictionary<NSString, AnyObject>)
        };
        let r = rect(
            number(frame, ns_string!("X"))?,
            number(frame, ns_string!("Y"))?,
            number(frame, ns_string!("Width"))?,
            number(frame, ns_string!("Height"))?,
        );
        (r == bounds).then_some((
            number(&entry, ns_string!("kCGWindowNumber"))? as u32,
            number(&entry, ns_string!("kCGWindowOwnerPID"))? as i32,
        ))
    })
}

/// A critically damped spring; retargeting keeps its velocity, so motion never kinks.
#[derive(Clone, Copy, Debug, Default)]
struct Spring {
    x: f64,
    v: f64,
}
impl Spring {
    fn step(&mut self, to: f64, dt: f64) {
        let w = std::f64::consts::TAU / RESPONSE;
        let (d, e) = (self.x - to, (-w * dt).exp());
        let c = self.v + w * d;
        self.x = to + (d + c * dt) * e;
        self.v = (self.v - c * w * dt) * e;
    }
}
/// Centre x, centre y, depth, turn, scale, alpha and stacking key.
type Motion = [Spring; 7];
const TOLERANCE: [f64; 7] = [0.05, 0.05, 0.05, 1e-4, 1e-4, 2e-3, 1e-3];
fn values(p: &Pose, alpha: f32) -> [f64; 7] {
    [
        p.center.x,
        p.center.y,
        p.z,
        p.turn,
        p.scale,
        alpha as f64,
        p.key,
    ]
}

/// One preview: its own window, placed by WindowServer, holding the live frame and its mirror.
struct Card {
    window_id: u32,
    process_id: i32,
    title: String,
    size: CGSize,
    home: CGRect,
    on_screen: bool,
    local: bool,
    /// Content is drawn at this fraction of the real size; placement scales it.
    content: f64,
    root: Retained<CALayer>,
    body: Retained<CALayer>,
    preview: PreviewLayer,
    reflection: Retained<CALayer>,
    mirror: Retained<CALayer>,
    _surface: WindowSurface,
    window: CgsWindow,
    pose: Pose,
    motion: Motion,
    ready: bool,
}
impl Card {
    fn new<T>(item: &SwitcherItem<T>, stage: Stage, origin: CGPoint, scale: f64) -> Option<Self> {
        let home = rect(
            item.frame.origin.x,
            item.frame.origin.y,
            item.frame.size.width.max(1.),
            item.frame.size.height.max(1.),
        );
        // Sharp at the largest size any arrangement shows it, never larger than the window.
        let content = (carousel_fit(stage, home.size) * stage.depth_scale(-0.2 * stage.unit))
            .max(stack_fit(stage, home.size))
            .min(1.);
        let (w, h) = (home.size.width * content, home.size.height * content);
        let bounds = rect(0., 0., w + 2. * PAD, h * (1. + REFLECT) + 2. * PAD);
        let root = layer(bounds, scale);
        root.setGeometryFlipped(true);
        let card = rect(PAD, PAD, w, h);
        let body = layer(card, scale);
        body.setCornerRadius(CORNER);
        body.setBorderColor(Some(&color(0.68, 0.86, 1., 0.95)));
        body.setAllowsEdgeAntialiasing(true);
        root.addSublayer(&body);
        let preview = PreviewLayer::new();
        preview.layer().setFrame(body.bounds());
        preview.layer().setCornerRadius(CORNER);
        preview.layer().setMasksToBounds(true);
        body.addSublayer(preview.layer());
        // The floor mirror shares the preview's IOSurface; no extra capture or copy.
        let reflection = layer(rect(PAD, PAD + h, w, h * REFLECT), scale);
        reflection.setMasksToBounds(true);
        reflection.setOpacity(0.);
        let mask = gradient(reflection.bounds(), &[(0., 0.3), (1., 0.)], 1.);
        unsafe { reflection.setMask(Some(&mask)) };
        let mirror = layer(rect(0., 0., w, h), scale);
        mirror.setTransform(CATransform3D::new_scale(1., -1., 1.));
        mirror.setCornerRadius(CORNER);
        mirror.setMasksToBounds(true);
        reflection.addSublayer(&mirror);
        root.addSublayer(&reflection);
        let window = CgsWindow::new_compositor(
            rect(origin.x, origin.y, bounds.size.width, bounds.size.height),
            0.,
        )
        .ok()?;
        window.set_resolution(scale).ok()?;
        window.set_level(LEVEL).ok()?;
        window.disable_shadow();
        let surface = WindowSurface::new_scaled(window.id(), bounds, &root, scale).ok()?;
        let pose = flat(home.mid(), 1., 0., 0.);
        Some(Self {
            window_id: item.window_id,
            process_id: item.process_id,
            title: item.title.clone(),
            size: home.size,
            home,
            on_screen: item.on_screen && item.local,
            local: item.local,
            content,
            root,
            body,
            preview,
            reflection,
            mirror,
            _surface: surface,
            window,
            pose: Pose { opacity: 0., ..pose },
            motion: values(&pose, 0.).map(|x| Spring { x, v: 0. }),
            ready: false,
        })
    }

    fn set_frame(&mut self, frame: &PreviewFrame) -> bool {
        if !self.preview.set_frame(frame) {
            return false;
        }
        unsafe { self.mirror.setContents(self.preview.layer().contents().as_deref()) };
        self.mirror.setContentsRect(self.preview.layer().contentsRect());
        true
    }

    /// Without capture, an app-icon plate stands in for the window.
    fn fallback(&mut self, scale: f64) {
        self.body.setBackgroundColor(Some(&color(0.16, 0.17, 0.18, 1.)));
        if let Some(icon) = app_icon(self.process_id) {
            let b = self.body.bounds().size;
            let side = b.width.min(b.height) * 0.35;
            let l = layer(
                rect((b.width - side) / 2., (b.height - side) / 2., side, side),
                scale,
            );
            unsafe { l.setContents(Some(obj(&*icon))) };
            self.body.addSublayer(&l);
        }
        self.ready = true;
    }

    fn alpha(&self) -> f32 { if self.ready { self.pose.opacity } else { 0. } }

    /// `turn` is the direction cards travel around the ring for this selection change.
    /// `snap` places the card immediately (opening over the real windows).
    fn retarget(&mut self, p: Pose, turn: Option<f64>, snap: bool) {
        let old = std::mem::replace(&mut self.pose, p);
        // A card wrapping around the ring (moving against the turn), or appearing from
        // hidden, cuts and fades in instead of sweeping through the scene.
        let cut = match turn {
            Some(turn) => (p.slot - old.slot) * turn < -1e-9,
            None => self.motion[5].x < 0.01,
        };
        if snap || cut {
            let alpha = if snap { self.alpha() } else { 0. };
            self.motion = values(&p, alpha).map(|x| Spring { x, v: 0. });
        }
        fade(&self.reflection, p.reflect, FADE);
        self.body.setBorderWidth(if p.border {
            2. * self.content / p.scale.max(0.05)
        } else {
            0.
        });
    }

    /// Advance toward the target; true while still moving.
    fn advance(&mut self, dt: f64) -> bool {
        let target = values(&self.pose, self.alpha());
        let mut moving = false;
        for ((s, to), tol) in self.motion.iter_mut().zip(target).zip(TOLERANCE) {
            if (s.x - to).abs() < tol && s.v.abs() < tol * 10. {
                *s = Spring { x: to, v: 0. };
            } else {
                s.step(to, dt);
                moving = true;
            }
        }
        moving
    }

    /// The pose this card is showing right now.
    fn current(&self) -> Pose {
        let m = self.motion.map(|s| s.x);
        Pose {
            center: CGPoint::new(m[0], m[1]),
            z: m[2],
            turn: m[3],
            scale: m[4].max(1e-3),
            opacity: m[5].clamp(0., 1.) as f32,
            key: m[6],
            ..self.pose
        }
    }

    /// Window-local forward placement: the card's centre sits at the window's content centre.
    fn placement(&self, stage: Stage, origin: CGPoint) -> CATransform3D {
        let b = self.body.frame();
        CATransform3D::new_translation(-b.mid().x, -b.mid().y, 0.)
            .concat(stage.place(&self.current(), self.content))
            .concat(CATransform3D::new_translation(-origin.x, -origin.y, 0.))
    }
}
impl Drop for Card {
    fn drop(&mut self) { release_layer_tree(&self.root); }
}

enum Phase {
    /// The backdrop is transparent until on-screen previews can replace the real windows.
    Waiting(Instant),
    Open,
    Exiting {
        commit: bool,
        start: Instant,
        target: Option<CGRect>,
    },
}

/// Not Send or Sync; create, drive and drop it on the main thread.
pub struct WindowSwitcher<T> {
    items: Vec<T>,
    display_id: u32,
    bounds: CGRect,
    scale: f64,
    stage: Stage,
    layout: SwitcherLayout,
    cards: Vec<Card>,
    selected: usize,
    opened: Instant,
    navigated: bool,
    scroll: f64,
    /// Selection when a held swipe began; sideways travel is measured from it.
    scrub: Option<usize>,
    phase: Phase,
    sources: Vec<PreviewSource>,
    captures: Option<PreviewCompositor>,
    /// Last frame time and the stacking committed to WindowServer.
    clock: Option<Instant>,
    order: Vec<usize>,
    moving: bool,
    root: Retained<CALayer>,
    backdrop: Retained<CALayer>,
    /// The desktop picture, captured once and blurred; the live-screen blur stands in until then.
    wallpaper: Option<(u32, i32)>,
    wallpaper_layer: PreviewLayer,
    floor: Retained<CAGradientLayer>,
    title: Retained<CATextLayer>,
    _surface: WindowSurface,
    window: CgsWindow,
}
impl<T> Drop for WindowSwitcher<T> {
    fn drop(&mut self) {
        let _tx = LayerTransaction::disabled().flush_on_commit();
        self.cards.clear();
        release_layer_tree(&self.root);
    }
}
impl<T> WindowSwitcher<T> {
    /// `bounds` is the switcher display's global frame; `items` must not be empty.
    /// Nothing is shown until `start`.
    pub fn new(
        display_id: u32,
        bounds: CGRect,
        scale: f64,
        layout: SwitcherLayout,
        items: Vec<SwitcherItem<T>>,
    ) -> Option<Self> {
        if items.is_empty() {
            return None;
        }
        let _tx = LayerTransaction::disabled().flush_on_commit();
        let stage = Stage::new(bounds.size, layout);
        let (w, h) = (bounds.size.width, bounds.size.height);
        let root = layer(CGRect::new(CGPoint::ZERO, bounds.size), scale);
        root.setGeometryFlipped(true);
        root.setMasksToBounds(true);
        let backdrop = layer(root.bounds(), scale);
        let blur = backdrop_blur(36.);
        if let Some(blur) = &blur {
            blur.setFrame(root.bounds());
            backdrop.addSublayer(blur);
        }
        let wallpaper_layer = PreviewLayer::new();
        wallpaper_layer.layer().setFrame(root.bounds());
        wallpaper_layer.layer().setOpacity(0.);
        if let Some(filters) = blur_filters(40.) {
            unsafe { wallpaper_layer.layer().setFilters(Some(&filters)) };
        }
        // Blurred once, then composited as a still while the title changes above it.
        wallpaper_layer.layer().setShouldRasterize(true);
        backdrop.addSublayer(wallpaper_layer.layer());
        let tint = layer(root.bounds(), scale);
        tint.setBackgroundColor(Some(&color(0., 0., 0., if blur.is_some() { 0.12 } else { 0.6 })));
        backdrop.addSublayer(&tint);
        backdrop.setOpacity(0.);
        root.addSublayer(&backdrop);
        let floor = gradient(
            rect(0., (FLOOR - 0.06) * h, w, (1. - FLOOR + 0.06) * h),
            &[(0., 0.), (0.25, 0.4), (1., 0.62)],
            0.,
        );
        floor.setOpacity(0.);
        root.addSublayer(&floor);
        let title = CATextLayer::layer();
        title.setFrame(rect(80., 0.04 * h, w - 160., 28.));
        title.setContentsScale(scale);
        let font = NSFont::systemFontOfSize(20.);
        unsafe {
            title.setFont(Some(&*(Retained::as_ptr(&font) as *const CFType)));
            title.setTruncationMode(objc2_quartz_core::kCATruncationMiddle);
            title.setAlignmentMode(objc2_quartz_core::kCAAlignmentCenter);
        }
        title.setFontSize(20.);
        title.setForegroundColor(Some(&color(1., 1., 1., 0.92)));
        title.setShadowOpacity(0.5);
        title.setShadowRadius(4.);
        title.setShadowOffset(CGSize::ZERO);
        title.setOpacity(0.);
        root.addSublayer(&title);

        let mut cards = Vec::with_capacity(items.len());
        for item in &items {
            let mut card = Card::new(item, stage, bounds.origin, scale)?;
            card.on_screen &= card.home.max().x > 0.
                && card.home.max().y > 0.
                && card.home.min().x < w
                && card.home.min().y < h;
            cards.push(card);
        }

        let window = CgsWindow::new_compositor(bounds, 0.).ok()?;
        window.set_resolution(scale).ok()?;
        window.set_level(LEVEL).ok()?;
        window.disable_shadow();
        let surface = WindowSurface::new_scaled(window.id(), root.bounds(), &root, scale).ok()?;
        Some(Self {
            items: items.into_iter().map(|i| i.data).collect(),
            display_id,
            bounds,
            scale,
            stage,
            layout,
            cards,
            selected: 0,
            opened: Instant::now(),
            navigated: false,
            scroll: 0.,
            scrub: None,
            phase: Phase::Waiting(Instant::now()),
            sources: Vec::new(),
            captures: None,
            clock: None,
            order: Vec::new(),
            moving: true,
            wallpaper: wallpaper(bounds),
            root,
            backdrop,
            wallpaper_layer,
            floor,
            title,
            _surface: surface,
            window,
        })
    }

    pub fn bounds(&self) -> CGRect { self.bounds }

    pub fn selected(&self) -> &T { &self.items[self.selected] }

    /// True while `pump` must run every display frame: cards are moving, or the switcher is
    /// waiting for previews or a handoff.
    pub fn animating(&self) -> bool { self.moving || !matches!(self.phase, Phase::Open) }

    /// Present the switcher. `wake` may run on any queue when captured frames arrive and must
    /// only schedule `pump` on the owner. Without Screen Recording (`previews` false), app icons
    /// stand in.
    pub fn start(
        &mut self,
        previews: bool,
        wake: impl Fn() + Send + Sync + 'static,
        cache: &mut Option<SwitcherCache>,
    ) {
        let _tx = LayerTransaction::disabled().flush_on_commit();
        if previews {
            let key = (self.display_id, self.bounds, self.scale);
            let cached = cache.take().filter(|c| c.key == key).map(|c| c.captures);
            self.captures = match cached {
                Some(mut m) => m.attach_to(self.window.id(), &self.root).ok().map(|()| m),
                None => PreviewCompositor::new_attached(
                    self.window.id(),
                    &self.root,
                    self.bounds,
                    self.scale,
                    wake,
                )
                .map_err(|error| tracing::warn!(?error, "Switcher capture unavailable"))
                .ok(),
            };
            self.update_sources(true);
            if let Some(m) = &mut self.captures
                && let Err(error) = m.set_visual_scene(&[]).and_then(|()| m.show())
            {
                tracing::warn!(?error, "Switcher capture startup failed");
            }
        }
        if self.captures.is_none() {
            for c in &mut self.cards {
                c.fallback(self.scale);
            }
        }
        // Start exactly over the real windows; everything else waits, transparent, at its target.
        let targets = self.targets();
        let n = self.cards.len();
        for (i, (c, mut p)) in self.cards.iter_mut().zip(targets).enumerate() {
            if c.on_screen {
                p = flat(c.home.mid(), 1., 0., (n - i) as f64 * 1e-3);
            } else {
                p.opacity = 0.;
            }
            c.retarget(p, None, true);
        }
        self.update_title();
        let _ = self.window.order_above(None);
        if self.captures.is_none() {
            self.enter();
        }
        self.frame(Instant::now());
    }

    fn targets(&self) -> Vec<Pose> {
        let (st, sel) = (self.stage, self.selected);
        let (w, h, u) = (st.size.width, st.size.height, st.unit);
        let sizes: Vec<_> = self.cards.iter().map(|c| c.size).collect();
        match self.layout {
            SwitcherLayout::Carousel => carousel(st, &sizes, sel),
            SwitcherLayout::Stack => stack(st, &sizes, sel),
            SwitcherLayout::Grid => {
                let homes: Vec<_> = self.cards.iter().map(|c| c.home).collect();
                grid(&homes, rect(0.05 * w, 0.11 * h, 0.9 * w, 0.84 * h), 0.03 * u)
                    .into_iter()
                    .enumerate()
                    // The selection lifts slightly; nothing is dimmed.
                    .map(|(i, (center, scale))| {
                        let lift = if i == sel { 1.05 } else { 1. };
                        flat(center, scale * lift, 0., if i == sel { 1. } else { 0. })
                    })
                    .collect()
            }
        }
    }

    fn apply(&mut self, turn: Option<f64>) {
        let targets = self.targets();
        for (c, p) in self.cards.iter_mut().zip(targets) {
            c.retarget(p, turn, false);
        }
        let floor = if self.layout == SwitcherLayout::Carousel {
            1.
        } else {
            0.
        };
        fade(&self.floor, floor, FADE);
        self.moving = true;
    }

    /// Commit every card's current placement, alpha and stacking in one WindowServer
    /// transaction, after advancing the springs to `now`.
    fn frame(&mut self, now: Instant) {
        let dt = self.clock.map_or(0., |t| now.duration_since(t).as_secs_f64().min(0.05));
        self.clock = Some(now);
        if !self.moving {
            return;
        }
        let Ok(t) = WindowTransaction::new() else {
            return;
        };
        let mut moving = false;
        for c in &mut self.cards {
            moving |= c.advance(dt);
            t.set_transform(&c.window, c.placement(self.stage, self.bounds.origin));
            t.set_alpha(&c.window, c.motion[5].x.clamp(0., 1.) as f32);
        }
        let mut order: Vec<usize> = (0..self.cards.len()).collect();
        order.sort_by(|a, b| {
            self.cards[*a].motion[6].x.total_cmp(&self.cards[*b].motion[6].x).then(b.cmp(a))
        });
        if order != self.order {
            let mut below = &self.window;
            for &i in &order {
                t.order_above(&self.cards[i].window, below);
                below = &self.cards[i].window;
            }
            self.order = order;
        }
        t.commit();
        self.moving = moving;
    }

    fn enter(&mut self) {
        self.phase = Phase::Open;
        self.apply(None);
        fade(&self.backdrop, 1., FADE);
        fade(&self.title, 1., FADE);
        self.update_sources(false);
    }

    fn update_title(&self) {
        let text = CFString::from_str(&self.cards[self.selected].title);
        unsafe { self.title.setString(Some(obj(&*text))) };
    }

    /// Live capture only for cards that can be seen; the selection updates fastest.
    fn update_sources(&mut self, reset: bool) {
        if self.captures.is_none() {
            return;
        }
        let targets = self.targets();
        let exiting = matches!(self.phase, Phase::Exiting { .. });
        self.sources.clear();
        for (i, (c, p)) in self.cards.iter().zip(&targets).enumerate() {
            let selected = i == self.selected;
            let shown = p.opacity > 0. && !(exiting && !selected);
            self.sources.push(PreviewSource {
                window_id: c.window_id,
                process_id: c.process_id,
                mode: if shown {
                    CaptureMode::Live
                } else {
                    CaptureMode::Snapshot
                },
                resolution: CaptureResolution::Thumbnail {
                    width: (c.size.width * c.content * self.scale).ceil().max(1.) as usize,
                    height: (c.size.height * c.content * self.scale).ceil().max(1.) as usize,
                },
                priority: if selected {
                    3
                } else if shown {
                    1
                } else {
                    -1
                },
                frame_interval: Duration::from_secs_f64(if selected { 1. / 30. } else { 1. / 8. }),
            });
        }
        if let Some((window_id, process_id)) = self.wallpaper {
            // Blurred anyway: a quarter-resolution still is plenty.
            self.sources.push(PreviewSource {
                window_id,
                process_id,
                mode: CaptureMode::Snapshot,
                resolution: CaptureResolution::Thumbnail {
                    width: (self.stage.size.width / 4.).ceil() as usize,
                    height: (self.stage.size.height / 4.).ceil() as usize,
                },
                priority: 2,
                frame_interval: Duration::from_secs(1),
            });
        }
        let m = self.captures.as_mut().unwrap();
        let result = if reset {
            m.set_sources(&self.sources)
        } else {
            m.update_sources(&self.sources)
        };
        if let Err(error) = result {
            tracing::warn!(?error, "Switcher source update failed");
        }
    }

    fn select(&mut self, index: usize) {
        if index == self.selected {
            return;
        }
        let n = self.cards.len();
        let forward = (index + n - self.selected) % n <= n / 2;
        self.selected = index;
        self.navigated = true;
        self.update_title();
        if matches!(self.phase, Phase::Open) {
            let turn =
                (self.layout == SwitcherLayout::Carousel).then_some(if forward { -1. } else { 1. });
            self.apply(turn);
            self.update_sources(false);
        }
    }

    fn step(&mut self, delta: i64) {
        let n = self.cards.len() as i64;
        self.select((self.selected as i64 + delta).rem_euclid(n) as usize);
    }

    fn hit(&self, point: CGPoint) -> Option<usize> {
        let p = CGPoint::new(point.x - self.bounds.origin.x, point.y - self.bounds.origin.y);
        let mut hits: Vec<_> = self
            .cards
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let pose = Some(c.pose).filter(|p| p.opacity > 0.)?;
                contains(self.stage.project(&pose, c.size)?, p).then_some((pose.key, i))
            })
            .collect();
        hits.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        hits.first().map(|(_, i)| *i)
    }

    fn exit(&mut self, commit: bool) {
        let selected = &self.cards[self.selected];
        let target = (commit && selected.local).then_some(selected.home);
        let n = self.cards.len();
        for (i, c) in self.cards.iter_mut().enumerate() {
            let pose = match target {
                Some(t) if i == self.selected => {
                    flat(t.mid(), t.size.width / c.size.width, 0., 1e3)
                }
                None if c.on_screen => flat(c.home.mid(), 1., 0., (n - i) as f64 * 1e-3),
                _ => Pose { opacity: 0., ..c.pose },
            };
            c.retarget(
                Pose {
                    reflect: 0.,
                    border: false,
                    ..pose
                },
                None,
                false,
            );
        }
        for l in [&*self.backdrop, &**self.floor, &**self.title] {
            fade(l, 0., FADE * 0.8);
        }
        self.moving = true;
        let origin = self.bounds.origin;
        self.phase = Phase::Exiting {
            commit,
            start: Instant::now(),
            target: target.map(|t| {
                rect(
                    t.origin.x + origin.x,
                    t.origin.y + origin.y,
                    t.size.width,
                    t.size.height,
                )
            }),
        };
        self.update_sources(false);
    }

    /// Apply one semantic command. Exiting switchers ignore input.
    pub fn command(&mut self, command: SwitcherCommand) -> Option<SwitcherOutcome> {
        if matches!(self.phase, Phase::Exiting { .. }) {
            return None;
        }
        let _tx = LayerTransaction::disabled().flush_on_commit();
        let waiting = matches!(self.phase, Phase::Waiting(_));
        match command {
            SwitcherCommand::Dismiss if waiting => return Some(SwitcherOutcome::Close),
            SwitcherCommand::Dismiss => self.exit(false),
            SwitcherCommand::Commit { release } => {
                if release && !self.navigated && self.opened.elapsed() < QUICK {
                    self.step(1);
                }
                if !waiting {
                    self.exit(true);
                }
                return Some(SwitcherOutcome::Activate);
            }
            SwitcherCommand::Step(delta) => self.step(delta),
            SwitcherCommand::Layout(forward) => {
                self.layout = self.layout.next(forward);
                self.stage = Stage::new(self.stage.size, self.layout);
                self.navigated = true;
                if !waiting {
                    self.apply(None);
                    self.update_sources(false);
                }
            }
            SwitcherCommand::Hover(point) if self.layout == SwitcherLayout::Grid && !waiting => {
                if let Some(i) = self.hit(point) {
                    self.select(i);
                }
            }
            SwitcherCommand::Click(point) if !waiting => {
                let Some(i) = self.hit(point) else {
                    self.exit(false);
                    return None;
                };
                self.select(i);
                self.exit(true);
                return Some(SwitcherOutcome::Activate);
            }
            SwitcherCommand::Scroll(delta) => {
                self.scroll += if delta.x.abs() > delta.y.abs() {
                    delta.x
                } else {
                    delta.y
                };
                while self.scroll.abs() >= 40. {
                    let step = -self.scroll.signum();
                    self.scroll += step * 40.;
                    self.step(step as i64);
                }
            }
            _ => {}
        }
        None
    }

    /// Choose the previous window without presenting anything: the quick alt-tab.
    pub fn quick(&mut self) {
        if self.cards.len() > 1 {
            self.selected = 1;
        }
    }

    pub fn begin_scrub(&mut self) { self.scrub = Some(self.selected); }

    /// Sideways travel, in working-area widths, while a swipe holds the switcher open.
    /// Returns true when the selection moved.
    pub fn scrub(&mut self, travel: f64) -> bool {
        let Some(base) = self.scrub.filter(|_| !matches!(self.phase, Phase::Exiting { .. })) else {
            return false;
        };
        let n = self.cards.len() as i64;
        let index = (base as i64 + (travel / SCRUB).round() as i64).rem_euclid(n) as usize;
        if index == self.selected {
            return false;
        }
        let _tx = LayerTransaction::disabled().flush_on_commit();
        self.select(index);
        true
    }

    /// Call on every display frame while `animating`, and after `start`'s wake: pulls captured
    /// frames, advances motion and phase timing.
    pub fn pump(&mut self) {
        let _tx = LayerTransaction::disabled().flush_on_commit();
        let waiting = matches!(self.phase, Phase::Waiting(_));
        if let Some(m) = &mut self.captures {
            if let Err(error) = m.render(Instant::now()) {
                tracing::warn!(?error, "Switcher capture update failed");
            }
            if let Some((id, _)) = self.wallpaper
                && !self.wallpaper_layer.has_frame()
                && let Some(frame) = m.captured_frame(id)
                && self.wallpaper_layer.set_frame(&frame)
            {
                fade(self.wallpaper_layer.layer(), 1., FADE);
            }
            for c in &mut self.cards {
                if let Some(frame) = m.captured_frame(c.window_id)
                    && c.set_frame(&frame)
                    && !c.ready
                {
                    // Waiting cards replace their windows in place; later ones fade in.
                    c.ready = true;
                    if waiting {
                        c.motion[5] = Spring { x: c.alpha() as f64, v: 0. };
                    }
                    self.moving = true;
                }
            }
        }
        // Present once the windows being replaced have previews, or the selection has one
        // and a short wait passed; never an empty stage for a cold capture start.
        if let Phase::Waiting(start) = self.phase
            && (self.cards.iter().all(|c| c.ready || !c.on_screen)
                || (self.cards[self.selected].ready
                    && start.elapsed() > Duration::from_millis(250))
                || start.elapsed() > Duration::from_millis(900))
        {
            self.enter();
        }
        self.frame(Instant::now());
    }

    /// Set while a committed choice is animating to its window; identifies that transition.
    pub fn activation_stamp(&self) -> Option<Instant> {
        match self.phase {
            Phase::Exiting { commit: true, start, .. } => Some(start),
            _ => None,
        }
    }

    /// Follow the activated window to where it really landed (global frame).
    pub fn retarget(&mut self, stamp: Instant, frame: CGRect) {
        let Phase::Exiting { commit: true, start, target } = &mut self.phase else {
            return;
        };
        if *start != stamp || *target == Some(frame) || !self.cards[self.selected].local {
            return;
        }
        *target = Some(frame);
        let c = &mut self.cards[self.selected];
        let center = CGPoint::new(
            frame.mid().x - self.bounds.origin.x,
            frame.mid().y - self.bounds.origin.y,
        );
        c.retarget(
            flat(center, frame.size.width / c.size.width, 0., 1e3),
            None,
            false,
        );
        self.moving = true;
    }

    /// True once exit motion has settled and `placed(window_id, global_frame)` confirms the
    /// real window sits under its preview, or after a bounded wait.
    pub fn handoff_ready(&self, placed: impl Fn(u32, CGRect) -> bool) -> bool {
        let Phase::Exiting { commit, start, target } = self.phase else {
            return false;
        };
        if start.elapsed() > Duration::from_millis(1500) {
            return true;
        }
        if self.moving {
            return false;
        }
        match target.filter(|_| commit) {
            Some(target) => placed(self.cards[self.selected].window_id, target),
            None => true,
        }
    }

    /// Keep the warm capture host for the next invocation.
    pub fn retain(mut self, cache: &mut Option<SwitcherCache>) {
        if let Some(mut captures) = self.captures.take() {
            let _ = captures.hide();
            let _ = captures.set_visual_scene(&[]);
            let _ = captures.set_sources(&[]);
            *cache = Some(SwitcherCache {
                key: (self.display_id, self.bounds, self.scale),
                captures,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carousel_matches_cover_flow_proportions() {
        // Measured from the reference recording at 1108x720.
        let stage = Stage::new(CGSize::new(1108., 720.), SwitcherLayout::Carousel);
        let sizes = [
            CGSize::new(1100., 520.),
            CGSize::new(800., 950.),
            CGSize::new(1000., 700.),
            CGSize::new(1200., 800.),
            CGSize::new(900., 600.),
        ];
        let poses = carousel(stage, &sizes, 0);
        let edges = |i: usize| {
            let (p, s) = (&poses[i], sizes[i]);
            let pt = |x: f64, y: f64| stage.point(p, x * s.width / 2., y * s.height / 2.).unwrap();
            let (lt, lb, rt, rb) = (pt(-1., -1.), pt(-1., 1.), pt(1., -1.), pt(1., 1.));
            for (a, b) in [(lt, lb), (rt, rb)] {
                eprintln!("{i}: x {:.0} y {:.0}-{:.0}", a.x, a.y, b.y);
            }
            (lb.y - lt.y, rb.y - rt.y, lt.x, rt.x, lb.y.max(rb.y))
        };
        let (front, _, left, right, bottom) = edges(0);
        assert!((left - 278.).abs() < 6. && (right - 830.).abs() < 6.);
        assert!((bottom - FLOOR * 720.).abs() < 1.);
        assert!((front - 262.).abs() < 6.);
        // Neighbours turn inward: outer edges nearer and taller, inner edges tucked behind.
        let (inner, outer, nl, _, nb) = edges(1);
        assert!(outer > inner && nl < right && nl > right - 60.);
        assert!(nb >= bottom && nb < bottom + 12.);
        let (outer, inner, _, nr, _) = edges(4);
        assert!(outer > inner && nr > left && nr < left + 60.);
        // Paint order: the selection, then its neighbours, then the next ones out.
        assert!(poses[0].key > poses[1].key && poses[1].key > poses[2].key);
        assert_eq!((poses[1].slot, poses[4].slot), (1., -1.));
    }

    #[test]
    fn grid_keeps_reading_order_and_common_scale() {
        let homes = [
            rect(900., 500., 400., 300.),
            rect(0., 0., 800., 600.),
            rect(900., 0., 400., 300.),
            rect(0., 500., 600., 300.),
        ];
        let cells = grid(&homes, rect(0., 0., 1600., 1000.), 20.);
        assert!(cells.iter().all(|(_, s)| *s == cells[0].1 && *s <= 1.));
        assert!(cells[1].0.y < cells[3].0.y && cells[1].0.x < cells[2].0.x);
        assert!(cells[3].0.x < cells[0].0.x);
    }

    #[test]
    fn layouts_cycle_both_ways() {
        let l = SwitcherLayout::Carousel;
        assert_eq!(l.next(true), SwitcherLayout::Stack);
        assert_eq!(l.next(false), SwitcherLayout::Grid);
        assert_eq!(l.next(true).next(true).next(true), l);
    }
}
