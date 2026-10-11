# cgs

[![Crates.io](https://img.shields.io/crates/v/cgs.svg)](https://crates.io/crates/cgs)
[![Tests](https://github.com/acsandmann/cgs/actions/workflows/test.yml/badge.svg?branch=main)](https://github.com/acsandmann/cgs/actions/workflows/test.yml)
[![Documentation](https://docs.rs/cgs/badge.svg)](https://docs.rs/cgs)

`cgs` is a ui library for building native macOS interfaces with AppKit

Read the [API documentation](https://docs.rs/cgs).

License: GPL-3.0.

## Window-stack tabs

`TabBar` uses one nonactivating panel and one regular `NSGlassEffectView`, with a restrained semantic material tint, without a painted backing or panel shadow. Selection and hover highlights span the equal-width tab regions, with restrained semantic fills and a subtle selected edge. Its `canBecomeKeyWindow` and `canBecomeMainWindow` overrides return false. The panel's private AppKit `_hasActiveAppearance` override returns true to keep the material active without changing actual key-window state or activating the application. This is an undocumented compatibility dependency, isolated to `CgsTabPanel`; no `isKeyWindow` override or application activation is used by the component.

Selection glides over 180 ms using one persistent lightweight highlight layer. Tab content redistributes over 200 ms on insertion/removal, with a 140 ms fade; the glass surface remains stationary. Transitions replace animations by property and retarget from presentation-layer geometry. Native buttons always use the latest logical frames and callback indices, and departing visuals cannot receive input. Resize, scrolling, hide, and Reduce Motion cancel transitions. Accessibility changes update the surface and highlight immediately. Path-like titles use middle truncation; tooltips and accessibility keep the original title.

On macOS 27, the updated panel remained non-key/non-main and did not activate the preview app during native tab actions. Diagnostics showed two selection animations with Reduce Motion off and none with it on. Reduce Transparency and Increase Contrast were also toggled through System Settings and restored to their original values; live material/edge updates worked. With Reduce Motion enabled, the preview uses immediate transitions.

The preview includes Add window, Rapid selection, and Resize strip controls. Close a hovered tab to exercise removal. Use the ordinary preview for interactive changes, and `--panel-only` to verify actual nonactivating behavior.

If Liquid Glass is unavailable, or AppKit's base `NSPanel` no longer implements that appearance selector, the bar uses a single public `NSVisualEffectView` with active state, behind-window blending, and popover material. That fallback is active vibrancy, not Liquid Glass. Selector presence cannot guarantee unchanged behavior on future macOS releases; recheck rendering when updating OS support.

The non-key panel was compared before/after the single appearance override on macOS 27.0 (26A428), and against glass in an actual active reference window. The override restored the illuminated material edges and depth. Regular provided better contrast than Clear over bright content. Native tab clicks kept the panel non-key/non-main and the preview application inactive while Orion remained frontmost. Reduce Transparency was toggled on/off: opaque rendering and live glass restoration both worked; its original off value was restored. Native focus, hit-testing, accessibility, ownership, overflow and relative-order tests passed for both the glass path and a temporary forced-fallback build. macOS 26 was not available for rendering verification.

Run `cargo run --example tab_bar -- --panel-only --dark --diagnostics` to inspect the actual floating panel. `--light`, `--clear`, and `--many` exercise appearance, material and overflow. Use `--panel-only --anchor=WINDOW_NUMBER --frame=X,Y,WIDTH` to position it over another application's window in AppKit screen coordinates; ordering does not focus that application. The preview callbacks update tab selection, while Rift remains responsible for focusing its managed windows.

For a genuinely active comparison, use `--embedded --active-reference --bright --diagnostics`; `--dim` and `--colorful` provide other backgrounds. Only this explicit reference mode activates the disposable preview's ordinary window. Its first diagnostic may precede asynchronous activation; subsequent tab actions report actual application/key-window state. The component itself never activates a window.
