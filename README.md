# moesniper

> Escape-proof precision file editor for LLM agents. Hex-encoded content, line-range splicing, atomic writes, metabolic pacing.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)]()
[![Rust: 1.87+](https://img.shields.io/badge/rust-1.87%2B-orange.svg)]()

## Quick Start

```bash
cargo install moesniper

# Replace line 5
sniper file.rs 5 5 68656c6c6f

# Undo
sniper file.rs --undo
```

## Features

| Feature | What It Does |
|---------|-------------|
| **Hex-encoded content** | Zero shell corruption — all payloads are hex strings |
| **Line-range splicing** | Replace or delete any contiguous line range, 1-indexed |
| **Atomic writes** | Temp file + `rename(2)` — file is never in an inconsistent state |
| **Multi-step undo** | Each edit creates a backup; `--undo` pops the stack |
| **Manifest operations** | Batch JSON operations applied bottom-up (line numbers never shift) |
| **Dry-run preview** | `--dry-run` shows diff with `+`/`-`/`~` markers before applying |
| **Auto-indent** | `--auto-indent` detects expected indentation from context and fixes unindented LLM output |
| **Validate-indent** | `--validate-indent` warns on indentation mismatch without modifying content |
| **PID file locks** | Per-file locks with stale PID detection — auto-recovery if previous process died |
| **Metabolic pacing** | `llmosafe 0.6.2` `ResourceGuard::auto(0.5)` — adaptive back-pressure based on RSS, IO wait, and load |
| **Path traversal protection** | `..` components rejected, `SecurityPolicy` guards all file access |
| **Configurable limits** | File size, backup retention, lock timeout — all via environment variables |
| **JSON output** | `--json` for machine-readable results |
| **Backup retention** | Count-based + age-based purge policies |

## Usage

```
sniper <file> <start> <end> <hex>       Replace lines
sniper <file> <start> <end> --delete    Delete lines
sniper <file> --manifest <path>         Batch operations (JSON)
sniper <file> --undo                    Restore from backup
sniper encode [--stdin|<text>]          Hex-encode content
sniper encode --file <path>             Encode file contents
```

### Flags

| Flag | Effect |
|------|--------|
| `--dry-run` | Preview changes without writing |
| `--json` | Output machine-readable JSON |
| `--stdin` | Read content from stdin instead of hex arg |
| `--auto-indent` | Auto-detect and apply indentation |
| `--validate-indent` | Warn on indentation mismatch (dry-run only blocks non-dry-run) |

### Encoding

Content must be hex-encoded to prevent shell corruption:

```bash
echo -n 'fn main() {}' | sniper encode --stdin
# 666e206d61696e2829207b7d
```

### Manifest Format

```json
[
  {"start": 42, "end": 45, "hex": "6e6577"},
  {"start": 10, "delete": true}
]
```

Operations apply bottom-up (highest line first, so earlier edits don't shift later targets). Line numbers are 1-indexed.

### Examples

```bash
# Replace a function
sniper src/main.rs 42 42 $(echo 'new_call()' | sniper encode --stdin)

# Delete a block
sniper src/lib.rs 100 150 --delete

# Safe workflow (preview first)
sniper file.rs 1 5 7878 --dry-run && sniper file.rs 1 5 7878

# Batch edit via manifest
echo '[{"start":1,"end":1,"hex":"78"}]' | sniper file.rs --manifest /dev/stdin

# Insert at end of file
sniper file.rs 4 3 $(echo 'new_line' | sniper encode --stdin)
```

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `SNIPER_LOCK_TIMEOUT` | `30` | Lock acquisition timeout (seconds, min 1) |
| `SNIPER_MAX_FILE_SIZE` | `100MB` | Maximum file size to edit (`0` = unlimited) |
| `SNIPER_BACKUP_RETENTION_COUNT` | `50` | Number of backups to retain (`0` = unlimited) |
| `SNIPER_BACKUP_MAX_AGE_DAYS` | `30` | Max backup age in days (`0` = no limit) |
| `SNIPER_DISABLE_AUDIT` | (unset) | Set to any value to disable audit logging |

## How It Works

1. **Path validation** — `normalize_path()` rejects `..` traversal and canonicalizes
2. **File size check** — rejects files exceeding `SNIPER_MAX_FILE_SIZE`
3. **Lock acquisition** — PID file lock with configurable timeout; stale PID detection recovers from crashed processes
4. **Backup** — file copied to `.sniper/` with hash+timestamp name
5. **Splice** — lines loaded into memory, range replaced, result written atomically (temp file + rename)
6. **Metabolic pacing** — `llmosafe` checks RSS, IO wait, load average; sleeps if system is under pressure
7. **Purge** — old backups pruned by count and age per retention policy

## Contributing

See [CONTRIBUTING.md](docs/CONTRIBUTING.md).

## License

MIT — see [LICENSE](LICENSE).
