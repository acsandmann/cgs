# Metal window previews

`PreviewCompositor` is available in the normal `cgs` build. No Cargo feature,
configuration flag, or environment variable enables it. It requires macOS 14+
(`CAMetalDisplayLink`), a Metal device, and existing Screen Recording authorization.
It does not request or bypass TCC authorization. Its CGS hosting uses the existing
private SkyLight surface binding; SIP remains enabled.

```rust,ignore
let mut compositor = PreviewCompositor::new(display_bounds, backing_scale, wake_owner)?;
compositor.set_scene(&[
    Preview::window(window_server_id, owning_pid, display_local_frame),
])?;
compositor.show()?;
// On each coalesced wake, on the creating main thread:
compositor.render(std::time::Instant::now())?;
// Before hiding real windows for a transition, wait for is_ready().
compositor.transition_to(&destination, Transition::new(duration, Easing::EaseInOut))?;
// For an interactive transition:
compositor.set_progress(progress)?;
compositor.hide()?;
```

The wake callback may run on a framework queue. It must enqueue owner-thread work,
not reenter the compositor. There is one dirty-driven `CAMetalDisplayLink`; callers
must not add a timer or another display clock. Hidden compositors stop capture,
pause the link, and discard pending presentations. Up to 32 MiB of thumbnail samples
and texture views may survive close in an owner-thread reuse cache; native capture
samples are excluded. Streams stop, and outstanding GPU commands independently
retain their samples through completion. Expensive immutable Metal state is shared
across displays and reopenings.

## Capture and bounds

Single-window desktop-independent filters discover offscreen windows
with `onScreenWindowsOnly = false`, check the owning PID, ignore desktop clipping
and shadows, and deliver premultiplied sRGB BGRA frames. An uncached source gets one
`SCScreenshotManager.captureSampleBuffer` request for its initial frame. This is an
IOSurface-backed CMSampleBuffer, not a CGImage. Live sources then continue through
SCStream; one-shot completion cannot overwrite a newer stream frame. Snapshot sources
need no SCStream. No screenshot loop drives continuous capture. Complete frames replace a
one-sample mailbox; idle/blank/stopped/error notifications do not replace the last
valid frame. `source_status()` reports source state and freshness. An application
or locked/sleeping graphical session can stop producing content; this is not
reported as fresh live capture.

CVPixelBuffer -> IOSurface -> `newTextureWithDescriptor:iosurface:plane:` imports
storage directly. The backend performs no CGImage creation, CPU pixel readback,
bitmap scaling, texture upload, or intermediate image copy. At most three imported
texture views per source are cached. A retained **CMSampleBuffer**, not just an
IOSurface or texture, pins the capture pool entry while the GPU reads it.

There are at most eight source slots plus pending factories per compositor, sixteen
capture jobs per process including asynchronous retirement, and two GPU submissions
in flight. `process_streams` counts actual streams, while `process_capture_jobs` also
includes pending factories and one-shot requests.
The active-source budget reserves 256 MiB for eight aligned BGRA surfaces per
source (queue, mailboxes, texture views, and GPU retention). Previous GPU submissions
can briefly outlive admission. Presentation drawables and framework-private memory
are additional; `reserved_capture_bytes` is a reservation, not measured resident
memory. `latest_frame_bytes` counts active latest samples and retained cached frames. Capture admission failure
is explicit in per-source status; native requests are never silently downscaled.

`Live` retains/reuses a stream; `Snapshot` captures a single GPU sample and retains
it without creating a stream; `Inactive` releases its active source. Sources removed
from a scene retain one last valid GPU frame per window until the compositor hides,
without keeping their stream active. There is no session eviction by count or bytes.
Returning sources display that frame immediately while live capture restarts. Fresh
frames replace cached ones; discovery prunes destroyed windows and changed owners.
Hiding clears the compositor's session cache, retaining only bounded thumbnail
reuse entries (32 MiB total, at most 4 MiB per entry) for future openings. During the
open session, cached memory scales with the windows visited and their capture
resolution, in addition to the active-source reservation above. Priority selects admitted
sources. `Native` uses source scale and content metadata; `Thumbnail` requests
capture-system scaling. Resize/scale metadata can update native capture dimensions.
`refresh_sources()` asynchronously rediscovers sources and retries failed streams;
call it for authoritative window/display inventory changes. No inventory polling
thread runs. Each compositor keeps descriptions only for requested windows; the shared
short-lived reuse inventory retains the last discovery result.

