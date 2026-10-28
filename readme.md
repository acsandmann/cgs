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
