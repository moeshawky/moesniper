"""Tests for sniper-py Python bindings.

DNA:
    Tests verify encode/decode round-trip, read_file, edit, delete,
    manifest, undo, and config operations against the native PyO3 module.
    Each test uses tmp_path fixtures to isolate file operations.
"""

import json
from pathlib import Path

import pytest


@pytest.fixture
def sniper():
    """Import the installed moesniper package."""
    import moesniper as s

    return s


@pytest.fixture
def test_file(tmp_path: Path) -> Path:
    """Create a small test file with 5 numbered lines."""
    f = tmp_path / "test.txt"
    f.write_text("line1\nline2\nline3\nline4\nline5\n")
    return f


# ── Encoder / Decoder ────────────────────────────────────────────────────


class TestEncodeDecode:
    """Test hex encode/decode round-trip and edge cases."""

    def test_roundtrip(self, sniper):
        """Encode then decode returns original text."""
        text = "Hello, World!"
        assert sniper.decode(sniper.encode(text)) == text

    def test_empty_string(self, sniper):
        """Empty string round-trips correctly."""
        assert sniper.encode("") == ""
        assert sniper.decode("") == ""

    def test_unicode(self, sniper):
        """Unicode characters survive encode/decode."""
        text = "café résumé 日本語 🎉"
        encoded = sniper.encode(text)
        decoded = sniper.decode(encoded)
        assert decoded == text

    def test_decode_invalid_hex(self, sniper):
        """Non-hex characters raise ValueError."""
        with pytest.raises(ValueError, match="hex decode"):
            sniper.decode("zz")

    def test_decode_odd_length(self, sniper):
        """Odd-length hex raises ValueError."""
        with pytest.raises(ValueError, match="hex decode"):
            sniper.decode("486")


# ── read_file ─────────────────────────────────────────────────────────────


class TestReadFile:
    """Test reading file contents."""

    def test_read_existing_file(self, sniper, test_file):
        """Reading an existing file returns its contents."""
        content = sniper.read_file(str(test_file))
        assert content == "line1\nline2\nline3\nline4\nline5\n"

    def test_read_nonexistent_file(self, sniper):
        """Reading a missing file raises FileNotFoundError (IOError)."""
        with pytest.raises(OSError):
            sniper.read_file("/tmp/__nonexistent_sniper_test_file__")


# ── Edit (splice) ─────────────────────────────────────────────────────────


class TestEdit:
    """Test the edit operation (sniper_edit / sniper.edit)."""

    def test_replace_single_line(self, sniper, test_file):
        """Replacing one line changes the file content."""
        path = str(test_file)
        result = sniper.edit(path, 2, 2, "hello")
        assert result["status"] == "ok"
        assert result["lines_removed"] == 1
        assert result["lines_inserted"] == 1
        content = test_file.read_text()
        assert content == "line1\nhello\nline3\nline4\nline5\n"

    def test_replace_range(self, sniper, test_file):
        """Replacing a range removes old lines and inserts new."""
        path = str(test_file)
        result = sniper.edit(path, 2, 4, "X\nY")
        assert result["status"] == "ok"
        assert result["lines_removed"] == 3
        assert result["lines_inserted"] == 2
        content = test_file.read_text()
        assert content == "line1\nX\nY\nline5\n"

    def test_insert_at_end(self, sniper, test_file):
        """Inserting beyond the last line appends content."""
        path = str(test_file)
        result = sniper.edit(path, 6, 6, "appended")
        assert result["status"] == "ok"
        content = test_file.read_text()
        assert content == "line1\nline2\nline3\nline4\nline5\nappended\n"

    def test_out_of_bounds(self, sniper, test_file):
        """Start line beyond file length + 1 returns error."""
        path = str(test_file)
        result = sniper.edit(path, 99, 99, "x")
        assert result["status"] == "error"
        assert "out of bounds" in result["message"]

    def test_backup_created(self, sniper, test_file):
        """Edit creates a backup file."""
        path = str(test_file)
        result = sniper.edit(path, 1, 1, "modified")
        assert result["status"] == "ok"
        assert "backup_path" in result
        backup = Path(result["backup_path"])
        assert backup.exists()