## Transactional scrolling

Call `translate(delta)` inside the same `LayerTransaction` that moves sibling CA
content. It immediately translates the already-presented Metal canvas, including
clipping, without acquiring a drawable, capturing, waiting for the GPU, or rebuilding
per-window layers. Replacement GPU frames carry the scene's translation offset.
After GPU completion, the owner presents the drawable and rebases that offset in
one CA transaction with `presentsWithTransaction` enabled. There is no main-thread
`nextDrawable`, `waitUntilScheduled`, or `waitUntilCompleted`.

Readiness requires a real drawable presentation callback for a complete current
scene. Resize, hide, and destructive scene changes invalidate pending presentation
epochs so old work cannot resurrect cleared content. Scene culling preserves the
already-presented canvas until its replacement arrives instead of hiding all previews. Core Animation handles the
cached translation while Metal supplies new pixels. Rift retains fallback screenshots
across viewport changes, including results that complete after scrolling away. A new
Overview session refreshes them once; scrolling within that session reuses them.

## Validation and profiling

```sh
cargo test --manifest-path cgs/Cargo.toml --lib metal_pixels
cargo test --manifest-path cgs/Cargo.toml --test metal_preview
cargo run --release --manifest-path cgs/Cargo.toml --example metal_preview -- --self-test 5 5
cargo run --manifest-path cgs/Cargo.toml --example metal_preview -- --list
# Capture an existing application's window without moving it:
cargo run --release --manifest-path cgs/Cargo.toml --example metal_preview -- WINDOW_ID PID 10
# Pixel probe: cached movement must occur without another GPU submission:
cargo run --manifest-path cgs/Cargo.toml --example metal_preview -- --self-test 3 1 --scroll-probe /tmp/cgs-scroll
python3 cgs/benchmarks/metal_preview.py --binary target/release/examples/metal_preview --out /tmp/cgs-bench
```

Native presentation tests require an **unlocked, awake graphical session**. The
fixture creates its own CA windows, allows initial backing content to draw, then
parks them before capture begins. The compositor never moves a source window.
The fixture covers visible/occluded, one-pixel-parked, fully offscreen, simultaneous
sources, timed/interactive reversal, snapshot stop/restart, automatic native resize,
source destruction, hidden inactivity, and repeated teardown. Ten/twenty-window
scenes exercise admission; they do not claim twenty simultaneous native streams.

The GPU pixel test renders the shipped shader into a real Metal target and checks
transform, clipping, rounded-corner, alpha, and stacking results. CPU readback exists
only in that test. Percentiles use a fixed rolling 1024-sample reservoir.
Capture-to-present uses SCK WindowServer `displayTime` and actual MTLDrawable
`presentedTime` for newly used frames; zero/skipped presentations are excluded.

For profiling, launch normally under the authorized graphical application, then
attach Instruments / Metal System Trace. Launching through another tool may have
a different TCC authorization context. Record refresh mode, display scale, active
applications, thermal/power state, WindowServer load, and whether the session is
locked. The benchmark's CPU/RSS includes animated source fixtures. It does not
claim energy or WindowServer attribution from its process timings.

The 10 KiB shader library is shipped precompiled for Metal 2.4 / macOS 14+, so
consumers do not need the optional Xcode Metal compiler component. After editing
the source, regenerate it with `sh cgs/scripts/build-preview-shaders.sh`.

## Current verification scope

Development ran on macOS 27.0 (26A428), Apple M2 Max / 32 GiB, SIP enabled. The
initial asynchronous-presentation implementation captured a real Activity Monitor
window already parked at (1511, 930), full source size 740x945 logical points,
without moving it. Its imported latest sample occupied 11,370,240 bytes. The full
window was visually present in the CGS-hosted Metal overlay. Earlier native lifecycle
runs passed; an unlocked real-window opening demo later presented 99 frames in two seconds
through the transactional path. This verifies native presentation, not the full
Rift UI workflow or scrolling correctness.

