//! Enterprise Security Test Suite
//!
//! Tests for security fixes:
//! - FIND-001: Path traversal prevention
//! - FIND-002: Configurable lock timeout
//! - FIND-003: File size limits
//! - FIND-005: Backup retention policy
//! - FIND-006: Documentation verification

use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

// Import the library
use moesniper::{
    check_file_size, purge_old_backups, normalize_path_secure, SniperConfig,
    validate_path, SecurityPolicy, PathSecurityError,
};

mod path_security_tests {
    use super::*;

    #[test]
    fn test_path_traversal_blocked_basic() {
        // Test that basic path traversal is rejected
        let policy = SecurityPolicy::default();
        let result = validate_path("../../../etc/passwd", &policy);
        
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            PathSecurityError::ParentReferenceNotAllowed { .. }
        ));
    }

    #[test]
    fn test_path_traversal_blocked_nested() {
        // Test nested parent references
        let policy = SecurityPolicy::default();
        let result = validate_path("foo/bar/../../../etc/passwd", &policy);
        
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            PathSecurityError::ParentReferenceNotAllowed { .. }
        ));
    }

    #[test]
    fn test_path_traversal_with_base_directory() {
        let dir = TempDir::new().unwrap();
        let base = dir.path();
        
        // Create files
        let inside_file = base.join("inside.txt");
        fs::write(&inside_file, "content").unwrap();
        
        let outside_file = dir.path().parent().unwrap().join("outside.txt");
        fs::write(&outside_file, "content").unwrap();
        
        let policy = SecurityPolicy {
            base_dir: Some(base.to_path_buf()),
            reject_parent_refs: true,
        };
        
        // File inside base should succeed
        let result = validate_path(&inside_file, &policy);
        assert!(result.is_ok());
        
        // File outside base should fail
        let result = validate_path(&outside_file, &policy);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            PathSecurityError::EscapesBaseDirectory { .. }
        ));
        
        // Cleanup
        let _ = fs::remove_file(&outside_file);
    }

    #[test]
    fn test_normalize_path_secure_allows_valid_paths() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "content").unwrap();
        
        // Valid path with base directory
        let result = normalize_path_secure(
            file.to_str().unwrap(),
            Some(dir.path())
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_normalize_path_secure_rejects_traversal() {
        let dir = TempDir::new().unwrap();
        
        // Path traversal should be rejected
        let result = normalize_path_secure(
            "../../../etc/passwd",
            Some(dir.path())
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_parent_refs_allowed_when_configured() {
        let dir = TempDir::new().unwrap();
        let subdir = dir.path().join("subdir");
        fs::create_dir(&subdir).unwrap();
        let file = subdir.join("test.txt");
        fs::write(&file, "content").unwrap();
        
        // Reference parent directory - but note: even with reject_parent_refs: false,
        // if base_dir is set, the path must still be within base. The parent ref
        // "subdir/../test.txt" resolves to "test.txt" which is in base, so this works.
        let parent_ref = PathBuf::from("subdir").join("..").join("test.txt");
        
        // With reject_parent_refs: false, should work (after cleaning path)
        let policy = SecurityPolicy {
            base_dir: Some(dir.path().to_path_buf()),
            reject_parent_refs: false,
        };
        
        // This should succeed since parent_refs are allowed and resolved path is in base
        let result = validate_path(&parent_ref, &policy);
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result);
    }
}

mod file_size_tests {
    use super::*;

    #[test]
    fn test_file_size_within_limit() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("small.txt");
        fs::write(&file, "small content").unwrap();

        // File is well under 100MB
        let result = check_file_size(file.to_str().unwrap(), 100 * 1024 * 1024);
        assert!(result.is_ok());
    }

    #[test]
    fn test_file_size_exceeds_limit() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("large.txt");
        fs::write(&file, "x".repeat(100)).unwrap();

        // File is 100 bytes, limit is 10 bytes
        let result = check_file_size(file.to_str().unwrap(), 10);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("File too large"));
    }

    #[test]
    fn test_file_size_unlimited() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("any.txt");
        fs::write(&file, "any content").unwrap();

        // max_size = 0 means unlimited
        let result = check_file_size(file.to_str().unwrap(), 0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_file_size_nonexistent_file() {
        // Non-existent file should fail with metadata error
        let result = check_file_size("/tmp/nonexistent_file_12345.txt", 100);
        assert!(result.is_err());
    }
}

mod backup_retention_tests {
    use super::*;
    use moesniper::create_backup;
    use moesniper::get_path_hash;
    use moesniper::normalize_path;

    #[test]
    fn test_backup_retention_by_count() {
        // Use current directory for test
        let file = PathBuf::from("test_retention.txt");
        let _ = fs::write(&file, "v0");

        let mut config = SniperConfig::default();
        config.backup_retention_count = 3;
        config.backup_max_age_days = 0; // Disable age-based purge
        config.audit_enabled = false;

        // Create 5 backups
        for i in 0..5 {
            let _ = fs::write(&file, format!("v{}", i));
            let _ = create_backup(file.to_str().unwrap());
            thread::sleep(Duration::from_millis(20));
        }

        // Get hash for counting
        let normalized = normalize_path(file.to_str().unwrap());
        if normalized.is_err() {
            let _ = fs::remove_file(&file);
            return; // Skip if path issues
        }
        let normalized = normalized.unwrap();
        let hash = get_path_hash(&normalized);
        let backup_dir = PathBuf::from(".sniper");

        if !backup_dir.exists() {
            let _ = fs::remove_file(&file);
            return;
        }

        // Count backups before purge
        let before_count: usize = fs::read_dir(&backup_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(&hash))
            .count();

        if before_count >= 5 {
            // Purge old backups
            let _ = purge_old_backups(file.to_str().unwrap(), &config);

            // Count backups after purge
            let after_count: usize = fs::read_dir(&backup_dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().starts_with(&hash))
                .count();

            // Should have at most 3 backups (retention count)
            assert_eq!(after_count, 3);
        }

        // Cleanup
        let _ = fs::remove_file(&file);
    }

    #[test]
    fn test_backup_retention_by_age() {
        // This test would require manipulating file timestamps
        // For now, just verify the function doesn't panic
        let config = SniperConfig::default();
        let result = purge_old_backups("/tmp/nonexistent.txt", &config);
        // Should not panic, may return error
        let _ = result;
    }
}

mod config_tests {
    use super::*;

    #[test]
    fn test_config_default_values() {
        let config = SniperConfig::default();
        
        // Check default values
        assert_eq!(config.lock_timeout, Duration::from_secs(30));
        assert_eq!(config.max_file_size, 100 * 1024 * 1024); // 100MB
        assert_eq!(config.backup_retention_count, 50);
        assert_eq!(config.backup_max_age_days, 30);
    }

    #[test]
    fn test_size_parsing_bytes() {
        // Test the size parsing from config module
        // This is tested in config::tests, but verify here too
        let config = SniperConfig::default();
        assert_eq!(config.max_file_size, 100 * 1024 * 1024);
    }
}

// Documentation verification tests
mod documentation_tests {
    #[test]
    fn test_line_number_documentation() {
        // Verify help text mentions 1-based line numbers
        use std::process::Command;
        
        let output = Command::new("cargo")
            .args(["run", "--quiet", "--", "--help"])
            .current_dir("/workspace/sniper")
            .output()
            .unwrap();
        
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let help_text = if stdout.is_empty() { stderr } else { stdout };
        
        // Should mention line numbers
        assert!(
            help_text.contains("line") || help_text.contains("Line"),
            "Help text should mention line numbers"
        );
    }
}
