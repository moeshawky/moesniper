#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

/// Library version string (matches Cargo.toml).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Library name (matches Cargo.toml).
pub const NAME: &str = env!("CARGO_PKG_NAME");

/// Configuration types for sniper.
pub mod config;
/// Diff generation for dry-run previews.
pub mod diff;
/// Indentation validation and auto-fix utilities.
pub mod indent;
/// Path security validation and sanitization.
pub mod security;

/// Diff-based dry-run preview.
pub use diff::generate_preview;
/// Indentation validation and auto-fix.
pub use indent::{auto_indent_content, needs_indent_fix, validate_indentation};

pub use config::DalLevel;
/// Main configuration struct for sniper behavior.
pub use config::SniperConfig;
/// Re-exports for path security validation.
pub use security::{normalize_path_secure, validate_path, PathSecurityError, SecurityPolicy};

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime};

use llmosafe::ResourceGuard;
use sha2::Digest;

/// Directory name for storing backups and lock files.
///
/// Internal constant: used by backup, lock, and purge utilities.
/// Not intended for direct external use — access via `create_backup`,
/// `find_latest_backup`, and `purge_old_backups`.
pub const BACKUP_DIR: &str = ".sniper";

/// A single operation within a batch manifest.
///
/// Used by both the CLI binary and Python bindings to deserialize
/// manifest JSON. Operations are applied bottom-up (by start line,
/// descending) so that line numbers in earlier operations remain valid.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ManifestOp {
    /// 1-based start line (inclusive).
    pub start: usize,
    /// 1-based end line (exclusive). Defaults to start if absent.
    #[serde(default)]
    pub end: Option<usize>,
    /// Hex-encoded content to insert at this position.
    #[serde(default)]
    pub hex: Option<String>,
    /// If true, delete the range instead of inserting content.
    #[serde(default)]
    pub delete: Option<bool>,
}

/// Snapshot of memory statistics from a ResourceGuard at a point in time.
///
/// Captures available bytes, used bytes, and pressure percentage.
/// Used by `RiskTelemetry` to compose a complete resource picture.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryStats {
    /// Total system memory in bytes.
    pub available_bytes: u64,
    /// Memory currently used by the process in bytes.
    pub used_bytes: u64,
    /// Resource pressure as a percentage (0-100).
    pub pressure: u8,
}

/// Composite risk telemetry computed from a live ResourceGuard.
///
/// Combines entropy bits, classifier score, and memory stats into a single
/// risk assessment. Constructed via `from_guard()` which reads live system state.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RiskTelemetry {
    /// Raw entropy score (0-1000) from the ResourceGuard.
    pub combined_risk_bits: u64,
    /// Classifier score derived from entropy mapping (0.0-1.0).
    pub classifier_score: f64,
    /// Memory statistics snapshot at construction time.
    pub memory_stats: MemoryStats,
}

impl RiskTelemetry {
    /// Constructs a RiskTelemetry from a live ResourceGuard.
    ///
    /// Reads the guard's entropy, pressure, and memory metrics to produce
    /// a single risk assessment. The classifier_score is a sigmoid-mapped
    /// value derived from the raw entropy.
    ///
    /// # Arguments
    /// * `guard` - A live ResourceGuard instance with current system metrics.
    ///
    /// # Returns
    /// A RiskTelemetry with all fields populated from the guard's state.
    pub fn from_guard(guard: &ResourceGuard) -> Self {
        let entropy = guard.raw_entropy();
        let pressure = guard.pressure();
        let system_mem = ResourceGuard::system_memory_bytes() as u64;
        // memory_ceiling_bytes is a private field of ResourceGuard.
        // Derive used_bytes from system memory and pressure directly.
        // used_bytes = system_mem * pressure / 100
        let estimated_ceiling = system_mem / 2;
        let used_bytes = estimated_ceiling * u64::from(pressure) / 100;
        let classifier_score = sigmoid_f64(entropy as f64 / 1000.0);
        Self {
            combined_risk_bits: u64::from(entropy),
            classifier_score,
            memory_stats: MemoryStats {
                available_bytes: system_mem,
                used_bytes,
                pressure,
            },
        }
    }
}
/// Recommends an action based on the current resource risk level.
///
/// Maps classifier_score thresholds to human-readable recommendations:
/// - score >= 0.7: "Resource pressure high. Consider pausing."
/// - score >= 0.4: "Moderate load. Proceed with caution."
/// - score < 0.4: "Resources nominal."
pub fn recommend_from_risk(risk: &RiskTelemetry) -> String {
    if risk.classifier_score >= 0.7 {
        "Resource pressure high. Consider pausing.".into()
    } else if risk.classifier_score >= 0.4 {
        "Moderate load. Proceed with caution.".into()
    } else {
        "Resources nominal.".into()
    }
}

/// Compute sigmoid(x) = 1/(1+e^(-x)) using f64 precision.
///
/// Maps input range [0.0, 1.0] (entropy ratio) to approximately [0.5, 0.73].
/// Used by RiskTelemetry::from_guard to convert raw entropy to a classifier score.
fn sigmoid_f64(x: f64) -> f64 {
    1.0 / (1.0 + (-x).exp())
}