A five-source release run of the earlier asynchronous variant measured CPU
submission p50/p95/p99 139/183/198 us, GPU 117/160/289 us, and capture-to-present
46.5/56.5/60.2 ms. An experiment disabling additional layer display synchronization
measured 38.0/43.1/46.2 ms capture-to-present; two drawables performed worse than
three in that workload. These are single-run development measurements, **not final
transactional-backend benchmarks or 120 Hz certification**.

Still unverified: final scroll rebase in Rift, real Rift Overview runtime,
60/120 Hz controlled comparison, other macOS versions, Intel/multi-GPU devices,
fullscreen Spaces, display reconnection, sleep/wake recovery, TCC denial/revocation,
long-duration resident/IOSurface stability, energy/GPU/WindowServer utilization,
and a complete screenshot-fallback performance comparison. The existing screenshot
cache remains Rift's fallback; workspace switching itself is not yet animated by
this backend.

References inspected: Apple's ScreenCaptureKit sample and WWDC22 session 10155;
SketchyBar `window.c`, `surface.c`, `layer.m`; WebKit `IOSurface.mm`; Spacewalk
`DisplayCapture.swift` and transition staging; macompiz `LiveCapture.swift` and
`ScreenCapture.swift`. Spacewalk's display-wide stream is not a substitute for
independent offscreen-window filters, and macompiz's CVMetalTextureCache approach
was considered but not benchmarked against direct imports here.


Rift submits every managed source when Overview opens. Visible cards have higher
priority and receive live streams first; offscreen cards use queued one-shot samples.
The pool reserves room for background progress while warming, so lower-priority
requests are queued rather than silently truncated. Capture of the remaining windows
does not gate presentation of ready foreground frames. Every captured frame remains
cached until close. Standard selected cards request their actual Retina pixel size,
rather than unnecessarily capturing a much larger full native window. Drag previews
and the generic API can still request native capture.

The screenshot fallback stays dormant for healthy Metal sources, avoiding duplicate
discovery and capture. Failed visible sources without a usable retained frame may use it independently. Stream/filter
construction runs on bounded GCD tasks; owner-side admission precedes initial capture
and stream start. Cold Live sources first use a one-shot slot; after initial import,
the retained frame stays visible while a Live stream is constructed. Start,
configuration, and stop operations run on one serial control
queue, with completion-owned resources and ordered retirement if Overview closes
during startup. Updates keep the previous frame until new-size storage arrives. Hidden snapshot arrivals
do not trigger redraws of an unchanged foreground scene.

`PreviewCompositor::prepare()` starts discovery before the caller builds its UI.
Compositors share one immutable metadata inventory, reusable for five seconds and
explicitly refreshed for authoritative source changes. Unknown IDs force discovery;
PID ownership is checked. No stream, pixel cache, polling thread, or periodic clock
is kept active by this metadata cache. The most recent inventory remains retained
until replaced; its reuse lifetime is distinct from pixel-frame lifetime.

## Capture-all startup benchmark

Run from Rift's repository root:

```sh
cargo build --release --manifest-path cgs/Cargo.toml --example capture_all
python3 tests/benchmark_overview_capture.py --out /tmp/overview-capture --trials 3
```

The script queries all workspaces for each connected Rift display, writes only
WindowServer IDs (no application names/titles), and starts a new benchmark process
per trial. The cgs example accepts that optional generic ID file; cgs itself has no
Rift dependency. It measures initial discovery through the last complete SCK frame
imported by the compositor, including each bounded batch's stream startup/discovery.
Snapshots release active streams after their first frame; all captured frames remain
cached until hide. This is capture readiness, **not actual overlay presentation**.
It includes offscreen sources without moving them. Repeated processes may benefit
from warm system discovery; these are not reboot-cold trials.

On the development M2 Max / macOS 27.0 system, three trials per setting captured all
11 Rift-managed windows present at inventory time (10 workspaces):

