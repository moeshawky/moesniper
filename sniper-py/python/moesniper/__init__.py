"""Escape-proof precision file editing — Python bindings for moesniper.

Provides hex-encoded content operations, line-range splicing,
atomic writes, and undo via timestamped backups.

Usage:
    import sniper

    # Edit a file: replace lines 2-4 with new content
    result = sniper.edit("path/to/file.txt", 2, 4, "new content here")
    if result["status"] == "ok":
        print(f"Edit applied: {result}")

    # Delete lines 2-4
    result = sniper.delete("path/to/file.txt", 2, 4)

    # Batch edit via manifest
    ops = '[{"start": 1, "end": 1, "hex": "68656c6c6f"}, {"start": 3, "delete": true}]'
    result = sniper.manifest("path/to/file.txt", ops)

    # Undo last edit
    backup = sniper.undo("path/to/file.txt")

    # Hex encode/decode
    encoded = sniper.encode("Hello")
    decoded = sniper.decode(encoded)

    # Read file contents
    content = sniper.read_file("path/to/file.txt")

    # View configuration
    config = sniper.config()
"""

from moesniper._native import (
    sniper_config,
    sniper_decode,
    sniper_delete,
    sniper_edit,
    sniper_encode,
    sniper_manifest,
    sniper_read_file,
    sniper_undo,
)

edit = sniper_edit
delete = sniper_delete
manifest = sniper_manifest
undo = sniper_undo
encode = sniper_encode
decode = sniper_decode
read_file = sniper_read_file
config = sniper_config

__all__ = [
    "config",
    "decode",
    "delete",
    "edit",
    "encode",
    "manifest",
    "read_file",
    "undo",
]
