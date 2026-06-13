/// CLI help text for the sniper command.
pub const HELP: &str = r#"sniper — escape-proof precision file editor for LLM agents

USAGE:
    sniper <file> <start> <end> <hex>       Replace lines with hex content
    sniper <file> <start> <end> --delete    Delete line range
    sniper <file> <start> <end> --stdin     Read content from stdin
    sniper <file> <start> <end> <hex> --context <hash>  Verify context before applying
    sniper <file> --manifest <path>         Batch operations from JSON
    sniper <file> --undo                    Restore from backup
    sniper encode [--stdin|--file <path>|<text>]  Hex-encode content
    sniper context <file> <start> <end>     Compute context hash for a given line range

FLAGS:
    --dry-run           Preview changes without applying
    --json              Output machine-readable JSON
    --stdin             Read replacement content from stdin
    --context <hash>    Verify context SHA-256 hash (first 16 hex chars) before applying
    --auto-indent       Auto-detect and apply indentation from context
    --force-indent      Bypass indentation validation (allow unindented content)
    -v, --version       Print version and exit

CONTEXT VERIFICATION:
    The `--context <hash>` flag allows rejecting an edit if the file has changed
    around the edit site.

    The hash is the first 16 hex characters of the SHA-256 of the concatenated raw bytes
    (including all newlines) of exactly 3 lines before the `<start>` line, and exactly
    3 lines after the `<end>` line. Line numbers are not part of the hash.

    You can generate the context hash using the command:
        sniper context file.rs <start> <end>

QUICK START:
    # Replace line 5 with "hello"
    sniper file.rs 5 5 68656c6c6f

    # Delete lines 10-20
    sniper file.rs 10 20 --delete

    # Preview before applying
    sniper file.rs 1 5 787878 --dry-run

    # Undo last edit
    sniper file.rs --undo

    # Pipe content from stdin
    echo 'new line' | sniper file.rs 5 5 --stdin

ENCODING:
    Content must be hex-encoded to prevent shell corruption.
    Use `sniper encode --stdin` for safe encoding:

        echo -n 'fn main() {}' | sniper encode --stdin
        # Output: 666e206d61696e2829207b7d

    Or use xxd: echo -n 'text' | xxd -p | tr -d '\n'

MANIFEST FORMAT:
    [{"start": 42, "end": 45, "hex": "6e6577"}, {"start": 10, "delete": true}]

    Operations apply bottom-up (highest line first).
    Line numbers are 1-indexed.

BACKUPS:
    Every edit creates a backup in .sniper/
    Use --undo to restore the previous version.
    Backups are purged by count (default 50) and age (default 30 days).

INDENTATION:
    Indentation validation runs on every edit. If the replacement content has
    different leading whitespace than the surrounding lines, the edit is blocked.

    --auto-indent detects the expected indentation from surrounding lines
    and automatically prepends missing leading whitespace. Useful when LLM
    output omits indentation.

    --force-indent bypasses indentation validation entirely. Use when
    deliberately refactoring or inserting top-level content into indented files.

EXAMPLES:
    # Replace a function call
    sniper src/main.rs 42 42 $(echo 'new_call()' | sniper encode --stdin)

    # Delete a block
    sniper src/lib.rs 100 150 --delete

    # Batch edit with manifest
    echo '[{"start":1,"end":1,"hex":"78"}]' | sniper file.rs --manifest /dev/stdin

    # Safe workflow (dry-run first)
    sniper file.rs 1 5 7878 --dry-run && sniper file.rs 1 5 7878

    # Auto-indent unindented LLM output
    sniper file.rs 10 10 $(echo 'print("hello")' | sniper encode --stdin) --auto-indent

    # Context-verified edit (rejects if surrounding code changed)
    sniper file.rs 10 10 7878 --context 1a2b3c4d5e6f7a8b

CONFIGURATION:
    SNIPER_LOCK_TIMEOUT             Lock timeout in seconds (default: 30, min: 1)
    SNIPER_MAX_FILE_SIZE            Max file size, e.g. "100MB" (default: 100MB, 0=unlimited)
    SNIPER_BACKUP_RETENTION_COUNT   Backups to keep (default: 50, 0=unlimited)
    SNIPER_BACKUP_MAX_AGE_DAYS     Max backup age in days (default: 30, 0=no limit)
    SNIPER_DISABLE_AUDIT            Set to any value to disable audit logging
    SNIPER_DAL_LEVEL              Defense-Ascension Level: Baseline, Enhanced, Maximum (default: Baseline)
    SNIPER_PID_BASE_MS            PID base sleep in milliseconds (default: 0)
    SNIPER_PID_ENTROPY_SCALE      PID entropy multiplier (default: 0.5)
    SNIPER_PID_PRESSURE_SCALE     PID pressure multiplier (default: 1.0)

NOTES:
    - Line numbers: 1-indexed, inclusive on both ends
    - Empty content: Use empty hex string "" to delete
    - Insert at end: Use line N+1 where N is file length
    - All edits are atomic (temp file + rename)
    - PID-based file locks with stale lock auto-recovery
    - Metabolic pacing via llmosafe 0.7.1 ResourceGuard

For more: https://github.com/moeshawky/moesniper
"#;