| Capture | Concurrent streams | All-ready median (range) | Retained frames |
| --- | ---: | ---: | ---: |
| 640x480 thumbnail | 8 | 1.745 s (1.621–1.823 s) | 12.89 MiB |
| 640x480 thumbnail | 2 | 2.320 s (2.244–2.335 s) | 12.89 MiB |
| Native | 1 | 3.090 s (2.909–3.175 s) | 205.31 MiB |

All nine trials reported zero failed windows and zero retained frame bytes after
hide. Reported frame memory counts IOSurface storage retained by samples, not total
process/WindowServer/GPU memory. Native concurrency 2 in a preliminary 12-window run
rejected three windows under the active 256 MiB admission budget; serial native
capture avoids competing with another large source for that budget.

An unfiltered preliminary scan found 167 normal-layer window descriptions, of which
155 returned Blank rather than complete capture frames within the 3-second per-batch
timeout. This scan is not a measure of Rift's managed-window readiness. A separate
initial 12-managed-window thumbnail run succeeded in 1.997 s; the managed inventory
changed to 11 before the repeated measurements above.

These historical measurements motivated retaining thumbnails across Overview
openings while suspending capture when closed. The current implementation keeps
**all** GPU and fallback frames for the open session and a bounded 32 MiB GPU
thumbnail cache between sessions. Native frames are excluded from that reuse cache.
Rift's existing small fallback cache still trims on close. Machine-readable trial output is written
to the chosen output directory; no final scroll/presentation validation is implied.

The recorded nine-trial summary is checked in at `cgs/benchmarks/capture-all-results.json`.


## Startup optimizations (2026-10-26)

The following native measurements motivated the changes above, on the same M2 Max:

- Synchronous filter setup reached 83,348 us and stream construction 49,368 us on
  the owner thread. These operations now run in bounded factories; existing source
  descriptions and stream configuration are reused.
- A separate one-shot sample benchmark captured all 11 requested windows with
  IOSurface-backed storage: first sample 247.824 ms, all 602.169 ms, including
  discovery, at concurrency eight. Concurrency two took 672.401 ms for all 11.
- A prototype of the integrated initial-sample path with three foreground live
  sources captured all three in 287.193 ms from scene submission, and all 11 in
  946.133 ms. Its maximum owner capture-pump call was 209 us; hide reported zero
  retained frame bytes. This was a single preliminary run, before the final shared
  metadata cache and admission/retirement race guards. It is not a final latency claim.

The Mac locked during that revision's repeated cold/reopen run. It produced no
valid frames and was stopped; those timings are excluded. Subsequent unlocked
measurements for the newer implementation are reported below. No tests were added
or restored, per the user's instruction; default builds were checked.

Repeat the queued foreground/background and same-process metadata reuse measurement:

```sh
cargo build --release --manifest-path cgs/Cargo.toml --example capture_all
python3 tests/benchmark_overview_capture.py --foreground 3 --warm --timeout 8 --out /tmp/overview-startup
# Isolate the one-shot sample-buffer path:
target/release/examples/capture_all sample 8 8 /tmp/overview-startup/window-ids.txt
```

The foreground count models a capture workload; it does not claim the harness uses
Rift's exact card projection or measures keyboard-to-visible presentation latency.
Native preference remains available; `mixed` in the harness uses one native source
and thumbnails for the others. The script rejects incomplete trials rather than
including timeout durations in its performance summary.

