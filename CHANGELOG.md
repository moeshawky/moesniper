# Changelog

All notable changes to moesniper will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v0.0.0.html).

## [0.7.1] - 2026-06-01

### Added
- Insert-at-end now accepts start=end=N+1 for natural append expressions
- Bounds validation for `cmd_manifest_impl` to prevent splice panic on out-of-range manifests
- Insert-at-end exception for manifest bounds (parity with cmd_splice)
- 10+ regression tests: append-at-end (1/2/4-line), handle_backtrack_error, normalize_path

### Fixed
- Append-at-end (line N+1) rejected when start equals end (documented contract violation)
- Original file permissions lost during atomic write — now copied to temp file before rename
- `find_latest_backup` O(N log N) memory usage reduced to O(N) O(1) via iterator max
- Stale thread handle test ("0.5.0") corrected for llmosafe 0.6.2
- `test_normalize_path_missing_parent` assertion corrected for intentional non-existent path behavior

### Changed
- `write_atomic_impl` now uses `BufWriter` for buffered writes
- **llmosafe 0.6.2 → 0.7.1**: `ResourceGuard::for_testing()` available for deterministic test entropy/pressure. `PidInput` struct added (not used by sniper). `sift_observation_inner`, `GainSchedule`, `Setpoint` removed (zero impact). `DeadlineExceeded` (-7) remains a valid `KernelError` variant used in `check_blocking()` and C-ABI, but resource exhaustion now surfaces via `KernelError::ResourceExhaustion` from `ResourceGuard::check()` rather than OS-level IO error codes.

## [0.7.0] - 2026-05-30

### Added

- **Indentation engine** (`src/indent.rs`): Statistical step detection with 80% supermajority, raw frequency scoring, round-to-nearest, whitespace stripping, and smaller-is-step tiebreaker — robust against inconsistent LLM output. 40 adversarial tests covering tabs, spaces, mixed styles, blank lines, continuation lines, and brace openers.
- **Auto-indent** (`--auto-indent`): Detects expected indentation from surrounding context and automatically prepends missing leading whitespace.
- **Force-indent** (`--force-indent`): Bypasses indentation validation for deliberate refactoring.
- **PID-based file locks**: Lock content contains the process PID, enabling stale lock detection and auto-recovery via `/proc/{pid}` liveness check. Configurable timeout via `SNIPER_LOCK_TIMEOUT`.
- **Context verification** (`--context <hash>`): Verifies SHA-256 hash (first 16 hex chars) of 3 lines before and after the edit target. Rejects edits if surrounding code changed since line numbers were computed.
- **Manifest promotion detection**: Detects >=3 edits to the same file within the lock window and suggests batching with `--manifest` via `ai_hint` in JSON output.
- **Line shift tracking**: `line_shift` field in `CliResult` — positive means lines moved down, negative means up. Agents can mechanically adjust subsequent line targets.
- **Clipply configuration** (`clippy.toml`): msrv=1.87, cognitive-complexity-threshold=30. Lint policy in `Cargo.toml` denies `unreachable`, `todo`, `panic`; warns on `dbg_macro`, `cast_possible_truncation`, `cast_sign_loss`.
- **Crate-level lint enforcement**: `deny(clippy::unwrap_used, clippy::expect_used)` in `src/lib.rs` with `#[cfg_attr(test, allow(...))]`.
- **Secure temp files**: Nanosecond timestamp suffix (`{filepath}.sniper_tmp.{ts}`) prevents collision.
- **Atomic write integrity**: Trailing newline detection preserves original file format.
- **Path validation ordering**: `normalize_path` runs before `check_file_size`, closing a TOCTOU window.

### Changed

