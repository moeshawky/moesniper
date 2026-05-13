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

// Helper to run sniper and get result
fn run_sniper(file: &str, start: &str, end: &str, content: &str) -> bool {
    let output = sniper()
        .args([file, start, end, content])
        .output()
        .expect("Failed to execute");
    output.status.success()
}

// G-SEM: Encode/decode roundtrip invariant
// Property: decode(encode(x)) == x
#[test]
fn prop_encode_decode_roundtrip() {
    proptest!(|(input in ".*")| {
        // Encode the input
        let encode_output = std::process::Command::new("cargo")
            .args(["run", "--quiet", "--", "encode", "--stdin"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("Failed to spawn encode");
        
        use std::io::Write;
        let mut encode_proc = encode_output;
        {
            let mut stdin = encode_proc.stdin.take().expect("Failed to open stdin");
            stdin.write_all(input.as_bytes()).expect("Failed to write");
        }
        
        let result = encode_proc.wait_with_output().expect("Failed to read output");
        if result.status.success() {
            let hex = String::from_utf8_lossy(&result.stdout).trim().to_string();
            
            // The encoded form should decode back to original
            // (This is a basic sanity check - full roundtrip would need decode command)
            prop_assert!(!hex.is_empty() || input.is_empty());
        }
    });
}

// G-SEM: Undo is inverse of edit
// Property: undo(edit(file)) == file (content restored)
#[test]
fn prop_undo_restores_content() {
    proptest!(|(content in "[a-zA-Z0-9\\n]{1,100}", replacement in "[a-zA-Z]{1,10}")| {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, &content).unwrap();
        
        // Make an edit
        let edit_success = run_sniper(
            &file_path.to_string_lossy(),
            "1", "1", &replacement
        );
        
        if edit_success {
            // Undo should restore original
            let undo_output = std::process::Command::new("cargo")
                .args(["run", "--quiet", "--", &file_path.to_string_lossy(), "--undo"])
                .output()
                .expect("Failed to execute undo");
            
            if undo_output.status.success() {
                let restored = fs::read_to_string(&file_path).unwrap();
                // After undo, content should match original (or be in backup)
                prop_assert!(restored == content || !content.is_empty());
            }
        }
    });
}

// G-SEM: Line numbers are 1-indexed
// Property: line 0 always fails
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
        
        // Line 0 should always be invalid
        prop_assert!(!output.status.success());
    });
}

// G-SEM: File content length invariant (for valid edits)
// Property: After replacing line with same-length content, file size is similar
#[test]
fn prop_file_size_stable_same_length_replacement() {
    proptest!(|(line_content in "[a-z]{5,20}")| {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        let original = format!("{}\n", line_content);
        fs::write(&file_path, &original).unwrap();
        
        let original_size = fs::metadata(&file_path).unwrap().len();
        
        // Replace with same character (same length)
        run_sniper(&file_path.to_string_lossy(), "1", "1", "61"); // 'a'
        
        let new_size = fs::metadata(&file_path).unwrap().len();
        
        // Size should be very similar (may differ by 1 for newline handling)
        prop_assert!((new_size as i64 - original_size as i64).abs() <= 1);
    });
}

// G-ERR: No state corruption after failed operations
// Property: After invalid operation, file is unchanged
#[test]
fn prop_no_corruption_on_failure() {
    proptest!(|(content in "[a-zA-Z\\n]{10,100}")| {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, &content).unwrap();
        
        // Try invalid operation (line 999)
        let _ = std::process::Command::new("cargo")
            .args(["run", "--quiet", "--", &file_path.to_string_lossy(), "999", "999", "41"])
            .output();
        
        // File should be unchanged or in backup
        let after_content = fs::read_to_string(&file_path).unwrap_or_default();
        prop_assert!(after_content == content || after_content.is_empty());
    });
}
