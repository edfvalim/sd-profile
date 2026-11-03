# sd-profile

Profiles SD and microSD cards from the command line: card identity, speed,
reader, partition table, filesystems and mounts, as a readable report or JSON.

macOS for now; Linux comes later. It reports the card in a Mac's card
reader (identity, speed, reader and mounts from the I/O Registry, no root
needed) and, run as root, reads the card itself for its partition table,
filesystems and alignment. It also reads disk image files (a card backup
made with `dd`, a Raspberry Pi OS `.img`).

## Build

```
cargo build --release
```

## Usage

```
sd-profile                    the card in this computer's card reader
sudo sd-profile               the same, with its partitions and filesystems
sd-profile /dev/disk6         the card whose disk is disk6, when there are several
sd-profile card.img           a disk image
sd-profile --json ...         JSON instead of the readable report
```

The card's device belongs to root, so without `sudo` the report stops at
the card, reader and mounts, and the partition table says `needs root, run
with sudo`. The card is opened read-only, as root too.

```
Target           /dev/disk6 (card in the built-in reader)

Card             SDXC, 59.5 GiB (63864569856 bytes)
  Manufacturer   SanDisk (0x03)
  OEM ID         0x5344 ("SD")
  Product        "SN64G"
  Revision       8.0
  Serial number  0x8a411d8b
  Manufactured   2019-10
  SD spec        3.0+ (the OS reports no version above this one)
  Speed class    Class 10
  UHS-II         not supported
  Write protect  off

Reader           Apple Built In SDXC Reader
  Connection     built-in, PCI 17a0:9755 (subsystem 17a0:9755, revision 0x01)
  PCIe link      5.0 GT/s x1 (PCIe gen 2)
  UHS-II         supported
  Card bus       UHS-I SDR104, 4 data lines, 200 MHz

Mounts
  disk6s1        /Volumes/ID007 (exfat, nosuid, nodev, noatime)

Size             59.5 GiB (63864569856 bytes)
Sector size      512 bytes (124735488 sectors)

Partition table  MBR, disk signature 0x00000000

  #  Role     Boot  Start LBA    End LBA    Sectors      Size  Type
  1  primary            32768  124735487  124702720  59.5 GiB  0x07 NTFS or exFAT

Partition 1      exFAT
  Label          ID007
  Volume ID      6AE9-55F0
  Size           59.4 GiB in 486992 clusters of 128.0 KiB
  Free           59.4 GiB (100.0%)
  Revision       1.00
  Mounted        /Volumes/ID007 (read-write)
  Start          LBA 32768, a multiple of 16 MiB
  Alignment      ok, data area at 32 MiB, on the 16 MiB boundary for 59.5 GiB SDXC
```

The card's identity comes from its CID register as Apple's driver decodes
it, and matches what `system_profiler SPCardReaderDataType` shows (which
glues the OEM ID onto the product name, as `SDSN64G`). Manufacturer names come
from a short sourced list; the SD Association doesn't publish the IDs, so
unbranded cards often show none. A card behind a USB card reader is hidden
from the OS: the report names the reader and its USB IDs, and the card's
fields say `hidden_by_usb_reader`. MMC cards in an SD slot are recognized
as such. The write-protect line shows the card's lock switch and the
card's own protection bit. Without a target, USB readers are found
by their product name; a USB reader with an unusual name can be named by its
disk instead.

```
Target           card.img (disk image)
Size             128.0 MiB (134217728 bytes)
Sector size      512 bytes (262144 sectors)

Partition table  MBR, disk signature 0x00000000

  #  Role     Boot  Start LBA  End LBA  Sectors      Size  Type
  1  primary  *          8192   139263   131072  64.0 MiB  0x0c FAT32 (LBA)
  2  primary           139264   262143   122880  60.0 MiB  0x83 Linux

Partition 1      FAT32
  Label          BOOT
  Volume ID      8093-19ED
  Size           63.0 MiB in 129022 clusters of 512 B
  Free           63.0 MiB (100.0%)
  Formatter      BSD  4.4
  Alignment      ok, data area at 5 MiB, on the 32 KiB boundary for 128.0 MiB SDSC

Partition 2      ext4
  Label          rootfs
  UUID           d97db754-2e18-42d0-95c1-c9dd9ae14f64
  Size           60.0 MiB in 15360 blocks of 4.0 KiB
  Free           49.0 MiB (81.7%), 3.0 MiB of it reserved for root
  Inodes         15343 of 15360 free
  Features       has_journal ext_attr resize_inode dir_index orphan_file
                 filetype extent 64bit flex_bg metadata_csum_seed sparse_super
                 large_file huge_file dir_nlink extra_isize metadata_csum
  Alignment      ok, data area at 68 MiB, on the 32 KiB boundary for 128.0 MiB SDSC
```

