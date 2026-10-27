# Metal preview architecture and native validation — 2026-10-27

The renderer keeps the existing standalone `PreviewCompositor` API. `new_attached`
uses an existing hosted CA hierarchy; `new_canvas` shares its source inventory,
textures and capture lifetime with an independently movable canvas. The source
owner controls capture; hiding a canvas pauses only its display link. Hide the
canvases before hiding the source owner. All objects stay on the main thread.

## Sources, scenes and presentation

- `PreviewSource` describes identity (WindowServer ID and PID), mode, resolution,
  priority and minimum frame interval. `set_sources` replaces an inventory;
  `update_sources` changes existing policies without discovery or inventory copies.
- `set_visual_scene` and `set_preview_style` change visual state. They do not
  reconcile streams. Legacy `set_scene` still manages both for existing consumers.
- `PreviewCamera` supplies a shared scale/translation in the vertex shader. Per-item
  matrices, clipping, opacity, rounded corners, stacking and controlled transitions
  remain supported. Camera and visual changes do not reconfigure capture.
- `captured_frame` returns an immutable retained frame handle; `reuse_frame` seeds
  another renderer without copying pixels. Retain the handle through external GPU
  reads; never mutate the capture texture or IOSurface. Newer pixels take precedence.

One coalesced wake is shared by the canvases. Capture has a separate coalesced
mailbox signal: geometry, display-link and GPU notifications do not repeatedly
lock every capture mailbox or revisit stream control. A canvas can drive the
shared pool without rendering its source owner. A native regression reproduced
the original forwarding-wake stall, then passed with the shared wake.

`CAMetalDisplayLink` supplies drawables. There is no production `nextDrawable`,
GPU wait or CPU pixel readback. Two commands per canvas bound GPU submission.
Source samples remain retained until command completion. Metal owns committed
commands, so no owner-side retained command list or completion scan is needed.

