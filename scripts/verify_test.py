#!/usr/bin/env python3
"""Verify MixID end-to-end: analyze test_data/mix.wav and check detections
against expected.json. Exit 0 on success, 1 on failure."""

import json
import os
import subprocess
import sys

ROOT = os.path.join(os.path.dirname(__file__), "..")
DB = os.path.join(ROOT, "test_data", "test.db")
MIX = os.path.join(ROOT, "test_data", "mix.wav")

if os.path.exists(DB):
    os.remove(DB)


def run(*args):
    r = subprocess.run(args, capture_output=True, text=True, cwd=ROOT)
    if r.returncode != 0:
        print(r.stdout)
        print(r.stderr, file=sys.stderr)
        sys.exit(1)
    return r.stdout


run(
    "cargo",
    "run",
    "--release",
    "-p",
    "mixid-cli",
    "--",
    "--db",
    DB,
    "index",
    "test_data/songs",
)
out = run(
    "cargo",
    "run",
    "--release",
    "-p",
    "mixid-cli",
    "--",
    "--db",
    DB,
    "analyze",
    MIX,
    "--json",
)
result = json.loads(out)

with open(os.path.join(ROOT, "test_data", "expected.json")) as f:
    expected = json.load(f)

dets = result["detections"]
print(f"\n{len(dets)} detections:")
for d in dets:
    print(
        f"  [{d['t_start']:7.2f} - {d['t_end']:7.2f}] {d['title']} ({d['confidence']:.3f})"
    )

failures = []
for exp in expected:
    name = exp["file"].replace(".wav", "")
    matches = [d for d in dets if d["title"] == name]
    if not matches:
        failures.append(f"{name}: NOT detected")
        continue
    if len(matches) > 1:
        failures.append(f"{name}: detected {len(matches)} times (expected 1)")
    d = matches[0]
    # crossfades (5s) mean detection boundaries can lag nominal windows
    if abs(d["t_start"] - exp["start"]) > 6.5:
        failures.append(
            f"{name}: t_start {d['t_start']:.2f} vs expected {exp['start']} (±6.5s)"
        )
    if abs(d["t_end"] - exp["end"]) > 6.5:
        failures.append(
            f"{name}: t_end {d['t_end']:.2f} vs expected {exp['end']} (±6.5s)"
        )
    if (d["t_end"] - d["t_start"]) < 20.0:
        failures.append(f"{name}: duration {d['t_end'] - d['t_start']:.1f}s < 20s")
    if d["confidence"] < 0.25:
        failures.append(f"{name}: confidence {d['confidence']:.3f} < 0.25")

decoys = [d for d in dets if d["title"] == "songE"]
if decoys:
    failures.append(f"songE (decoy, not in mix) falsely detected: {decoys}")

if failures:
    print("\nFAIL:")
    for f_ in failures:
        print(f"  - {f_}")
    sys.exit(1)
print("\nPASS: all 4 songs detected at correct times, decoy rejected.")
