#!/bin/bash
# Saves the I/O Registry subtree of the built-in SD card reader, plus what
# system_profiler reports for it, as test fixtures for the I/O Registry
# mapping (src/platform/macos/map.rs). macOS only; no root needed.
#
# Usage: scripts/capture-macos-registry.sh <fixture-name>
#   e.g. scripts/capture-macos-registry.sh builtin-sdhc-mounted
#
# Writes tests/fixtures/macos/<fixture-name>.ioreg.plist and
# <fixture-name>.system_profiler.json. Put the card in the state the fixture
# is meant to show (mounted, ejected, slot empty) before running it.
set -euo pipefail

name=${1:?usage: $0 <fixture-name>}
out="$(cd "$(dirname "$0")/.." && pwd)/tests/fixtures/macos"
mkdir -p "$out"

# The reader's PCI device is the parent of the AppleSDXC driver entry. Its
# name differs between Macs (pcie-sdreader on Apple silicon), so take it from
# the line above AppleSDXC in the service plane.
node=$(ioreg -w0 -p IOService | grep -B1 'class AppleSDXC,' | head -1 |
    sed -E 's/.*\+-o ([^ @<]+).*/\1/')
if [ -z "$node" ]; then
    echo "ERROR: no built-in SD reader (AppleSDXC) in the I/O Registry" >&2
    exit 1
fi

ioreg -a -l -r -n "$node" > "$out/$name.ioreg.plist"
system_profiler SPCardReaderDataType -json > "$out/$name.system_profiler.json"
echo "ok: $out/$name.ioreg.plist (from $node)"
