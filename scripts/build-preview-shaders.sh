#!/bin/sh
# Regenerate the shipped macOS 14+ Metal 2.4 library after editing preview.metal.
set -eu
cd "$(dirname "$0")/.."
air=$(mktemp -t cgs-preview).air
trap 'rm -f "$air" "${air%.air}"' EXIT
xcrun -sdk macosx metal -std=macos-metal2.4 -mmacosx-version-min=14.0 -c src/compositor/preview.metal -o "$air"
xcrun -sdk macosx metallib "$air" -o src/compositor/preview.metallib