/// PID-inspired pacing configuration for metabolic sleep timing.
///
/// Controls how long the process sleeps between resource check and file rename.
/// The formula is: `base_ms + (entropy * entropy_scale) + (pressure * pressure_scale)`.
///
/// Defaults produce zero sleep when entropy is low, scaling up with system load.
///
/// **Internal use only** — constructed by [`write_atomic_impl`] from [`SniperConfig`].
/// Not part of the public API surface; exposed as `pub(crate)` for test access.
#[derive(Debug, Clone)]
pub(crate) struct PidConfig {
    /// Base sleep duration in milliseconds (always applied).
    pub base_ms: u64,
    /// Multiplier for entropy score (0-1000) to produce milliseconds.
    pub entropy_scale: f64,
    /// Multiplier for pressure percentage (0-100) to produce milliseconds.
    pub pressure_scale: f64,
}

impl Default for PidConfig {
    fn default() -> Self {
        Self {
            base_ms: 0,
            entropy_scale: 0.5,
            pressure_scale: 1.0,
        }
    }
}

impl PidConfig {
    /// Computes the sleep duration from current entropy and pressure.
    ///
    /// # Arguments
    /// * `entropy` - Raw entropy score (0-1000) from ResourceGuard.
    /// * `pressure` - Resource pressure percentage (0-100) from ResourceGuard.
    ///
    /// # Returns
    /// A `Duration` representing how long to sleep before proceeding.
    // u64/f64 casts are explicit and bounded; truncation is controlled by input range
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn sleep_duration(&self, entropy: u64, pressure: u8) -> Duration {
        let ms = self.base_ms
            + ((entropy as f64) * self.entropy_scale) as u64
            + ((pressure as f64) * self.pressure_scale) as u64;
        Duration::from_millis(ms)
    }
}

