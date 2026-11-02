# sd-profile

Profiles SD and microSD cards from the command line: card identity, speed,
reader, partition table, filesystems and mounts, as a readable report or JSON.

Early development. macOS first, Linux later. This version reads disk image
files (a card backup made with `dd`, a Raspberry Pi OS `.img`) and reports
their partition table; card devices come later.

## Build

```
cargo build --release
```

## Usage

```
sd-profile card.img           readable report
sd-profile --json card.img    JSON
```

```
Target           card.img (disk image)
Size             128.0 MiB (134217728 bytes)
Sector size      512 bytes (262144 sectors)

Partition table  MBR, disk signature 0x00000000

  #  Role     Boot  Start LBA  End LBA  Sectors      Size  Type
  1  primary  *          8192   139263   131072  64.0 MiB  0x0c FAT32 (LBA)
  2  primary           139264   262143   122880  60.0 MiB  0x83 Linux
```

It reads MBR (including logical partitions in an extended partition) and
GPT (checking both header copies and their checksums), and recognizes blank
disks and filesystems written to the whole disk without a partition table.
Problems in the table, such as partitions past the end of the disk,
overlaps or a damaged GPT header, are reported as warnings.

Targets are always opened read-only.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Profile printed |
| 1 | The target can't be opened or isn't supported |
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
uppercase strings.

## License

MIT. See [LICENSE](LICENSE).
