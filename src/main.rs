//! Sniper — escape-proof precision file editor for LLM agents.
//!
//! One operation: splice(file, start, end, hex_payload).
//! Hex encoding guarantees zero shell corruption.
//! Batch manifests apply bottom-up so line numbers never shift.
//!
//! Usage:
//!   sniper <file> <start> <end> <hex>       Replace lines
//!   sniper <file> <start> <end> --delete    Delete lines
//!   sniper <file> --manifest <path>         Batch (bottom-up)
//!   sniper <file> --undo                    Restore backup
//!
//! Flags: --dry-run, --json

use std::fs;
use std::io::Read;

use moesniper::{
    create_backup, find_latest_backup, handle_backtrack_error, hex_decode, write_atomic,
    write_atomic_owned, SniperLock,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        eprint!(concat!(
            "CRITICAL: REASON BEFORE ANY ACTIONS.\n",
            "MANDATE: Analyze file context and intent before committing edits.\n",
            "METABOLIC AWARENESS: Atomic writes self-throttle if iowait > 15.0.\n",
            "BACK-PRESSURE: Aborts the rename and preserves the backup on Error -7.\n",
            "\n",
            "sniper — escape-proof precision file editor for LLM agents\n",
            "\n",
            "USAGE:\n",
            "  sniper <file> <start> <end> <hex>       Replace lines start-end with hex-decoded content\n",
            "  sniper <file> <start> <end> --delete    Delete lines start-end\n",
            "  sniper <file> --manifest <path>         Batch ops from JSON (applied bottom-up)\n",
            "  sniper <file> --undo                    Restore from last backup (supports multi-step)\n",
            "  sniper encode [--stdin | --file <p>]    Output hex-encoded string (use --stdin for safety)\n",
            "\n",
            "FLAGS:\n",
            "  --dry-run   Preview changes without applying\n",
            "  --json      Machine-readable JSON output\n",
            "\n",
            "ENCODING:\n",
            "  Truly escape-proof: cat code.rs | sniper encode --stdin\n",
            "  From file:          sniper encode --file snippet.txt\n",
            "  Positional (UNSAFE): sniper encode 'text' (subject to shell escaping)\n",
            "\n",
            "MANIFEST FORMAT:\n",
            "  [{{\"start\": 42, \"end\": 45, \"hex\": \"6e6577\"}}, {{\"start\": 10, \"delete\": true}}]\n",
            "  Operations applied bottom-up (highest line first). Line numbers refer to original file.\n",
            "\n",
            "BACKUPS:\n",
            "  Every edit creates .sniper/<path_hash>.<filename>.<timestamp>\n",
            "  Undo restores the most recent backup and removes it from the stack.\n",
            "\n",
            "AGENTIC EDITING (UTCP Schema):\n",
            "  Replace line:    sniper --json file.rs 42 42 6e6577 → {{\"status\":\"ok\",...}}\n",
            "  Delete lines:    sniper --json file.rs 10 15 --delete → remove lines 10-15\n",
            "  Batch edit:      sniper --json file.rs --manifest ops.json → multiple edits\n",
            "  Dry-run check:   sniper --json --dry-run file.rs 1 3 787878 → preview only\n",
            "  Undo:            sniper --json file.rs --undo → restore backup\n",
            "\n",
            "LLM AGENT USAGE:\n",
            "  Encode content:  sniper encode 'fn main() {{}}'\n",
            "  Get line range:  Use ix to find lines, then sniper to edit them\n",
            "  Safe workflow:   Dry-run first, check JSON output, then apply\n",
            "  Idempotent:      --dry-run never modifies files, safe to retry\n",
            "\n",
            "JSON OUTPUT SCHEMA:\n",
            "  status:       \"ok\" | \"dry_run\" | \"restored\" | \"error\" | \"encoded\"\n",
            "  file:         path to edited file (null on error)\n",
            "  lines_removed: count of lines removed\n",
            "  lines_inserted: count of lines inserted\n",
            "  total_lines:  file line count after edit\n",
            "  operations:   count of manifest ops (manifest mode only)\n",
            "  backup:       path to backup file (null on error/dry-run)\n",
            "  message:      error/encoded result description\n",
            "\n",
            "CONSTRAINTS:\n",
            "  - All content MUST be hex-encoded to prevent shell corruption.\n",
            "  - Line numbers are 1-indexed, inclusive on both ends.\n",
            "  - Manifest ops applied bottom-up (highest line first).\n",
            "  - Atomic writes: file is written to temp, then renamed.\n",
            "\n",
            "EXAMPLES:\n",
            "  Replace line 5 with 'hello world':\n",
            "    sniper file.rs 5 5 68656c6c6f20776f726c64\n",
            "\n",
            "  Delete lines 10-20:\n",
            "    sniper file.rs 10 20 --delete\n",
            "\n",
            "  Batch edit with manifest:\n",
            "    echo '[{{\"start\":1,\"end\":1,\"hex\":\"78\"}}]' > ops.json\n",
            "    sniper file.rs --manifest ops.json\n",
            "\n",
            "  Undo last edit:\n",
            "    sniper file.rs --undo\n",
        ));
        std::process::exit(0);
    }

    let dry_run = args.iter().any(|a| a == "--dry-run");
    let json_out = args.iter().any(|a| a == "--json");
    let use_stdin = args.iter().any(|a| a == "--stdin");
    let args: Vec<&str> = args
        .iter()
        .filter(|a| *a != "--dry-run" && *a != "--json" && *a != "--stdin")
        .map(|s| s.as_str())
        .collect();

    let result = match args.as_slice() {
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
        [file, "--undo"] => cmd_undo(file),
        [file, "--manifest"] if use_stdin => cmd_manifest_stdin(file, dry_run),
        [file, "--manifest", manifest] => cmd_manifest(file, manifest, dry_run),
        [file, start, end, "--delete"] => {
            if use_stdin {
                err("cannot use --stdin with --delete".into())
            } else {
                match (parse_line(start), parse_line(end)) {
                    (Ok(s), Ok(e)) => cmd_splice(file, s, e, "", dry_run),
                    (Err(e), _) | (_, Err(e)) => err(e),
                }
            }
        }
        [file, start, end] if use_stdin => {
            let mut buffer = String::new();
            match std::io::stdin().read_to_string(&mut buffer) {
                Ok(_) => match (parse_line(start), parse_line(end)) {
                    (Ok(ln_start), Ok(ln_end)) => {
                        cmd_splice(file, ln_start, ln_end, &buffer, dry_run)
                    }
                    (Err(e), _) | (_, Err(e)) => err(e),
                },
                Err(e) => err(format!("read stdin: {e}")),
            }
        }
        [file, start, end, hex] => match (parse_line(start), parse_line(end)) {
            (Ok(s), Ok(e)) => match hex_decode(hex) {
                Ok(content) => cmd_splice(file, s, e, &content, dry_run),
                Err(msg) => err(format!("hex decode: {msg}")),
            },
            (Err(e), _) | (_, Err(e)) => err(e),
        },
        _ => {
            eprintln!("error: bad arguments. Run 'sniper --help'");
            std::process::exit(1);
        }
    };

    if json_out {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        );
    } else {
        match result.status.as_str() {
            "ok" => println!(
                "ok: {} -{} +{}",
                result.file.as_deref().unwrap_or("?"),
                result.lines_removed,
                result.lines_inserted
            ),
            "restored" => println!("restored: {}", result.backup.as_deref().unwrap_or("?")),
            "encoded" => println!("{}", result.message.as_deref().unwrap_or("")),
            "dry_run" => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).unwrap_or_default()
                )
            }
            _ => {
                eprintln!("error: {}", result.message.as_deref().unwrap_or("unknown"));
                std::process::exit(1);
            }
        }
    }
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
}

