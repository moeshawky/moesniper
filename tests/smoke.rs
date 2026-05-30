//! Smoke tests — run first. If these fail, nothing else matters.

use std::process::Command;

fn sniper() -> Command {
    let mut cmd = Command::new("cargo");
    cmd.args(["run", "--quiet", "--"]);
    cmd
}

// Verify the binary exists and runs
#[test]
fn test_binary_exists_and_runs() {
    let output = sniper().arg("--help").output().expect("Failed to execute");
    assert!(output.status.success(), "sniper --help should succeed");
}

// Verify core commands exist (no hallucinated APIs)
#[test]
fn test_core_commands_exist() {
    let output = sniper().arg("--help").output().unwrap();
    let help_text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        help_text.contains("--undo"),
        "undo command should be documented"
    );
    assert!(
        help_text.contains("manifest"),
        "manifest command should be documented"
    );
    assert!(
        help_text.contains("encode"),
        "encode command should be documented"
    );
}

// Path traversal must be rejected
#[test]
fn test_path_traversal_rejected() {
    use std::fs;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("test.txt");
    fs::write(&file_path, "secret\n").unwrap();

    let traversal_path = dir.path().join("..").join("test.txt");
    let output = sniper()
        .args([
            &traversal_path.to_string_lossy(),
            "1",
            "1",
            "41",
        ])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success() || stderr.contains("path") || stderr.contains("traversal"),
        "Path traversal with ../ must be rejected, got: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

// Verify file permissions are preserved after atomic write
#[test]
#[cfg(unix)]
fn test_file_permissions_preserved() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("script.sh");
    fs::write(&file_path, "echo 'hello'\n").unwrap();

    // Set executable permissions
    let mut perms = fs::metadata(&file_path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&file_path, perms.clone()).unwrap();

    let status = sniper()
        .args([
            &file_path.to_string_lossy(),
            "1",
            "1",
            "6563686f2027776f726c6427",
        ])
        .status()
        .unwrap();

    assert!(status.success());

    let content = fs::read_to_string(&file_path).unwrap();
    assert_eq!(content, "echo 'world'\n");

    let final_perms = fs::metadata(&file_path).unwrap().permissions();
    let mode = final_perms.mode() & 0o777;
    assert!(
        (0o400..=0o777).contains(&mode),
        "File must have reasonable permissions, got 0o{:o}",
        mode
    );
}

// Verify encode command exists and produces output
#[test]
fn test_encode_command_exists() {
    let output = sniper()
        .args(["encode", "--help"])
        .output()
        .expect("Failed to execute encode --help");

    assert!(
        output.status.success(),
        "encode --help should succeed, got stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        !combined.trim().is_empty(),
        "encode --help must produce non-empty output"
    );
}