Presentation is handed to the owner after command-buffer scheduling, rather than
GPU completion. Presentation and cached-layer translation rebasing occur in a CA
transaction. This follows Apple's sequencing requirement without blocking the
owner in `waitUntilScheduled`. Completion releases samples and wakes the owner
only for backpressure or an error. See Apple's
[presentsWithTransaction contract](https://developer.apple.com/documentation/quartzcore/cametallayer/presentswithtransaction).

The existing settings remain: transactional presentation, displaySyncEnabled false,
three drawables, preferred frame latency one, and a 30–120 Hz display-link range.
Command-buffer-associated presentation was not adopted: Apple explicitly excludes
it for transactional presentation. There is no claimed win from changing drawable
count, vsync or latency settings; a full 60 Hz/120 Hz hardware sweep remains open.

## Capture and memory

The existing SCK desktop-independent single-window filters, permission checks,
PID validation, async first-frame sample capture, latest-sample mailboxes, direct
IOSurface textures, three-view texture cache and retained last-valid frames remain.

Each owner admits at most eight sources. Across owners, at most sixteen capture
jobs and 512 MiB of conservative surface reservations are allowed. Negative-priority
snapshots use at most fourteen jobs, leaving two for foreground arrivals. Reserve
accounting survives asynchronous stop and holds a configuration's high-water size
until retirement. It is not a claim about exact framework-private allocation.

The open-session frame cache is limited to 128 MiB per owner and evicts lower
priority/older frames first. Capacity failures are observable as source failures,
so evicted snapshots do not continuously recapture themselves. The existing
closed-session thumbnail cache remains 32 MiB on the owner thread. It retains no
streams or clocks. Caller-held frame handles and in-flight samples can outlive
active-source bookkeeping and must be included in consumer memory accounting.

Thumbnail reconfiguration has an asymmetric dead band: ordinary growth up to
12.5% and shrinkage down to 60% retain the current resolution when aspect ratio is
stable. Increasing importance can upgrade immediately. Paused snapshots can
upgrade asynchronously while presenting their previous frame. Explicit refresh
also refreshes retained snapshots. Fresh foreground work precedes speculative
refresh, including when reopening with cached pixels.

## Rift integration

Rift uses one CGS overview window/surface per display. Workspace canvases attach
inside its existing clipped workspace layers and share one capture owner. Vertical
scrolling moves the workspace parent; horizontal scrolling translates only that
workspace's cached Metal canvas, in the same transaction as its card/caption layers.
A later GPU frame rebases the translation atomically. Rendering is independent of
window hosting; CGS contains no workspace, selection or keyboard policy.

Stable horizontal movement updates retained card/drag-target geometry without
allocating another projection. Source/scene reconstruction occurs at membership
boundaries and waits for an available display update. Hover changes touch affected
entries; source policy updates contain only changed identities. Hidden identities
are capture requests, rather than invisible rendering quads.

Rift requests 60 fps for selected/dragged sources and 30 fps for other live sources;
these are independent of display-rate geometry motion. Hidden/clipped sources use
snapshots. Dragging can reuse a retained source frame across displays and move its
cached canvas with the caption. Paused canvas contexts are retained within Rift's
256 MiB closed presentation-cache reservation, including workspace drawable pools.

The user explicitly requested removal of screenshot fallback. Rift's old SCK
session, discovery, result channel, capture scheduler, CGImage resizing/copying,
image cache and direct SCK dependency are removed. App icons and CA captions remain.
There is no fallback capture path when Metal or permission is unavailable.

## Evidence and limits

Raw native trials and limitations are recorded in
[`metal-preview-2026-10-27.json`](../benchmarks/metal-preview-2026-10-27.json).

Three alternating trials per side used identical standalone release profiles and
locks on an M2 Max/macOS 27.0, with one built-in ProMotion display. Medians of
per-trial results follow. RSS was sampled at the measurement boundary; it includes
source fixtures and is not total WindowServer/SCK/GPU allocation.

| Windows | First hardware present before/after (ms) | Capture-to-present p95 before/after (ms) | Measurement RSS before/after (MiB) |
| --- | --- | --- | --- |
| 1 | 369.0 / 336.8 | 42.74 / 44.20 | 68.0 / 68.1 |
| 5 | 458.0 / 591.6 | 43.43 / 44.08 | 83.8 / 83.4 |
| 10 | 577.4 / 534.0 | 53.18 / 48.62 | 103.3 / 103.3 |
| 20 | 625.6 / 613.9 | 53.76 / 48.28 | 141.5 / 141.4 |

These trials do **not** establish a general cold-start or encoding speedup.
Startup is still variable: the five-window attached workload was slower, while
the other sampled medians improved. Ten/twenty-window capture-to-present p95 fell
about 9–10%; smaller workloads did not improve. Late admission-limit bookkeeping
does not affect this single-owner workload. Initial profile-mismatched timing
runs were rejected as performance comparisons.

A later five-window diagnostic trial measured encoding p50/p95/p99 139/192/310 us,
GPU 120/159/1343 us, callback-to-owner 13/25/42 us, submission-to-present
33.06/37.04/37.24 ms, and capture-to-present 38.57/40.56/43.23 ms. GPU execution and
the owner handoff were small compared with presentation buffering. Hardware minus
display-link target was 8.33/12.50/12.50 ms. This is one diagnostic trial, not an
input-to-photon latency claim or a refresh-rate certification.

The scrolling improvement is independently observable: native screenshots taken
without yielding the owner show a 160-pixel horizontal move of only the intended
canvas, then a 160-pixel vertical move of both canvases, with **zero GPU submissions**
between input and screenshot. GPU rebase and immediate reversal preserve bounds.
Two canvases shared five streams; after closing, capture jobs/reservations were zero.
The initial attached implementation had an inverted cached Y translation; this
probe caught it and now verifies the correct hierarchy orientation.

The measurement-only drain also observed four baseline jobs still pending after
300 ms in one failed run. Completed trials reached zero by a two-second async
drain; production never blocks waiting for it.

Native tests also exercise offscreen/parked sources, transitions and reversal,
static snapshots, source policy changes without submission, snapshot quality
upgrades without losing valid pixels, resize, destruction, closing while work is
pending, repeated reopen, and capture/resource shutdown. GPU readback tests the
shipped metallib for camera, transforms, clipping, corners and premultiplied alpha.
Some static CA fixtures continue producing SCK Complete buffers after their explicit
animations stop; zero live-capture work is not claimed for that fixture. Snapshot
idle and closed rendering activity are asserted to stop.

The real-window capture harness captured all eleven managed windows in every
before/after integration trial and reported zero active frame bytes after hide.
Its profile-dependent CPU/RSS is not used as an equivalent-build comparison.
Native Spaces/fullscreen, multiple displays/reconnects, lock/sleep/wake, permission
revocation and a fixed-60-Hz/120-Hz sweep remain unverified: the VM tools are present
but no guest is provisioned. No host permission or display topology was changed.

## Reproduce

```sh
cargo build --release --example metal_preview
python3 benchmarks/metal_preview.py --binary target/release/examples/metal_preview --attached --out /tmp/metal-trials
# Isolate the three-second measurement from later lifecycle assertions:
target/release/examples/metal_preview --self-test 3 20 --attached --measure-only
# Run lifecycle and visual probes:
target/release/examples/metal_preview --self-test 3 5 --attached --scroll-probe /tmp/metal-scroll
uv run --with pillow python benchmarks/check_scroll.py /tmp/metal-scroll
target/release/examples/metal_preview --self-test 2 5 --canvas-probe /tmp/metal-canvases
uv run --with pillow python benchmarks/check_scroll.py /tmp/metal-canvases --canvases
cargo test --lib
```

Future work should measure the rebuilt Rift event path itself (HID timestamp to
hardware presentation), run the missing native matrix, and account for the active
workspace drawable pools on large/multiple displays. Independently movable cached
canvases trade additional layer/drawable pools for synchronous geometry; they do
not add capture streams. No render graphs, atlases, compute passes or indirect
command buffers were added. A future spatial overview can use one canvas/camera
when movement is shared, and multiple canvases only for independently moving content.
