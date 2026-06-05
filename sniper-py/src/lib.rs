use std::fs;

use pyo3::exceptions::{PyIOError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;

use moesniper::{
    check_file_size, create_backup, find_latest_backup, hex_decode, normalize_path,
    purge_old_backups, write_atomic, SniperConfig, SniperLock,
};

/// Python bindings for moesniper — escape-proof precision file editing.
///
/// Provides hex-encoded content operations, line-range splicing,
/// atomic writes, and undo via timestamped backups.
#[pymodule]
fn _native(_py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(sniper_edit, m)?)?;
    m.add_function(wrap_pyfunction!(sniper_delete, m)?)?;
    m.add_function(wrap_pyfunction!(sniper_manifest, m)?)?;
    m.add_function(wrap_pyfunction!(sniper_undo, m)?)?;
    m.add_function(wrap_pyfunction!(sniper_encode, m)?)?;
    m.add_function(wrap_pyfunction!(sniper_decode, m)?)?;
    m.add_function(wrap_pyfunction!(sniper_read_file, m)?)?;
    m.add_function(wrap_pyfunction!(sniper_config, m)?)?;
    Ok(())
}

/// Edit a file by replacing lines in range [start, end] with new content.
///
/// Args:
///     filepath (str): Path to the target file.
///     start (int): First line to replace (1-based, inclusive).
///     end (int): Last line to replace (1-based, inclusive). To insert at a
///         position, set start == end. Must satisfy 1 <= start <= end.
///     content (str): New content to insert. Empty string deletes the range.
///
/// Returns:
///     dict: Result with keys:
///         - status (str): "ok" on success, "error" on failure.
///         - lines_removed (int): Number of lines removed.
///         - lines_inserted (int): Number of lines inserted.
///         - total_lines (int): Total lines in the file after edit.
///         - backup_path (str): Path to the backup file created before edit.
///         - message (str, optional): Error message if status is "error".
///
/// Raises:
///     ValueError: Invalid path, line range out of bounds, file too large.
///     RuntimeError: Lock acquisition failure, backup failure, write failure.
///     IOError: File read/write failures.
#[pyfunction]
fn sniper_edit(
    py: Python<'_>,
    filepath: &str,
    start: usize,
    end: usize,
    content: &str,
) -> PyResult<Py<PyDict>> {
    let config = SniperConfig::from_env();

    let result = (|| -> Result<_, String> {
        normalize_path(filepath)?;
        check_file_size(filepath, config.max_file_size)?;
        let _lock = SniperLock::acquire_with_config(filepath, &config)?;

        let text = fs::read_to_string(filepath).map_err(|e| format!("read {filepath}: {e}"))?;
        let lines: Vec<String> = text.split_inclusive('\n').map(String::from).collect();

        if start < 1 || start > lines.len() + 1 {
            return Err(format!(
                "start line {start} out of bounds (file has {} lines)",
                lines.len()
            ));
        }
        if end < start || end > lines.len() + 1 {
            return Err(format!(
                "end line {end} out of bounds (file has {} lines)",
                lines.len()
            ));
        }

        let s = start - 1;
        let removed = if s < lines.len() {
            end.min(lines.len()) - s
        } else {
            0
        };

        let new_lines: Vec<String> = if content.is_empty() {
            vec![]
        } else {
            content.split_inclusive('\n').map(String::from).collect()
        };

        let bk = create_backup(filepath)?;

        let mut modified = lines;
        if s < modified.len() {
            modified.splice(s..end.min(modified.len()), new_lines.clone());
        } else {
            modified.extend(new_lines.clone());
        }

        let refs: Vec<&str> = modified.iter().map(|s| s.as_str()).collect();
        write_atomic(filepath, &refs)?;
        let _ = purge_old_backups(filepath, &config);

        Ok((modified.len(), removed, new_lines.len(), bk))
    })();

    let dict = PyDict::new(py);
    match result {
        Ok((total, removed, inserted, bk)) => {
            dict.set_item("status", "ok")?;
            dict.set_item("lines_removed", removed)?;
            dict.set_item("lines_inserted", inserted)?;
            dict.set_item("total_lines", total)?;
            dict.set_item("backup_path", bk)?;
        }
        Err(msg) => {
            dict.set_item("status", "error")?;
            dict.set_item("message", msg)?;
        }
    }
    Ok(dict.into())
}

