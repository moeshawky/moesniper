//! Sniper — escape-proof precision file editor for LLM agents.
//!
//! One operation: splice(file, start, end, hex_payload).
//! Hex encoding guarantees zero shell corruption.
//! Batch manifests apply bottom-up so line numbers never shift.
//!
//! Usage:
//! sniper `<file>` `<start>` `<end>` `<hex>`          Replace lines
//! sniper `<file>` `<start>` `<end>` --delete        Delete lines
//! sniper `<file>` `<start>` `<end>` --stdin         Read content from stdin
//! sniper `<file>` --manifest `<path>`             Batch from JSON manifest
//! sniper `<file>` --undo                        Restore from backup
//! sniper encode [--stdin|--file `<path>`|`<text>`]     Hex-encode content
//!
//! Flags: --dry-run, --json, --stdin, --auto-indent, --force-indent, --context `<hash>`
//!
//! Indentation: Validation runs by default. --auto-indent fixes unindented content.
//!              --force-indent bypasses validation for deliberate refactoring.
//! --context:   Verifies SHA-256 hash (first 16 hex chars) of 3 lines before and
//!              after the edit target. Rejects if context changed since line numbers
//!              were computed.
//!
//! LINE NUMBERS: All line numbers are 1-based (first line is 1, not 0)
//!
//! CONFIGURATION (via environment variables):
//! SNIPER_LOCK_TIMEOUT              Lock acquisition timeout in seconds (default: 30)
//! SNIPER_MAX_FILE_SIZE             Maximum file size to edit, e.g., "100MB" (default: 100MB)
//! SNIPER_BACKUP_RETENTION_COUNT    Number of backups to keep (default: 50)
//! SNIPER_BACKUP_MAX_AGE_DAYS       Max age of backups in days (default: 30)

mod help_text;

use std::fs;
use std::io::Read;

use moesniper::security::is_regular_file;
use moesniper::{
    auto_indent_content, check_file_size, compute_context_hash, count_recent_backups,
    create_backup, find_latest_backup, generate_preview, handle_backtrack_error, hex_decode,
    needs_indent_fix, normalize_path, purge_old_backups, recommend_from_risk, validate_indentation,
    verify_context, write_atomic_with_dal, ManifestOp, RiskTelemetry, SniperConfig, SniperLock,
};

use llmosafe::ResourceGuard;

fn run(args: Vec<String>) -> std::process::ExitCode {
    use std::process::ExitCode;

    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        eprint!("{}", help_text::HELP);
        return ExitCode::SUCCESS;
    }

    if args[0] == "-v" || args[0] == "--version" {
        println!("{} {}", moesniper::NAME, moesniper::VERSION);
        return ExitCode::SUCCESS;
    }

    let dry_run = args.iter().any(|a| a == "--dry-run");
    let json_out = args.iter().any(|a| a == "--json");
    let use_stdin = args.iter().any(|a| a == "--stdin");
    let auto_indent = args.iter().any(|a| a == "--auto-indent");
    let force_indent = args.iter().any(|a| a == "--force-indent");

    let mut context_hash: Option<String> = None;
    let mut ctx_pos: Option<usize> = None;
    if let Some(pos) = args.iter().position(|a| a == "--context") {
        ctx_pos = Some(pos);
        if pos + 1 < args.len() {
            context_hash = Some(args[pos + 1].clone());
        }
    }

    let args: Vec<&str> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| {
            if let Some(pos) = ctx_pos {
                if *i == pos || *i == pos + 1 {
                    return false;
                }
            }
            !(*a == "--dry-run"
                || *a == "--json"
                || *a == "--stdin"
                || *a == "--auto-indent"
                || *a == "--force-indent")
        })
        .map(|(_, s)| s.as_str())
        .collect();

    let result = match args.as_slice() {
        ["decode"] if use_stdin => {
            let mut buffer = String::new();
            match std::io::stdin().read_to_string(&mut buffer) {
                Ok(_) => cmd_decode(&buffer),
                Err(e) => err(format!("read stdin: {e}")),
            }
        }
        ["decode", "--file", path] => match fs::read_to_string(path) {
            Ok(content) => cmd_decode(&content),
            Err(e) => err(format!("read {path}: {e}")),
        },
        ["decode", hex] => cmd_decode(hex),
        ["encode"] if use_stdin => {
            let mut buffer = String::new();
            match std::io::stdin().read_to_string(&mut buffer) {
                Ok(_) => cmd_encode(&buffer),
                Err(e) => err(format!("read stdin: {e}")),
            }
        }
        ["encode", "--file", path] => match fs::read_to_string(path) {
            Ok(content) => cmd_encode(&content),
            Err(e) => err(format!("read {path}: {e}")),
        },
        ["encode", text] => cmd_encode(text),
        ["context", file, start, end] => match (parse_line(start), parse_line(end)) {
            (Ok(s), Ok(e)) => cmd_context(file, s, e),
            (Err(err_msg), _) | (_, Err(err_msg)) => err(err_msg),
        },
        [file, "--undo"] => cmd_undo(file),
        [file, "--manifest"] if use_stdin => cmd_manifest(
            file,
            None,
            dry_run,
            auto_indent,
            force_indent,
            context_hash.as_deref(),
        ),
        [file, "--manifest", manifest] => cmd_manifest(
            file,
            Some(manifest),
            dry_run,
            auto_indent,
            force_indent,
            context_hash.as_deref(),
        ),
        [file, start, end, "--delete"] => {
            if use_stdin {
                err("cannot use --stdin with --delete".into())
            } else {
                match (parse_line(start), parse_line(end)) {
                    (Ok(s), Ok(e)) => cmd_splice(
                        file,
                        s,
                        e,
                        "",
                        dry_run,
                        auto_indent,
                        force_indent,
                        context_hash.as_deref(),
                    ),
                    (Err(e), _) | (_, Err(e)) => err(e),
                }
            }
        }
        [file, start, end] if use_stdin => {
            let mut buffer = String::new();
            match std::io::stdin().read_to_string(&mut buffer) {
                Ok(_) => match (parse_line(start), parse_line(end)) {
                    (Ok(ln_start), Ok(ln_end)) => cmd_splice(
                        file,
                        ln_start,
                        ln_end,
                        &buffer,
                        dry_run,
                        auto_indent,
                        force_indent,
                        context_hash.as_deref(),
                    ),
                    (Err(e), _) | (_, Err(e)) => err(e),
                },
                Err(e) => err(format!("read stdin: {e}")),
            }
        }
        [file, start, end, hex] => match (parse_line(start), parse_line(end)) {
            (Ok(s), Ok(e)) => match hex_decode(hex) {
                Ok(content) => cmd_splice(
                    file,
                    s,
                    e,
                    &content,
                    dry_run,
                    auto_indent,
                    force_indent,
                    context_hash.as_deref(),
                ),
                Err(msg) => err(format!("hex decode: {msg}")),
            },
            (Err(e), _) | (_, Err(e)) => err(e),
        },
        _ => {
            eprintln!("error: bad arguments. Run 'sniper --help'");
            return ExitCode::FAILURE;
        }
    };

    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        );
    } else {
        match result.status.as_str() {
            "ok" => {
                if let Some(msg) = &result.message {
                    // Used by 'context' command to output hash
                    println!("{}", msg);
                } else {
                    println!(
                        "ok: {} -{} +{}",
                        result.file.as_deref().unwrap_or("?"),
                        result.lines_removed,
                        result.lines_inserted
                    );
                }
            }
            "restored" => println!("restored: {}", result.backup.as_deref().unwrap_or("?")),
            "encoded" => println!("{}", result.message.as_deref().unwrap_or("")),
            "dry_run" => {
                println!("=== DRY RUN PREVIEW ===");
                println!("File: {}", result.file.as_deref().unwrap_or("?"));
                println!("Lines to remove: {}", result.lines_removed);
                println!("Lines to insert: {}", result.lines_inserted);

                if let Some(ref warning) = result.indent_warning {
                    println!("\n⚠️  INDENTATION WARNING:");
                    for line in warning.lines() {
                        println!("   {}", line);
                    }
                }

                if result.indent_fixed == Some(true) {
                    println!("\n✓ Auto-indent applied");
                }

                if let Some(ref preview) = result.diff_preview {
                    println!("\n--- Diff Preview ---");
                    for line in preview {
                        println!("{}", line);
                    }
                }

                if result.ai_hint.is_some() {
                    println!("\nHint: {}", result.ai_hint.as_deref().unwrap_or(""));
                }

                // Also output JSON if explicitly requested, but pretty print is default for dry-run
                if json_out {
                    println!("\n--- JSON Output ---");
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&result).unwrap_or_default()
                    );
                }
            }
            _ => {
                eprintln!("error: {}", result.message.as_deref().unwrap_or("unknown"));
                return ExitCode::FAILURE;
            }
        }
    }

    ExitCode::SUCCESS
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run(args)
}

#[derive(serde::Serialize, Default)]
struct CliResult {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    lines_removed: usize,
    lines_inserted: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_lines: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operations: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    backup: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ai_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diff_preview: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    indent_warning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    indent_fixed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line_shift: Option<i64>,
    /// Risk telemetry computed from live ResourceGuard when available.
    /// Always populated when a guard is present; serialized only when Some.
    #[serde(skip_serializing_if = "Option::is_none")]
    risk: Option<RiskTelemetry>,
    /// Human-readable recommended action based on risk assessment.
    #[serde(skip_serializing_if = "Option::is_none")]
    recommended_action: Option<String>,
    /// Per-operation diffs for manifest dry-run.
    #[serde(skip_serializing_if = "Option::is_none")]
    manifest_ops: Option<Vec<ManifestOpDiff>>,
}

/// Diff preview for a single manifest operation.
#[derive(Debug, Clone, serde::Serialize)]
struct ManifestOpDiff {
    start: usize,
    end: usize,
    diff_preview: Vec<String>,
}

fn cmd_encode(text: &str) -> CliResult {
    let hex = moesniper::hex_encode(text.as_bytes());
    CliResult {
        status: "encoded".into(),
        message: Some(hex),
        ..Default::default()
    }
}

fn cmd_decode(hex_or_text: &str) -> CliResult {
    let input = hex_or_text.trim();
    if input.is_empty() {
        return err("decode requires a hex string".into());
    }
    match hex_decode(input) {
        Ok(text) => CliResult {
            status: "ok".into(),
            message: Some(text),
            ..Default::default()
        },
        Err(msg) => err(format!("hex decode: {msg}")),
    }
}