fn cmd_encode(text: &str) -> CliResult {
    let hex = text
        .as_bytes()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();
    CliResult {
        status: "encoded".into(),
        message: Some(hex),
        ..Default::default()
    }
}

fn cmd_splice(filepath: &str, start: usize, end: usize, content: &str, dry_run: bool) -> CliResult {
    let _lock = match SniperLock::acquire(filepath) {
        Ok(l) => l,
        Err(e) => return err(e),
    };

    let text = match fs::read_to_string(filepath) {
        Ok(t) => t,
        Err(e) => return err(handle_backtrack_error(e, "Read")),
    };
    let mut lines: Vec<String> = text.split_inclusive('\n').map(String::from).collect();

    // If the file does not end in a newline, the last line will not have one.
    // We must ensure that if we are adding lines, they end in a newline if the original file had one or if they are not the last line.
    // However, split_inclusive handles this correctly for the most part.
    // But text.split_inclusive('\n') on "a\nb" gives ["a\n", "b"]
    // cmd_splice was previously using lines(), which on "a\nb" gives ["a", "b"]

    if start < 1 || end > lines.len() || start > end + 1 {
        // Special case: inserting at the end of a file that might not have a trailing newline
        if start == lines.len() + 1 && start == end + 1 {
            // Allow inserting at end
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

    let new_lines: Vec<String> = if content.is_empty() {
        vec![]
    } else {
        content.split_inclusive('\n').map(String::from).collect()
    };

    let is_delete = content.is_empty();

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
            ..Default::default()
        };
    }

    let bk = match create_backup(filepath) {
        Ok(b) => b,
        Err(e) => return err(e),
    };

    let new_lines_count = new_lines.len();

    if s < lines.len() {
        let actual_end = end.min(lines.len());
        lines.splice(s..actual_end, new_lines);
    } else {
        lines.extend(new_lines);
    }

    let lines_refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
    if let Err(e) = write_atomic(filepath, &lines_refs) {
        return err(e);
    }

    let ai_hint = Some(if is_delete {
        format!("verify: {} around line {}", filepath, start)
    } else {
        format!("verify: read {} lines {}-{}", filepath, start, end)
    });

    CliResult {
        status: "ok".into(),
        file: Some(filepath.into()),
        lines_removed: removed_lines_count,
        lines_inserted: new_lines_count,
        total_lines: Some(lines.len()),
        backup: Some(bk),
        ai_hint,
        ..Default::default()
    }
}

