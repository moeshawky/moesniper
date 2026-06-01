#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod config;
pub mod security;

pub use config::SniperConfig;
pub use security::{normalize_path_secure, validate_path, PathSecurityError, SecurityPolicy};

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime};

use llmosafe::ResourceGuard;
use sha2::Digest;

pub const BACKUP_DIR: &str = ".sniper";

/// Strict hex decoding: skips whitespace, errors on non-hex or odd-length strings.
pub fn hex_decode(hex: &str) -> Result<String, String> {
    let clean: String = hex.chars().filter(|c| !c.is_whitespace()).collect();

    if !clean.len().is_multiple_of(2) {
        return Err(format!("odd-length hex string: {}", clean.len()));
    }

    if let Some(c) = clean.chars().find(|c| !c.is_ascii_hexdigit()) {
        return Err(format!("invalid hex character: '{c}'"));
    }

    let mut bytes = Vec::with_capacity(clean.len() / 2);
    for i in (0..clean.len()).step_by(2) {
        let res = u8::from_str_radix(&clean[i..i + 2], 16)
            .map_err(|e| format!("hex decode at byte {}: {e}", i / 2))?;
        bytes.push(res);
    }

    String::from_utf8(bytes).map_err(|e| format!("utf8 decode: {e}"))
}

/// Normalize a file path.
///
/// This function applies path traversal protection while maintaining
/// backward compatibility with existing code.
pub fn normalize_path(path: &str) -> Result<PathBuf, String> {
    // Use default security policy (rejects parent refs, allows absolute)
    let policy = SecurityPolicy::default();
    validate_path(path, &policy).map_err(|e| e.to_string())
}

/// Check if file size exceeds the configured limit.
pub fn check_file_size(filepath: &str, max_size: u64) -> Result<(), String> {
    if max_size == 0 {
        // Unlimited
        return Ok(());
    }

    let metadata = fs::metadata(filepath)
        .map_err(|e| format!("Failed to get metadata for {}: {}", filepath, e))?;

    let size = metadata.len();
    if size > max_size {
        Err(format!(
            "File too large: {} bytes (max: {} bytes). Use SNIPER_MAX_FILE_SIZE to increase limit.",
            size, max_size
        ))
    } else {
        Ok(())
    }
}

pub fn get_path_hash(path: &Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

pub fn create_backup(filepath: &str) -> Result<String, String> {
    let normalized = normalize_path(filepath)?;
    let hash = get_path_hash(&normalized);

    let dir = PathBuf::from(BACKUP_DIR);
    fs::create_dir_all(&dir).map_err(|e| format!("create backup dir: {e}"))?;

    let name = normalized
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    let ts = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .map_err(|e| format!("timestamp: {e}"))?;

    let backup_name = format!("{hash}.{name}.{ts}");
    let dst = dir.join(&backup_name);

    if normalized.exists() {
        fs::copy(&normalized, &dst).map_err(|e| format!("backup copy: {e}"))?;
    } else {
        fs::File::create(&dst).map_err(|e| format!("create empty backup: {e}"))?;
    }

    Ok(dst.to_string_lossy().into())
}

/// Purge old backups according to retention policy.
pub fn purge_old_backups(filepath: &str, config: &SniperConfig) -> Result<(), String> {
    if config.backup_retention_count == 0 && config.backup_max_age_days == 0 {
        // No retention policy configured
        return Ok(());
    }

    let normalized = normalize_path(filepath)?;
    let hash = get_path_hash(&normalized);
    let dir = PathBuf::from(BACKUP_DIR);

    if !dir.exists() {
        return Ok(());
    }

    // Collect all backups for this file
    let mut backups: Vec<_> = fs::read_dir(&dir)
        .map_err(|e| format!("read backup dir: {e}"))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(&hash))
        .filter_map(|e| {
            let path = e.path();
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((path, modified))
        })
        .collect();

    // Sort by modification time (oldest first)
    backups.sort_by_key(|(_, modified)| *modified);

    let now = SystemTime::now();
    let max_age = if config.backup_max_age_days > 0 {
        Some(Duration::from_secs(
            config.backup_max_age_days * 24 * 60 * 60,
        ))
    } else {
        None
    };

    let mut to_delete = Vec::new();

    // Age-based purge
    if let Some(max_age_duration) = max_age {
        for (path, modified) in &backups {
            if now.duration_since(*modified).unwrap_or(Duration::ZERO) > max_age_duration {
                to_delete.push(path.clone());
            }
        }
    }

    // Count-based purge (keep most recent N)
    if config.backup_retention_count > 0 && backups.len() > config.backup_retention_count {
        let to_remove = backups.len() - config.backup_retention_count;
        for (path, _) in backups.iter().take(to_remove) {
            if !to_delete.contains(path) {
                to_delete.push(path.clone());
            }
        }
    }

    // Delete marked backups
    for path in to_delete {
        let _ = fs::remove_file(&path);
        if config.audit_enabled {
            eprintln!("[SNIPER-AUDIT] Purged old backup: {:?}", path);
        }
    }

    Ok(())
}