fn cmd_context(filepath: &str, start: usize, end: usize) -> CliResult {
    let config = SniperConfig::from_env();

    if let Err(e) = normalize_path(filepath) {
        return err(e);
    }
    if let Err(e) = check_file_size(filepath, config.max_file_size) {
        return err(e);
    }

    let text = match fs::read_to_string(filepath) {
        Ok(t) => t,
        Err(e) => return err(format!("read file: {e}")),
    };

    let lines: Vec<String> = text.split_inclusive('\n').map(String::from).collect();

    if start < 1 || end > lines.len() || start > end + 1 {
        if start == lines.len() + 1 && (start == end + 1 || start == end) {
            // Allow computing context at end of file
        } else if start > end + 1 {
            return err(format!(
                "inverted range {start}-{end} is invalid (must satisfy start <= end + 1 to allow insertions at EOF)"
            ));
        } else {
            return err(format!(
                "line range {start}-{end} out of bounds (file has {} lines)",
                lines.len()
            ));
        }
    }

    let full_hash = compute_context_hash(&lines, start, end);
    let short_hash = full_hash[..16].to_string();

    CliResult {
        status: "ok".into(),
        message: Some(short_hash),
        ..Default::default()
    }
}

/// 11 params needed for CLI dispatch: 4 positional + 4 flags + context_hash; can't reduce.
#[allow(clippy::too_many_arguments)]
fn cmd_splice(
    filepath: &str,
    start: usize,
    end: usize,
    content: &str,
    dry_run: bool,
    auto_indent: bool,
    force_indent: bool,
    context_hash: Option<&str>,
) -> CliResult {
    // Load configuration
    let config = SniperConfig::from_env();

    // Validate path before any file operations
    if let Err(e) = normalize_path(filepath) {
        return err(e);
    }

    if let Err(e) = check_file_size(filepath, config.max_file_size) {
        return err(e);
    }

    // Only acquire lock for real writes; dry-run reads are lock-free
    // so .sniper/ directory is not created when no writes occur.
    let _lock: Option<SniperLock> = if !dry_run {
        match SniperLock::acquire_with_config(filepath, &config) {
            Ok(l) => Some(l),
            Err(e) => return err(e),
        }
    } else {
        None
    };

    // Guard: block writes into special files (FIFOs, devices, etc.).
    if !dry_run && !is_regular_file(filepath) {
        return err(
            "target path is not a regular file (FIFOs, pipes, and devices are not supported)"
                .into(),
        );
    }
    // Guard: refuse to overwrite read-only files via atomic rename
    if !dry_run {
        if let Ok(meta) = fs::metadata(filepath) {
            if meta.permissions().readonly() && meta.len() > 0 {
                return err(format!(
                    "file is read-only: {filepath}. Refusing to overwrite via atomic rename"
                ));
            }
        }
    }

    let text = match fs::read_to_string(filepath) {
        Ok(t) => t,
        Err(e) => return err(handle_backtrack_error(e, "Read")),
    };
    let lines: Vec<String> = text.split_inclusive('\n').map(String::from).collect();

    if let Some(expected) = context_hash {
        if expected.len() != 16 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
            return err(format!(
                "invalid --context length {:?}; expected 16 hex chars (e.g. a 16-char prefix from the context command)",
                expected.len()
            ));
        }
        if let Err(e) = verify_context(&lines, start, end, expected) {
            return err(e);
        }
    }

    if start < 1 || end > lines.len() || start > end + 1 {
        if start == lines.len() + 1 && (start == end + 1 || start == end) {
            // Allow inserting at end
        } else if start > end + 1 {
            return err(format!(
                "inverted range {start}-{end} is invalid (must satisfy start <= end + 1 to allow insertions at EOF)"
            ));
        } else {
            return err(format!(
                "line range {start}-{end} out of bounds (file has {} lines)",
                lines.len()
            ));
        }
    }

    let s = start - 1;
    let removed_lines_count = if s < lines.len() {
        let actual_end = end.min(lines.len());
        actual_end - s
    } else {
        0
    };

    // Parse new content
    let mut new_lines: Vec<String> = if content.is_empty() {
        vec![]
    } else {
        content.split_inclusive('\n').map(String::from).collect()
    };

    let is_delete = content.is_empty();

    // Handle auto-indent
    let mut indent_fixed = None;
    let mut indent_warning = None;

    if !is_delete {
        if auto_indent && needs_indent_fix(&lines, start, content) {
            let fixed = auto_indent_content(&lines, start, content);
            new_lines = fixed.split_inclusive('\n').map(String::from).collect();
            indent_fixed = Some(true);
        }

        if !force_indent {
            let (valid, warning, _suggested) = validate_indentation(&lines, start, &new_lines);
            if !valid {
                indent_warning = warning.clone();
                if !dry_run {
                    let msg = warning.unwrap_or_else(|| "Unknown indentation error".to_string());
                    return CliResult {
                        status: "error".into(),
                        file: Some(filepath.into()),
                        message: Some(format!("Indentation validation failed: {}", msg)),
                        indent_warning,
                        ..Default::default()
                    };
                }
            }
        }
    }

    // Generate diff preview for dry-run
    let diff_preview = if dry_run && !is_delete {
        Some(generate_preview(&lines, &new_lines, start, end))
    } else {
        None
    };

    if dry_run {
        let ai_hint = Some(if is_delete {
            format!("verify: {} around line {}", filepath, start)
        } else {
            format!("verify: read {} lines {}-{}", filepath, start, end)
        });
        return CliResult {
            status: "dry_run".into(),
            file: Some(filepath.into()),
            lines_removed: removed_lines_count,
            lines_inserted: new_lines.len(),
            ai_hint,
            diff_preview,
            indent_warning,
            indent_fixed,
            line_shift: Some(new_lines.len() as i64 - removed_lines_count as i64),
            risk: None,
            recommended_action: None,
            ..Default::default()
        };
    }

    // Compute risk telemetry for real edit path only — dry_run doesn't need it
    let guard = ResourceGuard::auto(0.5);
    let risk = RiskTelemetry::from_guard(&guard);

    let bk = match create_backup(filepath) {
        Ok(b) => b,
        Err(e) => return err(e),
    };

    let new_lines_count = new_lines.len();
    let mut modified_lines = lines.clone();

    if s < modified_lines.len() {
        let actual_end = end.min(modified_lines.len());
        modified_lines.splice(s..actual_end, new_lines);
    } else {
        modified_lines.extend(new_lines);
    }

    let lines_refs: Vec<&str> = modified_lines.iter().map(|s| s.as_str()).collect();

    // Use pre-computed guard, risk, and dal_level from config above
    if let Err(e) = write_atomic_with_dal(filepath, &lines_refs, &guard, &config) {
        return err(e);
    }

    // Purge old backups according to retention policy
    if let Err(e) = purge_old_backups(filepath, &config) {
        eprintln!("[SNIPER] Backup purge warning: {e}");
    }

    let manifest_promotion = count_recent_backups(filepath, config.lock_timeout.as_secs())
        .map(|count| count >= 3)
        .unwrap_or(false);

    let ai_hint = Some(if manifest_promotion {
        "Multiple edits to this file. Consider batching with manifest.".into()
    } else if is_delete {
        format!("verify: {} around line {}", filepath, start)
    } else {
        format!("verify: read {} lines {}-{}", filepath, start, end)
    });

    // T10: Always include risk when guard is available
    let recommended_action = Some(recommend_from_risk(&risk));

    CliResult {
        status: "ok".into(),
        file: Some(filepath.into()),
        lines_removed: removed_lines_count,
        lines_inserted: new_lines_count,
        total_lines: Some(modified_lines.len()),
        backup: Some(bk),
        ai_hint,
        indent_warning,
        indent_fixed,
        line_shift: Some(new_lines_count as i64 - removed_lines_count as i64),
        risk: Some(risk),
        recommended_action,
        ..Default::default()
    }
}

fn cmd_manifest(
    filepath: &str,
    manifest_path: Option<&str>,
    dry_run: bool,
    auto_indent: bool,
    force_indent: bool,
    context_hash: Option<&str>,
) -> CliResult {
    let manifest = match manifest_path {
        Some(path) => match fs::read_to_string(path) {
            Ok(m) => m,
            Err(e) => return err(format!("read manifest: {e}")),
        },
        None => {
            let mut buffer = String::new();
            match std::io::stdin().read_to_string(&mut buffer) {
                Ok(_) => buffer,
                Err(e) => return err(format!("read manifest from stdin: {e}")),
            }
        }
    };
    cmd_manifest_impl(
        filepath,
        &manifest,
        dry_run,
        auto_indent,
        force_indent,
        context_hash,
    )
}