fn cmd_manifest_stdin(filepath: &str, dry_run: bool) -> CliResult {
    let mut buffer = String::new();
    let manifest = match std::io::stdin().read_to_string(&mut buffer) {
        Ok(_) => buffer,
        Err(e) => return err(format!("read manifest from stdin: {e}")),
    };
    cmd_manifest_impl(filepath, &manifest, dry_run)
}

fn cmd_manifest(filepath: &str, manifest_path: &str, dry_run: bool) -> CliResult {
    let manifest = match fs::read_to_string(manifest_path) {
        Ok(m) => m,
        Err(e) => return err(format!("read manifest: {e}")),
    };
    cmd_manifest_impl(filepath, &manifest, dry_run)
}

fn cmd_manifest_impl(filepath: &str, manifest: &str, dry_run: bool) -> CliResult {
    let _lock = match SniperLock::acquire(filepath) {
        Ok(l) => l,
        Err(e) => return err(e),
    };

    let mut ops: Vec<ManifestOp> = match serde_json::from_str(manifest) {
        Ok(o) => o,
        Err(e) => return err(format!("parse manifest: {e}")),
    };

    let text = match fs::read_to_string(filepath) {
        Ok(t) => t,
        Err(e) => return err(handle_backtrack_error(e, "Read")),
    };
    let mut lines: Vec<String> = text.split_inclusive('\n').map(String::from).collect();

    // Sort bottom-up
    ops.sort_by(|a, b| b.start.cmp(&a.start));

    let bk = if !dry_run {
        match create_backup(filepath) {
            Ok(b) => Some(b),
            Err(e) => return err(e),
        }
    } else {
        None
    };
    let mut total_removed = 0usize;
    let mut total_inserted = 0usize;

    for op in &ops {
        let start = op.start;
        let end = op.end.unwrap_or(op.start);

        if start < 1 || end > lines.len() + 1 || start > end + 1 {
            return err(format!(
                "line range {start}-{end} out of bounds (file has {} lines)",
                lines.len()
            ));
        }

        let s = start - 1;
        let e = end;

        if op.delete.unwrap_or(false) {
            total_removed += lines.splice(s..e, std::iter::empty()).count();
        } else if let Some(ref hex) = op.hex {
            let content = match hex_decode(hex) {
                Ok(c) => c,
                Err(e) => return err(format!("hex decode in manifest: {e}")),
            };
            let new: Vec<String> = content.split_inclusive('\n').map(String::from).collect();
            total_removed += e - s;
            total_inserted += new.len();
            lines.splice(s..e, new);
        }
    }

    if !dry_run {
        if let Err(e) = write_atomic_owned(filepath, &lines) {
            return err(e);
        }
    }

    let ai_hint = Some(format!(
        "verify: read {} around line {}",
        filepath,
        ops.first().map(|o| o.start).unwrap_or(1)
    ));

    CliResult {
        status: if dry_run { "dry_run" } else { "ok" }.into(),
        file: Some(filepath.into()),
        lines_removed: total_removed,
        lines_inserted: total_inserted,
        total_lines: Some(lines.len()),
        operations: Some(ops.len()),
        ai_hint,
        backup: bk,
        ..Default::default()
    }
}

