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


# ── Indentation Utilities ─────────────────────────────────────────────────


class TestValidateIndentation:
    """Test validate_indentation Python binding."""

    @pytest.fixture
    def indented_file(self, tmp_path: Path) -> Path:
        """Create a test file with Python-style indentation."""
        f = tmp_path / "indented.py"
        f.write_text("def foo():\n    pass\n")
        return f

    def test_missing_indent_reported(self, sniper, indented_file):
        """Unindented replacement at expected-indent site fails validation."""
        result = sniper.validate_indentation(str(indented_file), 2, 2, "print('hello')")
        assert result["valid"] is False
        assert "4 space" in result["message"]

    def test_correct_indent_passes(self, sniper, indented_file):
        """Correctly indented replacement passes validation."""
        result = sniper.validate_indentation(str(indented_file), 2, 2, "    print('hello')")
        assert result["valid"] is True


class TestAutoIndentContent:
    """Test auto_indent_content Python binding."""

    def test_adds_expected_indent(self, sniper, test_file):
        """Auto-indent prepends the detected indent level."""
        test_file.write_text("def foo():\n    pass\n")
        result = sniper.auto_indent_content(str(test_file), 2, 2, "print('hello')")
        assert result == "    print('hello')"


class TestNeedsIndentFix:
    """Test needs_indent_fix Python binding."""

    def test_detects_unindented(self, sniper, test_file):
        """Unindented content triggers needs_indent_fix."""
        test_file.write_text("def foo():\n    pass\n")
        assert sniper.needs_indent_fix(str(test_file), 2, 2, "print('x')") is True

    def test_correct_indent_passes(self, sniper, test_file):
        """Already-correct indent returns False."""
        test_file.write_text("def foo():\n    pass\n")
        assert sniper.needs_indent_fix(str(test_file), 2, 2, "    print('x')") is False


# ── Context Verification ─────────────────────────────────────────────────


class TestVerifyContext:
    """Test verify_context Python binding."""

    def test_hash_mismatch(self, sniper, test_file):
        """A known-bad hash returns valid=False."""
        result = sniper.verify_context(str(test_file), 3, 3, "0000000000000000")
        assert result["valid"] is False


# ── Risk / Write DAL ──────────────────────────────────────────────────────


class TestRecommendFromRisk:
    """Test recommend_from_risk Python binding."""

    def test_returns_string(self, sniper):
        """recommend_from_risk returns a non-empty string."""
        rec = sniper.recommend_from_risk()
        assert isinstance(rec, str)
        assert len(rec) > 0


class TestWriteAtomicWithDal:
    """Test write_atomic_with_dal Python binding."""

    def test_writes_baseline(self, sniper, tmp_path):
        """Baseline DAL level writes content to file."""
        path = str(tmp_path / "dal_test.txt")
        assert not Path(path).exists()
        result = sniper.write_atomic_with_dal(path, "hello\nworld", "BASELINE")
        assert result["status"] == "ok"
        assert Path(path).read_text() == "hello\nworld"

    def test_invalid_dal_level(self, sniper):
        """Invalid DAL level raises ValueError."""
        with pytest.raises(ValueError):
            sniper.write_atomic_with_dal("/tmp/ignored.txt", "x", "INVALID")


# ── File Utilities ────────────────────────────────────────────────────────


class TestCheckFileSize:
    """Test check_file_size Python binding."""

    def test_within_limit(self, sniper, test_file):
        """File within size limit returns True."""
        size = test_file.stat().st_size
        assert sniper.check_file_size(str(test_file), size + 1) is True

    def test_exceeds_limit(self, sniper, test_file):
        """File exceeding limit raises OSError."""
        size = test_file.stat().st_size
        with pytest.raises(OSError):
            sniper.check_file_size(str(test_file), size - 1)


class TestNormalizePath:
    """Test normalize_path Python binding."""

    def test_normalizes_relative(self, sniper, test_file):
        """Relative path is expanded to absolute."""
        normalized = sniper.normalize_path(str(test_file))
        assert Path(normalized).is_absolute()

    def test_rejects_traversal(self, sniper):
        """Path traversal raises ValueError."""
        with pytest.raises(ValueError):
            sniper.normalize_path("../../../etc/passwd")


