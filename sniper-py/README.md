# sniper-py

> Python bindings for moesniper — escape-proof precision file editor for LLM agents.

[![PyPI](https://img.shields.io/pypi/v/moesniper)](https://pypi.org/project/moesniper/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)]()
[![Python: 3.10+](https://img.shields.io/badge/python-3.10%2B-blue.svg)]()

## Installation

```bash
pip install moesniper
```

## Overview

`moesniper` provides native Python bindings to the [`moesniper`](https://github.com/moeshawky/moesniper) Rust CLI — an escape-proof precision file editor designed for LLM agents. All file edits are:

- **Hex-encoded** to prevent shell corruption
- **Atomic** (temp file + rename — never inconsistent)
- **Tracked** via automatic backups and multi-step undo
- **Paced** with metabolic resource guards to prevent runaway edits

## Usage

```python
import moesniper

# Replace line 5 with plaintext content. The binding handles encoding internally.
result = moesniper.edit("file.rs", start=5, end=5, content="hello")
print(result["status"])  # "ok"
print(result["lines_inserted"])  # 1
```

## Features

| Feature | Description |
|---------|-------------|
| **Hex-encoded payloads** | All content is hex strings, zero shell injection risk |
| **Atomic writes** | Files are never in an inconsistent state during edit |
| **Multi-step undo** | Each edit creates a backup; undo restores previous state |
| **Dry-run preview** | Preview diffs before applying changes |
| **Indentation safety** | Validates and auto-corrects indentation on edits |
| **Resource pacing** | Built-in metabolic guards prevent runaway edits |

## License

MIT ([source](https://github.com/moeshawky/moesniper))
