# sd-profile

Profiles SD and microSD cards from the command line: card identity, speed,
reader, partition table, filesystems and mounts, as a readable report or JSON.

Early development. macOS first, Linux later. This version reports the card
in a Mac's card reader (identity, speed, reader and mounts, from the I/O
Registry, no root needed), and reads disk image files (a card backup made
with `dd`, a Raspberry Pi OS `.img`) for their partition table, filesystems
and alignment. Reading a card's partitions and filesystems comes next.

## Build

```
cargo build --release
```

## Usage

```
sd-profile                    the card in this computer's card reader
sd-profile /dev/disk6         the card whose disk is disk6, when there are several
sd-profile card.img           a disk image
sd-profile --json ...         JSON instead of the readable report
```

```
Target           /dev/disk6 (card in the built-in reader)

Card             SDHC, 14.8 GiB (15938355200 bytes)
  Manufacturer   0xdf (not a known manufacturer ID)
  OEM ID         0x3428 ("4(")
  Product        "SD"
  Revision       2.0
  Serial number  0x0000039e
  Manufactured   2020-10
  SD spec        3.0+ (macOS reports no version above this one)
  Speed class    Class 10
  UHS-II         not supported
  Write protect  off

Reader           Apple Built In SDXC Reader
  Connection     built-in, PCI 17a0:9755 (subsystem 17a0:9755, revision 0x01)
  PCIe link      5.0 GT/s x1 (PCIe gen 2)
  UHS-II         supported
  Card bus       UHS-I SDR104, 4 data lines, 200 MHz

Mounts
  disk6s1        /Volumes/BOOTFS (msdos, nosuid, nodev, noatime)

Size             14.8 GiB (15938355200 bytes)
Sector size      512 bytes (31129600 sectors)

Partition table  unavailable (needs root, run with sudo)
```

The card's identity comes from its CID register as Apple's driver decodes
it, and matches what `system_profiler SPCardReaderDataType` shows (which
glues the OEM ID onto the product name, as `4(SD`). Manufacturer names come
from a short sourced list; the SD Association doesn't publish the IDs, so
unbranded cards often show none. A card behind a USB card reader is hidden
from the OS: the report names the reader and its USB IDs, and the card's
fields say `hidden_by_usb_reader`. Without a target, USB readers are found
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
overlaps or a damaged GPT header, are reported as warnings.

For each partition it reads the filesystem: FAT12, FAT16, FAT32, exFAT,
ext2, ext3 and ext4, with label, volume ID or UUID, cluster or block size,
size and free space (counted in the allocation table, bitmap or group
descriptors, not taken from stored hints), and the ext feature flags. NTFS
is recognized but not profiled. Warnings cover filesystems that weren't
cleanly unmounted, recorded errors, stale free-space hints, bad checksums
and filesystems larger than their partition.

Alignment compares where each partition's data area starts (the first
cluster on FAT and exFAT) with the boundary the SD Association's File System
Specification recommends for the card's capacity: 8 to 64 KiB for SDSC,
4 MiB for SDHC, 16 to 64 MiB for SDXC. The partition start is checked too,
except on FAT12 and FAT16, where the specification deliberately places it
off the boundary. For a disk image, the image's size stands in for the
capacity. A partition at LBA 63, the layout macOS Disk Utility writes for
MBR, is called out.

Targets are always opened read-only.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Profile printed |
| 1 | The target can't be opened or isn't supported, or no card reader was found |
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