- **Indentation validation now always-on**: Runs on every edit by default. Use `--force-indent` to bypass. Removed dead `--validate-indent` flag.
- **llmosafe 0.5.0 → 0.6.2**: `ResourceGuard::auto(0.5)` replaces hardcoded memory ceiling. `ResourceGuard::check()` called before atomic rename for proper `KernelError` propagation.
- **MSRV 1.70 → 1.87**: `is_multiple_of()` stabilized in Rust 1.87.
- **Documentation unified**: All 5 surfaces (main.rs doc, help text, README, Cargo.toml, tests/README) tell the same story. No dead flags, no missing features, no internal methodology jargon.
- **Help text expanded**: Added `--context`, `--force-indent`, env var reference, context-verified edit example. Encode subcommand documents all 3 modes.
- **Dead code removed**: Removed `use_os_locking` config field, `SNIPER_USE_OS_LOCKING` env var, `libc` dependency (zero imports).
- **Clippy clean**: 184 tests, zero warnings, zero panics.

### Security

- Path traversal protection (`SecurityPolicy`, `normalize_path`).
- Symlink-safe operations (no `follow_symlinks` without validation).
- PID-based per-file locking with stale lock auto-recovery.
- Atomic writes with unique temp file names.
- Symlink traversal rejection within base directory.

### Fixed

- **Boundary hardening**: Path validation before file ops, lock PID liveness check, temp file uniqueness, concurrent lock purge race detection.
- **Test quality**: Removed tautological assertions, added exact-match checks, corrected weak oracle patterns.

---

## [0.5.1] - 2026-05-14

### Added
- Comprehensive test suite: 85+ tests (smoke, edge case, property-based, regression, security, integration).
- Test documentation (`tests/README.md`).

### Changed
- Help text extracted to `src/help_text.rs` with improved organization.
- Documentation improvements.

---

## [0.5.0] - 2026-05-14

### Added
- Enterprise-grade path security validation (`src/security.rs`).
- Auto-indent and validate-indent flags.
- Dry-run diff preview.
- Configuration via environment variables: `SNIPER_LOCK_TIMEOUT`, `SNIPER_MAX_FILE_SIZE`, `SNIPER_BACKUP_RETENTION_COUNT`, `SNIPER_BACKUP_MAX_AGE_DAYS`.
- AI hint system for verification guidance.
- JSON output mode (`--json` flag).

### Changed
- Upgraded to llmosafe 0.5.0 with Backtrack Signal support.
- Improved trailing newline handling (PR #8).
- Optimized string handling with `split_inclusive`.
- Enhanced error messages with actionable hints.

### Security
- Path traversal protection.
- Symlink attack mitigation.
- File locking with configurable timeout.
- Atomic file writes.

---

## [0.4.0] - 2025-04-22

### Added
- Manifest batch operations.
- Multi-step undo stack.
- Hex encoding command (`sniper encode`).
- Backup retention policies.

### Changed
- Improved line number parsing (1-indexed).
- Better error handling for out-of-bounds operations.

---

## [0.3.0] - 2025-04-21

### Added
- Basic line splicing.
- Hex-encoded content support.
- Atomic file writes.
- Backup creation on every edit.

---

## [0.2.0] - 2025-04-20

### Added
- Command-line interface.
- Line-range deletion.
- Basic error reporting.

---

## [0.1.0] - 2025-04-19

### Added
- Initial release.
- Core splicing functionality.
- Hex decode/encode.

---

## Version History

| Version | Date | Key Feature |
|---------|------|-------------|
| 0.7.1 | 2026-06-01 | Append-at-end fix, manifest bounds, permission preservation, perf optimization |
| 0.7.0 | 2026-05-30 | Indent engine, PID locks, context verification, docs unification |
| 0.5.1 | 2026-05-14 | Test suite expansion, help text refactor |
| 0.5.0 | 2026-05-14 | Enterprise security, auto-indent, dry-run |
| 0.4.0 | 2025-04-22 | Manifest operations, multi-step undo |
| 0.3.0 | 2025-04-21 | Basic splicing, hex encoding |
| 0.2.0 | 2025-04-20 | CLI, deletion support |
| 0.1.0 | 2025-04-19 | Initial release |
