# moesniper 🎯

> Escape-proof precision file editor for LLM agents offering hex-encoded content, line-range splicing, and atomic writes.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)]()

## See It In Action

```bash
# 1. Encode your text safely
HEX=$(sniper encode "fn main() { println!(\"Hello World\"); }")

# 2. Splice lines 1-1 with the hex payload
sniper target.rs 1 1 $HEX
```

## Features

- **Multi-Step Undo**: Support for sequential rollbacks. Each `--undo` pops the stack.
- **Path Normalization**: Uses canonicalized paths to ensure history consistency across relative/absolute path calls.
- **Strict Hex Decoding**: Errors on malformed or odd-length hex strings to prevent silent data corruption.
- **Internal Encoding**: `sniper encode` sub-command for generating payloads without external `xxd` dependencies.
- **Metabolic Safety**: Integrated with `llmosafe` v0.4.2 for adaptive back-pressure and load-aware throttling.
- **Newline Fidelity**: Measures and preserves the original file's trailing newline state.

## Quick Start

```bash
cargo install --path .

# Edit a file
sniper file.rs 10 15 "68656c6c6f"

# Roll back multiple steps
sniper file.rs --undo
sniper file.rs --undo
```

## Contributing
Contributions are welcome! Please see [CONTRIBUTING.md](docs/CONTRIBUTING.md) for details.

## License
`moesniper` is released under the [MIT License](LICENSE).

## The Engine
Uses temporary files and atomic renaming. Edits are processed through memory, guarded by `llmosafe`'s `ResourceGuard`. If system entropy is high, the tool applies dynamic back-pressure (sleep) before completing the write.

## Context
Text editing is brittle for LLMs due to shell escaping and terminal corruption. `moesniper` replaces naive `sed` or `cat` patching with precision, deterministic splicing using hex-encoded payloads.
