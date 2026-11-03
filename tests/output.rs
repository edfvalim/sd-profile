//! End-to-end runs of the binary against the disk images in
//! `tests/fixtures/disk/`, compared with saved snapshots. No card and no root
//! needed. Runs without a target depend on the computer's card readers, so
//! they aren't here; the I/O Registry mapping is tested against captures in
//! `src/platform/macos/map.rs`.
//!
//! Each image is decompressed into a scratch directory and the binary runs
//! there with a relative path, so the snapshots don't depend on where the
//! repository lives.

use std::path::Path;

use assert_cmd::Command;
use tempfile::TempDir;

const IMAGES: &[&str] = &[
    "blank",
    "gpt-exfat",
    "gpt-multi",
    "mbr-ext",
    "mbr-extended",
    "mbr-fat12-fat16",
    "mbr-fat32",
    "mbr-fat32-truncated",
    "mbr-sd-aligned",
    "superfloppy-ext4",
    "superfloppy-fat32",
];

/// Decompresses `<name>.img.zst` into a fresh directory.
fn unpack(name: &str) -> TempDir {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/disk")
        .join(format!("{name}.img.zst"));
    let compressed = std::fs::read(&fixture).expect("fixture is readable");
    let image = zstd::decode_all(compressed.as_slice()).expect("fixture decompresses");
    let dir = tempfile::tempdir().expect("scratch directory is created");
    std::fs::write(dir.path().join(format!("{name}.img")), image).expect("image is written");
    dir
}

fn sd_profile(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("sd-profile").expect("binary is built");
    cmd.current_dir(dir);
    cmd
}

fn stdout_of(cmd: &mut Command) -> String {
    let output = cmd.assert().success().get_output().stdout.clone();
    String::from_utf8(output).expect("output is UTF-8")
}

#[test]
fn human_report_of_each_image() {
    for name in IMAGES {
        let dir = unpack(name);
        let out = stdout_of(sd_profile(dir.path()).arg(format!("{name}.img")));
        insta::assert_snapshot!(format!("human-{name}"), out);
    }
}

#[test]
fn json_of_each_image() {
    for name in IMAGES {
        let dir = unpack(name);
        let out = stdout_of(sd_profile(dir.path()).args(["--json", &format!("{name}.img")]));
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("output is JSON");
        assert_eq!(parsed["schema_version"], 1);
        insta::assert_snapshot!(format!("json-{name}"), out);
    }
}

#[test]
fn missing_file_fails_with_message() {
    let dir = tempfile::tempdir().unwrap();
    sd_profile(dir.path())
        .arg("missing.img")
        .assert()
        .code(1)
        .stdout("")
        .stderr("ERROR: cannot open missing.img: No such file or directory (os error 2)\n");
}

#[cfg(target_os = "macos")]
#[test]
fn non_disk_device_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    sd_profile(dir.path())
        .arg("/dev/null")
        .assert()
        .code(1)
        .stderr("ERROR: /dev/null is not a disk device\n");
}

#[cfg(target_os = "macos")]
#[test]
fn partition_is_rejected_with_its_disk() {
    let dir = tempfile::tempdir().unwrap();
    // disk0s1 exists on every Mac; the check runs before any lookup.
    sd_profile(dir.path())
        .arg("/dev/disk0s1")
        .assert()
        .code(1)
        .stderr("ERROR: /dev/disk0s1 is a partition; name the whole disk, /dev/disk0\n");
}

#[cfg(target_os = "macos")]
#[test]
fn internal_disk_is_not_a_card() {
    let dir = tempfile::tempdir().unwrap();
    sd_profile(dir.path())
        .arg("/dev/disk0")
        .assert()
        .code(1)
        .stderr("ERROR: /dev/disk0 is not a card in an SD card reader or a USB disk\n");
}

#[test]
fn directory_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    sd_profile(dir.path())
        .arg(".")
        .assert()
        .code(1)
        .stderr("ERROR: . is not a disk image file or a device\n");
}

#[test]
fn image_smaller_than_a_sector_reports_unavailable_table() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("tiny.img"), [0u8; 100]).unwrap();
    let out = stdout_of(sd_profile(dir.path()).args(["--json", "tiny.img"]));
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    let table = &parsed["layout"]["partition_table"];
    assert_eq!(table["value"], serde_json::Value::Null);
    assert_eq!(table["unavailable"], "read_failed");
}

#[test]
fn unknown_flag_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    sd_profile(dir.path()).arg("--frobnicate").assert().code(2);
}
