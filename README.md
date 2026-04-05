# moesniper 🎯

> Escape-proof precision file editor for LLM agents offering hex-encoded content, line-range splicing, and atomic writes.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)]()

## See It In Action

```bash
# Splice lines 10-20 safely with LLMOSafe throttling
sniper splice target.rs 10 20 "hex_encoded_content"
```

## Quick Start

```bash
cargo install --path .
sniper undo target.rs
```

## The Contract
`moesniper` enables LLMs to perform atomic, backup-preserving edits. Now integrated with LLMOSafe v0.4.0 for metabolic integrity (auto-throttling on high I/O) and Backtrack Signal (-7) handling.

## The Engine
Uses temporary files and atomic renaming. Edits are processed through memory, guarded by LLMOSafe. If a write triggers a back-pressure signal, the rename is aborted, and backups are preserved.

## Context
Text editing is brittle for LLMs. `moesniper` replaces naive `sed` or `cat` patching with precision, deterministic splicing.