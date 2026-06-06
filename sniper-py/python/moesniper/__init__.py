"""Escape-proof precision file editing — Python bindings for moesniper.

Provides hex-encoded content operations, line-range splicing,
atomic writes, and undo via timestamped backups.

Usage:
    import moesniper

    # Version
    v = moesniper.version()

    # High-level editing
    result = moesniper.edit("path/to/file.txt", 2, 4, "new content here")
    result = moesniper.delete("path/to/file.txt", 2, 4)
    result = moesniper.manifest("path/to/file.txt", ops_json)
    backup = moesniper.undo("path/to/file.txt")

    # Encoding
    encoded = moesniper.encode("Hello")
    decoded = moesniper.decode(encoded)
    content = moesniper.read_file("path/to/file.txt")
    cfg = moesniper.config()

    # Indentation utilities
    ok = moesniper.validate_indentation("file.rs", 10, 12, "    print(x)")
    fixed = moesniper.auto_indent_content("file.rs", 10, 12, "print(x)")
    needs = moesniper.needs_indent_fix("file.rs", 10, 12, "print(x)")

    # Context verification
    res = moesniper.verify_context("file.rs", 10, 12, "1a2b3c4d5e6f7a8b")

    # Risk / DAL
    rec = moesniper.recommend_from_risk()
    res = moesniper.write_atomic_with_dal("file.txt", "content", "ENHANCED")

    # File utilities
    normalized = moesniper.normalize_path("~/file.txt")
    backup = moesniper.create_backup("file.txt")
    latest = moesniper.find_latest_backup("file.txt")
    count = moesniper.count_recent_backups("file.txt", 3600)
    within = moesniper.check_file_size("file.txt", 1024 * 1024)
    purged = moesniper.purge_old_backups("file.txt", 50, 30)
"""

from moesniper._native import (  # noqa: I001
    auto_indent_content_py as auto_indent_content,
    check_file_size_py as check_file_size,
    count_recent_backups_py as count_recent_backups,
    create_backup_py as create_backup,
    find_latest_backup_py as find_latest_backup,
    needs_indent_fix_py as needs_indent_fix,
    normalize_path_py as normalize_path,
    purge_old_backups_py as purge_old_backups,
    recommend_from_risk_py as recommend_from_risk,
    sniper_config,
    sniper_decode,
    sniper_delete,
    sniper_edit,
    sniper_encode,
    sniper_manifest,
    sniper_read_file,
    sniper_undo,
    validate_indentation_py as validate_indentation,
    verify_context_py as verify_context,
    version_py as version,
    write_atomic_with_dal_py as write_atomic_with_dal,
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
    "auto_indent_content",
    "check_file_size",
    "config",
    "count_recent_backups",
    "create_backup",
    "decode",
    "delete",
    "edit",
    "encode",
    "find_latest_backup",
    "manifest",
    "needs_indent_fix",
    "normalize_path",
    "purge_old_backups",
    "read_file",
    "recommend_from_risk",
    "undo",
    "validate_indentation",
    "verify_context",
    "version",
    "write_atomic_with_dal",
]
