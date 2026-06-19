# Changelog

All notable changes to moesniper will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.7.12] - 2026-06-19

### Changed
- **llmosafe upgraded to 0.7.7:** Bumped dependency version across Rust crates and Python bindings; updated version strings in help text and docstrings.

## [0.7.11] - 2026-06-19

### Fixed
- **Symlink destruction on edit:** `normalize_path` canonicalized path is now propagated to all downstream file operations, resolving symlinks before atomic rename. Editing through a symlink now follows to the target file.
- **Python binding security parity:** Moved 3 pre-edit guards (regular-file check, read-only check, null-byte scan) into library-layer `validate_edit_target()` called from `write_atomic_with_dal`, protecting all entry points.
- **PID default documentation drift:** Removed stale `PidConfig::default()` dead code; synced help text defaults (entropy_scale: 0.1, pressure_scale: 0.2) to match `SniperConfig` source of truth.
- **`sniper decode` undocumented:** Added `sniper decode` subcommand to `--help` USAGE section.
- **File-relative lock directory:** Locks are now created in the target file's parent directory (`.sniper/`), not CWD, preventing scoping bypass.
- **Lowered metabolic pacing defaults:** `SNIPER_PID_ENTROPY_SCALE` default changed from 0.5 to 0.1, `SNIPER_PID_PRESSURE_SCALE` from 1.0 to 0.2.
- **File-relative backup directory:** Backups now use `backup_dir_for()` helper with file-relative paths + CWD fallback for unwritable parents.
- **Empty hex warning:** Warning emitted on stderr when empty hex string acts as implicit delete.
- **Null-byte scan:** First 4KB scanned before `read_to_string` for binary file detection (heuristic).
- **Empty file no-op guard:** Explicit error for delete operations on empty files; insert operations allowed.
- **Clippy lint:** Replaced `.map_or(false, ...)` with `.is_some_and(...)` at manifest empty-hex check.

### Security
- **Path sanitization (TOCTOU):** All 5 CLI/Python entry points now propagate canonicalized paths, eliminating path-vs-symlink discrepancy between validation and file operations.
- **Python binding hardening:** Regular-file check, read-only guard, and null-byte scan now apply to Python bindings via library-layer enforcement.
- **Lock file recovery edge case:** Documented residual risk from non-PID garbage in lock files (boundary test artifact, not production vector).

## [0.7.10] - 2026-06-15

### Fixed
- **auto-indent closing brace over-indentation:** `auto_indent_content` now correctly dedents closing braces (`}`, `)`, `]`) to block level instead of body level.
- **tabs overhang space-to-tab conversion:** Multi-level space-indented content in tab-indented files now preserves space overhang correctly.
- **dry-run `.sniper/` directory creation:** Both `cmd_splice` and `cmd_manifest_impl` now gate lock acquisition behind `!dry_run`, preventing persistent `.sniper/` directory creation during dry-run operations.
- **Python dry-run `.sniper/` leak:** Python bindings (`sniper_edit`, `sniper_manifest`) now gate lock and backup creation behind `!dry_run`, matching CLI behavior.
- **manifest same-start overlap:** Operations targeting the same line in a manifest now return an error instead of silently overwriting each other.
- **manifest context hash semantics:** Context hash is now verified once before the manifest loop against pre-manifest file state, instead of per-operation against mutated state.
- **clock-backwards lock timeout:** `SniperLock::acquire_with_config` now correctly handles clock-backwards events, preventing potential infinite hang.
- **manifest silent no-op:** Operations with neither `delete` nor `hex` now return an error instead of silently succeeding with zero changes.
- **Python bounds parity:** Python `sniper_edit` bounds checking now matches CLI behavior for end-of-file insert semantics.
- **Python error type classification:** `check_file_size_py` now raises `ValueError` for file-too-large errors instead of blanket `PyIOError`.
- **Python undo response shape:** `sniper_undo` now returns a consistent dict with `status` and `backup_path` keys.
- **error visibility:** `purge_old_backups` errors are now logged to stderr; lock timeout errors now include PID and lock path.
- **PidConfig visibility:** Demoted to `pub(crate)` with internal-use documentation.
- **Cargo.toml:** Added `readme` field for crates.io rendering.

### Added
- **Bug-hunt test suite:** 66 new tests covering manifest edge cases, auto-indent boundaries, dry-run side effects, and Python parity gaps.
- **Test protocol gap analysis:** 6 systemic protocol gaps documented (G-SYMMETRY, G-CROSS, G-SIDEFX, G-GUARD, G-DIFF, G-LOCK).

### Changed
- **`SniperConfig` deduplication:** `cmd_splice` and `cmd_manifest_impl` now use a single `SniperConfig::from_env()` call per operation.

## [0.7.9] - 2026-06-14

### Fixed
- **manifest dry-run per-operation diffs:** Added `manifest_ops` field to dry-run JSON output with per-operation diff previews for both insert and delete operations.
- **auto-indent module docstring bug (BUG-1):** Auto-indent now correctly ignores module-level docstrings when detecting indentation style, preventing 2-space module docstrings from polluting function body indentation.
- **context hash documentation (BUG-2):** Added `sniper context <file> <start> <end>` CLI command and documented exact SHA-256 computation (3 lines before/after, byte-level including newlines).
- **dry-run risk telemetry noise (BUG-3):** Removed `risk` and `recommended_action` fields from dry-run JSON output in both `sniper_edit` and `sniper_manifest`.
- **library extraction incomplete (v0.7.2):** Removed duplicate `diff`/`indent` module declarations from binary; binary now uses library exports. Removed dead `write_atomic_with_guard` function. Python `sniper_undo` now uses atomic temp+rename matching CLI behavior. Python `sniper_encode` delegates to library `hex_encode`.

