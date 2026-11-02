#!/bin/bash
# Builds the disk images in tests/fixtures/disk/ with macOS's own tools
# (diskutil, fdisk, newfs_msdos), so the parsers are tested against what real
# formatters write rather than hand-made bytes. No root needed: the images are
# attached as the current user and only ever written while attached here.
#
# Each image is compressed with zstd (mostly zeros, a few KB each). GUIDs,
# volume IDs and serials are random, so rerunning this changes the snapshots
# under tests/snapshots/; review and accept them with the new images.
#
# Usage: scripts/make-disk-fixtures.sh
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "ERROR: this script uses diskutil and runs on macOS only" >&2
  exit 1
fi
command -v zstd >/dev/null || { echo "ERROR: zstd not found (brew install zstd)" >&2; exit 1; }

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
out_dir="$repo_root/tests/fixtures/disk"
work_dir="$(mktemp -d)"
attached=""

cleanup() {
  if [[ -n "$attached" ]]; then
    hdiutil detach -force "$attached" >/dev/null 2>&1 || true
  fi
  rm -rf "$work_dir"
}
trap cleanup EXIT

MiB=$((1024 * 1024))

# blank <name> <bytes>: an unpartitioned image of zeros, attached without
# mounting. Sets $attached to the /dev/diskN node.
blank() {
  local image="$work_dir/$1.img"
  diskutil image create blank --format RAW --size "$2" --fs None "$image" >/dev/null
  attached="$(diskutil image attach --noMount "$image" | awk 'NR==1 {print $1}')"
}

detach() {
  diskutil eject "$attached" >/dev/null
  attached=""
}

# Single FAT32 partition the way macOS Disk Utility writes it: MBR, type 0x0B,
# starting at LBA 63 (not aligned to the card's erase blocks).
blank mbr-fat32 $((64 * MiB))
diskutil partitionDisk "$attached" MBR "MS-DOS FAT32" SDTEST R >/dev/null
detach

# Layout of the SD Association formatter and Raspberry Pi OS: partitions
# aligned to 4 MiB, a bootable FAT32 (LBA) partition and a Linux partition.
# The Linux partition stays unformatted: macOS has no mkfs.ext4.
blank mbr-sd-aligned $((128 * MiB))
printf '8192,131072,0x0C,*\n139264,122880,0x83,-\n' | fdisk -y -r "$attached" >/dev/null 2>&1
newfs_msdos -F 32 -v BOOT "${attached}s1" >/dev/null 2>&1
detach

# More than four partitions: diskutil puts the fourth one and later into an
# extended partition with logical partitions inside.
blank mbr-extended $((128 * MiB))
diskutil partitionDisk "$attached" 5 MBR \
  "MS-DOS FAT32" ONE 40M "MS-DOS FAT16" TWO 16M ExFAT THREE 16M \
  "MS-DOS FAT32" FOUR 40M "Free Space" REST R >/dev/null
detach

# Single exFAT partition on GPT, as cards over 32 GB often come.
blank gpt-exfat $((64 * MiB))
diskutil partitionDisk "$attached" GPT ExFAT CARD R >/dev/null
detach

# GPT with two partitions and free space at the end.
blank gpt-multi $((128 * MiB))
diskutil partitionDisk "$attached" 3 GPT \
  "MS-DOS FAT32" BOOT 40M ExFAT DATA 48M "Free Space" REST R >/dev/null
detach

# FAT32 on the whole device with no partition table ("superfloppy"). Its boot
# sector ends in 0x55AA like an MBR does.
blank superfloppy-fat32 $((64 * MiB))
newfs_msdos -F 32 -v FLOPPY "$attached" >/dev/null 2>&1
detach

# All zeros: a wiped card.
head -c $((1 * MiB)) /dev/zero >"$work_dir/blank.img"

# The first MiB of mbr-fat32: the partition runs past the end of the image.
head -c $((1 * MiB)) "$work_dir/mbr-fat32.img" >"$work_dir/mbr-fat32-truncated.img"

mkdir -p "$out_dir"
for image in "$work_dir"/*.img; do
  zstd -19 -q -f "$image" -o "$out_dir/$(basename "$image").zst"
  echo "ok $(basename "$image").zst"
done
