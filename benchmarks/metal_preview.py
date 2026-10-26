#!/usr/bin/env python3
"""Run real native capture/render/lifecycle workloads; no synthetic timing mocks.
Build: cargo build --release --example metal_preview
Run: python3 benchmarks/metal_preview.py --binary ../target/release/examples/metal_preview --out /tmp/cgs-bench
The fixture animates CGS source windows. CPU/RSS include those fixtures, not only the compositor.
Ten/twenty-window scenes intentionally exercise admission: eight independently live sources,
with one native-resolution capture and seven thumbnails; excluded sources report capacity.
"""
import argparse, json, pathlib, re, subprocess
p = argparse.ArgumentParser()
p.add_argument('--binary', type=pathlib.Path, required=True)
p.add_argument('--out', type=pathlib.Path, required=True)
p.add_argument('--seconds', type=float, default=5)
a = p.parse_args()
a.out.mkdir(parents=True, exist_ok=True)
results = []
for count in (1, 5, 10, 20):
    run = subprocess.run(['/usr/bin/time','-l',str(a.binary.resolve()),'--self-test',str(a.seconds),str(count)], capture_output=True, text=True)
    output = run.stdout + run.stderr
    (a.out / f'{count}-windows.log').write_text(output)
    if run.returncode: raise SystemExit(f'{count}-window workload failed; see {a.out}')
    record = {'windows':count}
    for key in ('captured_frames','replaced_frames','submitted_frames','presented_frames','streams','process_streams','latest_frame_bytes','reserved_capture_bytes','first_frame_us','last_callback_us','last_capture_to_gpu_us'):
        match = re.search(r'\b'+key+r': (\d+)', output)
        if match: record[key] = int(match[1])
    for key in ('cpu_render','gpu_render','capture_to_present'):
        match = re.search(key+r': FrameTimes \{ samples: (\d+), p50_us: (\d+), p95_us: (\d+), p99_us: (\d+) \}',output)
        if match: record[key] = dict(zip(('samples','p50_us','p95_us','p99_us'),map(int,match.groups())))
    match = re.search(r'(\d+)\s+maximum resident set size',output)
    if match: record['process_peak_rss_bytes'] = int(match[1])
    match = re.search(r'([\d.]+) real\s+([\d.]+) user\s+([\d.]+) sys',output)
    if match: record['process_time_seconds'] = dict(zip(('real','user','system'),map(float,match.groups())))
    results.append(record)
    print(json.dumps(record),flush=True)
(a.out/'results.json').write_text(json.dumps(results,indent=2)+'\n')
