# moesniper

Escape-proof precision file editor for LLM agents. All content is hex-encoded to guarantee zero shell corruption. Edits are applied via line-range splicing with atomic writes and automatic backups.

## Install

```bash
cargo install moesniper
```

## Usage

```bash
# Replace lines 10-12 with hex-decoded content
sniper file.rs 10 12 7573652070657473

# Delete lines 5-8
sniper file.rs 5 8 --delete

# Batch operations from a manifest file
sniper file.rs --manifest ops.json

# Undo last edit (restores backup)
sniper file.rs --undo
```

### Flags

| Flag | Description |
|------|-------------|
| `--dry-run` | Preview changes without writing |
| `--json` | Output machine-readable JSON |
| `--help` | Show full usage (including LLM agent guide) |

## LLM Agent Workflow

```bash
# 1. Find the lines you want to edit (using ix)
ix "fn main" file.rs

# 2. Encode your replacement content
echo -n 'fn main() {}' | xxd -p | tr -d '\n'
# Output: 666e206d61696e2829207b7d

# 3. Dry-run to preview (safe - never modifies files)
sniper --json --dry-run file.rs 5 5 666e206d61696e2829207b7d

# 4. Apply the edit
sniper --json file.rs 5 5 666e206d61696e2829207b7d

# 5. Undo if something went wrong
sniper --json file.rs --undo
```

## Hex Encoding

All replacement content must be hex-encoded. This prevents shell mangling of special characters.

```bash
# Encode your text
echo -n 'use petgraph' | xxd -p
# Output: 757365207065746772617068

# Decode a hex string
echo 757365207065746772617068 | xxd -r -p
```

## Manifest Format

A manifest is a JSON array of operations applied **bottom-up** (highest line first) so line numbers in the original file stay valid:

```json
[
  {"start": 10, "end": 12, "hex": "6e6577"},
  {"start": 5, "end": 7, "delete": true},
  {"start": 1, "hex": "6869"}
]
```

- `start`/`end`: 1-indexed line range (inclusive)
- `hex`: replacement content (hex-encoded)
- `delete`: set to `true` to delete the range (no hex needed)

Run: `sniper target_file.rs --manifest ops.json`

## JSON Output

```bash
sniper --json file.rs 5 5 6869
```

```json
{
  "status": "ok",
  "file": "file.rs",
  "lines_removed": 1,
  "lines_inserted": 1,
  "total_lines": 42,
  "backup": ".sniper/file.rs.1712345678"
}
```

## Backups

Every edit automatically creates a backup in `.sniper/`:

```
.sniper/
  file.rs.1712345678    # timestamped backup
```

Undo restores the most recent backup: `sniper file.rs --undo`

## License

MIT