/// Finds the most recent backup for a given file.
pub fn find_latest_backup(filepath: &str) -> Result<Option<PathBuf>, String> {
    let normalized = normalize_path(filepath)?;
    let hash = get_path_hash(&normalized);
    let dir = PathBuf::from(BACKUP_DIR);

    if !dir.exists() {
        return Ok(None);
    }

    let latest_backup = fs::read_dir(dir)
        .map_err(|e| format!("read backup dir: {e}"))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(&hash))
        .map(|e| e.path())
        .max();

    Ok(latest_backup)
}

pub fn write_atomic(filepath: &str, lines: &[&str]) -> Result<(), String> {
    let has_trailing_newline = check_trailing_newline(filepath)?;
    write_atomic_impl(filepath, lines, has_trailing_newline)
}

fn check_trailing_newline(filepath: &str) -> Result<bool, String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = match fs::File::open(filepath) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(format!("open {filepath}: {e}")),
    };
    let metadata = f
        .metadata()
        .map_err(|e| format!("metadata {filepath}: {e}"))?;
    if metadata.len() == 0 {
        return Ok(false);
    }
    if f.seek(SeekFrom::End(-1)).is_err() {
        return Ok(false);
    }
    let mut last_byte = [0u8; 1];
    if f.read_exact(&mut last_byte).is_err() {
        return Ok(false);
    }
    Ok(last_byte[0] == b'\n')
}

/// Unified atomic write with metabolic pacing via llmosafe 0.6.2.
///
/// Trailing newlines are stripped from each line, then:
/// - All lines except the last get a newline appended
/// - The last line gets a newline ONLY if the original file had one
///
/// This ensures deterministic behavior regardless of input format.
fn write_atomic_impl<S: AsRef<str>>(
    filepath: &str,
    lines: &[S],
    has_trailing_newline: bool,
) -> Result<(), String> {
    let ts = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = format!("{filepath}.sniper_tmp.{ts}");
    let f = fs::File::create(&tmp).map_err(|e| format!("create tmp: {e}"))?;
    let mut f = std::io::BufWriter::new(f);
    let num_lines = lines.len();
    for (i, line) in lines.iter().enumerate() {
        let mut bytes = line.as_ref().as_bytes();
        // Strip trailing newline from the line string to handle it uniformly
        if bytes.ends_with(b"\n") {
            bytes = &bytes[..bytes.len() - 1];
        }
        f.write_all(bytes).map_err(|e| format!("write: {e}"))?;
        let is_last = i == num_lines - 1;
        if !is_last || has_trailing_newline {
            f.write_all(b"\n")
                .map_err(|e| format!("write newline: {e}"))?;
        }
    }
    f.into_inner().map_err(|e| format!("flush: {e}"))?;
    // Metabolic Pacing: entropy-weighted sleep with auto-scaled memory ceiling.
    // ResourceGuard::auto(0.5) uses 50% of system memory as the safety ceiling,
    // adapting to different deployment environments.
    let guard = ResourceGuard::auto(0.5);
    guard.check().map_err(|e| format!("resource safety: {e}"))?;
    let entropy = guard.raw_entropy();
    if entropy > 500 {
        thread::sleep(Duration::from_millis((entropy / 2) as u64));
    }

    match fs::rename(&tmp, filepath) {
        Ok(_) => Ok(()),
        Err(e) => Err(handle_backtrack_error(e, "Atomic write")),
    }
}

