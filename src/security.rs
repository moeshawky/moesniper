//! Path security validation and sanitization.
//!
//! This module provides defense-in-depth path validation to prevent:
//! - Path traversal attacks (../../../etc/passwd)
//! - Symlink attacks
//! - Directory escape attempts

use std::fs;
use std::path::{Component, Path, PathBuf};

/// Security policy configuration for path validation.
///
/// Controls which path patterns are rejected and whether base directory
/// containment is enforced. Use `SecurityPolicy::default()` for the
/// standard policy that rejects parent references without base restriction.
#[derive(Debug, Clone)]
pub struct SecurityPolicy {
    /// Base directory that all paths must be within.
    /// When set, `validate_path` rejects any path that resolves outside
    /// this directory. When None, base directory containment is NOT enforced.
    pub base_dir: Option<PathBuf>,
    /// Whether to reject paths containing parent references (..).
    /// When true, any path component equal to `..` triggers
    /// `PathSecurityError::ParentReferenceNotAllowed`.
    pub reject_parent_refs: bool,
}

impl Default for SecurityPolicy {
    fn default() -> Self {
        Self {
            base_dir: None,           // No base restriction by default (backward compatible)
            reject_parent_refs: true, // Always reject parent refs
        }
    }
}

/// Path validation error types.
///
/// Produced by `validate_path` when a path violates the security policy.
/// Each variant identifies the specific violation for targeted error handling.
#[derive(Debug, Clone, PartialEq)]
pub enum PathSecurityError {
    /// Path contains a parent directory reference (..) and policy rejects it.
    /// The `component` field contains the offending path component string.
    ParentReferenceNotAllowed { component: String },
    /// Path resolves outside the allowed base directory after canonicalization.
    /// Contains both the resolved `path` and the configured `base` for diagnostics.
    EscapesBaseDirectory { path: PathBuf, base: PathBuf },
    /// Filesystem I/O error occurred during path resolution or canonicalization.
    /// The string contains the OS error description.
    IoError(String),
}

impl std::fmt::Display for PathSecurityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathSecurityError::ParentReferenceNotAllowed { component } => {
                write!(f, "Parent reference not allowed: {}", component)
            }
            PathSecurityError::EscapesBaseDirectory { path, base } => {
                write!(
                    f,
                    "Path escapes base directory: {:?} (base: {:?})",
                    path, base
                )
            }
            PathSecurityError::IoError(e) => {
                write!(f, "IO error: {}", e)
            }
        }
    }
}

impl std::error::Error for PathSecurityError {}

/// Validates and sanitizes a path according to the security policy.
pub fn validate_path<P: AsRef<Path>>(
    path: P,
    policy: &SecurityPolicy,
) -> Result<PathBuf, PathSecurityError> {
    let path = path.as_ref();

    // Explicit null-byte guard: C truncation can hide embedded NULs.
    if path.as_os_str().as_encoded_bytes().contains(&0) {
        return Err(PathSecurityError::IoError(
            "path contains a null byte".to_string(),
        ));
    }

    // Layer 1: Check for parent references
    for component in path.components() {
        if component == Component::ParentDir && policy.reject_parent_refs {
            return Err(PathSecurityError::ParentReferenceNotAllowed {
                component: "..".to_string(),
            });
        }
    }

    // Layer 2: Base directory containment (only if configured)
    let base_dir = policy.base_dir.as_ref();

    let canonical = if path.exists() {
        fs::canonicalize(path).map_err(|e| PathSecurityError::IoError(e.to_string()))?
    } else if let Some(base) = base_dir {
        // For non-existent files with base_dir, resolve against base
        let resolved = base.join(path);
        clean_path(&resolved)
    } else {
        // For non-existent files without base_dir, use clean_path
        clean_path(path)
    };

    if let Some(base) = base_dir {
        let canonical_base = base.canonicalize().map_err(|e| {
            PathSecurityError::IoError(format!("Failed to canonicalize base directory: {}", e))
        })?;

        if !canonical.starts_with(&canonical_base) {
            return Err(PathSecurityError::EscapesBaseDirectory {
                path: canonical,
                base: canonical_base,
            });
        }
    }

    Ok(canonical)
}

/// Returns true when `path` is an existing regular file.
///
/// Useful as a quick pre-read guard to reject FIFOs, pipes, and other
/// special files that would otherwise block or behave unexpectedly.
pub fn is_regular_file<P: AsRef<Path>>(path: P) -> bool {
    fs::metadata(path.as_ref())
        .map(|m| m.is_file())
        .unwrap_or(false)
}

/// Clean a path by removing redundant components (. and ..).
fn clean_path(path: &Path) -> PathBuf {
    let mut components: Vec<Component> = Vec::new();

    for component in path.components() {
        match component {
            Component::CurDir => {
                continue;
            }
            Component::ParentDir => {
                if let Some(Component::Normal(_)) = components.last() {
                    components.pop();
                } else if components.is_empty() {
                    // Leading .. - keep it for absolute paths
                    components.push(component);
                }
            }
            other => {
                components.push(other);
            }
        }
    }

    components.iter().collect()
}