Apple documents the single-frame buffer API in
[captureSampleBuffer](https://developer.apple.com/documentation/screencapturekit/scscreenshotmanager/capturesamplebuffer(contentfilter:configuration:completionhandler:)).

## Desktop-to-Overview opening and reusable pixels (2026-10-26)

Rift passes actual active-workspace window bounds, converted to display-local
coordinates, to `PreviewCompositor::show_animated(&origin, transition)`. The existing
scene is the destination. A critically damped 320 ms GPU transition interpolates
geometry, opacity, corner radius, and borders. Full-display clipping stays in effect
until movement finishes, then the destination ribbon clip applies. Inactive-workspace
cards fade into their destination geometry. Capture settings always come from the
destination, so a desktop-sized origin does not accidentally request native buffers.
The timeline waits for the first visible source frame. Re-submitting its destination or changing hover borders preserves the animation;
geometry/capture-setting changes cancel it immediately. Rift
respects Reduce Motion and retains transactional scrolling.

The behavioral reference was OmniWM's [`OverviewAnimator.swift`](https://github.com/OmniNull/OmniWM/blob/main/Sources/OmniWM/Core/Overview/OverviewAnimator.swift),
[`OverviewNativeTransition.swift`](https://github.com/OmniNull/OmniWM/blob/main/Sources/OmniWM/Core/Overview/OverviewNativeTransition.swift),
and [`OverviewLayerRenderer+Motion.swift`](https://github.com/OmniNull/OmniWM/blob/main/Sources/OmniWM/Core/Overview/OverviewLayerRenderer%2BMotion.swift).
This implementation uses the existing cgs scene renderer rather than introducing
per-window Core Animation presentation layers or another animation clock.

The owner-thread reuse cache matches WindowServer ID and owning PID supplied by
the caller. Cached pixels are available before asynchronous discovery finishes;
discovery prunes destroyed sources and changed owners. They are last-valid pixels,
not fresh live capture. Snapshot sources refresh once per session, while Live
sources start streams. Sample age remains observable through source status. No
capture stream or display clock is kept running to preserve cached pixels.
Framework-private memory and presentation drawables remain additional to the
32 MiB retained-frame budget. The cache can stay allocated while Overview is closed.

The capture-all harness separately records reuse readiness, first drawable
presentation, and fresh sample arrivals. `warm_summary` requires a new sample
received during the new session; reuse must not masquerade as a fresh capture.
An optional final argument after `warm` delays reopen, exercising expired metadata.
The `metal_preview WINDOW_ID PID SECONDS --from X Y WIDTH HEIGHT` demo exercises
opening from supplied display-local geometry without moving the source window.

A startup experiment compared background `nextDrawable` acquisition before display-
link attachment against normal `CAMetalDisplayLink` pacing. Attaching a display link
before calling `nextDrawable` throws `CAMetalLayerInvalidOperation`, even if paused.
The corrected direct-acquisition experiment was slower, so it was removed. In three
alternating unlocked runs per path with ten windows, hardware-timestamp first
presentation on reopen was 105/250/210 ms with the display link (median 210 ms),
versus 360/272/259 ms for direct acquisition (median 272 ms). Reusable pixels were
available in 49/68/52 ms on the display-link runs. These measure compositor startup
and presentation, not the entire Rift UI opening time. The retained pacing code
remains the single display link. A blank first scene does not consume a drawable.

Presentation startup now uses the first actual `MTLDrawable.presentedTime`, relative
to Metal initialization. Checking the counter from a run-loop observer can report a
later time and is not used as the presentation timestamp. Source fresh-sample times
and cached-pixel readiness remain separate. The initial reuse-only nine-window
benchmark measured 44–61 ms availability; it did not measure on-screen appearance.

The visible-root startup experiment likewise had no consistent benefit (thumbnail
first presentation 91/255/266 ms, median 255 ms) and was reverted. The final code
keeps the previous hidden-until-first-presentation behavior. A six-second delayed
reopen, after shared discovery metadata expired, had all ten cached thumbnails
available in 16.6 ms; first hardware presentation was 402.4 ms and fresh foreground
samples finished in 591.3 ms in that single run. This exposes remaining WindowServer/
initial-presentation variability; cached availability is not an instant-presentation
claim. The recorded comparison and experiment results are in
`cgs/benchmarks/overview-startup-2026-10-26.json`. Final Rift UI opening/interaction
still requires checking in the running rebuilt application.

The final two-second real-window opening demo captured a native 960x945-point
application window without moving it, presented 71 frames, and reported actual
first presentation at 264.5 ms from Metal initialization. CPU submission p50/p95/p99
was 58/145/717 us, GPU 202/445/1205 us, and capture-to-present 33.3/42.2/45.0 ms.
This is a short native demo, not a full Rift workflow or refresh-rate certification.