/// Verify pre-edit context: hash 3 lines before start and 3 lines after end,
/// compare against the expected hash. Returns Ok if match, Err with message if not.
pub fn verify_context(
    lines: &[String],
    start: usize,
    end: usize,
    expected_hash: &str,
) -> Result<(), String> {
    let before_start = start.saturating_sub(1).saturating_sub(3);
    let before_end = (start.saturating_sub(1)).min(lines.len());
    let after_start = end;
    let after_end = (end + 3).min(lines.len());

    let mut hasher = sha2::Sha256::new();
    for i in before_start..before_end {
        if i < lines.len() {
            hasher.update(lines[i].as_bytes());
        }
    }
    for i in after_start..after_end {
        if i < lines.len() {
            hasher.update(lines[i].as_bytes());
        }
    }
    let hash = hasher.finalize();
    let actual_hex: String = hash.iter().map(|b| format!("{:02x}", b)).collect();
    let actual_short = &actual_hex[..16];

    if actual_short == expected_hash {
        Ok(())
    } else {
        Err(format!(
            "context mismatch: content around line {} changed. Re-read the file and retry.",
            start
        ))
    }
}

/// Counts backups for a file hash created within the last `window_secs` seconds.
/// Returns the count of recent backups. Used for manifest promotion detection.
pub fn count_recent_backups(filepath: &str, window_secs: u64) -> Result<usize, String> {
    let normalized = normalize_path(filepath)?;
    let hash = get_path_hash(&normalized);
    let dir = PathBuf::from(BACKUP_DIR);

    if !dir.exists() {
        return Ok(0);
    }

    let now = SystemTime::now();
    let cutoff = now - Duration::from_secs(window_secs);

    let count = fs::read_dir(&dir)
        .map_err(|e| format!("read backup dir: {e}"))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(&hash))
        .filter(|e| {
            e.metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|t| t > cutoff)
                .unwrap_or(false)
        })
        .count();

    Ok(count)
}

/// Centralized handling for llmosafe Backtrack Signal (-7).
///
/// In llmosafe 0.6.2+, resource exhaustion surfaces via `KernelError` from
/// `ResourceGuard::check()` rather than OS signals on IO operations.
/// This function remains as a defensive fallback — if the OS ever returns
/// error code -7 (llmosafe's legacy DeadlineExceeded code) on an IO operation,
/// it will be caught here.
pub fn handle_backtrack_error(e: std::io::Error, context: &str) -> String {
    if e.raw_os_error() == Some(-7) {
        format!("CRITICAL: {context} aborted via llmosafe Backtrack Signal (-7). Immune memory triggered: current state matches a previously rolled-back failure pattern.")
    } else {
        format!("{context}: {e}")
    }
}

/// File-based lock with configurable timeout and stale lock detection.
pub struct SniperLock {
    lock_path: PathBuf,
}

/// Check if a process with the given PID is alive.
#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    use std::path::Path;
    Path::new(&format!("/proc/{}", pid)).exists()
}

#[cfg(not(unix))]
fn is_process_alive(_pid: u32) -> bool {
    true
}

impl SniperLock {
    /// Acquire a lock with configurable timeout.
    pub fn acquire(filepath: &str) -> Result<Self, String> {
        Self::acquire_with_config(filepath, &SniperConfig::from_env())
    }

    /// Acquire a lock with explicit configuration.
    pub fn acquire_with_config(filepath: &str, config: &SniperConfig) -> Result<Self, String> {
        let normalized = normalize_path(filepath)?;
        let hash = get_path_hash(&normalized);
        let dir = PathBuf::from(BACKUP_DIR);
        fs::create_dir_all(&dir).map_err(|e| format!("create .sniper: {e}"))?;
        let lock_path = dir.join(format!("sniper.{}.lock", hash));

        let start = SystemTime::now();
        let timeout = config.lock_timeout;
        let check_interval = Duration::from_millis(50);

        loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(mut f) => {
                    let pid = std::process::id();
                    let _ = write!(f, "{}", pid);
                    return Ok(Self { lock_path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if start.elapsed().unwrap_or(Duration::ZERO) > timeout {
                        if let Ok(content) = fs::read_to_string(&lock_path) {
                            if let Ok(pid) = content.trim().parse::<u32>() {
                                if !is_process_alive(pid) {
                                    let _ = fs::remove_file(&lock_path);
                                    continue;
                                }
                            }
                        }
                        return Err(format!(
                            "timeout: another sniper process is editing {} (lock held for >{:?})",
                            filepath, timeout
                        ));
                    }
                    thread::sleep(check_interval);
                }
                Err(e) => return Err(format!("lock acquire for {filepath}: {e}")),
            }
        }
    }
}