/// Secure version of normalize_path.
pub fn normalize_path_secure(path: &str, base_dir: Option<&Path>) -> Result<PathBuf, String> {
    let policy = SecurityPolicy {
        base_dir: base_dir.map(|p| p.to_path_buf()),
        ..SecurityPolicy::default()
    };

    validate_path(path, &policy).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_valid_path() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "test").unwrap();

        let policy = SecurityPolicy::default();
        let result = validate_path(&file, &policy);
        assert!(result.is_ok());
    }

    #[test]
    fn test_path_traversal_blocked() {
        let policy = SecurityPolicy::default();
        let malicious = "../../../etc/passwd";

        let result = validate_path(malicious, &policy);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            PathSecurityError::ParentReferenceNotAllowed { .. }
        ));
    }

    #[test]
    fn test_escapes_base_directory() {
        let dir = TempDir::new().unwrap();
        let outside = dir.path().parent().unwrap().join("outside.txt");
        fs::write(&outside, "content").unwrap();

        let policy = SecurityPolicy {
            base_dir: Some(dir.path().to_path_buf()),
            ..SecurityPolicy::default()
        };

        let result = validate_path(&outside, &policy);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            PathSecurityError::EscapesBaseDirectory { .. }
        ));
    }

    #[test]
    fn test_clean_path() {
        let messy = PathBuf::from("/tmp/subdir/../file.txt");
        let cleaned = clean_path(&messy);
        assert_eq!(cleaned, PathBuf::from("/tmp/file.txt"));
    }

    #[test]
    fn test_normalize_path_secure_rejects_traversal() {
        let dir = TempDir::new().unwrap();

        let result = normalize_path_secure("../../../etc/passwd", Some(dir.path()));
        assert!(result.is_err());
    }

    #[test]
    fn test_new_file_path_validation() {
        let dir = TempDir::new().unwrap();
        let new_file = dir.path().join("subdir").join("new.txt");
        fs::create_dir_all(new_file.parent().unwrap()).unwrap();

        let policy = SecurityPolicy::default();
        // This should succeed even though file doesn't exist
        let result = validate_path(&new_file, &policy);
        assert!(result.is_ok());
    }

    #[test]
    fn test_base_dir_nonexistent_file_inside() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_path_buf();
        let target = base.join("nonexistent.txt");

        let policy = SecurityPolicy {
            base_dir: Some(base),
            reject_parent_refs: true,
        };

        let result = validate_path(&target, &policy);
        assert!(result.is_ok());
    }

    #[test]
    fn test_base_dir_nonexistent_file_escapes_via_parent_ref() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_path_buf();
        // Create a path that looks like it's in base but escapes it via parent refs
        // We set reject_parent_refs = false to test the base_dir escape logic specifically
        let target = base.join("..").join("outside.txt");

        let policy = SecurityPolicy {
            base_dir: Some(base),
            reject_parent_refs: false,
        };

        let result = validate_path(&target, &policy);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            PathSecurityError::EscapesBaseDirectory { .. }
        ));
    }

    #[test]
    fn test_base_dir_nonexistent_absolute_path_escapes() {
        let dir = TempDir::new().unwrap();
        let base = dir.path().to_path_buf();

        #[cfg(unix)]
        let target = PathBuf::from("/tmp/some_random_nonexistent_file.txt");

        #[cfg(windows)]
        let target = PathBuf::from("C:\\temp\\some_random_nonexistent_file.txt");

        let policy = SecurityPolicy {
            base_dir: Some(base),
            reject_parent_refs: true,
        };

        let result = validate_path(&target, &policy);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            PathSecurityError::EscapesBaseDirectory { .. }
        ));
    }

    #[test]
    fn test_table_driven_traversal() {
        let policy = SecurityPolicy::default();
        let cases = vec![
            "../etc/passwd",
            "foo/bar/../../../etc/passwd",
            "foo/../bar",
            "../",
            "..",
            "a/b/c/..",
        ];

        for case in cases {
            let result = validate_path(case, &policy);
            assert!(result.is_err(), "Expected {} to fail", case);
            assert!(
                matches!(
                    result.unwrap_err(),
                    PathSecurityError::ParentReferenceNotAllowed { .. }
                ),
                "Expected ParentReferenceNotAllowed for {}",
                case
            );
        }
    }

    #[test]
    fn test_null_byte_rejected() {
        let policy = SecurityPolicy::default();
        // Use OsStr to embed an actual null byte that Rust's C layer would truncate.
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let with_nul = OsStr::from_bytes(b"file.txt\x00.txt");
        let result = validate_path(with_nul, &policy);
        assert!(result.is_err(), "Path with null byte must be rejected");
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("null byte"),
            "Error should mention null byte: {}",
            err
        );
    }

    #[test]
    fn test_is_regular_file() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("regular.txt");
        fs::write(&file, "content").unwrap();
        assert!(is_regular_file(&file));
        assert!(!is_regular_file(dir.path().join("nonexistent.txt")));
        assert!(!is_regular_file(dir.path()));
    }
}