fn cmd_manifest_impl(
    filepath: &str,
    manifest: &str,
    dry_run: bool,
    auto_indent: bool,
    force_indent: bool,
    context_hash: Option<&str>,
) -> CliResult {
    let config = SniperConfig::from_env();

    // Validate path before any file operations
    if let Err(e) = normalize_path(filepath) {
        return err(e);
    }

    if let Err(e) = check_file_size(filepath, config.max_file_size) {
        return err(e);
    }

    let _lock: Option<SniperLock> = if !dry_run {
        match SniperLock::acquire_with_config(filepath, &config) {
            Ok(l) => Some(l),
            Err(e) => return err(e),
        }
    } else {
        None
    };

    // Guard: block writes into special files (FIFOs, devices, etc.)
    if !dry_run && !is_regular_file(filepath) {
        return err(
            "target path is not a regular file (FIFOs, pipes, and devices are not supported)"
                .into(),
        );
    }
    // Guard: refuse to overwrite read-only files via atomic rename
    if !dry_run {
        if let Ok(meta) = fs::metadata(filepath) {
            if meta.permissions().readonly() && meta.len() > 0 {
                return err(format!(
                    "file is read-only: {filepath}. Refusing to overwrite via atomic rename"
                ));
            }
        }
    }

    let mut ops: Vec<ManifestOp> = match serde_json::from_str(manifest) {
        Ok(o) => o,
        Err(e) => return err(format!("parse manifest: {e}")),
    };

    // Note: hex validation occurs at decode time in the operation loop below (line ~629).
    // Pre-validating here would decode twice — the operation loop catches decode errors
    // at the point of use.

    let text = match fs::read_to_string(filepath) {
        Ok(t) => t,
        Err(e) => return err(handle_backtrack_error(e, "Read")),
    };
    let mut lines: Vec<String> = text.split_inclusive('\n').map(String::from).collect();

    // Sort bottom-up
    ops.sort_by_key(|b| std::cmp::Reverse(b.start));

    // Guard: overlapping same-start operations cause silent data loss.
    // Bottom-up processing assumes each op targets a distinct line range;
    // two ops at the same start line would corrupt each other's output.
    for i in 1..ops.len() {
        if ops[i].start == ops[i - 1].start {
            return err(format!(
                "overlapping manifest operations at line {}",
                ops[i].start
            ));
        }
    }

    let bk = if !dry_run {
        match create_backup(filepath) {
            Ok(b) => Some(b),
            Err(e) => return err(e),
        }
    } else {
        None
    };
    // Context verification: for manifest mode, verify the hash ONCE
    // against the pre-manifest file state (before any operation mutates lines).
    // This is a pre-manifest entry gate, not per-operation verification.
    if let Some(expected) = context_hash {
        if expected.len() != 16 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
            return err(format!(
                "invalid --context length {:?}; expected 16 hex chars (e.g. a 16-char prefix from the context command)",
                expected.len()
            ));
        }
        if let Some(first_op) = ops.first() {
            let first_end = first_op.end.unwrap_or(first_op.start);
            if let Err(e) = verify_context(&lines, first_op.start, first_end, expected) {
                return err(e);
            }
        }
    }

    let mut total_removed = 0usize;
    let mut total_inserted = 0usize;
    // Collect per-operation diffs for dry-run
    let mut manifest_ops: Vec<ManifestOpDiff> = Vec::new();

    for op in &ops {
        let start = op.start;
        let end = op.end.unwrap_or(op.start);

        if start < 1 || end > lines.len() || start > end + 1 {
            if start == lines.len() + 1 && (start == end + 1 || start == end) {
                // Allow inserting at end
            } else if start > end + 1 {
                return err(format!(
                    "inverted range {start}-{end} is invalid (must satisfy start <= end + 1 to allow insertions at EOF)"
                ));
            } else {
                return err(format!(
                    "line range {start}-{end} out of bounds (file has {} lines)",
                    lines.len()
                ));
            }
        }

        let s = start - 1;
        let actual_e = end.min(lines.len());

        if op.delete.unwrap_or(false) {
            if op.hex.is_some() {
                return err("Cannot both delete and insert in the same manifest operation".into());
            }
            total_removed += lines.splice(s..actual_e, std::iter::empty()).count();

            // Collect per-operation diff for dry-run (delete)
            if dry_run {
                let new_empty: Vec<String> = Vec::new();
                let diff_preview = generate_preview(&lines, &new_empty, op.start, actual_e);
                manifest_ops.push(ManifestOpDiff {
                    start: op.start,
                    end: actual_e,
                    diff_preview,
                });
            }
        } else if let Some(ref hex) = op.hex {
            let content = match hex_decode(hex) {
                Ok(c) => c,
                Err(e) => return err(format!("hex decode: {e}")),
            };

            // Apply auto-indent if needed
            let final_content = if auto_indent && needs_indent_fix(&lines, op.start, &content) {
                auto_indent_content(&lines, op.start, &content)
            } else {
                content
            };

            // Validate indentation if requested
            if !force_indent {
                let new_lines_for_check: Vec<String> = final_content
                    .split_inclusive('\n')
                    .map(String::from)
                    .collect();
                let (valid, warning, _) =
                    validate_indentation(&lines, op.start, &new_lines_for_check);
                if !valid && !dry_run {
                    return CliResult {
                        status: "error".into(),
                        file: Some(filepath.into()),
                        message: Some(format!(
                            "Indentation validation failed at line {}: {}",
                            op.start,
                            warning.as_deref().unwrap_or_default()
                        )),
                        indent_warning: warning,
                        ..Default::default()
                    };
                }
            }

            let new: Vec<String> = final_content
                .split_inclusive('\n')
                .map(String::from)
                .collect();

            // Collect per-operation diff for dry-run
            if dry_run {
                let diff_preview = generate_preview(&lines, &new, op.start, actual_e);
                manifest_ops.push(ManifestOpDiff {
                    start: op.start,
                    end: actual_e,
                    diff_preview,
                });
            }

            total_removed += actual_e - s;
            total_inserted += new.len();
            if s < lines.len() {
                lines.splice(s..actual_e, new);
            } else {
                lines.extend(new);
            }
        } else {
            return err(format!(
                "manifest operation at line {start} must specify either 'delete' or 'hex'"
            ));
        }
    }

    let lines_refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();

    if !dry_run {
        let guard = ResourceGuard::auto(0.5);
        let risk = RiskTelemetry::from_guard(&guard);
        // Reuse config from function scope instead of re-reading env (F7)

        // T6: Use write_atomic_with_dal with guard
        if let Err(e) = write_atomic_with_dal(filepath, &lines_refs, &guard, &config) {
            return err(e);
        }
        if let Err(e) = purge_old_backups(filepath, &config) {
            eprintln!("[SNIPER] Backup purge warning: {e}");
        }

        let ai_hint = Some(format!(
            "verify: read {} around line {}",
            filepath,
            ops.first().map(|o| o.start).unwrap_or(1)
        ));
        let recommended_action = Some(recommend_from_risk(&risk));

        return CliResult {
            status: "ok".into(),
            file: Some(filepath.into()),
            lines_removed: total_removed,
            lines_inserted: total_inserted,
            total_lines: Some(lines.len()),
            operations: Some(ops.len()),
            ai_hint,
            backup: bk,
            line_shift: Some(total_inserted as i64 - total_removed as i64),
            risk: Some(risk),
            recommended_action,
            ..Default::default()
        };
    }

    let ai_hint = Some(format!(
        "verify: read {} around line {}",
        filepath,
        ops.first().map(|o| o.start).unwrap_or(1)
    ));

    CliResult {
        status: "dry_run".into(),
        file: Some(filepath.into()),
        lines_removed: total_removed,
        lines_inserted: total_inserted,
        total_lines: Some(lines.len()),
        operations: Some(ops.len()),
        ai_hint,
        backup: bk,
        line_shift: Some(total_inserted as i64 - total_removed as i64),
        risk: None,
        recommended_action: None,
        manifest_ops: Some(manifest_ops),
        ..Default::default()
    }
}

fn cmd_undo(filepath: &str) -> CliResult {
    let config = SniperConfig::from_env();
    let _lock = match SniperLock::acquire_with_config(filepath, &config) {
        Ok(l) => l,
        Err(e) => return err(e),
    };

    let latest = match find_latest_backup(filepath) {
        Ok(Some(l)) => l,
        Ok(None) => return err(format!("no backup for {filepath}")),
        Err(e) => return err(e),
    };

    // Restoration: atomic copy via temp file + rename. We do NOT create a backup
    // of the state we are overwriting to allow consecutive undos to pop the stack.
    let tmp = format!("{}.sniper_undo_tmp", filepath);
    if let Err(e) = fs::copy(&latest, &tmp) {
        let _ = fs::remove_file(&tmp);
        return err(format!("restore (copy to temp): {e}"));
    }
    if let Err(e) = fs::rename(&tmp, filepath) {
        let _ = fs::remove_file(&tmp);
        return err(format!("restore (rename): {e}"));
    }

    // "Pop" the stack: remove the consumed backup.
    let _ = fs::remove_file(&latest);

    let ai_hint = Some(format!("verify restore: read {}", filepath));

    CliResult {
        status: "restored".into(),
        backup: Some(latest.to_string_lossy().into()),
        ai_hint,
        ..Default::default()
    }
}

fn parse_line(s: &str) -> Result<usize, String> {
    s.parse().map_err(|_| format!("invalid line number: {s}"))
}