/// Delete lines from a file in the range [start, end).
///
/// Args:
///     filepath (str): Path to the target file.
///     start (int): First line to delete (1-based, inclusive).
///     end (int): Last line to delete (1-based, exclusive).
///
/// Returns:
///     dict: Same shape as sniper_edit. On success, lines_inserted is 0.
///
/// Raises:
///     Same as sniper_edit.
#[pyfunction]
fn sniper_delete(py: Python<'_>, filepath: &str, start: usize, end: usize) -> PyResult<Py<PyDict>> {
    sniper_edit(py, filepath, start, end, "")
}

/// Apply a batch of edit/delete operations from a JSON manifest string.
///
/// Operations are applied bottom-up (by start line, descending) so that
/// line numbers in earlier operations remain valid after later operations.
///
/// Args:
///     filepath (str): Path to the target file.
///     operations_json (str): JSON string — an array of operation objects.
///         Each object has:
///             start (int): 1-based start line.
///             end (int, optional): 1-based end line (default: start).
///             hex (str, optional): Hex-encoded content to insert.
///             delete (bool, optional): If true, deletes the range.
///
/// Returns:
///     dict: Result with keys:
///         - status (str): "ok" or "error".
///         - lines_removed (int): Total lines removed across all ops.
///         - lines_inserted (int): Total lines inserted across all ops.
///         - total_lines (int): Total lines in the file after manifest.
///         - operations (int): Number of operations applied.
///         - message (str, optional): Error message on failure.
///
/// Raises:
///     ValueError: Invalid JSON, invalid hex, invalid line ranges.
///     RuntimeError: Lock/backup/write failures.
///     IOError: File read failures.
#[pyfunction]
fn sniper_manifest(py: Python<'_>, filepath: &str, operations_json: &str) -> PyResult<Py<PyDict>> {
    let config = SniperConfig::from_env();

    let result = (|| -> Result<_, String> {
        normalize_path(filepath)?;
        check_file_size(filepath, config.max_file_size)?;
        let _lock = SniperLock::acquire_with_config(filepath, &config)?;

        let mut ops: Vec<ManifestOp> =
            serde_json::from_str(operations_json).map_err(|e| format!("parse JSON: {e}"))?;

        // Pre-validate all hex fields
        for op in &ops {
            if let Some(ref hex) = op.hex {
                hex_decode(hex)?;
            }
        }

        let text = fs::read_to_string(filepath).map_err(|e| format!("read {filepath}: {e}"))?;
        let mut lines: Vec<String> = text.split_inclusive('\n').map(String::from).collect();

        // Apply bottom-up so line numbers in earlier ops remain valid
        ops.sort_by_key(|b| std::cmp::Reverse(b.start));

        let bk = create_backup(filepath)?;
        let mut total_removed = 0usize;
        let mut total_inserted = 0usize;

        for op in &ops {
            let s = op.start;
            let e = op.end.unwrap_or(op.start);

            if s < 1 || e > lines.len() + 1 || s > e + 1 {
                return Err(format!(
                    "line range {s}-{e} out of bounds (file has {} lines)",
                    lines.len()
                ));
            }

            let range_start = s - 1;

            if op.delete.unwrap_or(false) {
                let removed = lines.splice(range_start..e, std::iter::empty()).count();
                total_removed += removed;
            } else if let Some(ref hex) = op.hex {
                let decoded = hex_decode(hex)?;
                let new: Vec<String> = decoded.split_inclusive('\n').map(String::from).collect();
                let removed = lines.splice(range_start..e, new.clone()).count();
                total_removed += removed;
                total_inserted += new.len();
            }
        }

        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        write_atomic(filepath, &refs)?;
        let _ = purge_old_backups(filepath, &config);

        Ok((lines.len(), total_removed, total_inserted, ops.len(), bk))
    })();

    let dict = PyDict::new(py);
    match result {
        Ok((total, removed, inserted, count, _bk)) => {
            dict.set_item("status", "ok")?;
            dict.set_item("lines_removed", removed)?;
            dict.set_item("lines_inserted", inserted)?;
            dict.set_item("total_lines", total)?;
            dict.set_item("operations", count)?;
        }
        Err(msg) => {
            dict.set_item("status", "error")?;
            dict.set_item("message", msg)?;
        }
    }
    Ok(dict.into())
}