class TestCreateBackup:
    """Test create_backup Python binding."""

    def test_creates_backup_file(self, sniper, test_file):
        """create_backup returns path to an existing backup."""
        path = sniper.create_backup(str(test_file))
        assert Path(path).exists()
        assert ".sniper" in path


class TestFindLatestBackup:
    """Test find_latest_backup Python binding."""

    def test_returns_none_when_no_backup(self, sniper, test_file):
        """No backups means None result."""
        assert sniper.find_latest_backup(str(test_file)) is None

    def test_finds_backup_after_edit(self, sniper, test_file):
        """Edit creates a backup that find_latest_backup can locate."""
        sniper.edit(str(test_file), 1, 1, "changed")
        latest = sniper.find_latest_backup(str(test_file))
        assert latest is not None


class TestCountRecentBackups:
    """Test count_recent_backups Python binding."""

    def test_counts_within_window(self, sniper, test_file):
        """Recently created backups are counted."""
        sniper.edit(str(test_file), 1, 1, "a")
        sniper.edit(str(test_file), 1, 1, "b")
        count = sniper.count_recent_backups(str(test_file), 3600)
        assert isinstance(count, int)
        assert count >= 1


class TestPurgeOldBackups:
    """Test purge_old_backups Python binding."""

    def test_purge_returns_int(self, sniper, test_file):
        """purge_old_backups returns an integer count."""
        purged = sniper.purge_old_backups(str(test_file), 50, 30)
        assert isinstance(purged, int)


# ── Version ───────────────────────────────────────────────────────────────


class TestVersion:
    """Test version Python binding."""

    def test_returns_name_and_version(self, sniper):
        """version() returns dict with name and version keys."""
        v = sniper.version()
        assert isinstance(v, dict)
        assert "name" in v
        assert "version" in v
        assert v["name"] == "moesniper"

    def test_version_is_semver(self, sniper):
        """version string follows semver pattern."""
        v = sniper.version()
        parts = v["version"].split(".")
        assert len(parts) == 3
        assert all(p.isdigit() for p in parts)


# ── Generate Preview ──────────────────────────────────────────────────────


class TestGeneratePreview:
    """Test generate_preview Python binding."""

    def test_generates_preview_lines(self, sniper, test_file):
        """generate_preview returns a dict with preview list."""
        result = sniper.generate_preview(str(test_file), 2, 4, "replacement\n")
        assert "preview" in result
        assert isinstance(result["preview"], list)
        assert len(result["preview"]) > 0


# ── Manifest with Indent ──────────────────────────────────────────────────


class TestManifestIndent:
    """Test manifest operation with indent parameters."""

    def test_manifest_dry_run(self, sniper, test_file):
        """Manifest dry_run does not modify the file."""
        path = str(test_file)
        original = test_file.read_text()
        ops = json.dumps([{"start": 1, "end": 1, "hex": sniper.encode("x")}])
        result = sniper.manifest(path, ops, dry_run=True)
        assert result["status"] == "ok"
        assert test_file.read_text() == original

    def test_manifest_force_indent(self, sniper, test_file):
        """Manifest with force_indent=True bypasses validation."""
        path = str(test_file)
        ops = json.dumps([{"start": 1, "end": 1, "hex": sniper.encode("x")}])
        result = sniper.manifest(path, ops, force_indent=True)
        assert result["status"] == "ok"
        assert result["backup_path"] != ""

    def test_manifest_returns_risk(self, sniper, test_file):
        """Manifest result includes risk, recommended_action, and backup_path."""
        path = str(test_file)
        ops = json.dumps([{"start": 1, "end": 1, "hex": sniper.encode("x")}])
        result = sniper.manifest(path, ops)
        assert result["status"] == "ok"
        assert "risk" in result
        assert "recommended_action" in result
        assert "backup_path" in result

    def test_manifest_no_indent_needed(self, sniper, test_file):
        """Manifest with auto_indent=True on already-correct content succeeds."""
        path = str(test_file)
        ops = json.dumps([{"start": 1, "end": 1, "hex": sniper.encode("x")}])
        result = sniper.manifest(path, ops, auto_indent=True)
        assert result["status"] == "ok"
