# AGENTS.md — moesniper Entry Gate
*Dual-language (Rust + Python) precision file editor for LLM agents — atomic writes, hex encoding, zero shell corruption.*

### What This Is
moesniper is a Cargo workspace with two crates: the Rust core library/CLI (`moesniper`) and Python bindings (`sniper-py`) via PyO3 + maturin. Published to crates.io and PyPI. Security-first design: atomic writes via `rename(2)`, path validation, hex-encoded payloads. Enterprise-grade tooling enforced at every level.

### Iron Law
```
EVERY RUST CHANGE MUST PASS: cargo fmt --check + cargo clippy -- -D warnings + cargo test
EVERY PYTHON CHANGE MUST PASS: ruff check + ruff format --check + mypy strict + pytest
GATES ARE NON-NEGOTIABLE — NO MERGE WITHOUT PASSING ALL GATES
RUST CORE BEFORE PYTHON BINDINGS — changes to src/ must be verified before sniper-py/ updates
NEVER COMMIT SECRETS, CREDENTIALS, OR KEYS — .env, *.key, *.pem are gitignored
```

### Quality Gates — Rust
| Gate | Command | Domain |
|------|---------|--------|
| G1: Format | `cargo fmt --check` | All crates |
| G2: Clippy | `cargo clippy --workspace --all-targets -- -D warnings` | All crates |
| G3: Tests | `cargo test --workspace` | All crates |
| G4: Docs | `cargo doc --workspace --no-deps` | All crates |
| G5: Deps | `cargo deny check` | All crates |
| G6: Build | `cargo build --workspace` | All crates |

Config files: `clippy.toml` (msrv 1.87, cognitive-complexity=30), `rustfmt.toml` (edition 2021, max_width=100), `deny.toml` (MIT + Apache-2.0).

### Quality Gates — Python
| Gate | Command | Domain |
|------|---------|--------|
| P1: Lint | `ruff check python/` | sniper-py |
| P2: Format | `ruff format python/ --check` | sniper-py |
| P3: Types | `mypy python/` (strict mode) | sniper-py |
| P4: Tests | `pytest tests/` | sniper-py |
| P5: Build | `maturin develop` | sniper-py |

Config files: `sniper-py/pyproject.toml` (Ruff 100-char, mypy strict, Python >=3.10), `sniper-py/.pre-commit-config.yaml`.

### Project Architecture
```
Workspace: Cargo.toml (root) → members = ["sniper-py"]

moesniper (Rust lib + CLI)
├── src/lib.rs           Public API: edit, delete, manifest, undo, encode, decode
├── src/main.rs          CLI binary
├── src/config.rs        SniperConfig, DalLevel
├── src/diff.rs          Dry-run preview generation
├── src/indent.rs        Indentation validation + auto-fix
├── src/security.rs      Path sanitization, SecurityPolicy
├── src/help_text.rs     CLI help strings
└── tests/               Integration, property, regression, edge-case, fuzz tests

sniper-py (PyO3 bindings + Python wrapper)
├── src/lib.rs           PyO3 module: sniper_edit, sniper_undo, sniper_manifest, etc.
├── python/moesniper/    Python package (wraps _native module)
└── tests/test_basic.py  Python integration tests
```

### File Map
| Path | Purpose |
|------|---------|
| `Cargo.toml` | Workspace manifest, dependency versions, lint policy |
| `Cargo.lock` | Pinned dependency tree (committed — binary crate convention) |
| `src/` | Rust core library + CLI binary |
| `tests/` | Rust tests — integration, property-based, regression, edge cases |
| `benches/` | Criterion benchmarks |
| `sniper-py/` | Python bindings crate (maturin + PyO3) |
| `.github/workflows/` | CI/CD: wheels (manylinux + musl), crates.io publish, release, stale |
| `.agents/` | Specialized agent dispatch files (release-agent.md, etc.) |
| `.sniper/` | Runtime backup directory (gitignored — managed by the tool itself) |

### Release Process

```
VERSION BUMP ORDER (NON-NEGOTIABLE):
  1. Cargo.toml           → version = "X.Y.Z"
  2. sniper-py/Cargo.toml  → version = "X.Y.Z"
  3. sniper-py/pyproject.toml → version = "X.Y.Z"  ← CRITICAL: bump BEFORE tag
  4. CHANGELOG.md          → move [Unreleased] → [X.Y.Z]
  5. git add + commit
  6. git tag vX.Y.Z  ← tag triggers CI: GitHub Release, PyPI wheels, crates.io verify
  7. git push origin master && git push origin vX.Y.Z
  8. cargo publish  ← manual crates.io publish (CI only verifies)
  9. gh workflow run wheels.yml --ref master  ← PyPI wheels (tag auto-triggers but manual
     dispatch confirms latest master with correct versions)

WARNING — PyPI wheels silent-skip trap:
  The wheels CI uses `skip-existing: true`. If sniper-py/pyproject.toml version is stale
  (not bumped before tag push), maturin builds with the OLD version, PyPI rejects it as
  duplicate, and the publish job reports SUCCESS — silently shipping nothing. PyPI verifies
  the ".whl" filename version, not the git tag version. ALWAYS bump pyproject.toml BEFORE
  tagging.

WARNING — `git add -A` destroys the repo:
  Never use `git add -A` in this workspace. It will commit sniper-py/.venv/ (thousands
  of vendored pip/pytest files), .sniper/ backups, and other gitignored artifacts.
  Use `git add <specific files>` only. Verify with `git status` before every commit.
```

### BANNED
- `unwrap()` in library code — use `?` or `.map_err()` (denied at crate level in `src/lib.rs`)
- `unsafe` without `// SAFETY:` comment documenting invariants
- `# type: ignore` without specifying the mypy error code
- Editing `.sniper/` backup directory manually — use `--undo`
- Hex-encoded payloads without indentation preservation — validate before writing
- Wildcard dependency versions — pin to major.minor or exact
- Skipping pre-commit hooks — `pre-commit install` required in sniper-py/
- Committing without all quality gates passing — G1-G6 + P1-P5 are mandatory
- `git add -A` — commits gitignored artifacts (.venv/, .sniper/). Stage specific files.
- Tagging before bumping sniper-py/pyproject.toml — wheels CI silently skips PyPI publish.
