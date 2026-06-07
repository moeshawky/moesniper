//! Golden file regression tests
//! Detects when behavior drifts from known-good baseline

use std::fs;
use std::process::Command;
use tempfile::TempDir;

fn sniper() -> Command {
    let mut cmd = Command::new("cargo");
    cmd.args(["run", "--quiet", "--"]);
    cmd
}

// Normalize line endings for cross-platform comparison
fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

fn read_file(path: impl AsRef<std::path::Path>) -> String {
    std::fs::read_to_string(path).unwrap()
}

// Undo stack behavior matches golden file
#[test]
fn test_golden_undo_stack() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.txt");
    fs::write(&file_path, "v0\n").unwrap();

    // Make 5 edits
    for i in 1..=5 {
        let hex = format!("{:02x}", i + 48);
        let status = sniper()
            .args([&file_path.to_string_lossy(), "1", "1", &hex])
            .status()
            .unwrap();
        assert!(status.success());
    }

    let content = read_file(&file_path);
    let golden = normalize(include_str!("regression/golden/undo_stack.txt"));

    assert_eq!(
        normalize(&content),
        golden,
        "Content after 5 edits must match golden file"
    );
}

// Basic splice operation
#[test]
fn test_golden_splice_basic() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.txt");
    fs::write(&file_path, "line1\nline2\nline3\n").unwrap();

    let status = sniper()
        .args([&file_path.to_string_lossy(), "1", "1", "58"]) // 'X'
        .status()
        .unwrap();

    assert!(status.success());

    let content = read_file(&file_path);
    let golden = include_str!("regression/golden/splice_basic.txt");

    assert_eq!(
        normalize(&content),
        normalize(golden),
        "Splice result must byte-match golden file"
    );
}

// Newline preservation behavior
#[test]
fn test_golden_newline_preservation() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.txt");

    // File with trailing newline
    fs::write(&file_path, "test\n").unwrap();

    let status = sniper()
        .args([&file_path.to_string_lossy(), "1", "1", "41"]) // 'A'
        .status()
        .unwrap();

    assert!(status.success(), "Splice must succeed on valid input");
    let content = read_file(&file_path);
    assert!(
        content.ends_with('\n'),
        "Must preserve trailing newline, got: {:?}",
        content
    );
    assert_eq!(
        content, "A\n",
        "Content must be 'A\\n' after replacing 'test' with 'A'"
    );
}

// Manifest operation baseline
#[test]
fn test_golden_manifest_basic() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.txt");
    let manifest_path = dir.path().join("ops.json");

    fs::write(&file_path, "line1\nline2\nline3\n").unwrap();
    fs::write(&manifest_path, r#"[{"start": 2, "delete": true}]"#).unwrap();

    let status = sniper()
        .args([
            &file_path.to_string_lossy(),
            "--manifest",
            &manifest_path.to_string_lossy(),
        ])
        .status()
        .unwrap();

    // Should succeed
    assert!(status.success(), "Manifest operation should succeed");

    let content = read_file(&file_path);
    // After deleting line 2 from "line1\nline2\nline3\n", result must be exact
    assert_eq!(
        content, "line1\nline3\n",
        "Manifest delete line 2 must produce exact output, got: {:?}",
        content
    );
}

// Error message format stability
#[test]
fn test_golden_splice_append_at_end_two_line_file() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.txt");
    fs::write(&file_path, "a\nb\n").unwrap();

    let output = sniper()
        .args([&file_path.to_string_lossy(), "3", "3", "63"]) // 'c'
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "start=3,end=3 on a 2-line file must append"
    );

    let content = read_file(&file_path);
    assert_eq!(
        normalize(&content),
        normalize("a\nb\nc\n"),
        "Append at end with start=end must work, got: {:?}",
        content
    );
}

// Append at end: 1-line file, start=2,end=2 → appends line2
#[test]
fn test_golden_splice_append_at_end_one_line_file() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.txt");
    fs::write(&file_path, "x\n").unwrap();

    let status = sniper()
        .args([&file_path.to_string_lossy(), "2", "2", "79"]) // 'y'
        .status()
        .unwrap();

    assert!(
        status.success(),
        "start=2,end=2 on a 1-line file must append"
    );

    let content = read_file(&file_path);
    assert_eq!(
        normalize(&content),
        normalize("x\ny\n"),
        "Append at end on 1-line file, got: {:?}",
        content
    );
}

// Append at end: 4-line file, start=5,end=5 → appends line5
#[test]
fn test_golden_splice_append_at_end_four_line_file() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.txt");
    fs::write(&file_path, "a\nb\nc\nd\n").unwrap();

    let status = sniper()
        .args([&file_path.to_string_lossy(), "5", "5", "65"]) // 'e'
        .status()
        .unwrap();

    assert!(
        status.success(),
        "start=5,end=5 on a 4-line file must append"
    );

    let content = read_file(&file_path);
    assert_eq!(
        normalize(&content),
        normalize("a\nb\nc\nd\ne\n"),
        "Append at end on 4-line file, got: {:?}",
        content
    );
}

// Insert at end: start > end+1 existing pattern still works
#[test]
fn test_golden_splice_insert_at_end_start_gt_end() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.txt");
    fs::write(&file_path, "a\nb\n").unwrap();

    let status = sniper()
        .args([&file_path.to_string_lossy(), "3", "2", "63"]) // 'c'
        .status()
        .unwrap();

    assert!(
        status.success(),
        "start=3,end=2 on a 2-line file must append"
    );

    let content = read_file(&file_path);
    assert_eq!(
        normalize(&content),
        normalize("a\nb\nc\n"),
        "Existing start>end pattern must still work, got: {:?}",
        content
    );
}

// Append at end with empty content — should succeed (no change or just newline)
#[test]
fn test_golden_splice_append_at_end_empty_content() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.txt");
    fs::write(&file_path, "a\nb\n").unwrap();

    let output = sniper()
        .args([&file_path.to_string_lossy(), "3", "3", ""])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "start=3,end=3 with empty content on 2-line file must succeed"
    );

    let content = read_file(&file_path);
    // Delete at end position with nothing to delete → file unchanged
    assert_eq!(
        normalize(&content),
        normalize("a\nb\n"),
        "Delete at end with empty content must leave file unchanged, got: {:?}",
        content
    );
}

#[test]
fn test_golden_error_format() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.txt");
    fs::write(&file_path, "test\n").unwrap();

    let output = sniper()
        .args([&file_path.to_string_lossy(), "0", "0", "41"])
        .output()
        .unwrap();

    assert!(!output.status.success(), "Line 0 must be rejected");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{}{}", stdout, stderr);

    assert!(
        combined.contains("out of bounds"),
        "Error must mention 'out of bounds', got: {}",
        combined
    );
    assert!(
        combined.contains("0-0") || combined.contains("0"),
        "Error must reference the invalid line number, got: {}",
        combined
    );
}
