use std::fs;

use pyo3::exceptions::{PyIOError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;

use llmosafe::ResourceGuard;

use moesniper::{
    auto_indent_content, check_file_size, count_recent_backups, create_backup, find_latest_backup,
    generate_preview, handle_backtrack_error, hex_decode, hex_encode, needs_indent_fix,
    normalize_path, purge_old_backups, recommend_from_risk, validate_indentation, verify_context,
    write_atomic_with_dal, ManifestOp, RiskTelemetry, SniperConfig, SniperLock, NAME, VERSION,
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
    m.add_function(wrap_pyfunction!(validate_indentation_py, m)?)?;
    m.add_function(wrap_pyfunction!(auto_indent_content_py, m)?)?;
    m.add_function(wrap_pyfunction!(needs_indent_fix_py, m)?)?;
    m.add_function(wrap_pyfunction!(verify_context_py, m)?)?;
    m.add_function(wrap_pyfunction!(recommend_from_risk_py, m)?)?;
    m.add_function(wrap_pyfunction!(write_atomic_with_dal_py, m)?)?;
    m.add_function(wrap_pyfunction!(check_file_size_py, m)?)?;
    m.add_function(wrap_pyfunction!(normalize_path_py, m)?)?;
    m.add_function(wrap_pyfunction!(create_backup_py, m)?)?;
    m.add_function(wrap_pyfunction!(find_latest_backup_py, m)?)?;
    m.add_function(wrap_pyfunction!(count_recent_backups_py, m)?)?;
    m.add_function(wrap_pyfunction!(purge_old_backups_py, m)?)?;
    m.add_function(wrap_pyfunction!(version_py, m)?)?;
    m.add_function(wrap_pyfunction!(generate_preview_py, m)?)?;
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
///     auto_indent (bool, optional): Auto-detect and apply indentation. Default False.
///     force_indent (bool, optional): Bypass indentation validation. Default False.
///     context_hash (str, optional): SHA-256 prefix (16 hex chars) for pre-edit verification.
///     dry_run (bool, optional): Preview changes without applying. Default False.
///
/// Returns:
///     dict: Result with keys:
///         - status (str): "ok" on success, "error" on failure.
///         - lines_removed (int): Number of lines removed.
///         - lines_inserted (int): Number of lines inserted.
///         - total_lines (int): Total lines in the file after edit.
///         - backup_path (str): Path to the backup file created before edit.
///         - risk (str, optional): JSON-encoded risk telemetry.
///         - recommended_action (str, optional): Resource-driven recommendation.
///         - ai_hint (str, optional): AI-consumable guidance hint.
///         - diff_preview (str, optional): Dry-run diff preview (only present when dry_run=True).
///         - message (str, optional): Error message if status is "error".
///
/// Raises:
///     ValueError: Invalid path, line range out of bounds, file too large.
///     RuntimeError: Lock acquisition failure, backup failure, write failure.
///     IOError: File read/write failures.
/// 8 params required for PyO3 function signature; can't reduce.
#[allow(clippy::too_many_arguments)]
#[pyfunction(signature = (filepath, start, end, content, auto_indent=None, force_indent=None, context_hash=None, dry_run=None))]
fn sniper_edit(
    py: Python<'_>,
    filepath: &str,
    start: usize,
    end: usize,
    content: &str,
    auto_indent: Option<bool>,
    force_indent: Option<bool>,
    context_hash: Option<&str>,
    dry_run: Option<bool>,
) -> PyResult<Py<PyDict>> {
    let config = SniperConfig::from_env();
    let guard = ResourceGuard::auto(0.5);
    let risk = RiskTelemetry::from_guard(&guard);

    let result = (|| -> Result<_, String> {
        normalize_path(filepath)?;
        check_file_size(filepath, config.max_file_size)?;
        // Gate lock behind dry_run: dry-run reads should not create .sniper/
        let _lock: Option<SniperLock> = if !dry_run.unwrap_or(false) {
            Some(SniperLock::acquire_with_config(filepath, &config)?)
        } else {
            None
        };

        let text = fs::read_to_string(filepath).map_err(|e| format!("read {filepath}: {e}"))?;
        let lines: Vec<String> = text.split_inclusive('\n').map(String::from).collect();

        if start < 1 || end > lines.len() || start > end + 1 {
            if start == lines.len() + 1 && (start == end + 1 || start == end) {
                // Allow inserting at end (same parity as CLI)
            } else {
                return Err(format!(
                    "line range {start}-{end} out of bounds (file has {} lines)",
                    lines.len()
                ));
            }
        }

        // Context verification: reject if surrounding code has changed
        if let Some(expected) = context_hash {
            verify_context(&lines, start, end, expected)?;
        }

        let s = start - 1;
        let removed = if s < lines.len() {
            end.min(lines.len()) - s
        } else {
            0
        };

        let processed_content = if content.is_empty() {
            String::new()
        } else {
            let mut c = content.to_string();
            if auto_indent.unwrap_or(false) && needs_indent_fix(&lines, start, end, &c) {
                c = auto_indent_content(&lines, start, end, &c);
            }
            c
        };

        let new_lines: Vec<String> = if processed_content.is_empty() {
            vec![]
        } else {
            processed_content
                .split_inclusive('\n')
                .map(String::from)
                .collect()
        };

        // Indentation validation (skip if force_indent or dry_run)
        if !force_indent.unwrap_or(false) && !dry_run.unwrap_or(false) && !new_lines.is_empty() {
            let (valid, _, _warning) = validate_indentation(&lines, start, end, &new_lines);
            if !valid {
                return Err("indentation validation failed: replacement content indent does not match surrounding context".to_string());
            }
        }

        // Gate backup behind dry_run: dry-run should not create .sniper/ backups
        let bk = if !dry_run.unwrap_or(false) {
            create_backup(filepath)?
        } else {
            String::new()
        };

        let mut modified = lines;
        let new_lines_len = new_lines.len();
        if s < modified.len() {
            modified.splice(s..end.min(modified.len()), new_lines);
        } else {
            modified.extend(new_lines);
        }

        let refs: Vec<&str> = modified.iter().map(|s| s.as_str()).collect();

        if dry_run.unwrap_or(false) {
            let preview = text
                .lines()
                .enumerate()
                .map(|(i, line)| {
                    let ln = i + 1;
                    if ln >= start && ln <= end {
                        if processed_content.is_empty() {
                            format!("- {}", line)
                        } else {
                            format!("- {}\n+ {}", line, processed_content)
                        }
                    } else {
                        format!("  {}", line)
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            Ok((modified.len(), removed, new_lines_len, bk, Some(preview)))
        } else {
            write_atomic_with_dal(filepath, &refs, &guard, config.dal_level)?;
            let _ = purge_old_backups(filepath, &config);

            Ok((modified.len(), removed, new_lines_len, bk, None))
        }
    })();

    let dict = PyDict::new(py);
    match result {
        Ok((total, removed, inserted, bk, preview)) => {
            dict.set_item("status", "ok")?;
            dict.set_item("lines_removed", removed)?;
            dict.set_item("lines_inserted", inserted)?;
            dict.set_item("total_lines", total)?;
            dict.set_item("backup_path", bk)?;

            if let Some(ref preview_str) = preview {
                dict.set_item("diff_preview", preview_str)?;
            }

            let risk_json = serde_json::to_string(&risk).unwrap_or_default();
            dict.set_item("risk", risk_json)?;
            dict.set_item("recommended_action", recommend_from_risk(&risk))?;

            let backup_count =
                count_recent_backups(filepath, config.lock_timeout.as_secs()).unwrap_or(0);
            if backup_count >= 3 {
                dict.set_item(
                    "ai_hint",
                    "Multiple edits to this file. Consider batching with manifest.",
                )?;
            }
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
/// Delegates to sniper_edit with an empty content string.
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
#[pyfunction(signature = (filepath, start, end))]
fn sniper_delete(py: Python<'_>, filepath: &str, start: usize, end: usize) -> PyResult<Py<PyDict>> {
    sniper_edit(py, filepath, start, end, "", None, None, None, None)
}

/// Apply a batch of edit/delete operations from a JSON manifest string.
///
/// Operations are applied bottom-up (by start line, descending) so that
/// line numbers in earlier operations remain valid after later operations.
/// Writes are gated by a ResourceGuard and Defense-Ascension Level.
/// Supports auto-indentation detection and per-operation indentation validation
/// mirroring the CLI manifest path.
///
/// Args:
///     filepath (str): Path to the target file.
///     operations_json (str): JSON string — an array of operation objects.
///         Each object has:
///             start (int): 1-based start line.
///             end (int, optional): 1-based end line (default: start).
///             hex (str, optional): Hex-encoded content to insert.
///             delete (bool, optional): If true, deletes the range.
///     auto_indent (bool, optional): Auto-detect and apply indentation per op. Default None.
///     force_indent (bool, optional): Bypass indentation validation per op. Default None.
///     context_hash (str, optional): SHA-256 prefix (16 hex chars) verified
///         against surrounding context before each hex operation. Rejects edits
///         if original content changed since line numbers were computed.
///     dry_run (bool, optional): Preview without applying changes. Default None.
///
/// Returns:
///     dict: Result with keys:
///         - status (str): "ok" or "error".
///         - lines_removed (int): Total lines removed across all ops.
///         - lines_inserted (int): Total lines inserted across all ops.
///         - total_lines (int): Total lines in the file after manifest.
///         - operations (int): Number of operations applied.
///         - backup_path (str): Path to the backup file (empty string if dry_run).
///         - risk (str): JSON-encoded risk telemetry.
///         - recommended_action (str): Resource-driven recommendation.
///         - ai_hint (str, optional): AI-consumable guidance hint.
///         - message (str, optional): Error message on failure.
///
/// Raises:
///     ValueError: Invalid JSON, invalid hex, invalid line ranges.
///     RuntimeError: Lock/backup/write/resource failures.
///     IOError: File read failures.
/// Indent params, dry_run, and context_hash default to None for backward compatibility.
#[pyfunction(signature = (filepath, operations_json, auto_indent=None, force_indent=None, context_hash=None, dry_run=None))]
fn sniper_manifest(
    py: Python<'_>,
    filepath: &str,
    operations_json: &str,
    auto_indent: Option<bool>,
    force_indent: Option<bool>,
    context_hash: Option<&str>,
    dry_run: Option<bool>,
) -> PyResult<Py<PyDict>> {
    let config = SniperConfig::from_env();
    let guard = ResourceGuard::auto(0.5);
    let risk = RiskTelemetry::from_guard(&guard);

    let result = (|| -> Result<_, String> {
        normalize_path(filepath)?;
        check_file_size(filepath, config.max_file_size)?;
        // Gate lock behind dry_run: dry-run reads should not create .sniper/
        let _lock: Option<SniperLock> = if !dry_run.unwrap_or(false) {
            Some(SniperLock::acquire_with_config(filepath, &config)?)
        } else {
            None
        };

        let mut ops: Vec<ManifestOp> =
            serde_json::from_str(operations_json).map_err(|e| format!("parse JSON: {e}"))?;

        for op in &ops {
            if let Some(ref hex) = op.hex {
                hex_decode(hex)?;
            }
        }

        let text = fs::read_to_string(filepath).map_err(|e| handle_backtrack_error(e, "Read"))?;
        let mut lines: Vec<String> = text.split_inclusive('\n').map(String::from).collect();

        ops.sort_by_key(|b| std::cmp::Reverse(b.start));

        // Guard: overlapping same-start operations cause silent data loss.
        // Bottom-up processing assumes each op targets a distinct line range;
        // two ops at the same start line would corrupt each other's output.
        for i in 1..ops.len() {
            if ops[i].start == ops[i - 1].start {
                return Err(format!(
                    "overlapping manifest operations at line {}",
                    ops[i].start
                ));
            }
        }

        let bk = if !dry_run.unwrap_or(false) {
            create_backup(filepath)?
        } else {
            String::new()
        };
        let mut total_removed = 0usize;
        let mut total_inserted = 0usize;

        // Context verification: for manifest mode, verify the hash ONCE
        // against the pre-manifest file state (before any operation mutates lines).
        if let Some(expected) = context_hash {
            if let Some(first_op) = ops.first() {
                let first_end = first_op.end.unwrap_or(first_op.start);
                verify_context(&lines, first_op.start, first_end, expected)?;
            }
        }

        for op in &ops {
            let s = op.start;
            let e = op.end.unwrap_or(op.start);

            if s < 1 || e > lines.len() || s > e + 1 {
                if s == lines.len() + 1 && (s == e + 1 || s == e) {
                    // Allow inserting at end
                } else {
                    return Err(format!(
                        "line range {s}-{e} out of bounds (file has {} lines)",
                        lines.len()
                    ));
                }
            }

            let range_start = s - 1;
            let actual_e = e.min(lines.len());

            if op.delete.unwrap_or(false) && op.hex.is_some() {
                return Err("Cannot both delete and insert in the same operation".to_string());
            }

            if op.delete.unwrap_or(false) {
                let removed = lines
                    .splice(range_start..actual_e, std::iter::empty())
                    .count();
                total_removed += removed;
            } else if let Some(ref hex) = op.hex {
                let decoded = hex_decode(hex)?;

                // Apply auto-indent if needed (mirrors CLI cmd_manifest_impl)
                let final_content = if auto_indent.unwrap_or(false)
                    && needs_indent_fix(&lines, op.start, actual_e, &decoded)
                {
                    auto_indent_content(&lines, op.start, actual_e, &decoded)
                } else {
                    decoded
                };

                // Validate indentation (skip if force_indent or dry_run)
                if !force_indent.unwrap_or(false) && !dry_run.unwrap_or(false) {
                    let new_lines_for_check: Vec<String> = final_content
                        .split_inclusive('\n')
                        .map(String::from)
                        .collect();
                    let (valid, warning, _) =
                        validate_indentation(&lines, op.start, actual_e, &new_lines_for_check);
                    if !valid {
                        return Err(format!(
                            "indentation validation failed at line {}: {}",
                            op.start,
                            warning.as_deref().unwrap_or_default()
                        ));
                    }
                }

                let new: Vec<String> = final_content
                    .split_inclusive('\n')
                    .map(String::from)
                    .collect();
                let new_len = new.len();
                let removed = if range_start < lines.len() {
                    lines.splice(range_start..actual_e, new).count()
                } else {
                    lines.extend(new);
                    0
                };
                total_removed += removed;
                total_inserted += new_len;
            }
        }

        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        if !dry_run.unwrap_or(false) {
            write_atomic_with_dal(filepath, &refs, &guard, config.dal_level)?;
            let _ = purge_old_backups(filepath, &config);
        }

        Ok((lines.len(), total_removed, total_inserted, ops.len(), bk))
    })();

    let dict = PyDict::new(py);
    match result {
        Ok((total, removed, inserted, count, bk)) => {
            dict.set_item("status", "ok")?;
            dict.set_item("lines_removed", removed)?;
            dict.set_item("lines_inserted", inserted)?;
            dict.set_item("total_lines", total)?;
            dict.set_item("operations", count)?;
            dict.set_item("backup_path", bk)?;

            let risk_json = serde_json::to_string(&risk).unwrap_or_default();
            dict.set_item("risk", risk_json)?;
            dict.set_item("recommended_action", recommend_from_risk(&risk))?;

            let backup_count =
                count_recent_backups(filepath, config.lock_timeout.as_secs()).unwrap_or(0);
            if backup_count >= 3 {
                dict.set_item(
                    "ai_hint",
                    "Multiple edits to this file. Consider batching with manifest.",
                )?;
            }
        }
        Err(msg) => {
            dict.set_item("status", "error")?;
            dict.set_item("message", msg)?;
        }
    }
    Ok(dict.into())
}

/// Restore the most recent backup for a file.
///
/// Uses atomic temp+rename to prevent corruption if the process is
/// interrupted mid-restore. The consumed backup is removed after
/// successful restore to support consecutive undo operations.
///
/// Args:
///     filepath (str): Path to the target file.
///
/// Returns:
///     str: Path to the restored backup on success.
///
/// Raises:
///     RuntimeError: No backup found, lock acquisition failure.
///     IOError: Backup restore failure.
#[pyfunction]
fn sniper_undo(filepath: &str) -> PyResult<String> {
    let config = SniperConfig::from_env();
    let _lock = SniperLock::acquire_with_config(filepath, &config)
        .map_err(|e| PyRuntimeError::new_err(format!("lock acquire: {e}")))?;

    let latest = find_latest_backup(filepath)
        .map_err(|e| PyRuntimeError::new_err(format!("find backup: {e}")))?;

    match latest {
        Some(backup_path) => {
            // Atomic restore: copy to temp file, then rename over the target.
            // This matches the CLI undo behavior — prevents partial writes
            // if the process is interrupted between copy and rename.
            let tmp = format!("{}.sniper_undo_tmp", filepath);
            fs::copy(&backup_path, &tmp)
                .map_err(|e| PyIOError::new_err(format!("restore (copy to temp): {e}")))?;
            if let Err(e) = fs::rename(&tmp, filepath) {
                let _ = fs::remove_file(&tmp);
                return Err(PyIOError::new_err(format!("restore (rename): {e}")));
            }
            // Pop the stack: remove the consumed backup.
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
/// Delegates to the library's optimized hex encoder which uses a
/// pre-allocated buffer with direct byte-to-nibble mapping.
///
/// Args:
///     text (str): Content to encode.
///
/// Returns:
///     str: Hex-encoded version of the input.
#[pyfunction]
fn sniper_encode(text: &str) -> String {
    hex_encode(text.as_bytes())
}

/// Hex-decode a string.
///
/// Args:
///     hex_str (str): Hex-encoded string to decode.
///
/// Returns:
///     str: Decoded content.
///
/// Raises:
///     ValueError: Invalid hex input.
#[pyfunction]
fn sniper_decode(hex_str: &str) -> PyResult<String> {
    hex_decode(hex_str).map_err(|e| PyValueError::new_err(format!("hex decode: {e}")))
}

/// Read a file's full contents as a UTF-8 string.
///
/// Args:
///     filepath (str): Path to the target file.
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

/// Return the library name and version string.
///
/// Args:
///     None
///
/// Returns:
///     dict: Keys:
///         - name (str): Library name (e.g. "moesniper").
///         - version (str): Semantic version (e.g. "0.7.2").
#[pyfunction]
fn version_py(py: Python<'_>) -> PyResult<Py<PyDict>> {
    let dict = PyDict::new(py);
    dict.set_item("name", NAME)?;
    dict.set_item("version", VERSION)?;
    Ok(dict.into())
}

/// Validate indentation of content against surrounding lines.
///
/// Args:
///     filepath (str): Path to the target file.
///     start (int): Start line of the edit range (1-based).
///     end (int): End line of the edit range (1-based).
///     content (str): The content to validate.
///
/// Returns:
///     dict: Result with keys:
///         - valid (bool): True if indentation matches surroundings.
///         - message (str): Explanation of the validation result.
///
/// Raises:
///     IOError: File not found or read error.
#[pyfunction]
fn validate_indentation_py(
    py: Python<'_>,
    filepath: &str,
    start: usize,
    end: usize,
    content: &str,
) -> PyResult<Py<PyDict>> {
    let lines: Vec<String> = fs::read_to_string(filepath)
        .map_err(|e| PyIOError::new_err(format!("read {filepath}: {e}")))?
        .lines()
        .map(|s| s.to_string())
        .collect();
    let content_lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();

    let (valid, msg, detail) = validate_indentation(&lines, start, end, &content_lines);
    let dict = PyDict::new(py);
    dict.set_item("valid", valid)?;
    dict.set_item("message", msg.unwrap_or_default())?;
    dict.set_item("detail", detail.unwrap_or_default())?;
    Ok(dict.into())
}

/// Auto-detect and apply indentation from surrounding lines.
///
/// Args:
///     filepath (str): Path to the target file.
///     start (int): Start line of the edit range (1-based).
///     end (int): End line of the edit range (1-based).
///     content (str): The content to indent.
///
/// Returns:
///     str: Content with indentation applied.
///
/// Raises:
///     IOError: File not found or read error.
#[pyfunction]
fn auto_indent_content_py(
    filepath: &str,
    start: usize,
    end: usize,
    content: &str,
) -> PyResult<String> {
    let lines: Vec<String> = fs::read_to_string(filepath)
        .map_err(|e| PyIOError::new_err(format!("read {filepath}: {e}")))?
        .lines()
        .map(|s| s.to_string())
        .collect();

    Ok(auto_indent_content(&lines, start, end, content))
}

/// Check if content needs indentation fix.
///
/// Args:
///     filepath (str): Path to the target file.
///     start (int): Start line of the edit range (1-based).
///     end (int): End line of the edit range (1-based).
///     content (str): The content to check.
///
/// Returns:
///     bool: True if content needs indentation adjustment.
///
/// Raises:
///     IOError: File not found or read error.
#[pyfunction]
fn needs_indent_fix_py(filepath: &str, start: usize, end: usize, content: &str) -> PyResult<bool> {
    let lines: Vec<String> = fs::read_to_string(filepath)
        .map_err(|e| PyIOError::new_err(format!("read {filepath}: {e}")))?
        .lines()
        .map(|s| s.to_string())
        .collect();

    Ok(needs_indent_fix(&lines, start, end, content))
}

/// Verify context hash before applying an edit.
///
/// Args:
///     filepath (str): Path to the target file.
///     expected_hash (str): Expected SHA-256 prefix (16 hex chars).
///
/// Returns:
///     dict: Result with keys:
///         - valid (bool): True if hash matches.
///         - actual_hash (str): Actual hash prefix (16 hex chars).
///         - message (str): Explanation of the verification result.
///
/// Raises:
///     IOError: File not found or read error.
#[pyfunction]
fn verify_context_py(
    py: Python<'_>,
    filepath: &str,
    start: usize,
    end: usize,
    expected_hash: &str,
) -> PyResult<Py<PyDict>> {
    let lines: Vec<String> = fs::read_to_string(filepath)
        .map_err(|e| PyIOError::new_err(format!("read {filepath}: {e}")))?
        .lines()
        .map(|s| s.to_string())
        .collect();

    let dict = PyDict::new(py);
    match verify_context(&lines, start, end, expected_hash) {
        Ok(()) => {
            dict.set_item("valid", true)?;
            dict.set_item("message", "Context hash matches")?;
        }
        Err(msg) => {
            dict.set_item("valid", false)?;
            dict.set_item("message", msg)?;
        }
    }
    Ok(dict.into())
}

/// Get a recommended action based on risk telemetry.
///
/// Args:
///     None (reads current resource state).
///
/// Returns:
///     str: Recommended action (e.g., "proceed", "wait", "reduce_scope").
#[pyfunction]
fn recommend_from_risk_py() -> String {
    let guard = ResourceGuard::auto(0.5);
    let risk = RiskTelemetry::from_guard(&guard);
    recommend_from_risk(&risk)
}

/// Atomic write with Data Access Layer (DAL) protection.
///
/// Args:
///     filepath (str): Path to the target file.
///     content (str): Content to write.
///     dal_level (str): DAL level ("minimum", "moderate", "maximum").
///
/// Returns:
///     dict: Result with keys:
///         - status (str): "ok" on success, "error" on failure.
///         - message (str): Explanation of the result.
///         - wait_time_ms (int, optional): Time waited due to DAL.
///
/// Raises:
///     IOError: Write error or permission denied.
#[pyfunction]
fn write_atomic_with_dal_py(
    py: Python<'_>,
    filepath: &str,
    content: &str,
    dal_level: &str,
) -> PyResult<Py<PyDict>> {
    use moesniper::DalLevel;

    let level = match dal_level.to_uppercase().as_str() {
        "BASELINE" => DalLevel::Baseline,
        "ENHANCED" => DalLevel::Enhanced,
        "MAXIMUM" => DalLevel::Maximum,
        _ => {
            return Err(PyValueError::new_err(
                "Invalid DAL level. Use: BASELINE, ENHANCED, MAXIMUM",
            ))
        }
    };

    let _config = SniperConfig::from_env();
    let guard = ResourceGuard::auto(0.5);

    let lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    let lines_ref: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
    let result = write_atomic_with_dal(filepath, &lines_ref, &guard, level);

    let dict = PyDict::new(py);
    match result {
        Ok(()) => {
            dict.set_item("status", "ok")?;
            dict.set_item("message", "Write successful")?;
        }
        Err(e) => {
            dict.set_item("status", "error")?;
            dict.set_item("message", e)?;
        }
    }
    Ok(dict.into())
}

/// Check if a file exceeds the maximum size limit.
///
/// Args:
///     filepath (str): Path to the file.
///     max_size (int): Maximum size in bytes.
///
/// Returns:
///     bool: True if file size is within limit.
///
/// Raises:
///     ValueError: File exceeds the maximum size limit.
///     IOError: File not found, stat error, or other I/O error.
#[pyfunction]
fn check_file_size_py(filepath: &str, max_size: u64) -> PyResult<bool> {
    check_file_size(filepath, max_size)
        .map(|_| true)
        .map_err(|msg| {
            if msg.contains("File too large") {
                PyValueError::new_err(msg)
            } else {
                PyIOError::new_err(msg)
            }
        })
}

/// Normalize a file path (expand ~, resolve symlinks).
///
/// Args:
///     path (str): Path to normalize.
///
/// Returns:
///     str: Normalized absolute path.
///
/// Raises:
///     ValueError: Invalid path.
#[pyfunction]
fn normalize_path_py(path: &str) -> PyResult<String> {
    normalize_path(path)
        .map(|p| p.to_string_lossy().to_string())
        .map_err(PyValueError::new_err)
}

/// Create a backup of a file.
///
/// Args:
///     filepath (str): Path to the file to backup.
///
/// Returns:
///     str: Path to the created backup file.
///
/// Raises:
///     IOError: Backup creation failed.
#[pyfunction]
fn create_backup_py(filepath: &str) -> PyResult<String> {
    create_backup(filepath).map_err(PyIOError::new_err)
}

/// Find the latest backup for a file.
///
/// Args:
///     filepath (str): Path to the original file.
///
/// Returns:
///     str or None: Path to the latest backup, or None if no backups exist.
///
/// Raises:
///     IOError: Search failed.
#[pyfunction]
fn find_latest_backup_py(filepath: &str) -> PyResult<Option<String>> {
    find_latest_backup(filepath)
        .map(|opt| opt.map(|p| p.to_string_lossy().to_string()))
        .map_err(PyIOError::new_err)
}

/// Count recent backups within a time window.
///
/// Args:
///     filepath (str): Path to the original file.
///     window_secs (int): Time window in seconds.
///
/// Returns:
///     int: Number of backups within the window.
///
/// Raises:
///     IOError: Count failed.
#[pyfunction]
fn count_recent_backups_py(filepath: &str, window_secs: u64) -> PyResult<usize> {
    count_recent_backups(filepath, window_secs).map_err(PyIOError::new_err)
}

/// Purge old backups by count and age.
///
/// Args:
///     filepath (str): Path to the original file.
///     retention_count (int): Number of backups to retain.
///     max_age_days (int): Maximum age in days.
///
/// Returns:
///     int: Number of backups purged.
///
/// Raises:
///     IOError: Purge failed.
#[pyfunction]
fn purge_old_backups_py(
    filepath: &str,
    retention_count: usize,
    max_age_days: u64,
) -> PyResult<usize> {
    use moesniper::SniperConfig;
    let config = SniperConfig {
        backup_retention_count: retention_count,
        backup_max_age_days: max_age_days,
        ..SniperConfig::from_env()
    };
    // purge_old_backups returns Result<(), String> — count not exposed
    purge_old_backups(filepath, &config)
        .map(|_| 0)
        .map_err(PyIOError::new_err)
}

/// Generate a diff preview for dry-run without modifying files.
///
/// Args:
///     filepath (str): Path to the target file.
///     start (int): Start line of edit range (1-based).
///     end (int): End line of edit range (1-based).
///     replacement (str): Content that would replace the range.
///
/// Returns:
///     dict: Result with key:
///         - preview (list[str]): Lines of unified diff-style preview.
///
/// Raises:
///     IOError: File not found or read error.
#[pyfunction]
fn generate_preview_py(
    py: Python<'_>,
    filepath: &str,
    start: usize,
    end: usize,
    replacement: &str,
) -> PyResult<Py<PyDict>> {
    let text: Vec<String> = fs::read_to_string(filepath)
        .map_err(|e| PyIOError::new_err(format!("read {filepath}: {e}")))?
        .split_inclusive('\n')
        .map(String::from)
        .collect();
    let new_lines: Vec<String> = replacement
        .split_inclusive('\n')
        .map(String::from)
        .collect();

    let preview = generate_preview(&text, &new_lines, start, end);
    let dict = PyDict::new(py);
    dict.set_item("preview", preview)?;
    Ok(dict.into())
}