/// Restore the most recent backup of a file.
///
/// Finds the latest timestamped backup created by a previous edit or manifest
/// operation and restores it. The consumed backup is removed from the stack
/// so consecutive undo calls walk backward through the backup history.
///
/// Args:
///     filepath (str): Path to the file to restore.
///
/// Returns:
///     str: Path to the backup file that was restored. Empty string if no
///         backup exists or on error.
///
/// Raises:
///     RuntimeError: No backup found, lock failure, or restore failure.
///     IOError: File copy failure.
#[pyfunction]
fn sniper_undo(filepath: &str) -> PyResult<String> {
    let config = SniperConfig::from_env();
    let _lock = SniperLock::acquire_with_config(filepath, &config)
        .map_err(|e| PyRuntimeError::new_err(format!("lock acquire: {e}")))?;

    let latest = find_latest_backup(filepath)
        .map_err(|e| PyRuntimeError::new_err(format!("find backup: {e}")))?;

    match latest {
        Some(backup_path) => {
            fs::copy(&backup_path, filepath)
                .map_err(|e| PyIOError::new_err(format!("restore: {e}")))?;
            let _ = fs::remove_file(&backup_path);
            Ok(backup_path.to_string_lossy().into())
        }
        None => Err(PyRuntimeError::new_err(format!(
            "no backup found for {filepath}"
        ))),
    }
}

/// Hex-encode a string.
///
/// Each byte of the input is converted to its two-character hex
/// representation. The result is safe for shell transmission since it
/// contains only [0-9a-f] characters.
///
/// Args:
///     text (str): The text to encode.
///
/// Returns:
///     str: Hex-encoded string (lowercase hex digits, no whitespace).
///
/// # Examples
///     sniper_encode("Hello") -> "48656c6c6f"
#[pyfunction]
fn sniper_encode(text: &str) -> String {
    text.as_bytes()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

/// Hex-decode a string back to plain text.
///
/// Accepts hex strings with or without whitespace. Rejects odd-length
/// strings and non-hex characters.
///
/// Args:
///     hex_str (str): Hex-encoded string (uppercase or lowercase).
///
/// Returns:
///     str: Decoded plain text.
///
/// Raises:
///     ValueError: Invalid hex string (odd length, non-hex chars, or
///         decoded bytes are not valid UTF-8).
///
/// # Examples
///     sniper_decode("48656c6c6f") -> "Hello"
#[pyfunction]
fn sniper_decode(hex_str: &str) -> PyResult<String> {
    hex_decode(hex_str).map_err(|e| PyValueError::new_err(format!("hex decode: {e}")))
}

/// Read the entire contents of a file as a string.
///
/// Args:
///     filepath (str): Path to the file to read.
///
/// Returns:
///     str: Full file contents.
///
/// Raises:
///     IOError: File not found or permission denied.
#[pyfunction]
fn sniper_read_file(filepath: &str) -> PyResult<String> {
    fs::read_to_string(filepath).map_err(|e| PyIOError::new_err(format!("read {filepath}: {e}")))
}

/// Return the current sniper configuration as a dict.
///
/// Reads configuration from environment variables (SNIPER_LOCK_TIMEOUT,
/// SNIPER_MAX_FILE_SIZE, etc.) and returns all fields.
///
/// Args:
///     None
///
/// Returns:
///     dict: Configuration values:
///         - lock_timeout_secs (int)
///         - max_file_size (int)
///         - backup_retention_count (int)
///         - backup_max_age_days (int)
///         - audit_enabled (bool)
///         - dal_level (str)
///         - pid_base_ms (int)
///         - pid_entropy_scale (float)
///         - pid_pressure_scale (float)
#[pyfunction]
fn sniper_config(py: Python<'_>) -> PyResult<Py<PyDict>> {
    let config = SniperConfig::from_env();
    let dict = PyDict::new(py);
    dict.set_item("lock_timeout_secs", config.lock_timeout.as_secs())?;
    dict.set_item("max_file_size", config.max_file_size)?;
    dict.set_item("backup_retention_count", config.backup_retention_count)?;
    dict.set_item("backup_max_age_days", config.backup_max_age_days)?;
    dict.set_item("audit_enabled", config.audit_enabled)?;
    dict.set_item("dal_level", format!("{:?}", config.dal_level))?;
    dict.set_item("pid_base_ms", config.pid_base_ms)?;
    dict.set_item("pid_entropy_scale", config.pid_entropy_scale)?;
    dict.set_item("pid_pressure_scale", config.pid_pressure_scale)?;
    Ok(dict.into())
}

/// Internal structure for deserializing manifest operations from JSON.
#[derive(serde::Deserialize)]
struct ManifestOp {
    /// 1-based start line (inclusive).
    start: usize,
    /// 1-based end line (exclusive). Defaults to start if absent.
    #[serde(default)]
    end: Option<usize>,
    /// Hex-encoded content to insert at this position.
    #[serde(default)]
    hex: Option<String>,
    /// If true, delete the range instead of inserting content.
    #[serde(default)]
    delete: Option<bool>,
}
