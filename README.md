# sd-profile

[![CI](https://github.com/edfvalim/sd-profile/actions/workflows/ci.yml/badge.svg)](https://github.com/edfvalim/sd-profile/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/edfvalim/sd-profile?include_prereleases)](https://github.com/edfvalim/sd-profile/releases)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Platform: macOS](https://img.shields.io/badge/platform-macOS-lightgrey.svg)

Profiles SD and microSD cards from the command line: card identity, reader,
bus speed, mounts, partition table, filesystems and partition alignment, as
a readable report or JSON.

```console
$ sudo sd-profile
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

It runs on macOS. Linux support is planned.

## Features

- **Card identity** from the CID register: manufacturer, OEM ID, product,
  revision, serial number, manufacturing date, SD spec version, speed
  class, UHS-II support and write protection (lock switch and the card's
  own protection bit). MMC cards in an SD slot are recognized as such.
- **Reader**: built-in (PCI, with link speed) or USB (with its USB IDs),
  and the bus mode the card negotiated, such as UHS-I SDR104.
- **Mounts** of the card's volumes, with their options.
- **Partition tables**: MBR, including logical partitions in an extended
  partition, and GPT, with both header copies and their checksums checked.
  Blank disks and filesystems written to the whole disk are recognized.
- **Filesystems**: FAT12, FAT16, FAT32, exFAT, ext2, ext3 and ext4. Free
  space is counted from the allocation structures, not taken from stored
  hints. NTFS is recognized but not profiled.
- **Health warnings**: not cleanly unmounted, recorded errors, stale
  free-space hints, bad checksums, a filesystem larger than its partition,
  and problems in the partition table.
- **Alignment** of each partition against the boundary the SD Association
  recommends for the card's capacity.
- **Disk images**: card backups made with `dd`, Raspberry Pi OS `.img`
  files.
- **JSON output** with a versioned schema, where every value names its
  source or the reason it is missing.
- **Read-only**: every target is opened read-only, as root too.

## Install

### Prebuilt binary

Each [release](https://github.com/edfvalim/sd-profile/releases) has
binaries for Apple silicon (`aarch64-apple-darwin`) and Intel
(`x86_64-apple-darwin`) Macs, each with a `.sha256` checksum.

```console
$ curl -LO https://github.com/edfvalim/sd-profile/releases/download/v0.1.0-rc.1/sd-profile-v0.1.0-rc.1-aarch64-apple-darwin.tar.gz
$ tar xzf sd-profile-v0.1.0-rc.1-aarch64-apple-darwin.tar.gz
$ sudo install sd-profile-v0.1.0-rc.1-aarch64-apple-darwin/sd-profile /usr/local/bin/
```

The binaries are not notarized. A copy downloaded with a browser is
quarantined by macOS; clear that with
`xattr -d com.apple.quarantine sd-profile` (`curl` downloads aren't
quarantined).

### From source

Needs Rust 1.98 or later.

```console
$ cargo install --git https://github.com/edfvalim/sd-profile
```

Or from a clone:

```console
$ git clone https://github.com/edfvalim/sd-profile
$ cd sd-profile
$ cargo build --release
$ ./target/release/sd-profile --help
```

## Usage

| Command | Profiles |
|---|---|
| `sd-profile` | The card in this computer's card reader: card, reader and mounts |
| `sudo sd-profile` | The same, plus its partitions and filesystems |
| `sd-profile /dev/disk6` | The card whose disk is `disk6`, when there are several |
| `sd-profile card.img` | A disk image |
| `sd-profile --json ...` | Any of the above, as JSON |

Name the whole disk (`/dev/disk6`), not a partition (`/dev/disk6s1`).

### With and without root

The card's identity, reader and mounts come from the macOS I/O Registry and
the mount table, which need no privileges. The card's disk device belongs to
root, so reading its partition table and filesystems needs `sudo`. Without
it, the report stops after the mounts:

```console
$ sd-profile
...
Mounts
  disk6s1        /Volumes/ID007 (exfat, nosuid, nodev, noatime)

Size             59.5 GiB (63864569856 bytes)
Sector size      512 bytes (124735488 sectors)

Partition table  unavailable (needs root, run with sudo)
```

Disk images need no root, only read access to the file.

### Card readers

The built-in reader of a Mac shows the card's identity and bus speed. A
card behind a USB card reader is hidden from the OS: the report names the
reader and its USB IDs, and the card's identity fields say
`hidden_by_usb_reader`. Without a target, USB readers are found by their
product name; one with an unusual name can be named by its disk instead.

The identity is the CID register as Apple's driver decodes it, and matches
what `system_profiler SPCardReaderDataType` shows (which glues the OEM ID
onto the product name, as `SDSN64G`). Manufacturer names come from a short
sourced list; the SD Association doesn't publish the IDs, so unbranded
cards often show none.

## Example outputs

<details>
<summary>Raspberry Pi OS card, unmounted, with alignment warnings</summary>

```console
$ sudo sd-profile
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

Mounts           none

Size             59.5 GiB (63864569856 bytes)
Sector size      512 bytes (124735488 sectors)

Partition table  MBR, disk signature 0x27b94958

  #  Role     Boot  Start LBA  End LBA  Sectors       Size  Type
  1  primary            16384  1064959  1048576  512.0 MiB  0x0c FAT32 (LBA)
  2  primary          1064960  6012927  4947968    2.4 GiB  0x83 Linux

Partition 1      FAT32
  Label          bootfs
  Volume ID      0398-3084
  Size           504.1 MiB in 1032408 clusters of 512 B
  Free           428.5 MiB (85.0%)
  Formatter      mkfs.fat
  Mounted        no
  Start          LBA 16384, a multiple of 8 MiB
  Alignment      off, the start and the data area are not on the 16 MiB boundary for 59.5 GiB SDXC; data area at 16274 KiB, a multiple of 2 KiB

WARN: partition 1: its clusters straddle the 16 MiB boundaries the SD specification recommends for this capacity, which slows writes and wears the card

Partition 2      ext4
  Label          rootfs
  UUID           02f61d5e-92cb-4f9c-a781-099d97795020
  Size           2.4 GiB in 618496 blocks of 4.0 KiB
  Free           433.9 MiB (18.0%), 120.8 MiB of it reserved for root
  Inodes         81204 of 154736 free
  Features       has_journal ext_attr resize_inode dir_index orphan_file
                 filetype extent flex_bg metadata_csum_seed sparse_super
                 large_file dir_nlink extra_isize metadata_csum
  Last mounted   /
  Mounted        no
  Start          LBA 1064960, a multiple of 8 MiB
  Alignment      off, the start and the data area are not on the 16 MiB boundary for 59.5 GiB SDXC; data area at 520 MiB, a multiple of 8 MiB

WARN: partition 2: its clusters straddle the 16 MiB boundaries the SD specification recommends for this capacity, which slows writes and wears the card
```

</details>

<details>
<summary>GPT card with a gap in the entry numbers (card and reader sections trimmed)</summary>

```console
$ sudo sd-profile
...
Mounts
  disk6s2        /Volumes/P1 (msdos, nosuid, nodev, noatime)
  disk6s4        /Volumes/P3 (exfat, nosuid, nodev, noatime)

Size             59.5 GiB (63864569856 bytes)
Sector size      512 bytes (124735488 sectors)

Partition table  GPT, disk GUID 1E085500-4347-4DEC-8066-D6981CB42D04
Header           primary
Usable sectors   LBA 34 to 124735454
Entry slots      128
Protective MBR   yes

  #  Start LBA    End LBA    Sectors       Size  Type                  Name
  1         40     409639     409600  200.0 MiB  EFI system            EFI System Partition
  2     411648    8222719    7811072    3.7 GiB  Microsoft basic data
  4   16033792  124733439  108699648   51.8 GiB  Microsoft basic data

Partition 1      FAT32
  Label          EFI
  Volume ID      67E3-17ED
  Size           196.9 MiB in 403266 clusters of 512 B
  Free           196.9 MiB (100.0%)
  Formatter      BSD  4.4
  Mounted        no
  Start          LBA 40, a multiple of 4 KiB
  Alignment      off, the start and the data area are not on the 16 MiB boundary for 59.5 GiB SDXC; data area at 3187 KiB, a multiple of 1 KiB

WARN: partition 1: its clusters straddle the 16 MiB boundaries the SD specification recommends for this capacity, which slows writes and wears the card

Partition 2      FAT32
  Label          P1
  Volume ID      BA24-1E1F
  Size           3.7 GiB in 974476 clusters of 4.0 KiB
  Free           3.7 GiB (100.0%)
  Formatter      BSD  4.4
  Mounted        /Volumes/P1 (read-write)
  Start          LBA 411648, a multiple of 1 MiB
  Alignment      off, the start and the data area are not on the 16 MiB boundary for 59.5 GiB SDXC; data area at 213454 KiB, a multiple of 2 KiB

WARN: partition 2: its clusters straddle the 16 MiB boundaries the SD specification recommends for this capacity, which slows writes and wears the card

Partition 4      exFAT
  Label          P3
  Volume ID      6AE9-4CB8
  Size           51.8 GiB in 424480 clusters of 128.0 KiB
  Free           51.8 GiB (100.0%)
  Revision       1.00
  Mounted        /Volumes/P3 (read-write)
  Start          LBA 16033792, a multiple of 1 MiB
  Alignment      off, the start and the data area are not on the 16 MiB boundary for 59.5 GiB SDXC; data area at 7845 MiB, a multiple of 1 MiB

WARN: partition 4: its clusters straddle the 16 MiB boundaries the SD specification recommends for this capacity, which slows writes and wears the card
```

Partitions are numbered as in the table, which is also how macOS names
them: `disk6s4` is entry 4, even when entry 3 is empty.

</details>

<details>
<summary>Disk image with ext2, ext3 and ext4 partitions</summary>

```console
$ sd-profile mbr-ext.img
Target           mbr-ext.img (disk image)
Size             64.0 MiB (67108864 bytes)
Sector size      512 bytes (131072 sectors)

Partition table  MBR, disk signature 0x00000000

  #  Role     Boot  Start LBA  End LBA  Sectors      Size  Type
  1  primary             2048    43007    40960  20.0 MiB  0x83 Linux
  2  primary            43008    83967    40960  20.0 MiB  0x83 Linux
  3  primary            83968   124927    40960  20.0 MiB  0x83 Linux

Partition 1      ext2
  Label          two
  UUID           7a8913ea-ff85-4a3f-83be-f57eb5e34921
  Size           20.0 MiB in 20480 blocks of 1.0 KiB
  Free           15.6 MiB (77.8%), 1.0 MiB of it reserved for root
  Inodes         5096 of 5112 free
  Features       ext_attr resize_inode dir_index filetype sparse_super
                 large_file
  Start          LBA 2048, a multiple of 1 MiB
  Alignment      ok, data area at 1 MiB, on the 16 KiB boundary for 64.0 MiB SDSC

Partition 2      ext3
  Label          three
  UUID           08e5bf8a-2c67-4860-891d-9c595e9bca0d
  Size           20.0 MiB in 20480 blocks of 1.0 KiB
  Free           14.6 MiB (72.8%), 1.0 MiB of it reserved for root
  Inodes         5096 of 5112 free
  Features       has_journal ext_attr resize_inode dir_index filetype
                 sparse_super large_file
  Start          LBA 43008, a multiple of 1 MiB
  Alignment      ok, data area at 21 MiB, on the 16 KiB boundary for 64.0 MiB SDSC

Partition 3      ext4
  Label          four
  UUID           9def6b63-9922-4532-900b-a030b8bcc495
  Size           20.0 MiB in 20480 blocks of 1.0 KiB
  Free           14.4 MiB (71.9%), 1.0 MiB of it reserved for root
  Inodes         5095 of 5112 free
  Features       has_journal ext_attr resize_inode dir_index orphan_file
                 filetype extent 64bit flex_bg metadata_csum_seed sparse_super
                 large_file huge_file dir_nlink extra_isize metadata_csum
  Start          LBA 83968, a multiple of 1 MiB
  Alignment      ok, data area at 41 MiB, on the 16 KiB boundary for 64.0 MiB SDSC
```

</details>

## What it checks

### Alignment

Flash cards erase and write in large units, and clusters that straddle
them cost the card extra work on every write. The SD Association's File
System Specification recommends a boundary for each capacity:

| Capacity, up to | Class | Boundary |
|---|---|---|
| 8 MiB | SDSC | 8 KiB |
| 64 MiB | SDSC | 16 KiB |
| 256 MiB | SDSC | 32 KiB |
| 2 GiB | SDSC | 64 KiB |
| 32 GiB | SDHC | 4 MiB |
| 128 GiB | SDXC | 16 MiB |
| 512 GiB | SDXC | 32 MiB |
| 2 TiB | SDXC | 64 MiB |

Above 2 TiB (SDUC) the specification gives no boundary to check against;
the report then shows where the data area starts and says no boundary is
known.

sd-profile checks where each partition's data area starts (the first
cluster on FAT and exFAT) against that boundary. The partition start is
checked too, except on FAT12 and FAT16, where the specification
deliberately places it off the boundary. For a disk image, the image's size
stands in for the capacity. A partition at LBA 63, the layout macOS Disk
Utility writes for MBR, is called out.

### Health warnings

Warnings are printed as `WARN:` lines under the partition they concern:

- the filesystem wasn't cleanly unmounted
- the filesystem recorded errors
- the stored free-space hint disagrees with the allocation count
- a checksum doesn't match
- the filesystem is larger than its partition
- the partition table has problems: partitions past the end of the disk,
  overlaps, a damaged GPT header

A filesystem mounted read-write keeps its "not cleanly unmounted" flag set
and its free-space hint out of date until it is unmounted (macOS and Linux
both do this), so those two warnings are left out for it. Unmount the card
first to check them.

### Failed reads

Reads that fail once the card is found go into the report rather than
stopping it: the field says `read failed` and why. An I/O error from the
card adds that it may have been pulled out, or be faulty. If the card is
gone or different when the run ends, sd-profile prints an error and exits 1
instead of a profile that mixes two cards.

## JSON output

`--json` prints the same profile as JSON. The top-level `schema_version`
changes only when a key is removed, renamed or changes type; new keys can
appear without a bump.

Every value the tool reads says where it came from, or why it is missing:

```json
{"value": {...}, "source": "partition_table"}
{"value": null, "unavailable": "read_failed", "detail": "..."}
```

Inside a value the tool did read, a plain `null` means the thing is absent,
not unknown: no volume ID, never mounted.

- Sizes and sector numbers are JSON numbers. Identifiers and bit fields
  (MBR type IDs, disk signatures, GPT attributes) are hex strings, and
  GUIDs are uppercase strings. FAT and exFAT volume IDs look like
  `1A2B-3C4D` and ext UUIDs are lowercase, as `blkid` shows them.
- `card`, `reader` and `mounts` describe the card, the reader it sits in
  and the filesystems mounted from it. For a disk image they are `null`
  with `"unavailable": "disk_image"`. A card that is in the reader but
  ejected keeps its identity, while its disk, mounts and bus speed are
  `card_inactive`.
- Partitions that can hold data are listed under `layout.volumes`, each
  with its `filesystem` (tagged by `type`) and `alignment` (tagged by
  `status`). A filesystem on the whole disk is a volume with
  `"partition": null`.

<details>
<summary>Full JSON for the exFAT card at the top of this page</summary>

```json
{
  "schema_version": 1,
  "target": {
    "kind": "device",
    "disk": {
      "name": "disk6",
      "path": "/dev/disk6",
      "raw_path": "/dev/rdisk6"
    }
  },
  "card": {
    "value": {
      "type": {
        "value": "sdxc",
        "source": "io_registry"
      },
      "capacity_bytes": {
        "value": 63864569856,
        "source": "io_registry"
      },
      "manufacturer": {
        "value": {
          "id": "0x03",
          "name": "SanDisk"
        },
        "source": "io_registry"
      },
      "oem_id": {
        "value": {
          "id": "0x5344",
          "text": "SD"
        },
        "source": "io_registry"
      },
      "product_name": {
        "value": "SN64G",
        "source": "io_registry"
      },
      "product_revision": {
        "value": "8.0",
        "source": "io_registry"
      },
      "serial_number": {
        "value": "0x8a411d8b",
        "source": "io_registry"
      },
      "manufacturing_date": {
        "value": "2019-10",
        "source": "io_registry"
      },
      "spec_version": {
        "value": "3.0+",
        "source": "io_registry"
      },
      "speed_class": {
        "value": 10,
        "source": "io_registry"
      },
      "uhs_ii_supported": {
        "value": false,
        "source": "io_registry"
      },
      "write_protect": {
        "switch": {
          "value": false,
          "source": "io_registry"
        },
        "register": {
          "value": false,
          "source": "io_registry"
        }
      }
    },
    "source": "io_registry"
  },
  "reader": {
    "value": {
      "connection": {
        "kind": "built_in",
        "pci": {
          "vendor_id": "0x17a0",
          "device_id": "0x9755",
          "subsystem_vendor_id": "0x17a0",
          "subsystem_id": "0x9755",
          "revision_id": "0x01",
          "link": {
            "value": {
              "generation": 2,
              "transfer_rate": "5.0 GT/s",
              "width": 1
            },
            "source": "io_registry"
          }
        }
      },
      "vendor_name": {
        "value": "Apple",
        "source": "io_registry"
      },
      "product_name": {
        "value": "Built In SDXC Reader",
        "source": "io_registry"
      },
      "uhs_ii_supported": {
        "value": true,
        "source": "io_registry"
      },
      "card_link": {
        "value": {
          "speed_mode": "sdr104",
          "bus_width": 4,
          "clock_khz": 200000
        },
        "source": "io_registry"
      }
    },
    "source": "io_registry"
  },
  "mounts": {
    "value": [
      {
        "device": "disk6s1",
        "partition": 1,
        "mount_point": "/Volumes/ID007",
        "os_fs_type": "exfat",
        "options": [
          "nosuid",
          "nodev",
          "noatime"
        ]
      }
    ],
    "source": "mount_table"
  },
  "layout": {
    "size_bytes": {
      "value": 63864569856,
      "source": "io_registry"
    },
    "block_size": {
      "value": 512,
      "source": "io_registry"
    },
    "partition_table": {
      "value": {
        "scheme": "mbr",
        "disk_signature": "0x00000000",
        "partitions": [
          {
            "number": 1,
            "role": "primary",
            "bootable": false,
            "type": {
              "id": "0x07",
              "name": "NTFS or exFAT"
            },
            "start_lba": 32768,
            "sectors": 124702720,
            "offset_bytes": 16777216,
            "size_bytes": 63847792640
          }
        ],
        "issues": []
      },
      "source": "partition_table"
    },
    "volumes": [
      {
        "partition": 1,
        "start_lba": 32768,
        "sectors": 124702720,
        "offset_bytes": 16777216,
        "size_bytes": 63847792640,
        "filesystem": {
          "value": {
            "type": "exfat",
            "label": {
              "value": "ID007",
              "source": "root_directory"
            },
            "volume_id": "6AE9-55F0",
            "revision": "1.00",
            "sector_size_bytes": 512,
            "cluster_size_bytes": 131072,
            "cluster_count": 486992,
            "size_bytes": 63831015424,
            "free_bytes": {
              "value": 63826296832,
              "source": "allocation_bitmap"
            },
            "data_offset_bytes": 16777216,
            "fat_count": 1,
            "percent_in_use": 0,
            "issues": []
          },
          "source": "boot_sector"
        },
        "alignment": {
          "start": {
            "disk_offset_bytes": 16777216,
            "aligned_to_bytes": 16777216
          },
          "data": {
            "disk_offset_bytes": 33554432,
            "aligned_to_bytes": 33554432
          },
          "dos_cylinder_start": false,
          "status": "aligned",
          "boundary": {
            "bytes": 16777216,
            "capacity_class": "sdxc",
            "capacity_bytes": 63864569856
          }
        }
      }
    ]
  }
}
```

</details>

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Profile printed |
| 1 | The target can't be opened or isn't supported, no card reader was found, or the card was pulled out or swapped while it was being read |
| 2 | Invalid arguments |

## Known limitations

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

## License

MIT. See [LICENSE](LICENSE).