It reads MBR (including logical partitions in an extended partition) and
GPT (checking both header copies and their checksums), and recognizes blank
disks and filesystems written to the whole disk without a partition table.
Problems in the table, such as partitions past the end of the disk,
overlaps or a damaged GPT header, are reported as warnings. Partitions are numbered as in the table, which is also how macOS names
them (`disk6s4` is entry 4, even when entry 3 is empty).

For each partition it reads the filesystem: FAT12, FAT16, FAT32, exFAT,
ext2, ext3 and ext4, with label, volume ID or UUID, cluster or block size,
size and free space (counted in the allocation table, bitmap or group
descriptors, not taken from stored hints), and the ext feature flags. NTFS
is recognized but not profiled. Warnings cover filesystems that weren't
cleanly unmounted, recorded errors, stale free-space hints, bad checksums
and filesystems larger than their partition. A filesystem mounted
read-write keeps its "not cleanly unmounted" flag set and its free-space
hint out of date until it is unmounted (macOS and Linux both do this), so
those two warnings are left out for it; unmount the card first to check
them.

Alignment compares where each partition's data area starts (the first
cluster on FAT and exFAT) with the boundary the SD Association's File System
Specification recommends for the card's capacity: 8 to 64 KiB for SDSC,
4 MiB for SDHC, 16 to 64 MiB for SDXC. The partition start is checked too,
except on FAT12 and FAT16, where the specification deliberately places it
off the boundary. For a disk image, the image's size stands in for the
capacity. A partition at LBA 63, the layout macOS Disk Utility writes for
MBR, is called out.

Targets are always opened read-only.

Reads that fail once the card is found go into the report rather than
stopping it: the field says `read failed` and why. An I/O error from the
card adds that it may have been pulled out, or be faulty. If the card is
gone or different when the run ends, the tool prints an error and exits 1
instead of a profile that mixes two cards.

### Known limitations

- macOS only. Linux support is planned.
- Cards behind a USB reader are handled from registry captures and code,
  not yet tested with a real USB reader.
- USB readers with several slots: only the first slot is profiled.
- A partition mounted twice shows only one of its mounts in its volume
  section (all of them are listed under Mounts).
- Behind a USB reader the card's identity is hidden, so a card swapped for
  another of the same size during a run goes unnoticed.
- FAT labels are decoded as ASCII, or as UTF-8 when they are valid UTF-8
  (what macOS's `newfs_msdos` writes). Other non-ASCII bytes, in an OEM
  code page the volume doesn't record, show as U+FFFD.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Profile printed |
| 1 | The target can't be opened or isn't supported, no card reader was found, or the card was pulled out or swapped while it was being read |
| 2 | Invalid arguments |

### JSON

The JSON output carries `schema_version`, which changes only when a key is
removed, renamed or changes type. Values the tool couldn't get are `null`
with a reason:

```json
{"value": {...}, "source": "partition_table"}
{"value": null, "unavailable": "read_failed", "detail": "..."}
```

Sizes and sector numbers are JSON numbers. Identifiers and bit fields (MBR
type IDs, disk signatures, GPT attributes) are hex strings, and GUIDs are
uppercase strings. FAT and exFAT volume IDs look like `1A2B-3C4D` and ext
UUIDs are lowercase, as `blkid` shows them.

Inside a value the tool did read, a plain `null` means the thing is absent,
not unknown: no volume ID, never mounted.

`card`, `reader` and `mounts` describe the card, the reader it sits in and
the filesystems mounted from it; for a disk image they are `null` with
`"unavailable": "disk_image"`. A card that is in the reader but ejected keeps
its identity, while its disk, mounts and bus speed are `card_inactive`.

Partitions that can hold data are listed under `layout.volumes`, each with
its `filesystem` (tagged by `type`) and `alignment` (tagged by `status`). A
filesystem on the whole disk is a volume with `"partition": null`.

## License

MIT. See [LICENSE](LICENSE).