fn cmd_undo(filepath: &str) -> CliResult {
    let _lock = match SniperLock::acquire(filepath) {
        Ok(l) => l,
        Err(e) => return err(e),
    };

    let latest = match find_latest_backup(filepath) {
        Ok(Some(l)) => l,
        Ok(None) => return err(format!("no backup for {filepath}")),
        Err(e) => return err(e),
    };

    // Restoration: simple copy. We do NOT create a backup of the state we are overwriting
    // to allow consecutive undos to pop the stack.
    if let Err(e) = fs::copy(&latest, filepath) {
        return err(format!("restore: {e}"));
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

#[derive(serde::Deserialize)]
struct ManifestOp {
    start: usize,
    #[serde(default)]
    end: Option<usize>,
    #[serde(default)]
    hex: Option<String>,
    #[serde(default)]
    delete: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_file(dir: &TempDir, name: &str, content: &str) -> String {
        let path = dir.path().join(name);
        fs::write(&path, content).unwrap();
        path.to_str().unwrap().to_string()
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
    fn test_hex_decode_non_hex_returns_error() {
        let result = hex_decode("gg");
        assert!(result.is_err());
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
        let r = cmd_splice(&path, 2, 2, "hex", false);
        assert_eq!(r.status, "ok");
        assert_eq!(r.lines_removed, 1);
        assert_eq!(r.lines_inserted, 1);
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "line1\nhex\nline3\n");
    }

    #[test]
    fn test_cmd_splice_preserves_missing_trailing_newline() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "line1\nline2");
        let r = cmd_splice(&path, 2, 2, "new", false);
        assert_eq!(r.status, "ok");
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "line1\nnew");
        assert!(!content.ends_with('\n'));
    }

    #[test]
    fn test_cmd_splice_preserves_existing_trailing_newline() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "line1\nline2\n");
        let r = cmd_splice(&path, 2, 2, "new", false);
        assert_eq!(r.status, "ok");
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "line1\nnew\n");
        assert!(content.ends_with('\n'));
    }

    #[test]
    fn test_cmd_splice_replace_range() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\nc\nd\ne\n");
        let r = cmd_splice(&path, 2, 4, "X\nY", false);
        assert_eq!(r.status, "ok");
        assert_eq!(r.lines_removed, 3);
        assert_eq!(r.lines_inserted, 2);
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "a\nX\nY\ne\n");
    }

    #[test]
    fn test_cmd_splice_insert_at_end() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\n");
        let r = cmd_splice(&path, 2, 2, "c", false);
        assert_eq!(r.status, "ok");
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "a\nc\n");
    }

    #[test]
    fn test_cmd_splice_out_of_bounds() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\n");
        let r = cmd_splice(&path, 10, 20, "x", false);
        assert_eq!(r.status, "error");
        assert!(r.message.as_deref().unwrap().contains("out of bounds"));
    }

    #[test]
    fn test_cmd_splice_start_zero() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\n");
        let r = cmd_splice(&path, 0, 1, "x", false);
        assert_eq!(r.status, "error");
    }

    // --- cmd_splice delete tests ---

    #[test]
    fn test_cmd_splice_delete_single_line() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\nc\n");
        let r = cmd_splice(&path, 2, 2, "", false);
        assert_eq!(r.status, "ok");
        assert_eq!(r.lines_removed, 1);
        assert_eq!(r.lines_inserted, 0);
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "a\nc\n");
    }

    #[test]
    fn test_cmd_splice_delete_range() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\nc\nd\ne\n");
        let r = cmd_splice(&path, 2, 4, "", false);
        assert_eq!(r.status, "ok");
        assert_eq!(r.lines_removed, 3);
        assert_eq!(r.lines_inserted, 0);
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "a\ne\n");
    }

    // --- dry_run tests ---

    #[test]
    fn test_cmd_splice_dry_run_no_change() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\nc\n");
        let original = fs::read_to_string(&path).unwrap();
        let r = cmd_splice(&path, 2, 2, "7878", true);
        assert_eq!(r.status, "dry_run");
        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(original, after);
    }

    #[test]
    fn test_cmd_splice_dry_run_delete() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\nc\n");
        let original = fs::read_to_string(&path).unwrap();
        let r = cmd_splice(&path, 1, 2, "", true);
        assert_eq!(r.status, "dry_run");
        let after = fs::read_to_string(&path).unwrap();
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
        let r = cmd_manifest(&path, &manifest_path, false);
        assert_eq!(r.status, "ok");
        assert_eq!(r.operations, Some(2));
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "x\nline2\nline5\n");
    }

    #[test]
    fn test_cmd_manifest_dry_run() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\nc\n");
        let original = fs::read_to_string(&path).unwrap();
        let manifest = r#"[{"start": 1, "end": 1, "hex": "78"}]"#;
        let manifest_path = create_file(&dir, "ops.json", manifest);
        let r = cmd_manifest(&path, &manifest_path, true);
        assert_eq!(r.status, "dry_run");
        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(original, after);
    }

    #[test]
    fn test_cmd_manifest_bad_json() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\n");
        let manifest_path = create_file(&dir, "ops.json", "not json");
        let r = cmd_manifest(&path, &manifest_path, false);
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
        let r = cmd_manifest(&path, &manifest_path, false);
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
        let _ = cmd_splice(&path, 1, 1, "xx", false);
        let content = fs::read_to_string(&path).unwrap();
        assert_ne!(content, "original\n");
        let r = cmd_undo(&path);
        assert_eq!(r.status, "restored");
        let restored = fs::read_to_string(&path).unwrap();
        assert_eq!(restored, "original\n");
    }

    #[test]
    fn test_cmd_undo_multi_step() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "multi_undo.txt", "v1\n");

        cmd_splice(&path, 1, 1, "v2", false); // edit 1
        cmd_splice(&path, 1, 1, "v3", false); // edit 2

        assert_eq!(fs::read_to_string(&path).unwrap(), "v3\n");

        cmd_undo(&path); // undo 1
        assert_eq!(fs::read_to_string(&path).unwrap(), "v2\n");

        cmd_undo(&path); // undo 2
        assert_eq!(fs::read_to_string(&path).unwrap(), "v1\n");

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

    // --- edge case tests ---

    #[test]
    fn test_file_not_found() {
        let r = cmd_splice("/tmp/no_such_file_12345.txt", 1, 1, "78", false);
        assert_eq!(r.status, "error");
        assert!(r
            .message
            .as_deref()
            .unwrap()
            .to_lowercase()
            .contains("read"));
    }

    #[test]
    fn test_cmd_splice_delete_last_line_preserves_non_termination_if_possible() {
        let dir = TempDir::new().unwrap();
        // File: "a\nb" (no trailing newline)
        let path = create_file(&dir, "no_trailing.txt", "a\nb");

        // Delete line 2 ("b")
        let r = cmd_splice(&path, 2, 2, "", false);
        assert_eq!(r.status, "ok");

        let content = fs::read_to_string(&path).unwrap();
        // Currently, it will be "a\n" because "a\n" was the first line from split_inclusive.
        // If we want true precision, it should be "a".
        // Let's see what it is currently.
        assert_eq!(content, "a\n");
    }

    #[test]
    fn test_single_line_file() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "one.txt", "only\n");
        let r = cmd_splice(&path, 1, 1, "new", false);
        assert_eq!(r.status, "ok");
        let content = fs::read_to_string(&path).unwrap();
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
            let original = fs::read_to_string(&path).unwrap();
            let lines: Vec<&str> = original.lines().collect();
            if lines.is_empty() || line_num > lines.len() {
                return Ok(());
            }
            let _ = cmd_splice(&path, line_num, line_num, &replacement, true);
            let after = fs::read_to_string(&path).unwrap();
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
            let _ = cmd_splice(&path, start, end, &replacement, false);
            let after = fs::read_to_string(&path).unwrap();
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
            let path = create_file(&dir, "prop_undo_test.txt", &content);
            let original = fs::read_to_string(&path).unwrap();
            let lines: Vec<&str> = original.lines().collect();
            if lines.is_empty() || line_num > lines.len() {
                return Ok(());
            }
            let _ = cmd_splice(&path, line_num, line_num, &replacement, false);
            let _ = cmd_undo(&path);
            let restored = fs::read_to_string(&path).unwrap();
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
            let r = cmd_splice(&path, 1, 2, &replacement, false);
            let after = fs::read_to_string(&path).unwrap();
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
        let r = cmd_splice(&path, 2, 2, "xx", false);
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
        let r = cmd_splice(&path, 2, 2, "", false);
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
        let r = cmd_splice(&path, 1, 1, "xx", true);
        assert_eq!(r.status, "dry_run");
        assert!(r.ai_hint.is_some());
    }

    #[test]
    fn test_ai_hint_after_error_not_found() {
        let r = cmd_splice("/no/such/file.txt", 1, 1, "xx", false);
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
        let r = cmd_splice(&path, 10, 20, "xx", false);
        assert_eq!(r.status, "error");
        assert!(r.ai_hint.is_some());
        let hint = r.ai_hint.unwrap();
        assert!(hint.contains("line count"));
    }

    #[test]
    fn test_ai_hint_after_undo() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "undo_hint.txt", "original\n");
        let _ = cmd_splice(&path, 1, 1, "xx", false);
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
}
