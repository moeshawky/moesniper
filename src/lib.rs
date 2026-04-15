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

pub fn normalize_path(path: &str) -> Result<PathBuf, String> {
    let p = Path::new(path);
    if p.exists() {
        p.canonicalize()
            .map_err(|e| format!("canonicalize {path}: {e}"))
    } else {
        // Fallback for new files: canonicalize parent and join name
        let parent = p.parent().unwrap_or_else(|| Path::new("."));
        let abs_parent = parent
            .canonicalize()
            .map_err(|e| format!("canonicalize parent of {path}: {e}"))?;
        let name = p
            .file_name()
            .ok_or_else(|| format!("invalid filename: {path}"))?;
        Ok(abs_parent.join(name))
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
        .map(|d| d.as_nanos()) // Nano-precision to prevent collision in fast scripts
        .map_err(|e| format!("timestamp: {e}"))?;

    let backup_name = format!("{hash}.{name}.{ts}");
    let dst = dir.join(&backup_name);

    if normalized.exists() {
        fs::copy(&normalized, &dst).map_err(|e| format!("backup copy: {e}"))?;
    } else {
        // For new files, create an empty backup to mark the "nothing" state
        fs::File::create(&dst).map_err(|e| format!("create empty backup: {e}"))?;
    }

    Ok(dst.to_string_lossy().into())
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

    // Sort by name (which ends in timestamp)
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
/// Ensures consistent line endings and preserves trailing newline state.
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

        // Add newline if the provided line does not already end with one,
        // and it is either not the last line OR the original file had a trailing newline.
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

/// Simple file-based lock in the .sniper directory.
/// Locks are file-specific to prevent concurrent edits/undos to the same file.
pub struct SniperLock {
    lock_path: PathBuf,
}

impl SniperLock {
    pub fn acquire(filepath: &str) -> Result<Self, String> {
        let normalized = normalize_path(filepath)?;
        let hash = get_path_hash(&normalized);
        let dir = PathBuf::from(BACKUP_DIR);
        fs::create_dir_all(&dir).map_err(|e| format!("create .sniper: {e}"))?;
        let lock_path = dir.join(format!("sniper.{}.lock", hash));

        // Simple spin-lock with 2s timeout
        let start = SystemTime::now();
        loop {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(_) => return Ok(Self { lock_path }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if start.elapsed().unwrap_or(Duration::ZERO).as_secs() > 2 {
                        return Err(format!(
                            "timeout: another sniper process is editing {} (lock held)",
                            filepath
                        ));
                    }
                    thread::sleep(Duration::from_millis(50));
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
