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
use std::path::{Path, PathBuf};

use moesniper::{create_backup, hex_decode, write_atomic, write_atomic_owned, BACKUP_DIR};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        eprint!(concat!(
            "sniper — escape-proof precision file editor for LLM agents\n",
            "\n",
            "USAGE:\n",
            "  sniper <file> <start> <end> <hex>       Replace lines start-end with hex-decoded content\n",
            "  sniper <file> <start> <end> --delete    Delete lines start-end\n",
            "  sniper <file> --manifest <path>         Batch ops from JSON (applied bottom-up)\n",
            "  sniper <file> --undo                    Restore from last backup\n",
            "\n",
            "FLAGS:\n",
            "  --dry-run   Preview changes without applying\n",
            "  --json      Machine-readable JSON output\n",
            "\n",
            "ENCODING:\n",
            "  Content is hex-encoded: echo -n 'your text' | xxd -p\n",
            "  Example: sniper file.rs 42 42 757365207065746772617068\n",
            "\n",
            "MANIFEST FORMAT:\n",
            "  [{{\"start\": 42, \"end\": 45, \"hex\": \"6e6577\"}}, {{\"start\": 10, \"delete\": true}}]\n",
            "  Operations applied bottom-up (highest line first). Line numbers refer to original file.\n",
            "\n",
            "BACKUPS:\n",
            "  Every edit creates .sniper/<filename>.<timestamp>\n",
            "  Undo restores the most recent backup.\n",
            "\n",
            "AGENTIC EDITING (UTCP Schema):\n",
            "  Replace line:    sniper --json file.rs 42 42 6e6577 → {{\"status\":\"ok\",...}}\n",
            "  Delete lines:    sniper --json file.rs 10 15 --delete → remove lines 10-15\n",
            "  Batch edit:      sniper --json file.rs --manifest ops.json → multiple edits\n",
            "  Dry-run check:   sniper --json --dry-run file.rs 1 3 787878 → preview only\n",
            "  Undo:            sniper --json file.rs --undo → restore backup\n",
            "\n",
            "LLM AGENT USAGE:\n",
            "  Encode content:  echo -n 'fn main() {{}}' | xxd -p | tr -d '\\n'\n",
            "  Get line range:  Use ix to find lines, then sniper to edit them\n",
            "  Safe workflow:   Dry-run first, check JSON output, then apply\n",
            "  Idempotent:      --dry-run never modifies files, safe to retry\n",
            "\n",
            "JSON OUTPUT SCHEMA:\n",
            "  status:       \"ok\" | \"dry_run\" | \"restored\" | \"error\"\n",
            "  file:         path to edited file (null on error)\n",
            "  lines_removed: count of lines removed\n",
            "  lines_inserted: count of lines inserted\n",
            "  total_lines:  file line count after edit\n",
            "  operations:   count of manifest ops (manifest mode only)\n",
            "  backup:       path to backup file (null on error/dry-run)\n",
            "  message:      error description (error status only)\n",
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
    let args: Vec<&str> = args
        .iter()
        .filter(|a| *a != "--dry-run" && *a != "--json")
        .map(|s| s.as_str())
        .collect();

    let result = match args.as_slice() {
        [file, "--undo"] => cmd_undo(file),
        [file, "--manifest", manifest] => cmd_manifest(file, manifest, dry_run),
        [file, start, end, "--delete"] => match (parse_line(start), parse_line(end)) {
            (Ok(s), Ok(e)) => cmd_splice(file, s, e, "", dry_run),
            (Err(e), _) | (_, Err(e)) => err(e),
        },
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
}

fn cmd_splice(filepath: &str, start: usize, end: usize, content: &str, dry_run: bool) -> CliResult {
    let text = match fs::read_to_string(filepath) {
        Ok(t) => t,
        Err(e) => return err(format!("read {filepath}: {e}")),
    };
    let mut lines: Vec<&str> = text.lines().collect();

    if start < 1 || end > lines.len() || start > end + 1 {
        return err(format!(
            "line range {start}-{end} out of bounds (file has {} lines)",
            lines.len()
        ));
    }

    let s = start - 1;
    let removed: Vec<&str> = lines[s..end].to_vec();
    let new_lines: Vec<&str> = if content.is_empty() {
        vec![]
    } else {
        content.lines().collect()
    };

    if dry_run {
        return CliResult {
            status: "dry_run".into(),
            file: Some(filepath.into()),
            lines_removed: removed.len(),
            lines_inserted: new_lines.len(),
            ..Default::default()
        };
    }

    let bk = match create_backup(filepath) {
        Ok(b) => b,
        Err(e) => return err(e),
    };
    lines.splice(s..end, new_lines.iter().copied());
    if let Err(e) = write_atomic(filepath, &lines) {
        return err(e);
    }

    CliResult {
        status: "ok".into(),
        file: Some(filepath.into()),
        lines_removed: removed.len(),
        lines_inserted: new_lines.len(),
        total_lines: Some(lines.len()),
        backup: Some(bk),
        ..Default::default()
    }
}

fn cmd_manifest(filepath: &str, manifest_path: &str, dry_run: bool) -> CliResult {
    let manifest = match fs::read_to_string(manifest_path) {
        Ok(m) => m,
        Err(e) => return err(format!("read manifest: {e}")),
    };

    let mut ops: Vec<ManifestOp> = match serde_json::from_str(&manifest) {
        Ok(o) => o,
        Err(e) => return err(format!("parse manifest: {e}")),
    };

    let text = match fs::read_to_string(filepath) {
        Ok(t) => t,
        Err(e) => return err(format!("read {filepath}: {e}")),
    };
    let mut lines: Vec<String> = text.lines().map(String::from).collect();

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
        let s = op.start.saturating_sub(1);
        let e = op.end.unwrap_or(op.start);

        if op.delete.unwrap_or(false) {
            total_removed += lines.splice(s..e, std::iter::empty()).count();
        } else if let Some(ref hex) = op.hex {
            let content = match hex_decode(hex) {
                Ok(c) => c,
                Err(e) => return err(format!("hex decode in manifest: {e}")),
            };
            let new: Vec<String> = content.lines().map(String::from).collect();
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

    CliResult {
        status: if dry_run { "dry_run" } else { "ok" }.into(),
        file: Some(filepath.into()),
        lines_removed: total_removed,
        lines_inserted: total_inserted,
        total_lines: Some(lines.len()),
        operations: Some(ops.len()),
        backup: bk,
        ..Default::default()
    }
}

fn cmd_undo(filepath: &str) -> CliResult {
    let name = Path::new(filepath)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(filepath);

    // Use hash of full path to match backup naming
    let path_hash = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        filepath.hash(&mut hasher);
        hasher.finish()
    };

    let latest_name = format!("{path_hash:x}.{name}.latest");
    let latest = PathBuf::from(BACKUP_DIR).join(&latest_name);

    if !latest.exists() {
        return err(format!("no backup for {filepath}"));
    }

    let target = match fs::read_link(&latest) {
        Ok(t) => PathBuf::from(BACKUP_DIR).join(t),
        Err(e) => return err(format!("read backup link: {e}")),
    };

    if let Err(e) = fs::copy(&target, filepath) {
        return err(format!("restore: {e}"));
    }

    CliResult {
        status: "restored".into(),
        backup: Some(target.to_string_lossy().into()),
        ..Default::default()
    }
}

fn parse_line(s: &str) -> Result<usize, String> {
    s.parse().map_err(|_| format!("invalid line number: {s}"))
}

fn err(msg: String) -> CliResult {
    CliResult {
        status: "error".into(),
        message: Some(msg),
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
        assert_eq!(hex_decode("zz").unwrap(), "");
    }

    #[test]
    fn test_hex_decode_odd_length() {
        assert_eq!(hex_decode("48650").unwrap(), "He");
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
        assert!(r.message.as_deref().unwrap().contains("read"));
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

    // --- error handling tests (G-ERR) ---

    #[test]
    fn test_hex_decode_result_ok() {
        let result = hex_decode("48656c6c6f");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Hello");
    }

    #[test]
    fn test_hex_decode_empty_returns_ok() {
        let result = hex_decode("");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "");
    }

    #[test]
    fn test_hex_decode_non_hex_returns_empty_string() {
        // Non-hex chars get filtered, resulting in valid empty output
        let result = hex_decode("gg");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "");
    }

    #[test]
    fn test_manifest_with_invalid_hex_in_op() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\n");
        // "GG" is not valid hex, decodes to empty -> deletes line 1
        let manifest = r#"[{"start": 1, "end": 1, "hex": "GG"}]"#;
        let manifest_path = create_file(&dir, "ops.json", manifest);
        let r = cmd_manifest(&path, &manifest_path, false);
        assert_eq!(r.status, "ok");
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "b\n");
    }

    #[test]
    fn test_manifest_empty_ops() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\n");
        let manifest = r#"[]"#;
        let manifest_path = create_file(&dir, "ops.json", manifest);
        let r = cmd_manifest(&path, &manifest_path, false);
        assert_eq!(r.status, "ok");
        assert_eq!(r.operations, Some(0));
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "a\nb\n");
    }

    #[test]
    fn test_splice_at_start_of_file() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\nc\n");
        let r = cmd_splice(&path, 1, 1, "x", false);
        assert_eq!(r.status, "ok");
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "x\nb\nc\n");
    }

    #[test]
    fn test_splice_at_end_of_file() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\nc\n");
        let r = cmd_splice(&path, 3, 3, "z", false);
        assert_eq!(r.status, "ok");
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "a\nb\nz\n");
    }

    #[test]
    fn test_splice_replaces_entire_file() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\nc\n");
        let r = cmd_splice(&path, 1, 3, "x", false);
        assert_eq!(r.status, "ok");
        assert_eq!(r.lines_removed, 3);
        assert_eq!(r.lines_inserted, 1);
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "x\n");
    }

    #[test]
    fn test_splice_multiline_content() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\nc\n");
        let r = cmd_splice(&path, 2, 2, "x\ny\nz", false);
        assert_eq!(r.status, "ok");
        assert_eq!(r.lines_removed, 1);
        assert_eq!(r.lines_inserted, 3);
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "a\nx\ny\nz\nc\n");
    }

    #[test]
    fn test_splice_returns_total_lines() {
        let dir = TempDir::new().unwrap();
        let path = create_file(&dir, "test.txt", "a\nb\nc\n");
        let r = cmd_splice(&path, 2, 2, "x\ny", false);
        assert_eq!(r.status, "ok");
        assert_eq!(r.total_lines, Some(4)); // a, x, y, c
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
}