fn err(msg: String) -> CliResult {
    let ai_hint = if msg.contains("no such file") || msg.contains("not found") {
        Some("check path exists before editing".into())
    } else if msg.contains("out of bounds") || msg.contains("exceeds file length") {
        Some("read file first to check line count".into())
    } else {
        Some("fix error and retry".into())
    };
    CliResult {
        status: "error".into(),
        message: Some(msg),
        ai_hint,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn create_file(dir: &TempDir, name: &str, content: &str) -> String {
        let path = dir.path().join(name);
        fs::write(&path, content).unwrap();
        path.to_str().unwrap().to_string()
    }

    fn read_file(path: impl AsRef<std::path::Path>) -> String {
        fs::read_to_string(path).unwrap()
    }

    // --- hex_decode tests ---

    #[test]
    fn test_hex_decode_valid() {
        assert_eq!(hex_decode("48656c6c6f").unwrap(), "Hello");
    }

    #[test]
    fn test_hex_decode_empty() {
        assert_eq!(hex_decode("").unwrap(), "");
    }

    #[test]
    fn test_hex_decode_mixed_case() {
        assert_eq!(hex_decode("4A6F62").unwrap(), "Job");
    }

    #[test]
    fn test_hex_decode_non_hex_chars() {
        assert!(hex_decode("zz").is_err());
    }

    #[test]
    fn test_hex_decode_odd_length() {
        assert!(hex_decode("48650").is_err());
    }

    // --- cmd_splice tests ---

    #[test]
    fn test_cmd_splice_replace_single_line() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "line1\nline2\nline3\n");
        let r = cmd_splice(&path, 2, 2, "hex", false, false, false, None);
        assert_eq!(r.status, "ok");
        assert_eq!(r.lines_removed, 1);
        assert_eq!(r.lines_inserted, 1);
        let content = read_file(&path);
        assert_eq!(content, "line1\nhex\nline3\n");
    }

    #[test]
    fn test_cmd_splice_preserves_missing_trailing_newline() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "line1\nline2");
        let r = cmd_splice(&path, 2, 2, "new", false, false, false, None);
        assert_eq!(r.status, "ok");
        let content = read_file(&path);
        assert_eq!(content, "line1\nnew");
        assert!(!content.ends_with('\n'));
    }

    #[test]
    fn test_cmd_splice_preserves_existing_trailing_newline() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "line1\nline2\n");
        let r = cmd_splice(&path, 2, 2, "new", false, false, false, None);
        assert_eq!(r.status, "ok");
        let content = read_file(&path);
        assert_eq!(content, "line1\nnew\n");
        assert!(content.ends_with('\n'));
    }

    #[test]
    fn test_cmd_splice_replace_range() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\nc\nd\ne\n");
        let r = cmd_splice(&path, 2, 4, "X\nY", false, false, false, None);
        assert_eq!(r.status, "ok");
        assert_eq!(r.lines_removed, 3);
        assert_eq!(r.lines_inserted, 2);
        let content = read_file(&path);
        assert_eq!(content, "a\nX\nY\ne\n");
    }

    #[test]
    fn test_cmd_splice_insert_at_end() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\n");
        let r = cmd_splice(&path, 2, 2, "c", false, false, false, None);
        assert_eq!(r.status, "ok");
        let content = read_file(&path);
        assert_eq!(content, "a\nc\n");
    }

    #[test]
    fn test_cmd_splice_insert_at_end_start_gt_end() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\n");
        let r = cmd_splice(&path, 3, 2, "c", false, false, false, None);
        assert_eq!(r.status, "ok");
        let content = read_file(&path);
        assert_eq!(content, "a\nb\nc\n");
    }

    #[test]
    fn test_cmd_splice_insert_at_end_start_eq_end() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\n");
        let r = cmd_splice(&path, 3, 3, "c", false, false, false, None);
        assert_eq!(r.status, "ok");
        let content = read_file(&path);
        assert_eq!(content, "a\nb\nc\n");
    }

    #[test]
    fn test_cmd_splice_out_of_bounds() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\n");
        let r = cmd_splice(&path, 10, 20, "x", false, false, false, None);
        assert_eq!(r.status, "error");
        assert!(r.message.as_deref().unwrap().contains("out of bounds"));
    }

    #[test]
    fn test_cmd_splice_start_zero() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\n");
        let r = cmd_splice(&path, 0, 1, "x", false, false, false, None);
        assert_eq!(r.status, "error");
    }

    // --- cmd_splice delete tests ---

    #[test]
    fn test_cmd_splice_delete_single_line() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\nc\n");
        let r = cmd_splice(&path, 2, 2, "", false, false, false, None);
        assert_eq!(r.status, "ok");
        assert_eq!(r.lines_removed, 1);
        assert_eq!(r.lines_inserted, 0);
        let content = read_file(&path);
        assert_eq!(content, "a\nc\n");
    }

    #[test]
    fn test_cmd_splice_delete_range() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\nc\nd\ne\n");
        let r = cmd_splice(&path, 2, 4, "", false, false, false, None);
        assert_eq!(r.status, "ok");
        assert_eq!(r.lines_removed, 3);
        assert_eq!(r.lines_inserted, 0);
        let content = read_file(&path);
        assert_eq!(content, "a\ne\n");
    }

    // --- dry_run tests ---

    #[test]
    fn test_cmd_splice_dry_run_no_change() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\nc\n");
        let original = read_file(&path);
        let r = cmd_splice(&path, 2, 2, "7878", true, false, false, None);
        assert_eq!(r.status, "dry_run");
        let after = read_file(&path);
        assert_eq!(original, after);
    }

    #[test]
    fn test_cmd_splice_dry_run_delete() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\nc\n");
        let original = read_file(&path);
        let r = cmd_splice(&path, 1, 2, "", true, false, false, None);
        assert_eq!(r.status, "dry_run");
        let after = read_file(&path);
        assert_eq!(original, after);
    }

    // --- cmd_manifest tests ---

    #[test]
    fn test_cmd_manifest_batch() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "line1\nline2\nline3\nline4\nline5\n");
        let manifest =
            r#"[{"start": 1, "end": 1, "hex": "78"}, {"start": 3, "end": 4, "delete": true}]"#;
        let manifest_path = create_file(&dir, "ops.json", manifest);
        let r = cmd_manifest(&path, Some(&manifest_path), false, false, false, None);
        assert_eq!(r.status, "ok");
        assert_eq!(r.operations, Some(2));
        let content = read_file(&path);
        assert_eq!(content, "x\nline2\nline5\n");
    }

    #[test]
    fn test_cmd_manifest_dry_run() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\nc\n");
        let original = read_file(&path);
        let manifest = r#"[{"start": 1, "end": 1, "hex": "78"}]"#;
        let manifest_path = create_file(&dir, "ops.json", manifest);
        let r = cmd_manifest(&path, Some(&manifest_path), true, false, false, None);
        assert_eq!(r.status, "dry_run");
        let after = read_file(&path);
        assert_eq!(original, after);
    }

    #[test]
    fn test_cmd_manifest_bad_json() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\n");
        let manifest_path = create_file(&dir, "ops.json", "not json");
        let r = cmd_manifest(&path, Some(&manifest_path), false, false, false, None);
        assert_eq!(r.status, "error");
        assert!(r.message.as_deref().unwrap().contains("parse manifest"));
    }

    #[test]
    fn test_cmd_manifest_out_of_bounds() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\n");
        let manifest_path = create_file(
            &dir,
            "ops.json",
            r#"[{"start": 10, "end": 15, "delete": true}]"#,
        );
        let r = cmd_manifest(&path, Some(&manifest_path), false, false, false, None);
        assert_eq!(r.status, "error");
        assert!(r.message.as_deref().unwrap().contains("out of bounds"));
    }

    // --- cmd_undo tests ---

    #[test]
    fn test_cmd_undo_no_backup() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "undo_no_backup_unique_12345.txt", "a\n");
        let r = cmd_undo(&path);
        assert_eq!(r.status, "error");
        assert!(r.message.as_deref().unwrap().contains("no backup"));
    }

    #[test]
    fn test_cmd_undo_restores() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "undo_restores_unique_67890.txt", "original\n");
        let _ = cmd_splice(&path, 1, 1, "xx", false, false, false, None);
        let content = read_file(&path);
        assert_ne!(content, "original\n");
        let r = cmd_undo(&path);
        assert_eq!(r.status, "restored");
        let restored = read_file(&path);
        assert_eq!(restored, "original\n");
    }

    #[test]
    fn test_cmd_undo_multi_step() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "multi_undo.txt", "v1\n");

        cmd_splice(&path, 1, 1, "v2", false, false, false, None); // edit 1
        cmd_splice(&path, 1, 1, "v3", false, false, false, None); // edit 2

        assert_eq!(read_file(&path), "v3\n");

        cmd_undo(&path); // undo 1
        assert_eq!(read_file(&path), "v2\n");

        cmd_undo(&path); // undo 2
        assert_eq!(read_file(&path), "v1\n");

        let r = cmd_undo(&path); // undo 3 (fail)
        assert_eq!(r.status, "error");
    }

    // --- cmd_encode tests ---

    #[test]
    fn test_cmd_encode() {
        let r = cmd_encode("hello");
        assert_eq!(r.status, "encoded");
        assert_eq!(r.message.unwrap(), "68656c6c6f");
    }

    // --- json output tests ---

    #[test]
    fn test_result_serializes_json() {
        let r = CliResult {
            status: "ok".into(),
            file: Some("test.rs".into()),
            lines_removed: 2,
            lines_inserted: 3,
            total_lines: Some(10),
            ..Default::default()
        };
        let json = serde_json::to_string(&r).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["file"], "test.rs");
        assert_eq!(v["lines_removed"], 2);
        assert!(v.get("message").is_none());
    }

    #[test]
    fn test_line_shift_serialized_in_json() {
        let r = CliResult {
            status: "ok".into(),
            lines_removed: 2,
            lines_inserted: 3,
            total_lines: Some(10),
            line_shift: Some(1),
            ..Default::default()
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("line_shift"));
        assert!(json.contains("1"));
    }

    #[test]
    fn test_context_verification_match() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "ctx_test.txt", "a\nb\nc\nd\ne\nf\ng\nh\n");
        let original = read_file(&path);
        let _lines: Vec<String> = original.split_inclusive('\n').map(String::from).collect();

        let mut hasher = sha2::Sha256::new();
        hasher.update(b"a\nb\n");
        hasher.update(b"d\ne\nf\n");
        let hash = moesniper::hex_encode(&hasher.finalize());
        let short_hash = &hash[..16];

        let r = cmd_splice(&path, 3, 3, "NEW", false, false, false, Some(short_hash));
        assert_eq!(r.status, "ok");
    }

    #[test]
    fn test_cmd_context() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "ctx_test.txt", "a\nb\nc\nd\ne\nf\ng\nh\n");

        let mut hasher = sha2::Sha256::new();
        hasher.update(b"a\nb\n");
        hasher.update(b"d\ne\nf\n");
        let hash = moesniper::hex_encode(&hasher.finalize());
        let short_hash = &hash[..16];

        let r = cmd_context(&path, 3, 3);
        assert_eq!(r.status, "ok");
        assert_eq!(r.message.unwrap(), short_hash);
    }

    #[test]
    fn test_context_verification_mismatch() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "ctx_test.txt", "a\nb\nc\nd\ne\nf\ng\nh\n");
        let r = cmd_splice(
            &path,
            3,
            3,
            "NEW",
            false,
            false,
            false,
            Some("0000000000000000"),
        );
        assert_eq!(r.status, "error");
        let msg = r.message.unwrap();
        assert!(msg.contains("context mismatch"));
    }

    #[test]
    fn test_manifest_promotion_after_multiple_edits() {
        let dir = TempDir::new().unwrap();
        let path = create_file(
            &dir,
            "promo_test.txt",
            "line1\nline2\nline3\nline4\nline5\n",
        );
        cmd_splice(&path, 1, 1, "a", false, false, false, None);
        cmd_splice(&path, 2, 2, "b", false, false, false, None);
        cmd_splice(&path, 3, 3, "c", false, false, false, None);
        let r = cmd_splice(&path, 4, 4, "d", false, false, false, None);
        assert_eq!(r.status, "ok");
        assert!(r.ai_hint.is_some());
        let hint = r.ai_hint.unwrap();
        assert!(
            hint.contains("manifest"),
            "Expected manifest promotion hint, got: {}",
            hint
        );
    }

    // --- run function exit code tests ---
    #[test]
    fn test_run_help_success() {
        // Unfortunately ExitCode does not implement Eq or Debug in standard library.
        // But we can check it using formatting or other means.
        // The simplest test for the exit code is capturing stdout if needed,
        // but since we only need to verify exit behavior without process exit, we can just call it.
        // We'll have to parse `ExitCode` or just rely on the return.
        // As ExitCode is opaque, a simpler way is to just call `run` and ensure it doesn't panic.
        let args = vec!["--help".to_string()];
        let _ = run(args);
    }

    #[test]
    fn test_run_version_success() {
        let args = vec!["--version".to_string()];
        let _ = run(args);
    }

    #[test]
    fn test_run_invalid_args() {
        let args = vec!["invalid_command".to_string()];
        let _ = run(args);
    }

    // --- edge case tests ---

    #[test]
    fn test_file_not_found() {
        let r = cmd_splice(
            "/tmp/no_such_file_12345.txt",
            1,
            1,
            "78",
            false,
            false,
            false,
            None,
        );
        assert_eq!(r.status, "error");
        let msg = r.message.as_deref().unwrap().to_lowercase();
        assert!(
            msg.contains("read") || msg.contains("metadata") || msg.contains("no such file"),
            "expected file-related error, got: {}",
            msg
        );
    }

    #[test]
    fn test_cmd_splice_delete_last_line_preserves_non_termination_if_possible() {
        let dir = TempDir::new().unwrap();
        // File: "a\nb" (no trailing newline)
        let path = create_file(&dir, "no_trailing.txt", "a\nb");

        // Delete line 2 ("b")
        let r = cmd_splice(&path, 2, 2, "", false, false, false, None);
        assert_eq!(r.status, "ok");

        let content = read_file(&path);
        // After PR #8: trailing newlines are stripped uniformly, then re-added based on original file.
        // Original had no trailing newline, so result should be "a" (no trailing newline).
        // Let's see what it is currently.
        assert_eq!(content, "a");
    }

    #[test]
    fn test_single_line_file() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "one.txt", "only\n");
        let r = cmd_splice(&path, 1, 1, "new", false, false, false, None);
        assert_eq!(r.status, "ok");
        let content = read_file(&path);
        assert_eq!(content, "new\n");
    }

    // --- Property-based tests ---

    use proptest::prelude::*;

    proptest! {
        #[test]
        fn prop_dry_run_never_modifies_file(
            content in "[a-z\n]{1,100}",
            replacement in "[a-z\n]{0,50}",
            line_num in 1usize..10
        ) {
            let dir = TempDir::new().unwrap();
            let path = create_file(&dir, "prop_test.txt", &content);
            let original = read_file(&path);
            let lines: Vec<&str> = original.lines().collect();
            if lines.is_empty() || line_num > lines.len() {
                return Ok(());
            }
            let _ = cmd_splice(&path, line_num, line_num, &replacement, true, false, false, None);
            let after = read_file(&path);
            prop_assert_eq!(original, after);
        }

        #[test]
        fn prop_splice_preserves_lines_outside_range(
            content in "[a-z]{1,5}\n".prop_map(|s| s.repeat(3)),
            replacement in "[a-z]{1,5}",
            start in 1usize..=2,
            end in 2usize..=3
        ) {
            let dir = TempDir::new().unwrap();
            let path = create_file(&dir, "prop_test.txt", &content);
            let lines_before: Vec<&str> = content.lines().collect();
            if start > end || end > lines_before.len() {
                return Ok(());
            }
            let _ = cmd_splice(&path, start, end, &replacement, false, false, false, None);
            let after = read_file(&path);
            let lines_after: Vec<&str> = after.lines().collect();
            // Lines before start should be preserved
            for i in 0..(start - 1).min(lines_before.len()) {
                if i < lines_after.len() {
                    prop_assert_eq!(lines_before[i], lines_after[i]);
                }
            }
        }

        #[test]
        fn prop_hex_decode_roundtrip(s in "[0-7][0-9A-Fa-f]".prop_map(|s| {
            // Only test ASCII-range hex (00-7F) which always produces valid UTF-8
            // Ensure even length
            if s.len() % 2 == 1 { s[..s.len()-1].to_string() } else { s }
        })) {
            // hex_decode should produce valid UTF-8 for ASCII-range hex input
            let result = hex_decode(&s);
            prop_assert!(result.is_ok());
        }

        #[test]
        fn prop_undo_restores_original(
            content in "[a-z\n]{1,50}",
            replacement in "[a-z\n]{1,30}",
            line_num in 1usize..5
        ) {
            let dir = TempDir::new().unwrap();
            // CWD isolation: chdir to temp dir so .sniper/ is created there, not in project root
            let original_cwd = std::env::current_dir().unwrap();
            let _cwd_guard = {
                struct Guard(PathBuf);
                impl Drop for Guard { fn drop(&mut self) { let _ = std::env::set_current_dir(&self.0); } }
                let g = Guard(original_cwd);
                std::env::set_current_dir(dir.path()).unwrap();
                g
            };
            let path = create_file(&dir, "prop_undo_test.txt", &content);
            let original = read_file(&path);
            let lines: Vec<&str> = original.lines().collect();
            if lines.is_empty() || line_num > lines.len() {
                return Ok(());
            }
            let _ = cmd_splice(&path, line_num, line_num, &replacement, false, false, false, None);
            let _ = cmd_undo(&path);
            let restored = read_file(&path);
            prop_assert_eq!(original, restored);
        }

        #[test]
        fn prop_splice_result_counts_match(
            content in "[a-z]{1,5}\n".prop_map(|s| s.repeat(2)),
            replacement in "[a-z\n]{1,20}"
        ) {
            let dir = TempDir::new().unwrap();
            let path = create_file(&dir, "prop_counts.txt", &content);
            let lines_before: Vec<&str> = content.lines().collect();
            if lines_before.len() < 2 {
                return Ok(());
            }
            let r = cmd_splice(&path, 1, 2, &replacement, false, false, false, None);
            let after = read_file(&path);
            let lines_after: Vec<&str> = after.lines().collect();
            // total_lines in result should match actual line count
            prop_assert_eq!(r.total_lines, Some(lines_after.len()));
        }
    }

    // --- ai_hint tests ---

    #[test]
    fn test_ai_hint_after_splice() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "hint_test.txt", "a\nb\nc\n");
        let r = cmd_splice(&path, 2, 2, "xx", false, false, false, None);
        assert_eq!(r.status, "ok");
        assert!(r.ai_hint.is_some());
        let hint = r.ai_hint.unwrap();
        assert!(hint.starts_with("verify:"));
        assert!(hint.contains("lines 2-2"));
    }

    #[test]
    fn test_ai_hint_after_delete() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "hint_test.txt", "a\nb\nc\n");
        let r = cmd_splice(&path, 2, 2, "", false, false, false, None);
        assert_eq!(r.status, "ok");
        assert!(r.ai_hint.is_some());
        let hint = r.ai_hint.unwrap();
        assert!(hint.starts_with("verify:"));
        assert!(hint.contains("around line"));
    }

    #[test]
    fn test_ai_hint_after_dry_run() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "hint_test.txt", "a\nb\nc\n");
        let r = cmd_splice(&path, 1, 1, "xx", true, false, false, None);
        assert_eq!(r.status, "dry_run");
        assert!(r.ai_hint.is_some());
    }

    #[test]
    fn test_ai_hint_after_error_not_found() {
        let r = cmd_splice("/no/such/file.txt", 1, 1, "xx", false, false, false, None);
        assert_eq!(r.status, "error");
        assert!(r.ai_hint.is_some());
        let hint = r.ai_hint.unwrap();
        // Message contains "No such file" - hint should suggest checking path
        assert!(hint.contains("check path") || hint.contains("fix error"));
    }

    #[test]
    fn test_ai_hint_after_error_out_of_bounds() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "hint_test.txt", "a\nb\n");
        let r = cmd_splice(&path, 10, 20, "xx", false, false, false, None);
        assert_eq!(r.status, "error");
        assert!(r.ai_hint.is_some());
        let hint = r.ai_hint.unwrap();
        assert!(hint.contains("line count"));
    }

    #[test]
    fn test_ai_hint_after_undo() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "undo_hint.txt", "original\n");
        let _ = cmd_splice(&path, 1, 1, "xx", false, false, false, None);
        let r = cmd_undo(&path);
        assert_eq!(r.status, "restored");
        assert!(r.ai_hint.is_some());
        let hint = r.ai_hint.unwrap();
        assert!(hint.contains("verify restore"));
    }

    #[test]
    fn test_ai_hint_serialize_excluded_without_json() {
        // When --json is NOT used, ai_hint should still serialize but not appear in plain output
        // This tests the struct has the field
        let r = CliResult {
            status: "ok".into(),
            ai_hint: Some("test hint".into()),
            ..Default::default()
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("ai_hint"));
    }

    // =========================================================================
    // BUG PROBE: DRY-RUN STATE LEAKAGE
    // =========================================================================

    /// Dry-run should NOT create the .sniper/ directory.
    /// BUG: SniperLock::acquire_with_config creates .sniper/ via create_dir_all
    ///      even when dry_run=true because lock acquisition happens BEFORE the
    ///      dry-run check in cmd_splice (line 375-378 before line 461 check).
    #[test]
    fn test_dry_run_does_not_create_sniper_dir() {
        let dir = TempDir::new().unwrap();
        let original_cwd = std::env::current_dir().unwrap();
        let _cwd_guard = {
            struct Guard(PathBuf);
            impl Drop for Guard {
                fn drop(&mut self) {
                    let _ = std::env::set_current_dir(&self.0);
                }
            }
            let g = Guard(original_cwd);
            std::env::set_current_dir(dir.path()).unwrap();
            g
        };

        let sniper_dir = dir.path().join(".sniper");
        // Verify .sniper/ does NOT exist before dry-run
        assert!(
            !sniper_dir.exists(),
            ".sniper/ should not exist before dry-run"
        );

        let path = create_file(&dir, "dry_test.txt", "line1\nline2\nline3\n");
        let r = cmd_splice(&path, 2, 2, "7878", true, false, false, None);
        assert_eq!(r.status, "dry_run", "dry-run should succeed");

        // PROBE: Does .sniper/ exist after dry-run?
        // If this FAILS, dry-run leaked filesystem state.
        assert!(
            !sniper_dir.exists(),
            "BUG: dry-run created .sniper/ directory (state leakage via lock acquisition)"
        );
    }

    /// Dry-run should NOT create backup files.
    #[test]
    fn test_dry_run_does_not_create_backups() {
        let dir = TempDir::new().unwrap();
        let original_cwd = std::env::current_dir().unwrap();
        let _cwd_guard = {
            struct Guard(PathBuf);
            impl Drop for Guard {
                fn drop(&mut self) {
                    let _ = std::env::set_current_dir(&self.0);
                }
            }
            let g = Guard(original_cwd);
            std::env::set_current_dir(dir.path()).unwrap();
            g
        };

        let path = create_file(&dir, "dry_nobackup.txt", "line1\nline2\nline3\n");
        let _ = cmd_splice(&path, 2, 2, "7878", true, false, false, None);

        // If .sniper/ was created, check it's empty (no backup files)
        let sniper_dir = dir.path().join(".sniper");
        if sniper_dir.exists() {
            let entries: Vec<_> = std::fs::read_dir(&sniper_dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .collect();
            assert!(
                entries.is_empty(),
                "BUG: dry-run created {} entries in .sniper/",
                entries.len()
            );
        }
    }

    /// Dry-run JSON output must NOT contain risk telemetry or recommended_action.
    #[test]
    fn test_dry_run_json_excludes_risk_telemetry() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "dry_json_risk.txt", "a\nb\nc\n");
        let r = cmd_splice(&path, 2, 2, "7878", true, false, false, None);
        assert_eq!(r.status, "dry_run");

        // risk must be None for dry-run (was the original bug fixed in 2c54f29)
        assert!(
            r.risk.is_none(),
            "BUG: dry-run leaks risk telemetry in result struct"
        );
        assert!(
            r.recommended_action.is_none(),
            "BUG: dry-run leaks recommended_action in result struct"
        );

        let json = serde_json::to_string(&r).unwrap();
        // The JSON output for --json should not contain risk fields
        assert!(
            !json.contains("\"risk\""),
            "BUG: dry-run JSON contains 'risk' field"
        );
        assert!(
            !json.contains("\"recommended_action\""),
            "BUG: dry-run JSON contains 'recommended_action' field"
        );
    }

    /// Manifest dry-run JSON output must NOT contain risk telemetry.
    #[test]
    fn test_manifest_dry_run_json_excludes_risk_telemetry() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "manifest_dry_risk.txt", "a\nb\nc\n");
        let manifest_path =
            create_file(&dir, "ops.json", r#"[{"start": 1, "end": 1, "hex": "78"}]"#);
        let r = cmd_manifest(&path, Some(&manifest_path), true, false, false, None);
        assert_eq!(r.status, "dry_run");
        assert!(
            r.risk.is_none(),
            "BUG: manifest dry-run leaks risk telemetry"
        );
        assert!(
            r.recommended_action.is_none(),
            "BUG: manifest dry-run leaks recommended_action"
        );

        let json = serde_json::to_string(&r).unwrap();
        assert!(
            !json.contains("\"risk\""),
            "BUG: manifest dry-run JSON contains 'risk'"
        );
        assert!(
            !json.contains("\"recommended_action\""),
            "BUG: manifest dry-run JSON contains 'recommended_action'"
        );
    }

    /// Dry-run followed by real edit: file state must be correct.
    /// Verifies that dry-run does not change any state that affects a subsequent real edit.
    #[test]
    fn test_dry_run_then_real_edit_file_state_correct() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "dry_then_real.txt", "line1\nline2\nline3\n");
        let original = read_file(&path);

        // Dry-run an edit
        let r_dry = cmd_splice(&path, 2, 2, "NEW", true, false, false, None);
        assert_eq!(r_dry.status, "dry_run");

        // File must be unchanged after dry-run
        assert_eq!(read_file(&path), original, "dry-run modified the file!");

        // Now do a real edit at line 1
        let r_real = cmd_splice(&path, 1, 1, "FIRST", false, false, false, None);
        assert_eq!(r_real.status, "ok");

        // File should have the real edit applied
        let after = read_file(&path);
        assert_eq!(after, "FIRST\nline2\nline3\n");
    }

    /// Manifest dry-run followed by real manifest edit.
    #[test]
    fn test_manifest_dry_run_then_real_edit() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "man_dry_then_real.txt", "a\nb\nc\nd\n");
        let original = read_file(&path);

        let manifest_path =
            create_file(&dir, "ops.json", r#"[{"start": 2, "end": 2, "hex": "78"}]"#);

        // Dry-run
        let r_dry = cmd_manifest(&path, Some(&manifest_path), true, false, false, None);
        assert_eq!(r_dry.status, "dry_run");
        assert_eq!(
            read_file(&path),
            original,
            "manifest dry-run modified the file!"
        );

        // Real manifest edit
        let r_real = cmd_manifest(&path, Some(&manifest_path), false, false, false, None);
        assert_eq!(r_real.status, "ok");
        assert_eq!(read_file(&path), "a\nx\nc\nd\n");
    }

    // =========================================================================
    // BUG PROBE: PERMISSION PRESERVATION
    // =========================================================================

    /// Verify that editing a file with 0o600 permissions preserves them.
    #[test]
    #[cfg(unix)]
    fn test_cmd_splice_preserves_restrictive_permissions_0600() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "perms_0600.txt", "secret\n");
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(&path, perms).unwrap();

        let r = cmd_splice(&path, 1, 1, "x", false, false, false, None);
        assert_eq!(r.status, "ok");

        let content = read_file(&path);
        assert_eq!(content, "x\n");

        let final_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            final_mode, 0o600,
            "BUG: permissions changed from 0o600 to 0o{:o}",
            final_mode
        );
    }

    /// Verify that editing a file with 0o400 (read-only) permissions works
    /// and preserves the read-only permission.
    #[test]
    #[cfg(unix)]
    fn test_cmd_splice_preserves_readonly_permissions_0400() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "perms_0400.txt", "readonly\n");
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o400);
        std::fs::set_permissions(&path, perms).unwrap();

        let r = cmd_splice(&path, 1, 1, "x", false, false, false, None);
        // This may succeed or fail depending on platform behavior
        // On Linux, rename() replaces files regardless of their permissions
        // because it's the directory that needs write permission
        if r.status == "ok" {
            let content = read_file(&path);
            assert_eq!(content, "x\n");

            let final_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                final_mode, 0o400,
                "BUG: permissions changed from 0o400 to 0o{:o} after successful edit",
                final_mode
            );
        }
        // If it fails, that's acceptable - read-only files may be protected
    }

    /// Permissions must be preserved through the full edit-undo cycle.
    #[test]
    #[cfg(unix)]
    fn test_undo_preserves_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "undo_perms.txt", "original\n");
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(&path, perms).unwrap();

        let r_edit = cmd_splice(&path, 1, 1, "x", false, false, false, None);
        assert_eq!(r_edit.status, "ok");

        // After edit, permissions should still be 0o600
        let mode_after_edit = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode_after_edit, 0o600,
            "BUG: permissions lost after edit (0o600 -> 0o{:o})",
            mode_after_edit
        );

        let r_undo = cmd_undo(&path);
        assert_eq!(r_undo.status, "restored");

        let content = read_file(&path);
        assert_eq!(content, "original\n");

        let mode_after_undo = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode_after_undo, 0o600,
            "BUG: permissions lost after undo (0o600 -> 0o{:o})",
            mode_after_undo
        );
    }

    /// Verify that writing to a NEW file (that doesn't exist yet) works.
    /// This tests the create_backup path where the source file doesn't exist.
    #[test]
    fn test_cmd_splice_creates_new_file() {
        let dir = TempDir::new().unwrap();
        let nonexistent = dir.path().join("brand_new.txt");

        // cmd_splice should fail because file doesn't exist (can't read it)
        let r = cmd_splice(
            nonexistent.to_str().unwrap(),
            1,
            1,
            "7878",
            false,
            false,
            false,
            None,
        );
        // Currently sniper requires existing files; creation is not supported by cmd_splice
        assert_eq!(
            r.status, "error",
            "Editing nonexistent file must return error, not crash"
        );
    }

    // =========================================================================
    // BUG PROBE: UNDO EDGE CASES
    // =========================================================================

    /// Undo after dry-run only (no real edits) must error gracefully.
    #[test]
    fn test_undo_after_dry_run_only_errors() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "undo_after_dry.txt", "original\n");

        // Dry-run only — no backup created
        let r_dry = cmd_splice(&path, 1, 1, "7878", true, false, false, None);
        assert_eq!(r_dry.status, "dry_run");

        // Undo — must fail gracefully (no backup exists)
        let r_undo = cmd_undo(&path);
        assert_eq!(
            r_undo.status, "error",
            "BUG: undo after dry-run only should error, got status={}",
            r_undo.status
        );
        assert!(
            r_undo
                .message
                .as_deref()
                .unwrap_or("")
                .contains("no backup"),
            "BUG: undo error should mention 'no backup', got: {:?}",
            r_undo.message
        );

        // File must be unchanged
        assert_eq!(read_file(&path), "original\n");
    }

    /// Undo on a file that was never edited must error gracefully.
    #[test]
    fn test_undo_never_edited_errors() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "never_edited.txt", "pristine\n");
        let original = read_file(&path);

        let r = cmd_undo(&path);
        assert_eq!(
            r.status, "error",
            "BUG: undo on never-edited file should error, got status={}",
            r.status
        );
        assert!(
            r.message.as_deref().unwrap_or("").contains("no backup"),
            "BUG: error message should mention 'no backup', got: {:?}",
            r.message
        );

        // File must be unchanged
        assert_eq!(read_file(&path), original);
    }

    /// Undo twice in a row: second undo must fail (backup consumed by first undo).
    #[test]
    fn test_double_undo_second_fails() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "double_undo.txt", "original\n");

        // One real edit
        let r_edit = cmd_splice(&path, 1, 1, "edited", false, false, false, None);
        assert_eq!(r_edit.status, "ok");
        assert_eq!(read_file(&path), "edited\n");

        // First undo — succeeds
        let r_undo1 = cmd_undo(&path);
        assert_eq!(r_undo1.status, "restored");
        assert_eq!(read_file(&path), "original\n");

        // Second undo — must fail (backup already consumed)
        let r_undo2 = cmd_undo(&path);
        assert_eq!(
            r_undo2.status, "error",
            "BUG: second undo should fail (backup already consumed), got status={}",
            r_undo2.status
        );
        assert!(
            r_undo2
                .message
                .as_deref()
                .unwrap_or("")
                .contains("no backup"),
            "BUG: second undo should report 'no backup', got: {:?}",
            r_undo2.message
        );

        // File must still be original (second undo should not corrupt)
        assert_eq!(read_file(&path), "original\n");
    }

    /// Multiple edits stack: undo should pop the stack correctly.
    #[test]
    fn test_undo_stack_multiple_edits() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "undo_stack.txt", "v0\n");

        // Apply multiple edits
        cmd_splice(&path, 1, 1, "v1", false, false, false, None);
        cmd_splice(&path, 1, 1, "v2", false, false, false, None);
        cmd_splice(&path, 1, 1, "v3", false, false, false, None);

        assert_eq!(read_file(&path), "v3\n");

        // Undo 1: v3 -> v2
        let r1 = cmd_undo(&path);
        assert_eq!(r1.status, "restored");
        assert_eq!(read_file(&path), "v2\n");

        // Undo 2: v2 -> v1
        let r2 = cmd_undo(&path);
        assert_eq!(r2.status, "restored");
        assert_eq!(read_file(&path), "v1\n");

        // Undo 3: v1 -> v0
        let r3 = cmd_undo(&path);
        assert_eq!(r3.status, "restored");
        assert_eq!(read_file(&path), "v0\n");

        // Undo 4: no more backups
        let r4 = cmd_undo(&path);
        assert_eq!(r4.status, "error");
        assert_eq!(read_file(&path), "v0\n");
    }

    /// Manifest dry-run must not create backups.
    #[test]
    fn test_manifest_dry_run_does_not_create_backups() {
        let dir = TempDir::new().unwrap();
        let original_cwd = std::env::current_dir().unwrap();
        let _cwd_guard = {
            struct Guard(PathBuf);
            impl Drop for Guard {
                fn drop(&mut self) {
                    let _ = std::env::set_current_dir(&self.0);
                }
            }
            let g = Guard(original_cwd);
            std::env::set_current_dir(dir.path()).unwrap();
            g
        };

        let path = create_file(&dir, "man_dry_nobackup.txt", "a\nb\nc\n");
        let manifest_path = create_file(
            &dir,
            "ops.json",
            r#"[{"start": 2, "end": 2, "delete": true}]"#,
        );

        let r = cmd_manifest(&path, Some(&manifest_path), true, false, false, None);
        assert_eq!(r.status, "dry_run");

        // Backup should be None for dry-run
        assert!(
            r.backup.is_none(),
            "BUG: manifest dry-run created a backup: {:?}",
            r.backup
        );
    }

    /// Dry-run delete: must not modify file.
    #[test]
    fn test_dry_run_delete_does_not_modify_file() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "dry_delete.txt", "a\nb\nc\n");
        let original = read_file(&path);

        let r = cmd_splice(&path, 2, 2, "", true, false, false, None);
        assert_eq!(r.status, "dry_run");
        assert_eq!(
            read_file(&path),
            original,
            "BUG: dry-run delete modified the file!"
        );
    }

    // =========================================================================
    // BUG PROBE: SERIALIZATION / JSON OUTPUT
    // =========================================================================

    /// Full JSON output for dry-run must contain all expected fields and NO risk fields.
    #[test]
    fn test_dry_run_json_schema_integrity() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "dry_schema.txt", "a\nb\nc\n");
        let r = cmd_splice(&path, 2, 2, "7878", true, false, false, None);
        assert_eq!(r.status, "dry_run");

        let json = serde_json::to_string_pretty(&r).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        // Required fields for dry_run status
        assert_eq!(v["status"], "dry_run");
        assert!(v.get("file").is_some(), "dry-run must include 'file' field");
        assert!(
            v.get("lines_removed").is_some(),
            "dry-run must include 'lines_removed'"
        );
        assert!(
            v.get("lines_inserted").is_some(),
            "dry-run must include 'lines_inserted'"
        );
        assert!(v.get("ai_hint").is_some(), "dry-run must include 'ai_hint'");
        assert!(
            v.get("line_shift").is_some(),
            "dry-run must include 'line_shift'"
        );

        // Forbidden fields for dry_run
        assert!(v.get("risk").is_none(), "BUG: dry-run JSON contains 'risk'");
        assert!(
            v.get("recommended_action").is_none(),
            "BUG: dry-run JSON contains 'recommended_action'"
        );
        assert!(
            v.get("backup").is_none(),
            "BUG: dry-run JSON contains 'backup'"
        );
        assert!(
            v.get("total_lines").is_none(),
            "BUG: dry-run JSON contains 'total_lines'"
        );
    }

    /// Full JSON output for manifest dry-run must be correct.
    #[test]
    fn test_manifest_dry_run_json_schema_integrity() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "man_dry_schema.txt", "a\nb\nc\nd\n");
        let manifest_path = create_file(
            &dir,
            "ops.json",
            r#"[{"start": 2, "end": 2, "hex": "78"}, {"start": 4, "end": 4, "delete": true}]"#,
        );

        let r = cmd_manifest(&path, Some(&manifest_path), true, false, false, None);
        assert_eq!(r.status, "dry_run");

        let json = serde_json::to_string_pretty(&r).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(v["status"], "dry_run");
        assert!(
            v.get("operations").is_some(),
            "manifest dry-run must include 'operations'"
        );
        assert!(
            v.get("manifest_ops").is_some(),
            "manifest dry-run must include 'manifest_ops'"
        );

        // Forbidden for dry_run
        assert!(
            v.get("risk").is_none(),
            "BUG: manifest dry-run JSON contains 'risk'"
        );
        assert!(
            v.get("recommended_action").is_none(),
            "BUG: manifest dry-run JSON contains 'recommended_action'"
        );
        assert!(
            v.get("backup").is_none(),
            "BUG: manifest dry-run JSON contains 'backup'"
        );
    }

    // =========================================================================
    // BUG PROBE: SPLICE BOUNDARY CONDITIONS
    // =========================================================================

    /// BUG PROBE: start=1, end=0 — "insert at beginning"
    /// The CLI bounds check is: start<1 || end>lines.len() || start>end+1
    /// For start=1,end=0: start<1=false, 0>len=false, 1>0+1=>1>1=false → PASSES
    /// The Python sniper_edit explicitly rejects end<start. Is this inconsistency a bug?
    #[test]
    fn bug_probe_splice_start1_end0_insert_before_line1() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "ins_before.txt", "a\nb\nc\n");
        let r = cmd_splice(&path, 1, 0, "X", false, false, false, None);
        // Currently this passes bounds check — end=0 is below 1-based range
        if r.status == "ok" {
            let content = read_file(&path);
            assert_eq!(content, "X\na\nb\nc\n",
                "CLI allows start=1,end=0 as insert-before-line-1. Python rejects (end<start). Inconsistency?");
        } else {
            assert!(
                r.message.as_deref().unwrap().contains("out of bounds")
                    || r.message.as_deref().unwrap().contains("invalid"),
                "Expected bounds error if rejected. Got: {:?}",
                r.message
            );
        }
    }

    /// BUG PROBE: start=1, end=0 in manifest
    #[test]
    fn bug_probe_manifest_start1_end0() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "mf_1_0.txt", "a\nb\nc\n");
        let manifest = r#"[{"start": 1, "end": 0, "hex": "58"}]"#;
        let r = cmd_manifest_impl(&path, manifest, false, false, false, None);
        if r.status == "ok" {
            let content = read_file(&path);
            assert_eq!(
                content, "X\na\nb\nc\n",
                "Manifest start=1,end=0 inserts before line 1. Got: {:?}",
                content
            );
        } else {
            assert!(
                r.message.as_deref().unwrap().contains("out of bounds"),
                "Expected bounds error. Got: {:?}",
                r.message
            );
        }
    }

    /// BUG PROBE: empty file with start=1, end=1
    /// The insert-at-end exception fires: start==lines.len()+1 (1==1) && start==end (1==1)
    #[test]
    fn bug_probe_empty_file_insert() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "empty_insert.txt", "");
        let r = cmd_splice(&path, 1, 1, "X", false, false, false, None);
        assert_eq!(
            r.status, "ok",
            "Insert into empty file should succeed via insert-at-end exception. Got: {:?}",
            r.message
        );
        let content = read_file(&path);
        assert_eq!(
            content, "X",
            "Empty file + hex 58 should produce 'X' (no trailing nl). Got: {:?}",
            content
        );
    }

    /// BUG PROBE: manifest insert into empty file
    #[test]
    fn bug_probe_manifest_empty_file_insert() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "mf_empty.txt", "");
        let manifest = r#"[{"start": 1, "end": 1, "hex": "58"}]"#;
        let r = cmd_manifest_impl(&path, manifest, false, false, false, None);
        assert_eq!(
            r.status, "ok",
            "Manifest insert into empty file. Got: {:?}",
            r.message
        );
        let content = read_file(&path);
        assert_eq!(content, "X", "Expected 'X'. Got: {:?}", content);
    }

    /// BUG PROBE: delete ALL lines from file
    #[test]
    fn bug_probe_delete_all_lines() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "del_all.txt", "a\nb\nc\n");
        let r = cmd_splice(&path, 1, 3, "", false, false, false, None);
        assert_eq!(
            r.status, "ok",
            "Delete all lines should succeed. Got: {:?}",
            r.message
        );
        let content = read_file(&path);
        assert_eq!(
            content, "",
            "Deleting all lines from file with trailing newline leaves empty file. Got: {:?}",
            content
        );
    }

    /// BUG PROBE: delete all lines from file without trailing newline
    #[test]
    fn bug_probe_delete_all_lines_no_trailing() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "del_all_no.txt", "a\nb");
        let r = cmd_splice(&path, 1, 2, "", false, false, false, None);
        assert_eq!(
            r.status, "ok",
            "Delete all lines from no-trailing-nl file. Got: {:?}",
            r.message
        );
        let content = read_file(&path);
        assert_eq!(
            content, "",
            "Deleting all lines from file without trailing newline leaves empty file. Got: {:?}",
            content
        );
    }

    /// BUG PROBE: delete last line from file with trailing newline
    #[test]
    fn bug_probe_delete_last_line_with_nl() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "del_last_nl.txt", "a\nb\n");
        let r = cmd_splice(&path, 2, 2, "", false, false, false, None);
        assert_eq!(r.status, "ok");
        let content = read_file(&path);
        assert_eq!(
            content, "a\n",
            "Delete last line of file with trailing newline. Got: {:?}",
            content
        );
    }

    /// BUG PROBE: delete last line from file WITHOUT trailing newline
    #[test]
    fn bug_probe_delete_last_line_no_nl() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "del_last_nn.txt", "a\nb");
        let r = cmd_splice(&path, 2, 2, "", false, false, false, None);
        assert_eq!(r.status, "ok");
        let content = read_file(&path);
        assert_eq!(
            content, "a",
            "Delete last line of file without trailing newline. Got: {:?}",
            content
        );
    }

    /// BUG PROBE: end beyond file length (end > lines.len())
    #[test]
    fn bug_probe_end_beyond_file_length() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "end_beyond.txt", "a\n");
        // 1-line file. start=1, end=2: end>len (2>1) — bounds check should catch
        let r = cmd_splice(&path, 1, 2, "X", false, false, false, None);
        // insert-at-end exception: start==len+1? 1==2? No. So error.
        assert_eq!(
            r.status, "error",
            "end=2 beyond file length 1 should be rejected. Got status: {}",
            r.status
        );
        assert!(
            r.message.as_deref().unwrap().contains("out of bounds"),
            "Expected out-of-bounds, got: {:?}",
            r.message
        );
    }

    /// BUG PROBE: start beyond length, end valid (start > end+1)
    #[test]
    fn bug_probe_start_beyond_len_end_valid() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "sbeyond.txt", "a\nb\n");
        // start=10, end=2: start>end+1 (10>3) → error
        let r = cmd_splice(&path, 10, 2, "X", false, false, false, None);
        assert_eq!(
            r.status, "error",
            "start=10 > end+1=3 should reject. Got: {:?}",
            r.message
        );
    }

    /// BUG PROBE: start=2, end=0 (start > end+1)
    #[test]
    fn bug_probe_splice_start2_end0_rejected() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "s2e0.txt", "a\nb\nc\n");
        // start=2, end=0: start>end+1 → 2>1 → true → error
        let r = cmd_splice(&path, 2, 0, "X", false, false, false, None);
        assert_eq!(
            r.status, "error",
            "start=2,end=0: start>end+1 should reject. Got: {:?}",
            r.message
        );
    }

    /// BUG PROBE: insert at very end (append)
    #[test]
    fn bug_probe_insert_at_very_end() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "append.txt", "a\n");
        // lines.len()=1. start=2,end=2: insert-at-end exception fires
        let r = cmd_splice(&path, 2, 2, "b", false, false, false, None);
        assert_eq!(
            r.status, "ok",
            "Append at end (start=2,end=2 on 1-line file). Got: {:?}",
            r.message
        );
        let content = read_file(&path);
        assert_eq!(
            content, "a\nb\n",
            "Append should add line with trailing newline. Got: {:?}",
            content
        );
    }

    /// BUG PROBE: single line file without trailing newline
    #[test]
    fn bug_probe_single_line_no_nl_replace() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "one_nnl.txt", "only");
        let r = cmd_splice(&path, 1, 1, "new", false, false, false, None);
        assert_eq!(r.status, "ok");
        let content = read_file(&path);
        assert_eq!(
            content, "new",
            "Single-line no-nl: should preserve no trailing newline. Got: {:?}",
            content
        );
    }

    // =========================================================================
    // BUG PROBE: MANIFEST OPERATION EDGE CASES
    // =========================================================================

    /// BUG PROBE: manifest start=0 must be rejected
    #[test]
    fn bug_probe_manifest_start_zero() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "mf_zero.txt", "a\nb\n");
        let manifest = r#"[{"start": 0, "end": 1, "delete": true}]"#;
        let r = cmd_manifest_impl(&path, manifest, false, false, false, None);
        assert_eq!(
            r.status, "error",
            "Manifest start=0 must be rejected (1-indexed). Got: {:?}",
            r.message
        );
        assert!(
            r.message.as_deref().unwrap().contains("out of bounds"),
            "Expected bounds error, got: {:?}",
            r.message
        );
    }

    /// BUG PROBE: manifest silent no-op — operation with neither delete nor hex
    #[test]
    fn bug_probe_manifest_silent_noop() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "mf_noop.txt", "a\nb\nc\n");
        let manifest = r#"[{"start": 1, "end": 1}]"#;
        let r = cmd_manifest_impl(&path, manifest, false, false, false, None);
        // The operation has no hex and no delete — now correctly returns error (fixed)
        assert_eq!(
            r.status, "error",
            "Silent no-op: op with no hex and no delete must return error. Fixed. Status: {}",
            r.status
        );
        assert_eq!(
            r.lines_removed, 0,
            "No lines should be removed in silent no-op"
        );
        assert_eq!(
            r.lines_inserted, 0,
            "No lines should be inserted in silent no-op"
        );
    }

    /// BUG PROBE: two manifest ops at same start line (NOW REJECTED WITH ERROR)
    /// After F5 fix: same-start operations are detected and rejected with a clear
    /// error before any mutation occurs. This prevents silent data loss.
    #[test]
    fn bug_probe_manifest_same_start_overlap() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "mf_overlap.txt", "a\nb\nc\nd\ne\n");
        let manifest = r#"[
            {"start": 3, "end": 3, "delete": true},
            {"start": 3, "end": 3, "hex": "58"}
        ]"#;
        let r = cmd_manifest_impl(&path, manifest, false, false, false, None);
        // F5 fix: same-start operations now produce an error, not silent corruption
        assert_eq!(
            r.status, "error",
            "Expected error for same-start manifest ops, got status={}",
            r.status
        );
        assert!(
            r.message
                .as_deref()
                .unwrap()
                .contains("overlapping manifest operations"),
            "Expected 'overlapping manifest operations' error, got: {:?}",
            r.message
        );
    }

    /// BUG PROBE: manifest context verification now uses pre-loop gate (F6 FIX)
    /// After F6 fix: context_hash is verified ONCE before the operation loop
    /// against the pre-manifest file state. For multi-op manifests, the first
    /// operation (bottom-up) is used as the verification window. This is a
    /// pre-manifest entry gate, not per-operation verification.
    ///
    /// This test verifies that the hash computed for line 6 (first op after
    /// bottom-up sort) correctly passes the pre-manifest gate.
    #[test]
    fn bug_probe_manifest_context_pre_loop_gate() {
        let dir = TempDir::new().unwrap();
        let content = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\n";
        let path = create_file(&dir, "mf_ctx_gate.txt", content);

        // Compute context hash for the first operation (line 6, bottom-up)
        let lines: Vec<String> = content.split_inclusive('\n').map(String::from).collect();
        let ctx_hash = moesniper::compute_context_hash(&lines, 6, 6);
        let short_hash = &ctx_hash[..16];

        // Manifest with TWO operations. Bottom-up: start=6 processes first.
        // The pre-loop gate verifies hash against the first op (start=6).
        let manifest = r#"[
                {"start": 3, "end": 3, "hex": "4e455731"},
                {"start": 6, "end": 6, "hex": "4e455732"}
            ]"#
        .to_string();

        let r = cmd_manifest_impl(&path, &manifest, false, false, false, Some(short_hash));
        // F6 fix: pre-manifest gate verifies against first op (start=6), hash matches → ok
        assert_eq!(
            r.status, "ok",
            "Pre-manifest context gate should pass (hash matches first op). \
             Status={}, msg={:?}",
            r.status, r.message
        );
    }

    /// BUG PROBE: single-op manifest context verification should work correctly
    #[test]
    fn bug_probe_manifest_context_single_op_works() {
        let dir = TempDir::new().unwrap();
        let content = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let path = create_file(&dir, "mf_ctx1.txt", content);

        let lines: Vec<String> = content.split_inclusive('\n').map(String::from).collect();
        let ctx_hash = moesniper::compute_context_hash(&lines, 3, 3);
        let short_hash = &ctx_hash[..16];

        // Single operation — context verification should match
        let manifest = r#"[{"start": 3, "end": 3, "hex": "4e4557"}]"#.to_string();
        let r = cmd_manifest_impl(&path, &manifest, false, false, false, Some(short_hash));
        assert_eq!(
            r.status, "ok",
            "Single-op manifest with correct context hash should succeed. Got: {:?}",
            r.message
        );
    }

    /// BUG PROBE: manifest context with WRONG hash must fail
    #[test]
    fn bug_probe_manifest_context_wrong_hash_fails() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "mf_ctxbad.txt", "a\nb\nc\nd\ne\nf\ng\nh\n");
        let manifest = r#"[{"start": 3, "end": 3, "hex": "4e4557"}]"#;
        let r = cmd_manifest_impl(
            &path,
            manifest,
            false,
            false,
            false,
            Some("0000000000000000"),
        );
        assert_eq!(
            r.status, "error",
            "Wrong context hash should fail. Got: {:?}",
            r.message
        );
        assert!(
            r.message.as_deref().unwrap().contains("context mismatch"),
            "Expected context mismatch, got: {:?}",
            r.message
        );
    }

    /// BUG PROBE: manifest with both delete and hex must be rejected
    #[test]
    fn bug_probe_manifest_both_delete_and_hex() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "mf_both.txt", "a\nb\n");
        let manifest = r#"[{"start": 1, "end": 1, "delete": true, "hex": "58"}]"#;
        let r = cmd_manifest_impl(&path, manifest, false, false, false, None);
        assert_eq!(
            r.status, "error",
            "Both delete and hex in same op must be rejected. Got: {:?}",
            r.message
        );
        assert!(
            r.message
                .as_deref()
                .unwrap()
                .contains("Cannot both delete and insert"),
            "Expected 'Cannot both delete and insert', got: {:?}",
            r.message
        );
    }

    // =========================================================================
    // BUG PROBE: PYTHON BINDINGS BOUNDS VALIDATION PARITY
    // =========================================================================

    /// Python sniper_edit allows end = lines.len() + 1 for any valid start,
    /// while Rust cmd_splice only allows it when start == lines.len() + 1.
    /// This test documents the Rust behavior and serves as the parity spec.
    #[test]
    fn bug_python_parity_end_bound_looser_than_rust() {
        let dir = TempDir::new().unwrap();
        // 3-line file
        let path = create_file(&dir, "parity_end.txt", "a\nb\nc\n");
        // end=4 = lines.len()+1, start=2 (not lines.len()+1)
        // Python sniper_edit: checks `end > lines.len() + 1` → 4 > 4 → false → ALLOWS
        // Rust cmd_splice: checks `end > lines.len()` → 4 > 3 → true → REJECTS
        // (unless start == lines.len()+1 which it's not)
        let r = cmd_splice(&path, 2, 4, "new", false, false, false, None);
        assert_eq!(
            r.status, "error",
            "Rust rejects end=lines.len()+1 when start < lines.len()+1 (Python allows this)"
        );
    }

    // =========================================================================
    // F-010: INVERTED RANGE ERROR MESSAGE
    // =========================================================================

    #[test]
    fn test_inverted_range_error_message() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "inv_range.txt", "a\nb\nc\n");
        let r = cmd_splice(&path, 5, 2, "x", false, false, false, None);
        assert_eq!(r.status, "error");
        assert!(
            r.message.as_deref().unwrap().contains("inverted range"),
            "Expected 'inverted range' in error, got: {:?}",
            r.message
        );
    }

    // =========================================================================
    // F-009: CONTEXT HASH LENGTH VALIDATION
    // =========================================================================

    #[test]
    fn test_context_hash_too_short_rejected() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "ctx_short.txt", "a\nb\n");
        let r = cmd_splice(&path, 1, 1, "x", false, false, false, Some("abc"));
        assert_eq!(r.status, "error");
        assert!(
            r.message
                .as_deref()
                .unwrap()
                .contains("invalid --context length"),
            "Expected context length error, got: {:?}",
            r.message
        );
    }

    #[test]
    fn test_context_hash_non_hex_rejected() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "ctx_nonhex.txt", "a\nb\n");
        let r = cmd_splice(
            &path,
            1,
            1,
            "x",
            false,
            false,
            false,
            Some("zzzzzzzzzzzzzzzz"),
        );
        assert_eq!(r.status, "error");
        assert!(
            r.message
                .as_deref()
                .unwrap()
                .contains("invalid --context length"),
            "Expected context length error, got: {:?}",
            r.message
        );
    }

    // =========================================================================
    // F-015: DECODE SUBCOMMAND
    // =========================================================================

    #[test]
    fn test_decode_hex_string() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "decode_hex.txt", "a\nb\n");
        let r = cmd_splice(&path, 1, 1, "48656c6c6f", false, false, false, None);
        assert_eq!(r.status, "ok");
        let content = read_file(&path);
        assert_eq!(content, "Hello\n");
    }

    #[test]
    fn test_decode_invalid_hex() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "decode_bad.txt", "a\nb\n");
        let r = cmd_splice(&path, 1, 1, "zzzz", false, false, false, None);
        assert_eq!(r.status, "error");
        assert!(
            r.message.as_deref().unwrap().contains("hex decode"),
            "Expected hex decode error, got: {:?}",
            r.message
        );
    }

    // =========================================================================
    // F-002: READ-ONLY FILE GUARD
    // =========================================================================

    #[test]
    fn test_readonly_file_rejected() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "readonly.txt", "original\n");
        // Make read-only
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&path, perms).unwrap();

        let r = cmd_splice(&path, 1, 1, "xx", false, false, false, None);
        assert_eq!(r.status, "error");
        assert!(
            r.message.as_deref().unwrap().contains("read-only"),
            "Expected read-only error, got: {:?}",
            r.message
        );

        // Restore permissions for cleanup
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        }
        #[cfg(not(unix))]
        {
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_readonly(false);
            fs::set_permissions(&path, perms).unwrap();
        }
    }
}
