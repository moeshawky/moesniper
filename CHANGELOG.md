# Changelog

All notable changes to moesniper will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v0.0.0.html).

## [0.6.0] - 2026-05-28

### Added

#### Indentation Intelligence
- Auto-indent (`--auto-indent`): Detects expected indentation from surrounding context and automatically prepends missing leading whitespace. Statistical step detection with 80% supermajority, raw frequency scoring, round-to-nearest, whitespace stripping, and smaller-is-step tiebreaker — robust against "stupid indentation" from LLM output.
- Validate-indent (`--validate-indent`): Checks for indentation mismatch without modifying content. Blocks non-dry-run edits on mismatch. **Removed in 0.6.0** — validation is now always-on with `--force-indent` as the opt-out.
- Complete indent engine rewrite (`src/indent.rs`) with 40 adversarial tests covering tabs, spaces, mixed styles, blank lines, continuation lines, brace openers, and LLM-grade inconsistent spacing.

#### Lock Hardening
- PID-based lock files: lock content contains the process PID, enabling stale lock detection.
- Stale lock auto-recovery: On timeout, reads lock PID, checks `/proc/{pid}` liveness. Dead process → remove lock and retry. Garbage/non-numeric lock content treated as "not stale" (safe default).
- Configurable lock timeout via `SNIPER_LOCK_TIMEOUT`.

#### Secure Temp Files
- Nanosecond timestamp suffix on temp files (`{filepath}.sniper_tmp.{ts}`) instead of fixed name, preventing collision.

#### Path Validation Order
- `normalize_path` called before `check_file_size` in both `cmd_splice` and `cmd_manifest_impl`, eliminating a TOCTOU window between path validation and file operations.

#### Comprehensive Test Suite
- Boundary verification tests (`tests/boundary_tests.rs`): 29 tests covering PID lock acquisition, stale lock recovery, temp file uniqueness, path ordering, hex edge cases, backup contract, manifest validation, and concurrent lock purge.
- Regression golden file tests — undo stack behavior baseline, newline preservation, manifest regression detection.
- 184 total tests (19 lib, 90 main, 29 boundary, 19 enterprise, 6 integration, 6 unit edge, 5 property, 5 regression, 5 smoke).

### Changed

- **llmosafe 0.5.0 → 0.6.2**: `ResourceGuard::auto(0.5)` replaces hardcoded 256MB ceiling, adapting to deployment environment. Added `ResourceGuard::check()` call before atomic rename for proper `KernelError` propagation.
- **Dead code removed**: Removed `use_os_locking` config field and `SNIPER_USE_OS_LOCKING` env var (feature flag was parsed but never consumed — `flock()` was never implemented). Removed `libc` dependency (zero imports).
- **Clippy clean**: `field_reassign_with_default` fixed, `if_same_then_else` blocks merged, `is_multiple_of()` used instead of manual mod.
- **MSRV**: 1.70 → 1.87 (`is_multiple_of` stabilized in Rust 1.87).
- **Cargo.toml**: Added `rust-version = "1.87"` for MSRV discoverability.
- **Documentation**: `CHANGELOG.md`, `README.md` rewritten with v0.6.0 feature coverage. Help text expanded with `--stdin`, `--context`, `--force-indent`, auto-indent, and env var reference. Root docs consolidated into `docs/`.

### Security

- Path traversal protection (`SecurityPolicy`, `normalize_path`) — CWE-22 covered.
- Symlink-safe operations (no `follow_symlinks` without validation).
- PID-based per-file locking with stale lock auto-recovery.
- Atomic writes with unique temp file names.

### Audit

- **Structural audit**: All pub exports verified, zero dead code, all config fields consumed.
- **Publish signal**: GREEN — `cargo publish --dry-run` passes, 184 tests, 0 failures.

---

## [0.5.1] - 2026-05-14

### Added
- Comprehensive test suite: 85+ tests organized by failure mode (smoke, edge case, property-based, golden regression).
- Test documentation (`tests/README.md`).

### Changed
- Help text extracted to `src/help_text.rs` with improved organization.
- Documentation improvements.

---

## [0.5.0] - 2026-05-14

### Added
- Enterprise-grade path security validation (`src/security.rs`)
- Auto-indent and validate-indent flags
- Dry-run diff preview functionality
- Configuration via environment variables:
  - `SNIPER_LOCK_TIMEOUT`
  - `SNIPER_MAX_FILE_SIZE`
  - `SNIPER_BACKUP_RETENTION_COUNT`
  - `SNIPER_BACKUP_MAX_AGE_DAYS`
- AI hint system for verification guidance
- JSON output mode (`--json` flag)

### Changed
- Upgraded to llmosafe 0.5.0 with Backtrack Signal support
- Improved trailing newline handling (PR #8)
- Optimized string handling with `split_inclusive`
- Enhanced error messages with actionable hints

### Security
- Path traversal protection
- Symlink attack mitigation
- File locking with configurable timeout
- Atomic file writes

---

## [0.4.0] - 2025-04-22

### Added
- Manifest batch operations
- Multi-step undo stack
- Hex encoding command (`sniper encode`)
- Backup retention policies

### Changed
- Improved line number parsing (1-indexed)
- Better error handling for out-of-bounds operations

---

## [0.3.0] - 2025-04-21

### Added
- Basic line splicing functionality
- Hex-encoded content support
- Atomic file writes
- Backup creation on every edit

---

## [0.2.0] - 2025-04-20

### Added
- Command-line interface
- Line-range deletion support
- Basic error reporting

---

## [0.1.0] - 2025-04-19

### Added
- Initial release
- Core splicing functionality
- Hex decode/encode

---

## Version History Summary

| Version | Date | Key Feature |
|---------|------|-------------|
| 0.6.0 | 2026-05-28 | Indent engine, PID locks, lock hardening, llmosafe 0.6.2, 184 tests |
| 0.5.1 | 2026-05-14 | Test suite expansion, help text refactor |
| 0.5.0 | 2026-05-14 | Enterprise security + auto-indent + dry-run |
| 0.4.0 | 2025-04-22 | Manifest operations + multi-step undo |
| 0.3.0 | 2025-04-21 | Basic splicing + hex encoding |
| 0.2.0 | 2025-04-20 | CLI + deletion support |
| 0.1.0 | 2025-04-19 | Initial release |