# ── Delete ────────────────────────────────────────────────────────────────


class TestDelete:
    """Test the delete operation (sniper_delete / sniper.delete)."""

    def test_delete_single_line(self, sniper, test_file):
        """Deleting one line removes it from the file."""
        path = str(test_file)
        result = sniper.delete(path, 2, 2)
        assert result["status"] == "ok"
        assert result["lines_removed"] == 1
        assert result["lines_inserted"] == 0
        content = test_file.read_text()
        assert content == "line1\nline3\nline4\nline5\n"

    def test_delete_range(self, sniper, test_file):
        """Deleting a range removes multiple lines."""
        path = str(test_file)
        result = sniper.delete(path, 2, 4)
        assert result["status"] == "ok"
        assert result["lines_removed"] == 3
        content = test_file.read_text()
        assert content == "line1\nline5\n"


# ── Manifest ──────────────────────────────────────────────────────────────


class TestManifest:
    """Test batch manifest operations."""

    def test_batch_edit(self, sniper, test_file):
        """Multiple operations in a manifest are applied bottom-up."""
        path = str(test_file)
        ops = json.dumps(
            [
                {"start": 1, "end": 1, "hex": sniper.encode("x")},
                {"start": 3, "end": 4, "delete": True},
            ]
        )
        result = sniper.manifest(path, ops)
        assert result["status"] == "ok"
        assert result["operations"] == 2
        content = test_file.read_text()
        assert content == "x\nline2\nline5\n"

    def test_manifest_invalid_json(self, sniper, test_file):
        """Invalid JSON string returns error."""
        path = str(test_file)
        result = sniper.manifest(path, "not json")
        assert result["status"] == "error"
        assert "parse" in result["message"].lower()

    def test_manifest_invalid_hex(self, sniper, test_file):
        """Invalid hex in manifest returns error before any edits."""
        path = str(test_file)
        ops = json.dumps([{"start": 1, "hex": "zz"}])
        result = sniper.manifest(path, ops)
        assert result["status"] == "error"
        assert "hex" in result["message"].lower()


# ── Undo ──────────────────────────────────────────────────────────────────


class TestUndo:
    """Test undo restores original file content."""

    def test_undo_restores(self, sniper, test_file):
        """Undo after an edit restores the original content."""
        path = str(test_file)
        original = test_file.read_text()

        sniper.edit(path, 1, 1, "changed")
        assert test_file.read_text() != original

        backup = sniper.undo(path)
        assert backup != ""
        assert test_file.read_text() == original

    def test_undo_no_backup(self, sniper, test_file):
        """Undo on a file with no backup raises RuntimeError."""
        with pytest.raises(RuntimeError):
            sniper.undo(str(test_file))


# ── Config ────────────────────────────────────────────────────────────────


class TestConfig:
    """Test configuration access."""

    def test_config_returns_dict(self, sniper):
        """sniper.config() returns a dict with expected keys."""
        cfg = sniper.config()
        assert isinstance(cfg, dict)
        assert "lock_timeout_secs" in cfg
        assert "max_file_size" in cfg
        assert "backup_retention_count" in cfg
        assert "backup_max_age_days" in cfg
        assert "audit_enabled" in cfg
        assert "dal_level" in cfg
        assert cfg["lock_timeout_secs"] == 30
        assert cfg["max_file_size"] == 100 * 1024 * 1024
        assert cfg["backup_retention_count"] == 50
        assert cfg["audit_enabled"] is True

    def test_config_reflects_env(self, sniper, monkeypatch):
        """Config picks up environment variable overrides."""
        monkeypatch.setenv("SNIPER_LOCK_TIMEOUT", "5")
        cfg = sniper.config()
        assert cfg["lock_timeout_secs"] == 5
