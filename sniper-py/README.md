# sniper-py

> Python bindings for moesniper — escape-proof precision file editor for LLM agents.

[![PyPI](https://img.shields.io/pypi/v/sniper-py)](https://pypi.org/project/sniper-py/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)]()
[![Python: 3.10+](https://img.shields.io/badge/python-3.10%2B-blue.svg)]()

## Installation

```bash
pip install sniper-py
```

## Overview

`sniper-py` provides native Python bindings to the [`moesniper`](https://github.com/moeshawky/moesniper) Rust CLI — an escape-proof precision file editor designed for LLM agents. All file edits are:

- **Hex-encoded** to prevent shell corruption
- **Atomic** (temp file + rename — never inconsistent)
- **Tracked** via automatic backups and multi-step undo
- **Paced** with metabolic resource guards to prevent runaway edits

## Usage

```python
from sniper import splice

# Replace lines 5-5 with hex-encoded content
result = splice("file.rs", start=5, end=5, hex_content="68656c6c6f")
print(result.status)  # "ok"
print(result.line_shift)  # 0
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
