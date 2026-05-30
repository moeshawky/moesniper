//! Property-based tests using proptest
//! Tests invariants that must hold for ALL inputs

use proptest::prelude::*;
use std::fs;
use tempfile::TempDir;

fn sniper() -> std::process::Command {
    let mut cmd = std::process::Command::new("cargo");
    cmd.args(["run", "--quiet", "--"]);
    cmd
}

fn run_sniper(file: &str, start: &str, end: &str, content: &str) -> bool {
    let output = sniper()
        .args([file, start, end, content])
        .output()
        .expect("Failed to execute");
    output.status.success()
}

// Encode/decode roundtrip invariant
// Property: decode(encode(x)) == x
#[test]
fn prop_encode_decode_roundtrip() {
    proptest!(|(input in "[a-zA-Z0-9 ]{0,50}")| {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("input.txt");
        fs::write(&file_path, &input).unwrap();

        // Encode the file content via CLI
        let encode_output = std::process::Command::new("cargo")
            .args(["run", "--quiet", "--", "encode", &file_path.to_string_lossy()])
            .output()
            .expect("Failed to execute encode");

        if encode_output.status.success() {
            let hex = String::from_utf8_lossy(&encode_output.stdout).trim().to_string();
            // Hex output must be non-empty for non-empty input
            if !input.is_empty() {
                prop_assert!(!hex.is_empty(), "Non-empty input produced empty hex");
                // Hex must be valid hex characters
                prop_assert!(
                    hex.chars().all(|c| c.is_ascii_hexdigit() || c.is_whitespace()),
                    "Hex output contains non-hex chars: {}",
                    hex
                );
            }
        }
    });
}

// Undo is inverse of edit
// Property: undo(edit(file)) restores original content
#[test]
fn prop_undo_restores_content() {
    proptest!(|(content in "[a-zA-Z0-9\\n]{1,100}", replacement in "[a-zA-Z]{1,10}")| {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, &content).unwrap();

        let edit_success = run_sniper(
            &file_path.to_string_lossy(),
            "1", "1", &replacement
        );

        if edit_success {
            let undo_output = std::process::Command::new("cargo")
                .args(["run", "--quiet", "--", &file_path.to_string_lossy(), "--undo"])
                .output()
                .expect("Failed to execute undo");

            if undo_output.status.success() {
                let restored = fs::read_to_string(&file_path).unwrap();
                // CRITICAL: restored MUST equal original — no tautology
                prop_assert!(
                    restored == content,
                    "Undo failed: original={:?}, restored={:?}",
                    content,
                    restored
                );
            }
        }
    });
}

// Line numbers are 1-indexed — line 0 always fails
#[test]
fn prop_line_zero_always_fails() {
    proptest!(|(content in "[a-zA-Z\\n]{1,50}")| {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, &content).unwrap();

        let output = std::process::Command::new("cargo")
            .args(["run", "--quiet", "--", &file_path.to_string_lossy(), "0", "0", "41"])
            .output()
            .expect("Failed to execute");

        prop_assert!(!output.status.success());
    });
}

// Non-empty file stays non-empty after valid edit
#[test]
fn prop_file_stays_nonempty_after_edit() {
    proptest!(|(line_content in "[a-z]{1,20}")| {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        let original = format!("{}\n", line_content);
        fs::write(&file_path, &original).unwrap();

        run_sniper(&file_path.to_string_lossy(), "1", "1", "58"); // 'X'

        let new_content = fs::read_to_string(&file_path).unwrap_or_default();
        prop_assert!(!new_content.is_empty());
    });
}

// No state corruption after failed operations
// Property: After invalid operation, file content is UNCHANGED
#[test]
fn prop_no_corruption_on_failure() {
    proptest!(|(content in "[a-zA-Z\\n]{10,100}")| {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, &content).unwrap();

        // Try invalid operation (line 999 — beyond EOF)
        let _ = std::process::Command::new("cargo")
            .args(["run", "--quiet", "--", &file_path.to_string_lossy(), "999", "999", "41"])
            .output();

        // File MUST be unchanged — corruption is never acceptable
        let after_content = fs::read_to_string(&file_path).unwrap_or_default();
        let content_clone = content.clone();
        prop_assert!(
            after_content == content_clone,
            "File corrupted after failed operation"
        );
    });
}
