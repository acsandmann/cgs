# cgs

[![Crates.io](https://img.shields.io/crates/v/cgs.svg)](https://crates.io/crates/cgs)
[![Tests](https://github.com/acsandmann/cgs/actions/workflows/test.yml/badge.svg?branch=main)](https://github.com/acsandmann/cgs/actions/workflows/test.yml)
[![Documentation](https://docs.rs/cgs/badge.svg)](https://docs.rs/cgs)

`cgs` is a ui library for building native macOS interfaces with AppKit

Read the [API documentation](https://docs.rs/cgs).

Window previews can use the standalone/attached Metal `PreviewCompositor`, or
`PreviewLayer` to present the same captured IOSurfaces directly in an existing CA
hierarchy. A compositor with an empty visual scene owns capture only; its
`captured_frame` handles feed `PreviewLayer::set_frame`. Moving the CA tree never
submits Metal work or reconfigures capture. Layers pin their capture samples and
keep previous samples alive through the replacing CA transaction.

The native `metal_preview --self-test 2 5 --surface-probe OUT` harness checks parked
sources and stopped-capture transforms without GPU submissions. Validate its
screenshots with `uv run --with pillow python benchmarks/check_scroll.py OUT --surfaces`.

License: GPL-3.0.

## Choosing tabs and segments

Use `tabs::TabSelector` for glass pill tabs and settings choices (including icon
choices such as Top/Bottom). It uses AppKit’s tab-strip component, including its
rounded selection, tracking, and accessibility. `controls::SegmentedControl` is
for conventional segmented controls; wrapping it in `GlassEffectView` does not
turn its rectangular selection into native glass tabs. Do not stack glass behind
a `TabSelector` or override its segment style.

## Numeric inputs

`NumberField` includes native up/down stepping in its input bezel. Units belong
outside the bezel: use `NumberField::unit_field` or `SettingsRow::suffix`.
`NumberSlider` hides the redundant stepper automatically. When composing a slider
and field manually, call `field.hide_stepper()`. Size the numeric field itself,
not the combined field-and-unit wrapper, so units retain their natural width.
