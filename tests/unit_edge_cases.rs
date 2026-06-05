//! Edge case tests
use std::fs;
use tempfile::TempDir;

fn create_test_file(content: &str) -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.txt");
    fs::write(&file_path, content).unwrap();
    (dir, file_path)
}

// Empty file splice
#[test]
fn test_edge_case_empty_file() {
    let (_dir, file_path) = create_test_file("");
    let output = std::process::Command::new("cargo")
        .args([
            "run",
            "--quiet",
            "--",
            file_path.to_str().unwrap(),
            "1",
            "1",
            "41",
        ])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked"),
        "Empty file splice must not panic: {}",
        stderr
    );
    // Inserting into empty file should succeed or return a meaningful error
    let content_after = fs::read_to_string(&file_path).unwrap_or_default();
    if output.status.success() {
        assert_eq!(content_after, "A", "Empty file + hex 41 should insert 'A'");
    }
}

// Single char file — replace with different content
#[test]
fn test_edge_case_single_char() {
    let (_dir, file_path) = create_test_file("x");
    let output = std::process::Command::new("cargo")
        .args([
            "run",
            "--quiet",
            "--",
            file_path.to_str().unwrap(),
            "1",
            "1",
            "42",
        ])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked"),
        "Single char splice must not panic: {}",
        stderr
    );
    assert!(
        output.status.success(),
        "Single char splice should succeed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let content_after = fs::read_to_string(&file_path).unwrap();
    assert_eq!(
        content_after, "B",
        "Single char 'x' replaced with hex 42 ('B')"
    );
}

// Very long line (100K chars)
#[test]
fn test_edge_case_very_long_line() {
    let long_content = "x".repeat(100_000);
    let (_dir, file_path) = create_test_file(&long_content);
    let output = std::process::Command::new("cargo")
        .args([
            "run",
            "--quiet",
            "--",
            file_path.to_str().unwrap(),
            "1",
            "1",
            "41",
        ])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked"),
        "100K char line must not panic: {}",
        stderr
    );
    assert!(
        output.status.success(),
        "100K char line splice should succeed"
    );
}

// Unicode content
#[test]
fn test_edge_case_unicode() {
    let (_dir, file_path) = create_test_file("مرحبا");
    let output = std::process::Command::new("cargo")
        .args([
            "run",
            "--quiet",
            "--",
            file_path.to_str().unwrap(),
            "1",
            "1",
            "41",
        ])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked"),
        "Unicode content must not panic: {}",
        stderr
    );
    // Unicode file should be handled gracefully
    assert!(
        output.status.success(),
        "Unicode splice should succeed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

// Line zero must always fail (1-indexed)
#[test]
fn test_edge_case_line_zero() {
    let (_dir, file_path) = create_test_file("test\n");
    let output = std::process::Command::new("cargo")
        .args([
            "run",
            "--quiet",
            "--",
            file_path.to_str().unwrap(),
            "0",
            "0",
            "41",
        ])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "Line 0 must be rejected (1-indexed system)"
    );
}

// Beyond EOF must fail
#[test]
fn test_edge_case_beyond_eof() {
    let (_dir, file_path) = create_test_file("line1\n");
    let output = std::process::Command::new("cargo")
        .args([
            "run",
            "--quiet",
            "--",
            file_path.to_str().unwrap(),
            "100",
            "100",
            "41",
        ])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "Line 100 in 1-line file must be rejected"
    );
}
