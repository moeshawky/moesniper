//! Unit tests for G-EDGE failure mode
use std::fs;
use tempfile::TempDir;

fn create_test_file(content: &str) -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.txt");
    fs::write(&file_path, content).unwrap();
    (dir, file_path)
}

#[test]
fn test_edge_case_empty_file() {
    let (dir, file_path) = create_test_file("");
    let output = std::process::Command::new("cargo")
        .args(["run", "--quiet", "--", file_path.to_str().unwrap(), "1", "1", "41"])
        .output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("thread 'main' panicked"));
}

#[test]
fn test_edge_case_single_char() {
    let (dir, file_path) = create_test_file("x");
    let _ = std::process::Command::new("cargo")
        .args(["run", "--quiet", "--", file_path.to_str().unwrap(), "1", "1", "42"])
        .output().unwrap();
}

#[test]
fn test_edge_case_very_long_line() {
    let long_content = "x".repeat(100_000);
    let (dir, file_path) = create_test_file(&long_content);
    let output = std::process::Command::new("cargo")
        .args(["run", "--quiet", "--", file_path.to_str().unwrap(), "1", "1", "41"])
        .output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("thread 'main' panicked"));
}

#[test]
fn test_edge_case_unicode() {
    let (dir, file_path) = create_test_file("مرحبا");
    let output = std::process::Command::new("cargo")
        .args(["run", "--quiet", "--", file_path.to_str().unwrap(), "1", "1", "41"])
        .output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("thread 'main' panicked"));
}

#[test]
fn test_edge_case_line_zero() {
    let (dir, file_path) = create_test_file("test\n");
    let output = std::process::Command::new("cargo")
        .args(["run", "--quiet", "--", file_path.to_str().unwrap(), "0", "0", "41"])
        .output().unwrap();
    assert!(!output.status.success());
}

#[test]
fn test_edge_case_beyond_eof() {
    let (dir, file_path) = create_test_file("line1\n");
    let output = std::process::Command::new("cargo")
        .args(["run", "--quiet", "--", file_path.to_str().unwrap(), "100", "100", "41"])
        .output().unwrap();
    assert!(!output.status.success());
}