/// Hex encode a byte slice.
///
/// Uses a pre-allocated buffer with direct byte-to-hex-nibble mapping
/// for optimal performance (avoids per-character `format!` allocations).
pub fn hex_encode(data: &[u8]) -> String {
    let mut buf = Vec::with_capacity(data.len() * 2);
    for &b in data {
        buf.push(HEX_CHARS[(b >> 4) as usize]);
        buf.push(HEX_CHARS[(b & 0x0F) as usize]);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";

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

/// Returns a hex-encoded hash of a file path for backup naming.
pub fn get_path_hash(path: &Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

/// Creates a timestamped backup of a file in the backup directory.
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

    let mut to_delete = HashSet::new();

    // Age-based purge
    if let Some(max_age_duration) = max_age {
        for (path, modified) in &backups {
            if now.duration_since(*modified).unwrap_or(Duration::ZERO) > max_age_duration {
                to_delete.insert(path);
            }
        }
    }

    // Count-based purge (keep most recent N)
    if config.backup_retention_count > 0 && backups.len() > config.backup_retention_count {
        let to_remove = backups.len() - config.backup_retention_count;
        for (path, _) in backups.iter().take(to_remove) {
            to_delete.insert(path);
        }
    }

    // Delete marked backups — always log activity (audit_enabled is additive, not gating)
    for path in &to_delete {
        match fs::remove_file(path) {
            Ok(()) => eprintln!("[SNIPER] Purged old backup: {:?}", path),
            Err(e) => eprintln!("[SNIPER] Failed to purge backup {:?}: {e}", path),
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

/// Writes content to a file atomically using a temporary file and rename.
///
/// Creates an internal ResourceGuard for metabolic pacing. For callers that
/// already have a guard, use `write_atomic_with_dal`.
///
/// # Arguments
/// * `filepath` - Target file path.
/// * `lines` - Content lines to write.
///
/// # Returns
/// `Ok(())` on success. `Err(message)` if resource check fails or write fails.
pub fn write_atomic(filepath: &str, lines: &[&str]) -> Result<(), String> {
    let guard = ResourceGuard::auto(0.5);
    let has_trailing_newline = check_trailing_newline(filepath)?;
    write_atomic_impl(filepath, lines, has_trailing_newline, &guard)
}

/// Atomic write gated by a pre-created ResourceGuard with DAL-level enforcement.
///
/// Performs resource safety check BEFORE any file I/O begins, then delegates
/// the actual atomic write to `write_atomic_impl`. At `DalLevel::Maximum`,
/// an extra resource check is performed after the initial gate to confirm
/// resources remain safe after the first validation.
///
/// # Arguments
/// * `filepath` - Target file path.
/// * `lines` - Content lines to write.
/// * `guard` - Pre-created ResourceGuard for resource safety checks.
/// * `dal_level` - Current Defense-Ascension Level from SniperConfig.
///
/// # Returns
/// `Ok(())` on success. `Err(message)` if any resource check fails or write fails.
pub fn write_atomic_with_dal(
    filepath: &str,
    lines: &[&str],
    guard: &ResourceGuard,
    dal_level: DalLevel,
) -> Result<(), String> {
    // Gate: initial resource check before any I/O (T4/T9)
    guard.check().map_err(|e| format!("resource safety: {e}"))?;

    // DAL Maximum: extra resource check before proceeding
    if dal_level == DalLevel::Maximum {
        guard
            .check()
            .map_err(|e| format!("resource safety (DAL maximum): {e}"))?;
    }

    let has_trailing_newline = check_trailing_newline(filepath)?;
    write_atomic_impl(filepath, lines, has_trailing_newline, guard)
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

/// Unified atomic write with metabolic pacing via llmosafe 0.7.5.
///
/// Trailing newlines are stripped from each line, then:
/// - All lines except the last get a newline appended
/// - The last line gets a newline ONLY if the original file had one
///
/// This ensures deterministic behavior regardless of input format.
///
/// # Arguments
/// * `filepath` - Target file path for atomic write.
/// * `lines` - Content lines to write (trailing newlines stripped uniformly).
/// * `has_trailing_newline` - Whether the original file ended with a newline.
/// * `guard` - Pre-created ResourceGuard for metabolic pacing metrics.
///
/// # Returns
/// `Ok(())` on successful atomic rename. `Err(message)` on any I/O or resource failure.
///
/// # Errors
/// Returns error if temp file creation, write, flush, resource check, or rename fails.
fn write_atomic_impl<S: AsRef<str>>(
    filepath: &str,
    lines: &[S],
    has_trailing_newline: bool,
    guard: &ResourceGuard,
) -> Result<(), String> {
    let ts = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = format!("{filepath}.sniper_tmp.{ts}");

    struct CleanupGuard<'a> {
        path: &'a str,
        active: bool,
    }
    impl<'a> CleanupGuard<'a> {
        fn new(path: &'a str) -> Self {
            Self { path, active: true }
        }
        fn disarm(&mut self) {
            self.active = false;
        }
    }
    impl<'a> Drop for CleanupGuard<'a> {
        fn drop(&mut self) {
            if self.active {
                let _ = fs::remove_file(self.path);
            }
        }
    }
    let mut cleanup_guard = CleanupGuard::new(&tmp);

    let f = fs::File::create(&tmp).map_err(|e| format!("create tmp: {e}"))?;

    if let Ok(metadata) = fs::metadata(filepath) {
        let perms = metadata.permissions();
        if let Err(e) = fs::set_permissions(&tmp, perms) {
            eprintln!(
                "[SNIPER] Warning: failed to preserve file permissions for {:?}: {e}",
                filepath
            );
        }
    }

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
    // Metabolic Pacing: PID-configured sleep using the caller's ResourceGuard metrics.
    // The guard is created by the caller (write_atomic or write_atomic_with_dal)
    // and shared here to avoid dual-guard divergence.
    let entropy = guard.raw_entropy();
    let pressure = guard.pressure();
    let config = SniperConfig::from_env();
    let pid = PidConfig {
        base_ms: config.pid_base_ms,
        entropy_scale: config.pid_entropy_scale,
        pressure_scale: config.pid_pressure_scale,
    };
    let sleep = pid.sleep_duration(u64::from(entropy), pressure);
    if !sleep.is_zero() {
        thread::sleep(sleep);
    }

    match fs::rename(&tmp, filepath) {
        Ok(_) => {
            cleanup_guard.disarm();
            Ok(())
        }
        Err(e) => Err(handle_backtrack_error(e, "Atomic write")),
    }
}

/// Verify pre-edit context: hash 3 lines before start and 3 lines after end,
/// compare against the expected hash. Returns Ok if match, Err with message if not.
/// Compute context hash: hashes 3 lines before start and 3 lines after end.
/// Returns the full SHA-256 hash as a hex string.
pub fn compute_context_hash(lines: &[String], start: usize, end: usize) -> String {
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
    hex_encode(&hash)
}

pub fn verify_context(
    lines: &[String],
    start: usize,
    end: usize,
    expected_hash: &str,
) -> Result<(), String> {
    let actual_hex = compute_context_hash(lines, start, end);
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
/// `DeadlineExceeded` (-7) remains a valid `KernelError` variant in llmosafe 0.7.5+,
/// used in `check_blocking()`, `check_with_deadline()`, and the C-ABI decision codes.
/// However, resource exhaustion now surfaces via `KernelError` from
/// `ResourceGuard::check()` rather than OS-level signals on IO operations.
///
/// This function is a defensive fallback: if the OS ever returns raw error code -7
/// on an IO operation (e.g., via a legacy signal path or platform-specific
/// errno mapping), the distinctive backtrack message is emitted instead of
/// a generic IO error.
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

/// Check if a process with the given PID is alive.
///
/// Non-Unix fallback: always returns true because there is no portable
/// way to check process liveness. Stale lock detection relies on timeout
/// expiry only on non-Unix platforms.
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
                    if let Err(e) = write!(f, "{}", pid) {
                        return Err(format!("lock PID write for {filepath}: {e}"));
                    }
                    if let Err(e) = f.flush() {
                        return Err(format!("lock PID flush for {filepath}: {e}"));
                    }
                    return Ok(Self { lock_path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    // Safety: if clock moved backwards (elapsed() returns Err),
                    // treat as immediate timeout to avoid infinite hang.
                    if start.elapsed().unwrap_or(timeout + Duration::from_secs(1)) > timeout {
                        let holder_pid = fs::read_to_string(&lock_path)
                            .ok()
                            .and_then(|c| c.trim().parse::<u32>().ok());
                        if let Some(pid) = holder_pid {
                            if !is_process_alive(pid) {
                                let _ = fs::remove_file(&lock_path);
                                continue;
                            }
                        }
                        return Err(format!(
                            "timeout: another sniper process (PID {}) is editing {} \
                             (lock held for >{:?}; lock file: {:?})",
                            holder_pid.map_or("unknown".to_string(), |p| p.to_string()),
                            filepath,
                            timeout,
                            lock_path
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
    use std::fs;
    use std::io;
    use tempfile::TempDir;

    #[test]
    fn test_compute_context_hash() {
        let lines: Vec<String> = vec![
            "1".into(),
            "2".into(),
            "3".into(),
            "4".into(),
            "5".into(),
            "6".into(),
            "7".into(),
            "8".into(),
            "9".into(),
            "10".into(),
        ];

        let start = 5;
        let end = 6;

        // expected to hash lines 2, 3, 4 before and 7, 8, 9 after
        let mut hasher = sha2::Sha256::new();
        hasher.update(b"2");
        hasher.update(b"3");
        hasher.update(b"4");
        hasher.update(b"7");
        hasher.update(b"8");
        hasher.update(b"9");
        let expected_hash = hex_encode(&hasher.finalize());

        let hash = compute_context_hash(&lines, start, end);
        assert_eq!(hash, expected_hash);
    }

    #[test]
    fn test_resource_guard_for_testing_controlled_entropy() {
        let big_ceiling = usize::MAX / 2;
        let guard = ResourceGuard::for_testing(big_ceiling, 800, 30);
        assert_eq!(guard.raw_entropy(), 800);
        assert_eq!(guard.pressure(), 30);
        assert!(guard.check().is_ok());
    }

    #[test]
    fn test_resource_guard_for_testing_low_values() {
        let big_ceiling = usize::MAX / 2;
        let guard = ResourceGuard::for_testing(big_ceiling, 200, 10);
        assert_eq!(guard.raw_entropy(), 200);
        assert_eq!(guard.pressure(), 10);
        assert!(guard.check().is_ok());
    }

    #[test]
    fn test_handle_backtrack_error_signal_7() {
        let err = io::Error::from_raw_os_error(-7);
        let result = handle_backtrack_error(err, "TestContext");
        assert_eq!(
            result,
            "CRITICAL: TestContext aborted via llmosafe Backtrack Signal (-7). Immune memory triggered: current state matches a previously rolled-back failure pattern."
        );
    }

    #[test]
    fn test_handle_backtrack_error_other() {
        let err = io::Error::new(io::ErrorKind::NotFound, "file not found");
        let result = handle_backtrack_error(err, "TestContext");
        assert_eq!(result, "TestContext: file not found");
    }

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
        assert_eq!(
            normalized,
            dir.path().canonicalize().unwrap().join("new_file.txt")
        );
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
        let expected = dir
            .path()
            .canonicalize()
            .unwrap()
            .join("missing_dir")
            .join("new_file.txt");
        assert_eq!(normalized, expected);
    }

    #[test]
    fn test_normalize_path_invalid_filename() {
        let dir = TempDir::new().unwrap();
        let invalid_path = dir.path().join("missing_dir").join("..");
        let result = normalize_path(invalid_path.to_str().unwrap());
        assert!(result.is_err());
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
            create_backup(file.to_str().unwrap()).expect("Backup creation must succeed");
            thread::sleep(Duration::from_millis(10));
        }

        let normalized =
            normalize_path(file.to_str().unwrap()).expect("Path normalization must succeed");
        let hash = get_path_hash(&normalized);
        let backup_dir = PathBuf::from(BACKUP_DIR);
        assert!(
            backup_dir.exists(),
            "Backup dir must exist after creating backups"
        );

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

        purge_old_backups(file.to_str().unwrap(), &config).expect("Purge must succeed");

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
    fn test_hex_encode() {
        assert_eq!(hex_encode(b""), "");
        assert_eq!(hex_encode(b"hello"), "68656c6c6f");
        assert_eq!(hex_encode(b"\x00\xff"), "00ff");
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

    // ---------------------------------------------------------------------------
    // PidConfig::sleep_duration() tests
    // ---------------------------------------------------------------------------

    #[test]
    fn test_pid_config_sleep_duration_zero_defaults() {
        let pid = PidConfig::default();
        // Default: base_ms=0, entropy_scale=0.5, pressure_scale=1.0
        assert_eq!(pid.sleep_duration(0, 0), Duration::from_millis(0));
        // 1000 * 0.5 = 500
        assert_eq!(pid.sleep_duration(1000, 0), Duration::from_millis(500));
        // 100 * 1.0 = 100
        assert_eq!(pid.sleep_duration(0, 100), Duration::from_millis(100));
        // 500 + 50 = 550 (from 1000*0.5 + 50*1.0 = 500 + 50)
        assert_eq!(pid.sleep_duration(1000, 50), Duration::from_millis(550));
    }

    #[test]
    fn test_pid_config_sleep_duration_with_base_ms() {
        let pid = PidConfig {
            base_ms: 10,
            entropy_scale: 0.5,
            pressure_scale: 1.0,
        };
        // base_ms only, no entropy/pressure contribution
        assert_eq!(pid.sleep_duration(0, 0), Duration::from_millis(10));
        // base_ms + entropy contribution
        assert_eq!(pid.sleep_duration(1000, 0), Duration::from_millis(510)); // 10 + 500
        // base_ms + pressure contribution
        assert_eq!(pid.sleep_duration(0, 100), Duration::from_millis(110)); // 10 + 100
    }

    #[test]
    fn test_pid_config_sleep_duration_large_values() {
        let pid = PidConfig {
            base_ms: 5,
            entropy_scale: 1.0,
            pressure_scale: 2.0,
        };
        // 5 + (1000 * 1.0) + (100 * 2.0) = 5 + 1000 + 200 = 1205
        assert_eq!(pid.sleep_duration(1000, 100), Duration::from_millis(1205));
    }

    // ---------------------------------------------------------------------------
    // sigmoid_f64 tests
    // ---------------------------------------------------------------------------

    /// Test sigmoid_f64 with known mathematical values.
    /// sigmoid(x) = 1/(1+e^(-x))
    #[test]
    fn test_sigmoid_f64() {
        // sigmoid(0.0) = 0.5 exactly
        assert!((sigmoid_f64(0.0) - 0.5).abs() < f64::EPSILON);
        // sigmoid(1.0) ≈ 0.731
        assert!((sigmoid_f64(1.0) - 0.731).abs() < 0.01);
        // sigmoid(-1.0) ≈ 0.269
        assert!((sigmoid_f64(-1.0) - 0.269).abs() < 0.01);
        // sigmoid(10.0) ≈ 0.99995 (very close to 1.0)
        assert!((sigmoid_f64(10.0) - 1.0).abs() < 0.0001);
    }

    // ---------------------------------------------------------------------------
    // recommend_from_risk tests
    // ---------------------------------------------------------------------------

    /// Build a RiskTelemetry with a specific classifier_score for testing.
    fn make_risk_telemetry(score: f64) -> RiskTelemetry {
        RiskTelemetry {
            combined_risk_bits: 0,
            classifier_score: score,
            memory_stats: MemoryStats {
                available_bytes: 1024,
                used_bytes: 512,
                pressure: 0,
            },
        }
    }

    /// Test recommend_from_risk across all score thresholds.
    #[test]
    fn test_recommend_from_risk() {
        // score >= 0.7 → high pressure
        assert_eq!(
            recommend_from_risk(&make_risk_telemetry(0.9)),
            "Resource pressure high. Consider pausing."
        );
        // score >= 0.4 → moderate
        assert_eq!(
            recommend_from_risk(&make_risk_telemetry(0.5)),
            "Moderate load. Proceed with caution."
        );
        // score < 0.4 → nominal
        assert_eq!(
            recommend_from_risk(&make_risk_telemetry(0.0)),
            "Resources nominal."
        );
        // boundary: score 0.7 exactly → high pressure
        assert_eq!(
            recommend_from_risk(&make_risk_telemetry(0.7)),
            "Resource pressure high. Consider pausing."
        );
        // boundary: score 0.4 exactly → moderate
        assert_eq!(
            recommend_from_risk(&make_risk_telemetry(0.4)),
            "Moderate load. Proceed with caution."
        );
    }

    // ---------------------------------------------------------------------------
    // RiskTelemetry::from_guard tests
    // ---------------------------------------------------------------------------

    /// Test RiskTelemetry::from_guard with known entropy and pressure.
    #[test]
    fn test_risk_telemetry_from_guard() {
        use std::usize;
        let guard = ResourceGuard::for_testing(usize::MAX / 2, 500, 50);
        let telemetry = RiskTelemetry::from_guard(&guard);

        // entropy 500/1000 → sigmoid(0.5) ≈ 0.622
        assert!(
            telemetry.classifier_score > 0.5,
            "classifier_score ({}) should be > 0.5 for entropy 500",
            telemetry.classifier_score
        );
        assert!(
            telemetry.classifier_score < 0.8,
            "classifier_score ({}) should be < 0.8 for entropy 500",
            telemetry.classifier_score
        );
        // combined_risk_bits should match the raw entropy
        assert_eq!(
            telemetry.combined_risk_bits, 500,
            "combined_risk_bits should be 500, got {}",
            telemetry.combined_risk_bits
        );
        // pressure should be passed through
        assert_eq!(
            telemetry.memory_stats.pressure, 50,
            "pressure should be 50, got {}",
            telemetry.memory_stats.pressure
        );
        // used_bytes should be > 0 since pressure > 0
        assert!(
            telemetry.memory_stats.used_bytes > 0,
            "used_bytes should be > 0, got {}",
            telemetry.memory_stats.used_bytes
        );
        // available_bytes should be the system memory, which is always > 0
        assert!(
            telemetry.memory_stats.available_bytes > 0,
            "available_bytes should be > 0, got {}",
            telemetry.memory_stats.available_bytes
        );
    }

    // ---------------------------------------------------------------------------
    // verify_context tests
    // ---------------------------------------------------------------------------

    /// Test verify_context with a correct expected hash (match).
    #[test]
    fn test_verify_context_match() {
        let lines: Vec<String> = vec![
            "a".into(), "b".into(), "c".into(),
            "X".into(), "Y".into(), "Z".into(),
            "d".into(), "e".into(), "f".into(),
        ];
        // Edit at lines 4-6 (0-indexed: start=3, end=6)
        // compute_context_hash uses start and end as 1-indexed line numbers:
        //   before: start-1..start-1-3 = 3..0? No, let's look at the code.
        // compute_context_hash: before_start = start-1-3, before_end = start-1
        //   after_start = end, after_end = end+3
        // For start=3 (0-indexed line 3 = 4th line "X", but wait...)
        //
        // Actually compute_context_hash takes start and end as LINE NUMBERS (1-indexed).
        // Documentation says: "hashes 3 lines before start and 3 lines after end"
        // So start=4 (1-indexed), end=6 (1-indexed)
        // → hashes lines before 4: lines 1,2,3 → "a","b","c"
        // → hashes lines after 6: lines 7,8,9 → "d","e","f"
        let expected_hash = compute_context_hash(&lines, 4, 6);

        // verify_context should match with the correct hash
        let result = verify_context(&lines, 4, 6, &expected_hash[..16]);
        assert!(result.is_ok(), "verify_context should return Ok for matching hash, got {:?}", result);
    }

    /// Test verify_context with a wrong expected hash (mismatch).
    #[test]
    fn test_verify_context_mismatch() {
        let lines: Vec<String> = vec![
            "a".into(), "b".into(), "c".into(),
            "X".into(), "Y".into(), "Z".into(),
            "d".into(), "e".into(), "f".into(),
        ];
        let wrong_hash = "0000000000000000";
        let result = verify_context(&lines, 4, 6, wrong_hash);
        assert!(result.is_err(), "verify_context should return Err for wrong hash");
        let err_msg = result.unwrap_err();
        assert!(
            err_msg.contains("context mismatch"),
            "error message should contain 'context mismatch', got: {}",
            err_msg
        );
    }

    // ---------------------------------------------------------------------------
    // find_latest_backup tests
    //
    // These tests manipulate CWD (current working directory) because the backup
    // functions use a CWD-relative `.sniper/` directory. A mutex ensures
    // serial execution to prevent interference with other CWD-dependent tests.
    // ---------------------------------------------------------------------------

    /// Guard that restores the original working directory on drop.
    struct CwdGuard {
        original: PathBuf,
    }
    impl CwdGuard {
        fn new(original: PathBuf) -> Self {
            Self { original }
        }
    }
    impl Drop for CwdGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.original);
        }
    }

    /// Mutex to serialize all tests that manipulate CWD.
    static CWD_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// find_latest_backup: no .sniper/ directory → Ok(None).
    #[test]
    fn test_find_latest_backup_no_dir() {
        let _serial = CWD_MUTEX.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let original_cwd = std::env::current_dir().unwrap();
        let _cwd_guard = CwdGuard::new(original_cwd);
        std::env::set_current_dir(dir.path()).unwrap();

        let result = find_latest_backup("any_file.txt");
        assert!(result.is_ok(), "find_latest_backup should return Ok when no .sniper/ dir");
        assert!(result.unwrap().is_none(), "should return None when .sniper/ doesn't exist");
    }

    /// find_latest_backup: .sniper/ exists but no matching backup → Ok(None).
    #[test]
    fn test_find_latest_backup_empty() {
        let _serial = CWD_MUTEX.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let original_cwd = std::env::current_dir().unwrap();
        let _cwd_guard = CwdGuard::new(original_cwd);
        std::env::set_current_dir(dir.path()).unwrap();

        // Create .sniper/ directory and a test file
        fs::create_dir_all(BACKUP_DIR).unwrap();
        let test_file = dir.path().join("test_backup.txt");
        fs::write(&test_file, "hello").unwrap();

        let result = find_latest_backup(test_file.to_str().unwrap());
        assert!(result.is_ok(), "find_latest_backup should return Ok when .sniper/ is empty");
        assert!(result.unwrap().is_none(), "should return None when no matching backups exist");
    }

    /// find_latest_backup: .sniper/ has a matching backup → Ok(Some(path)).
    #[test]
    fn test_find_latest_backup_finds_existing() {
        let _serial = CWD_MUTEX.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let original_cwd = std::env::current_dir().unwrap();
        let _cwd_guard = CwdGuard::new(original_cwd);
        std::env::set_current_dir(dir.path()).unwrap();

        // Create .sniper/ directory and a test file
        fs::create_dir_all(BACKUP_DIR).unwrap();
        let test_file = dir.path().join("test_backup_find.txt");
        fs::write(&test_file, "content").unwrap();

        // Create a backup matching this file
        create_backup(test_file.to_str().unwrap()).expect("create_backup should succeed");

        let result = find_latest_backup(test_file.to_str().unwrap());
        assert!(result.is_ok(), "find_latest_backup should return Ok");
        let found = result.unwrap();
        assert!(found.is_some(), "should return Some(path) when backup exists");
        let found_path = found.unwrap();
        // Path is relative to CWD (the temp dir), verify it exists now
        assert!(found_path.exists(), "returned backup path should exist: {:?}", found_path);
    }

    // ---------------------------------------------------------------------------
    // count_recent_backups tests
    // ---------------------------------------------------------------------------

    /// count_recent_backups: no .sniper/ directory → Ok(0).
    #[test]
    fn test_count_recent_backups_no_dir() {
        let _serial = CWD_MUTEX.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let original_cwd = std::env::current_dir().unwrap();
        let _cwd_guard = CwdGuard::new(original_cwd);
        std::env::set_current_dir(dir.path()).unwrap();

        let result = count_recent_backups("any_file.txt", 3600);
        assert!(result.is_ok(), "count_recent_backups should return Ok when no .sniper/ dir");
        assert_eq!(result.unwrap(), 0, "should return 0 when .sniper/ doesn't exist");
    }

    /// count_recent_backups: with a recent backup and varied window sizes.
    #[test]
    fn test_count_recent_backups_with_recent() {
        let _serial = CWD_MUTEX.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let original_cwd = std::env::current_dir().unwrap();
        let _cwd_guard = CwdGuard::new(original_cwd);
        std::env::set_current_dir(dir.path()).unwrap();

        // Create .sniper/ directory and a test file
        fs::create_dir_all(BACKUP_DIR).unwrap();
        let test_file = dir.path().join("test_count.txt");
        fs::write(&test_file, "content").unwrap();

        // Create a backup
        create_backup(test_file.to_str().unwrap()).expect("create_backup should succeed");

        // With a 1-hour window, the backup should be found
        let count_large = count_recent_backups(test_file.to_str().unwrap(), 3600);
        assert!(count_large.is_ok(), "count_recent_backups should succeed");
        assert!(
            count_large.unwrap() >= 1,
            "should find at least 1 backup within 1-hour window"
        );

        // With a 0-second window, the backup should NOT be found
        // (cutoff is now - 0 = now; backup's mtime <= now, but strictly > cutoff)
        let count_zero = count_recent_backups(test_file.to_str().unwrap(), 0);
        assert!(count_zero.is_ok(), "count_recent_backups should succeed");
        assert_eq!(
            count_zero.unwrap(), 0,
            "should return 0 with 0-second window (no backups strictly after now)"
        );
    }

    // ---------------------------------------------------------------------------
    // create_backup nonexistent source test
    // ---------------------------------------------------------------------------

    /// create_backup: source file does not exist → creates empty backup.
    #[test]
    fn test_create_backup_nonexistent_source() {
        let _serial = CWD_MUTEX.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let original_cwd = std::env::current_dir().unwrap();
        let _cwd_guard = CwdGuard::new(original_cwd);
        std::env::set_current_dir(dir.path()).unwrap();

        let nonexistent = dir.path().join("does_not_exist.txt");

        let result = create_backup(nonexistent.to_str().unwrap());
        assert!(result.is_ok(), "create_backup should return Ok for nonexistent source, got {:?}", result);
        let backup_path_str = result.unwrap();
        let backup_path = std::path::Path::new(&backup_path_str);
        assert!(backup_path.exists(), "backup file should exist at {}", backup_path_str);
        // The backup should be empty (length 0)
        let metadata = fs::metadata(backup_path).expect("should be able to read backup metadata");
        assert_eq!(metadata.len(), 0, "backup file should be empty (length 0), got {}", metadata.len());
    }

    // ---------------------------------------------------------------------------
    // purge_old_backups: no-policy early-return path
    // ---------------------------------------------------------------------------

    /// When both backup_retention_count and backup_max_age_days are 0,
    /// purge_old_backups returns Ok(()) immediately (line 307-309).
    #[test]
    fn test_purge_old_backups_no_policy() {
        let _serial = CWD_MUTEX.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let original_cwd = std::env::current_dir().unwrap();
        let _cwd_guard = CwdGuard::new(original_cwd);
        std::env::set_current_dir(dir.path()).unwrap();

        let file = PathBuf::from("test_no_policy.txt");
        fs::write(&file, "content").unwrap();

        let config = SniperConfig {
            backup_retention_count: 0,
            backup_max_age_days: 0,
            ..SniperConfig::default()
        };

        let result = purge_old_backups(file.to_str().unwrap(), &config);
        assert!(
            result.is_ok(),
            "purge_old_backups should return Ok when no policy configured, got {:?}",
            result
        );
    }

    // ---------------------------------------------------------------------------
    // purge_old_backups: .sniper/ directory does not exist → early return
    // ---------------------------------------------------------------------------

    /// When .sniper/ directory doesn't exist, purge_old_backups returns
    /// Ok(()) without error (line 316-317).
    #[test]
    fn test_purge_old_backups_dir_not_exist() {
        let _serial = CWD_MUTEX.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let original_cwd = std::env::current_dir().unwrap();
        let _cwd_guard = CwdGuard::new(original_cwd);
        std::env::set_current_dir(dir.path()).unwrap();

        let file = PathBuf::from("test_dir_not_exist.txt");
        fs::write(&file, "content").unwrap();

        let config = SniperConfig {
            backup_retention_count: 5,
            ..SniperConfig::default()
        };

        let result = purge_old_backups(file.to_str().unwrap(), &config);
        assert!(
            result.is_ok(),
            "purge_old_backups should return Ok when .sniper/ doesn't exist, got {:?}",
            result
        );
    }

    // ---------------------------------------------------------------------------
    // purge_old_backups: age-based path — nothing old enough to delete
    // ---------------------------------------------------------------------------

    /// With backup_max_age_days set to 36500 (100 years), no recent backup
    /// should be old enough to be deleted. Verifies the age-based purge path
    /// (lines 347-353) runs without crashing and preserves all backups.
    #[test]
    fn test_purge_old_backups_by_age() {
        let _serial = CWD_MUTEX.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let original_cwd = std::env::current_dir().unwrap();
        let _cwd_guard = CwdGuard::new(original_cwd);
        std::env::set_current_dir(dir.path()).unwrap();

        let file = PathBuf::from("test_age_purge.txt");
        fs::write(&file, "content").unwrap();

        // Create .sniper/ dir
        fs::create_dir_all(BACKUP_DIR).unwrap();

        // Create dummy backup files whose names match the file hash prefix
        let normalized = normalize_path(file.to_str().unwrap())
            .expect("normalize_path should succeed");
        let hash = get_path_hash(&normalized);
        for i in 1..=3 {
            let backup_name = format!("{}.test_age_purge.txt.{}", hash, i * 1000);
            let backup_path = PathBuf::from(BACKUP_DIR).join(&backup_name);
            fs::File::create(&backup_path)
                .unwrap_or_else(|e| panic!("create backup {}: {}", backup_name, e));
        }

        // 36500 days ≈ 100 years — nothing should be old enough
        let config = SniperConfig {
            backup_max_age_days: 36500,
            backup_retention_count: 0,
            ..SniperConfig::default()
        };

        let result = purge_old_backups(file.to_str().unwrap(), &config);

        // Count remaining backups
        let remaining: Vec<_> = fs::read_dir(BACKUP_DIR)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(&hash))
            .collect();

        assert!(
            result.is_ok(),
            "purge_old_backups should return Ok with long max age, got {:?}",
            result
        );
        assert_eq!(
            remaining.len(),
            3,
            "all 3 backups should remain when max age is 100 years, got {}",
            remaining.len()
        );
    }

    // ---------------------------------------------------------------------------
    // write_atomic_with_dal: Maximum level (double check) path
    // ---------------------------------------------------------------------------

    /// write_atomic_with_dal with DalLevel::Maximum exercises the extra
    /// resource check at lines 436-440. Uses a ResourceGuard with zero
    /// entropy/pressure so check() always passes.
    #[test]
    fn test_write_atomic_with_dal_maximum() {
        let _serial = CWD_MUTEX.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let original_cwd = std::env::current_dir().unwrap();
        let _cwd_guard = CwdGuard::new(original_cwd);
        std::env::set_current_dir(dir.path()).unwrap();

        let file = PathBuf::from("test_dal_max.txt");
        fs::write(&file, "original\n").unwrap();

        let guard = ResourceGuard::for_testing(usize::MAX / 2, 0, 0);
        let result = write_atomic_with_dal(
            file.to_str().unwrap(),
            &["replaced"],
            &guard,
            DalLevel::Maximum,
        );

        let content = fs::read_to_string(&file).unwrap();
        assert!(
            result.is_ok(),
            "write_atomic_with_dal Maximum should succeed, got {:?}",
            result
        );
        assert_eq!(
            content, "replaced\n",
            "file content should be 'replaced\\n', got {:?}",
            content
        );
    }

    // ---------------------------------------------------------------------------
    // write_atomic_with_dal: Baseline level path
    // ---------------------------------------------------------------------------

    /// write_atomic_with_dal with DalLevel::Baseline exercises the standard
    /// path (no extra resource check). Uses a ResourceGuard with zero
    /// entropy/pressure so check() always passes.
    #[test]
    fn test_write_atomic_with_dal_baseline() {
        let _serial = CWD_MUTEX.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let original_cwd = std::env::current_dir().unwrap();
        let _cwd_guard = CwdGuard::new(original_cwd);
        std::env::set_current_dir(dir.path()).unwrap();

        let file = PathBuf::from("test_dal_baseline.txt");
        fs::write(&file, "original\n").unwrap();

        let guard = ResourceGuard::for_testing(usize::MAX / 2, 0, 0);
        let result = write_atomic_with_dal(
            file.to_str().unwrap(),
            &["baseline"],
            &guard,
            DalLevel::Baseline,
        );

        let content = fs::read_to_string(&file).unwrap();
        assert!(
            result.is_ok(),
            "write_atomic_with_dal Baseline should succeed, got {:?}",
            result
        );
        assert_eq!(
            content, "baseline\n",
            "file content should be 'baseline\\n', got {:?}",
            content
        );
    }

    // ---------------------------------------------------------------------------
    // SniperLock: stale PID cleanup on Unix
    // ---------------------------------------------------------------------------

    /// On Unix, when a lock file exists with a PID that is NOT alive,
    /// acquire_with_config removes the stale lock and acquires a fresh one
    /// (lines 720-726). Uses PID 99999 which is almost certainly not alive.
    #[cfg(unix)]
    #[test]
    fn test_lock_stale_pid_cleanup() {
        let _serial = CWD_MUTEX.lock().unwrap();
        let dir = TempDir::new().unwrap();
        let original_cwd = std::env::current_dir().unwrap();
        let _cwd_guard = CwdGuard::new(original_cwd);
        std::env::set_current_dir(dir.path()).unwrap();

        let file = PathBuf::from("test_stale_lock.txt");
        fs::write(&file, "content").unwrap();

        // Manually create .sniper/ and lock file with a stale PID
        fs::create_dir_all(BACKUP_DIR).unwrap();
        let normalized = normalize_path(file.to_str().unwrap())
            .expect("normalize_path should succeed");
        let hash = get_path_hash(&normalized);
        let lock_path = PathBuf::from(BACKUP_DIR).join(format!("sniper.{}.lock", hash));

        // Write a PID that is almost certainly NOT alive
        fs::write(&lock_path, "99999").unwrap();
        assert!(
            lock_path.exists(),
            "lock file should exist before acquire attempt"
        );

        // Config with very short timeout so the timeout check passes immediately
        let config = SniperConfig {
            lock_timeout: Duration::from_millis(1),
            ..SniperConfig::default()
        };

        let result = SniperLock::acquire_with_config(file.to_str().unwrap(), &config);

        assert!(
            result.is_ok(),
            "SniperLock::acquire_with_config should succeed after stale lock cleanup"
        );

        let lock = result.unwrap();
        let current_pid = std::process::id().to_string();
        let lock_content = fs::read_to_string(&lock.lock_path).unwrap();

        assert_eq!(
            lock_content.trim(),
            current_pid,
            "lock file should contain current PID after re-acquisition"
        );
    }
}
