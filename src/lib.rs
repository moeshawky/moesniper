pub mod config;
pub mod security;

pub use config::SniperConfig;
pub use security::{validate_path, SecurityPolicy, PathSecurityError, normalize_path_secure};

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime};

use llmosafe::ResourceGuard;

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
    validate_path(path, &policy)
        .map_err(|e| e.to_string())
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
        Some(Duration::from_secs(config.backup_max_age_days * 24 * 60 * 60))
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

    let mut backups: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("read backup dir: {e}"))?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(&hash))
        .map(|e| e.path())
        .collect();

    backups.sort();
    Ok(backups.pop())
}

pub fn write_atomic(filepath: &str, lines: &[&str]) -> Result<(), String> {
    let has_trailing_newline = check_trailing_newline(filepath)?;
    write_atomic_impl(filepath, lines, has_trailing_newline)
}

pub fn write_atomic_owned(filepath: &str, lines: &[String]) -> Result<(), String> {
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

/// Unified atomic write with metabolic pacing via llmosafe 0.5.0.
fn write_atomic_impl<S: AsRef<str>>(
    filepath: &str,
    lines: &[S],
    has_trailing_newline: bool,
) -> Result<(), String> {
    let tmp = format!("{filepath}.sniper_tmp");
    let mut f = fs::File::create(&tmp).map_err(|e| format!("create tmp: {e}"))?;

    if let Ok(metadata) = fs::metadata(filepath) {
        let perms = metadata.permissions();
        let _ = fs::set_permissions(&tmp, perms);
    }

    for (i, line) in lines.iter().enumerate() {
        let s = line.as_ref();
        f.write_all(s.as_bytes())
            .map_err(|e| format!("write: {e}"))?;

        if !s.ends_with('\n') && (i < lines.len() - 1 || has_trailing_newline) {
            f.write_all(b"\n")
                .map_err(|e| format!("write newline: {e}"))?;
        }
    }
    drop(f);

    // Metabolic Pacing: entropy-weighted sleep (256MB memory ceiling).
    let guard = ResourceGuard::new(256 * 1024 * 1024);
    let entropy = guard.raw_entropy();
    if entropy > 500 {
        thread::sleep(Duration::from_millis((entropy / 2) as u64));
    }

    match fs::rename(&tmp, filepath) {
        Ok(_) => Ok(()),
        Err(e) => Err(handle_backtrack_error(e, "Atomic write")),
    }
}

/// Centralized handling for llmosafe Backtrack Signal (-7).
pub fn handle_backtrack_error(e: std::io::Error, context: &str) -> String {
    if e.raw_os_error() == Some(-7) {
        format!("CRITICAL: {context} aborted via llmosafe 0.5.0 Backtrack Signal (-7). Immune memory triggered: current state matches a previously rolled-back failure pattern.")
    } else {
        format!("{context}: {e}")
    }
}

/// File-based lock with configurable timeout.
pub struct SniperLock {
    lock_path: PathBuf,
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
                Ok(_) => return Ok(Self { lock_path }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if start.elapsed().unwrap_or(Duration::ZERO) > timeout {
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

    #[test]
    fn test_check_file_size_within_limit() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("small.txt");
        fs::write(&file, "small content").unwrap();

        let result = check_file_size(file.to_str().unwrap(), 100);
        assert!(result.is_ok());
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

        // Create test file in current directory for backup test
        let file = PathBuf::from("test_purge_backup.txt");
        let _ = fs::write(&file, "test");

        // Create config with retention of 3
        let mut config = SniperConfig::default();
        config.backup_retention_count = 3;
        config.backup_max_age_days = 0; // Disable age-based purge

        // Create multiple backups
        for _ in 0..5 {
            let result = create_backup(file.to_str().unwrap());
            if result.is_err() {
                break; // If we can't create backups, skip test
            }
            thread::sleep(Duration::from_millis(10));
        }

        // Count backups before purge
        let normalized = normalize_path(file.to_str().unwrap());
        if normalized.is_err() {
            let _ = fs::remove_file(&file);
            return; // Skip test if path normalization fails
        }
        let normalized = normalized.unwrap();
        let hash = get_path_hash(&normalized);
        let backup_dir = PathBuf::from(BACKUP_DIR);
        
        if backup_dir.exists() {
            let before_count: usize = fs::read_dir(&backup_dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().starts_with(&hash))
                .count();
            
            if before_count >= 5 {
                // Purge
                let _ = purge_old_backups(file.to_str().unwrap(), &config);

                // Count backups after purge
                let after_count: usize = fs::read_dir(&backup_dir)
                    .unwrap()
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_name().to_string_lossy().starts_with(&hash))
                    .count();
                
                assert_eq!(after_count, 3);
            }
        }

        // Cleanup
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
