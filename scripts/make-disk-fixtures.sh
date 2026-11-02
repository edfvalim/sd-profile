#!/bin/bash
# Builds the disk images in tests/fixtures/disk/ with macOS's own tools
# (diskutil, fdisk, newfs_msdos) and e2fsprogs' mke2fs, so the parsers are
# tested against what real formatters write rather than hand-made bytes. No
# root needed: the images are attached as the current user, and mke2fs writes
# into the image file at the partition's offset.
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
mke2fs="$(brew --prefix e2fsprogs 2>/dev/null)/sbin/mke2fs"
[[ -x "$mke2fs" ]] || { echo "ERROR: mke2fs not found (brew install e2fsprogs)" >&2; exit 1; }

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

# Files written to every filesystem that gets content, so used and free space
# differ from a freshly formatted volume: a 3 MiB file that compresses well
# and a directory of small files.
content="$work_dir/content"
mkdir -p "$content/docs"
head -c $((3 * MiB)) < <(yes 'sd-profile fixture') >"$content/big.txt"
for i in 1 2 3; do echo "note $i" >"$content/docs/note$i.txt"; done

# add_files <partition device>: copies the content onto the mounted volume.
add_files() {
  local mount_point
  mount_point="$(diskutil info "$1" | sed -n 's/^ *Mount Point: *//p')"
  [[ -n "$mount_point" ]] || { echo "ERROR: $1 is not mounted" >&2; exit 1; }
  cp -R "$content/" "$mount_point/"
}

# ext <type> <name> <start lba> <sectors> <label> [mke2fs options...]: formats
# the partition at <start lba> of image <name> as ext2, ext3 or ext4, filled
# with the content. The image must not be attached.
ext() {
  local type="$1" image="$work_dir/$2.img" start="$3" sectors="$4" label="$5"
  shift 5
  "$mke2fs" -F -q -t "$type" -L "$label" -E offset=$((start * 512)) -d "$content" "$@" \
    "$image" $((sectors / 2))k
}

# Single FAT32 partition the way macOS Disk Utility writes it: MBR, type 0x0B,
# starting at LBA 63 (not aligned to the card's erase blocks).
blank mbr-fat32 $((64 * MiB))
diskutil partitionDisk "$attached" MBR "MS-DOS FAT32" SDTEST R >/dev/null
add_files "${attached}s1"
detach

# Layout of Raspberry Pi OS: partitions aligned to 4 MiB, a bootable FAT32
# (LBA) partition and an ext4 root filesystem with 4 KiB blocks.
blank mbr-sd-aligned $((128 * MiB))
printf '8192,131072,0x0C,*\n139264,122880,0x83,-\n' | fdisk -y -r "$attached" >/dev/null 2>&1
newfs_msdos -F 32 -v BOOT "${attached}s1" >/dev/null 2>&1
detach
ext ext4 mbr-sd-aligned 139264 122880 rootfs -b 4096

# FAT12 and FAT16 partitions, small enough that newfs_msdos picks those types.
blank mbr-fat12-fat16 $((32 * MiB))
printf '2048,8192,0x01,-\n10240,55296,0x06,-\n' | fdisk -y -r "$attached" >/dev/null 2>&1
newfs_msdos -F 12 -v SMALL "${attached}s1" >/dev/null 2>&1
newfs_msdos -F 16 -v MEDIUM "${attached}s2" >/dev/null 2>&1
detach

# ext2, ext3 and ext4 side by side, aligned to 1 MiB. At this size mke2fs
# picks 1 KiB blocks.
blank mbr-ext $((64 * MiB))
printf '2048,40960,0x83,-\n43008,40960,0x83,-\n83968,40960,0x83,-\n' |
  fdisk -y -r "$attached" >/dev/null 2>&1
detach
ext ext2 mbr-ext 2048 40960 two
ext ext3 mbr-ext 43008 40960 three
ext ext4 mbr-ext 83968 40960 four

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
add_files "${attached}s1"
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

# ext4 on the whole device, as `mkfs.ext4 /dev/sdb` leaves a card. Sector 0
# is all zeros: the superblock starts at byte 1024.
head -c $((8 * MiB)) /dev/zero >"$work_dir/superfloppy-ext4.img"
ext ext4 superfloppy-ext4 0 $((8 * MiB / 512)) FLOPPY

# All zeros: a wiped card.
head -c $((1 * MiB)) /dev/zero >"$work_dir/blank.img"

# The first MiB of mbr-fat32: the partition runs past the end of the image.
head -c $((1 * MiB)) "$work_dir/mbr-fat32.img" >"$work_dir/mbr-fat32-truncated.img"

mkdir -p "$out_dir"
for image in "$work_dir"/*.img; do
  zstd -19 -q -f "$image" -o "$out_dir/$(basename "$image").zst"
  echo "ok $(basename "$image").zst"
done