impl Drop for SniperLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.lock_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs;

    #[test]
    fn test_normalize_path_existing() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("existing.txt");
        fs::write(&file_path, "content").unwrap();

        let normalized = normalize_path(file_path.to_str().unwrap()).unwrap();
        assert_eq!(normalized, file_path.canonicalize().unwrap());
    }

    #[test]
    fn test_normalize_path_new_file() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("new_file.txt");

        let normalized = normalize_path(file_path.to_str().unwrap()).unwrap();
        assert_eq!(normalized, dir.path().canonicalize().unwrap().join("new_file.txt"));
    }

    #[test]
    fn test_normalize_path_missing_parent() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("missing_dir").join("new_file.txt");

        // normalize_path resolves by walking up to the first existing ancestor,
        // canonicalizing that, then appending the remaining components. This
        // allows paths with non-existent parent directories since sniper creates
        // files/directories during atomic write operations.
        let normalized = normalize_path(file_path.to_str().unwrap()).unwrap();
        let expected = dir.path().canonicalize().unwrap().join("missing_dir").join("new_file.txt");
        assert_eq!(normalized, expected);
    }

    #[test]
    fn test_normalize_path_invalid_filename() {
        let dir = TempDir::new().unwrap();
        let invalid_path = dir.path().join("missing_dir").join("..");
        let result = normalize_path(invalid_path.to_str().unwrap());
        assert!(result.is_err());
    }
}

    #[test]
    fn test_check_file_size_exceeds_limit() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("large.txt");
        fs::write(&file, "x".repeat(100)).unwrap();

        let result = check_file_size(file.to_str().unwrap(), 10);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("File too large"));
    }

    #[test]
    fn test_check_file_size_unlimited() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("any.txt");
        fs::write(&file, "any content").unwrap();

        // max_size = 0 means unlimited
        let result = check_file_size(file.to_str().unwrap(), 0);
        assert!(result.is_ok());
    }

    #[test]
    fn test_purge_old_backups_by_count() {
        use std::thread;

        let file = PathBuf::from("test_purge_backup.txt");
        fs::write(&file, "v0").unwrap();

        let config = SniperConfig {
            backup_retention_count: 3,
            backup_max_age_days: 0,
            ..SniperConfig::default()
        };

        // Create 5 backups
        for i in 1..=5 {
            fs::write(&file, format!("v{}", i)).unwrap();
            create_backup(file.to_str().unwrap())
                .expect("Backup creation must succeed");
            thread::sleep(Duration::from_millis(10));
        }

        let normalized = normalize_path(file.to_str().unwrap())
            .expect("Path normalization must succeed");
        let hash = get_path_hash(&normalized);
        let backup_dir = PathBuf::from(BACKUP_DIR);
        assert!(backup_dir.exists(), "Backup dir must exist after creating backups");

        let before_count: usize = fs::read_dir(&backup_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(&hash))
            .count();
        assert!(
            before_count >= 5,
            "Must have created at least 5 backups, got {}",
            before_count
        );

        purge_old_backups(file.to_str().unwrap(), &config)
            .expect("Purge must succeed");

        let after_count: usize = fs::read_dir(&backup_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(&hash))
            .count();
        assert!(
            after_count <= 3,
            "After purge with retention=3, expected <= 3 backups, got {}",
            after_count
        );

        let _ = fs::remove_file(&file);
    }

    #[test]
    fn test_normalize_path_with_security() {
        // Use current directory for security test
        let dir = std::env::current_dir().unwrap();
        let file = dir.join("test_normalize_path.txt");
        let _ = fs::write(&file, "test");

        // Valid path should work (relative to current dir)
        let result = normalize_path("test_normalize_path.txt");
        if file.exists() {
            let _ = fs::remove_file(&file);
        }
        assert!(result.is_ok());

        // Path traversal should fail
        let result = normalize_path("../../../etc/passwd");
        assert!(result.is_err());
    }
}
