pub const HELP: &str = r#"sniper — precision file editor for LLM agents

USAGE:
    sniper <file> <start> <end> <hex>       Replace lines with hex content
    sniper <file> <start> <end> --delete    Delete line range
    sniper <file> --manifest <path>         Batch operations from JSON
    sniper <file> --undo                    Restore from backup
    sniper encode [--stdin|--file <path>]   Hex-encode content

FLAGS:
    --dry-run       Preview changes without applying
    --json          Output machine-readable JSON
    --auto-indent   Auto-detect and apply indentation

QUICK START:
    # Replace line 5 with "hello"
    sniper file.rs 5 5 68656c6c6f

    # Delete lines 10-20
    sniper file.rs 10 20 --delete

    # Preview before applying
    sniper file.rs 1 5 787878 --dry-run

    # Undo last edit
    sniper file.rs --undo

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

EXAMPLES:
    Replace a function call:
        sniper src/main.rs 42 42 $(echo 'new_call()' | sniper encode --stdin)

    Delete a block:
        sniper src/lib.rs 100 150 --delete

    Batch edit with manifest:
        echo '[{"start":1,"end":1,"hex":"78"}]' | sniper file.rs --manifest /dev/stdin

    Safe workflow (dry-run first):
        sniper file.rs 1 5 7878 --dry-run && sniper file.rs 1 5 7878

NOTES:
    - Line numbers: 1-indexed, inclusive on both ends
    - Empty content: Use empty string "" to delete
    - Insert at end: Use line N+1 where N is file length
    - All edits are atomic (temp file + rename)

For more: https://github.com/moeshawky/moesniper
"#;
