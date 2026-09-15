"""Type stubs for moesniper native Python bindings."""

from typing import Any

def sniper_edit(
    filepath: str,
    start: int,
    end: int,
    content: str,
    auto_indent: bool | None = None,
    force_indent: bool | None = None,
    context_hash: str | None = None,
    dry_run: bool | None = None,
) -> dict[str, Any]:
    """Edit a file by replacing lines in range [start, end] with new content.

    Replaces the specified line range with the given content string.
    Supports hex-encoded content, auto-indentation, context verification,
    and dry-run preview mode.
    """
    ...

def sniper_delete(filepath: str, start: int, end: int) -> dict[str, Any]:
    """Delete lines from a file in the range [start, end).

    Delegates to sniper_edit with an empty content string.
    """
    ...

def sniper_manifest(
    filepath: str,
    operations_json: str,
    auto_indent: bool | None = None,
    force_indent: bool | None = False,
    context_hash: str | None = None,
    dry_run: bool | None = None,
) -> dict[str, Any]:
    """Apply a batch of edit/delete operations from a JSON manifest string.

    Operations are applied bottom-up (by start line, descending) so that
    line numbers in earlier operations remain valid after later operations.
    Supports auto-indentation detection and per-operation indentation validation.
    """
    ...

def sniper_undo(filepath: str) -> str:
    """Restore a file to its most recent backup state.

    Returns the path to the backup that was restored.
    Raises RuntimeError if no backup exists.
    """
    ...

def sniper_encode(text: str) -> str:
    """Hex-encode a text string.

    Returns the hexadecimal representation of the UTF-8 bytes of the input.
    """
    ...

def sniper_decode(hex_str: str) -> str:
    """Hex-decode a hex string back to the original text.

    Strict decoding: skips whitespace, errors on non-hex or odd-length strings.
    Raises ValueError on invalid input.
    """
    ...

def sniper_read_file(filepath: str) -> str:
    """Read and return the full contents of a file as a string.

    Raises OSError if the file does not exist or cannot be read.
    """
    ...

def sniper_config() -> dict[str, Any]:
    """Return the active configuration as a dictionary.

    Includes lock_timeout_secs, max_file_size, backup_retention_count,
    backup_max_age_days, audit_enabled, and dal_level.
    Reflects environment variable overrides.
    """
    ...

def validate_indentation_py(filepath: str, start: int, content: str) -> dict[str, Any]:
    """Validate that content matches the indentation style of surrounding lines.

    Returns a dict with keys: valid (bool), message (str), suggested (str or None).
    """
    ...

def auto_indent_content_py(filepath: str, start: int, content: str) -> str:
    """Adjust content indentation to match surrounding context.

    Returns the re-indented content string.
    """
    ...

def needs_indent_fix_py(filepath: str, start: int, content: str) -> bool:
    """Check whether content needs indentation correction.

    Returns True if any content line is shallower than expected or
    does not match the surrounding context's prefix.
    """
    ...

def verify_context_py(filepath: str, start: int, end: int, expected_hash: str) -> dict[str, Any]:
    """Verify that the context around a line range has not changed.

    Computes the SHA-256 prefix of surrounding lines and compares it
    against the expected 16-hex-char hash. Returns dict with valid (bool).
    """
    ...

def recommend_from_risk_py() -> str:
    """Return a human-readable recommendation based on current resource risk.

    Maps risk score thresholds to advice strings:
    high pressure, moderate load, or resources nominal.
    """
    ...

def write_atomic_with_dal_py(filepath: str, content: str, dal_level: str) -> dict[str, Any]:
    """Write content atomically with Defense-Ascension Level gating.

    Performs resource safety check before file I/O, at the specified DAL level.
    Returns a dict with status, lines_removed, lines_inserted, and backup_path.
    """
    ...

def check_file_size_py(filepath: str, max_size: int) -> bool:
    """Check whether a file is within the specified size limit.

    Returns True if file size <= max_size (or max_size is 0/unlimited).
    Raises OSError if file does not exist or exceeds limit.
    """
    ...

def normalize_path_py(path: str) -> str:
    """Normalize a file path, resolving relative components.

    Returns the absolute normalized path. Rejects parent references (..).
    Raises ValueError on invalid paths.
    """
    ...

def create_backup_py(filepath: str) -> str:
    """Create a timestamped backup of a file.

    Returns the path to the backup file in .sniper/ directory.
    """
    ...

def find_latest_backup_py(filepath: str) -> str | None:
    """Find the most recent backup for a given file.

    Returns the backup path or None if no backup exists.
    """
    ...

def count_recent_backups_py(filepath: str, window_secs: int) -> int:
    """Count backups created within the last window_secs seconds.

    Returns the number of recent backups for the file.
    """
    ...

def purge_old_backups_py(filepath: str, retention_count: int, max_age_days: int) -> int:
    """Purge old backups according to retention policy.

    Returns the number of backup files removed.
    """
    ...

def version_py() -> dict[str, Any]:
    """Return version information as a dict with name and version keys.

    name is the library name, version is the semver version string.
    """
    ...

def generate_preview_py(filepath: str, start: int, end: int, replacement: str) -> dict[str, Any]:
    """Generate a dry-run diff preview for a file edit.

    Returns a dict with a preview list showing before/after line ranges.
    """
    ...
