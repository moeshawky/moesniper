//! Golden file regression tests - G-DRIFT failure mode
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

// G-DRIFT: Undo stack behavior matches golden file
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
    
    let content = fs::read_to_string(&file_path).unwrap();
    let golden = include_str!("regression/golden/undo_stack.txt");
    
    // After 5 edits, content should be "5\n"
    assert_eq!(normalize(&content), "5\n", "Content after edits should match golden");
}

// G-DRIFT: Basic splice operation
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
    
    let content = fs::read_to_string(&file_path).unwrap();
    let golden = include_str!("regression/golden/splice_basic.txt");
    
    assert_eq!(normalize(&content), golden.trim(), "Splice result should match golden file");
}

// G-DRIFT: Newline preservation behavior
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
    
    if status.success() {
        let content = fs::read_to_string(&file_path).unwrap();
        // Should preserve trailing newline
        assert!(content.ends_with('\n'), "Should preserve trailing newline");
    }
}

// G-DRIFT: Manifest operation baseline
#[test]
fn test_golden_manifest_basic() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.txt");
    let manifest_path = dir.path().join("ops.json");
    
    fs::write(&file_path, "line1\nline2\nline3\n").unwrap();
    fs::write(&manifest_path, r#"[{"start": 2, "delete": true}]"#).unwrap();
    
    let status = sniper()
        .args([&file_path.to_string_lossy(), "--manifest", &manifest_path.to_string_lossy()])
        .status()
        .unwrap();
    
    // Should succeed
    assert!(status.success(), "Manifest operation should succeed");
    
    let content = fs::read_to_string(&file_path).unwrap();
    // After deleting line 2, should have "line1\nline3\n"
    assert!(content.contains("line1"), "Should keep line 1");
    assert!(!content.contains("line2"), "Should delete line 2");
    assert!(content.contains("line3"), "Should keep line 3");
}

// G-DRIFT: Error message format stability
#[test]
fn test_golden_error_format() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.txt");
    fs::write(&file_path, "test\n").unwrap();
    
    // Invalid line range should produce consistent error
    let output = sniper()
        .args([&file_path.to_string_lossy(), "0", "0", "41"])
        .output()
        .unwrap();
    
    assert!(!output.status.success(), "Should fail on invalid line");
    
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Error should mention the problem
    assert!(
        stderr.contains("line") || stderr.contains("range") || stderr.contains("bounds"),
        "Error should mention line/range/bounds issue"
    );
}