### Added
- **cargo-deny configuration:** Added `deny.toml` with advisory ignore for unmaintained `atty`, license allowlist (MIT, Apache-2.0, MPL-2.0, Unicode-3.0), and bans configured to allow multiple versions.

### Changed
- **llmosafe upgraded:** 0.7.4 → 0.7.5 (breaking ABI change in getter functions — no impact on sniper, which only uses `ResourceGuard`).
- **CHANGELOG format:** Updated SemVer spec reference from v0.0.0 to v2.0.0.

## [0.7.6] - 2026-06-08

### Fixed
- **context_hash unwired in manifest paths:** `sniper_manifest` (Python) and `cmd_manifest_impl` (CLI) now verify pre-edit context per operation, matching `sniper_edit` behavior. Removed `#[allow(unused_variables)]` suppression.
- **hex+delete conflict silently resolved:** Manifest operations specifying both `hex` and `delete: true` now return an error instead of silently discarding the hex content.
- **hex_encode unsafe block:** Replaced `unsafe { String::from_utf8_unchecked() }` with safe `String::from_utf8_lossy()`.
- **stale temp files in write_atomic_impl:** Temp files are now cleaned up on error paths via a `CleanupGuard` drop guard that removes the file if the atomic rename fails.
- **config f64 validation:** `pid_entropy_scale` and `pid_pressure_scale` now reject NaN, negative, infinite, and out-of-range values; validated to `(0.0..=100.0)`.

## [0.7.5] - 2026-06-08

### Fixed
- **auto-indent double-indentation:** Previously always stripped existing whitespace and applied flat expected indent, destroying multi-level structure and double-indenting already-correct content. Now uses a min-leading guard — content whose minimum indent matches or exceeds the expected level is left unchanged — with an overhang-preserving shift that maintains internal nesting.
- **manifest splice panic:** Inserting hex content at end-of-file (`start == end == lines.len() + 1`) now uses `.min()` guards on splice ranges with an `extend` fallback, matching the `cmd_splice` insert-at-end pattern. Applied to both CLI `cmd_manifest_impl` and Python bindings.
- **manifest splice panic on delete-at-end:** Delete path (`op.delete`) now uses `.min(lines.len())` on range end instead of raw `e`.
- **`needs_indent_fix` false positives:** Simplified to a min-leading check consistent with the auto-indent guard, preventing false positives on already-correct multi-level content.
- **detect_expected_indent bounds:** Added `.min(all_lines.len())` guard to prevent slice panic on out-of-range line indices.

### Performance
- **detect_space_step O(N²):** Replaced `Vec::contains` loop body with sort+dedup.
- **purge_old_backups O(N²):** Replaced `Vec` with `HashSet` for dedup.
- **hex_encode shared:** Extracted optimized byte-to-hex encoder with pre-allocated buffer and nibble mapping, replacing 3 duplicated inline `format!({:02x})` sites.

### Portability
- **Test paths:** Replaced hardcoded `"cargo"` and `"/workspace/sniper"` with `env!("CARGO")` / `env!("CARGO_MANIFEST_DIR")` in `tests/enterprise_security.rs`.

## [0.7.4] - 2026-06-06

### Added
- `.agents/` directory to `.gitignore` for agentic dev artifacts

### Changed
- Version bumped for release

---

## [0.7.3] - 2026-06-04

### Added
- Python parity bindings and CLI `--version` flag for `moesniper`
- `*.so` to `.gitignore` to prevent binary wheel commits

### Fixed
- PyO3 manifest safety parity across all audit findings

---

## [0.7.2] - 2026-06-03

### Added
- PID pacing configuration via `SNIPER_PID_BASE_MS`, `SNIPER_PID_ENTROPY_SCALE`, `SNIPER_PID_PRESSURE_SCALE` environment variables
- Workspace configuration including `sniper-py` Python bindings crate
- Risk telemetry and `recommended_action` now computed for both dry-run and real paths

### Fixed
- Eliminated redundant `SniperConfig::from_env()` calls in write path — config now forwarded through call chain
- Removed redundant `ResourceGuard::auto(0.5)` in `write_atomic_impl` — caller's guard forwarded instead
- Help text updated with missing DAL and PID environment variables

---

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
| 0.7.4 | 2026-06-06 | Release tooling, .agents/ gitignore |
| 0.7.3 | 2026-06-04 | Python bindings parity, PyO3 safety |
| 0.7.2 | 2026-06-03 | PID pacing, config forwarding, risk telemetry |
| 0.7.1 | 2026-06-01 | Append-at-end fix, manifest bounds, permission preservation, perf optimization |
| 0.7.0 | 2026-05-30 | Indent engine, PID locks, context verification, docs unification |
| 0.5.1 | 2026-05-14 | Test suite expansion, help text refactor |
| 0.5.0 | 2026-05-14 | Enterprise security, auto-indent, dry-run |
| 0.4.0 | 2025-04-22 | Manifest operations, multi-step undo |
| 0.3.0 | 2025-04-21 | Basic splicing, hex encoding |
| 0.2.0 | 2025-04-20 | CLI, deletion support |
| 0.1.0 | 2025-04-19 | Initial release |
