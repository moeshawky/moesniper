# Publishing moesniper — Success Log & Reference

> This document records the release procedure for PyPI, TestPyPI, crates.io, and GitHub Releases. The last published release is v0.7.12.

---

## Quick Reference

| Registry | Package | Version | Status |
|----------|---------|---------|--------|
| **PyPI** | `moesniper` | 0.7.12 | ✅ Published |
| **crates.io** | `moesniper` | 0.7.12 | ✅ Published |
| **GitHub Release** | `moesniper` | v0.7.12 | ✅ Created |

Linux wheels use the CPython 3.10 stable ABI (`cp310-abi3`) and target x86_64 and aarch64.

---

## Prerequisites (One-time Setup)

### PyPI Trusted Publishing (OIDC)
1. **PyPI.org** → Account → Publishing → "Add a new trusted publisher"
   - Repository: `moeshawky/moesniper`
   - Workflow name: `Wheels`
   - Environment: `pypi`
2. **TestPyPI.org** → Same steps, environment: `testpypi`
3. **GitHub repo** → Settings → Environments → Create `pypi` and `testpypi` environments
   - No protection rules needed (OIDC handles auth)

### crates.io
1. **crates.io** → Account → API Token → Create token
2. **GitHub repo** → Settings → Secrets → Actions → `CARGO_REGISTRY_TOKEN` = token
3. **GitHub repo** → Settings → Environments → Create `cratesio` environment

---

## Version Bump Checklist

Before tagging, update **ALL THREE** version locations:

```bash
# 1. Cargo.toml (workspace root)
# version = "X.Y.Z"

# 2. sniper-py/Cargo.toml
# version = "X.Y.Z"

# 3. sniper-py/pyproject.toml (CRITICAL - maturin reads this!)
# version = "X.Y.Z"
```

> **⚠️ GOTCHA**: Maturin reads version from `sniper-py/pyproject.toml` `[project]` section, NOT from Cargo.toml. Forgetting this produces wheels with the old version.

---

## Release Procedure

### 1. Prepare Changes
```bash
# Ensure formatting and lints are clean
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings

# Ensure all tests and documentation pass
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

# Audit advisories, licenses, dependency sources, and unused dependencies
cargo deny check
cargo machete

# Verify package contents and registry acceptance
cargo package -p moesniper --list
cargo publish -p moesniper --dry-run --locked
cd sniper-py
maturin build --release --out dist
twine check dist/*.whl
cd ..

# Update CHANGELOG.md with a dated release section and synchronize all versions
```

### 2. Commit & Tag
```bash
git status --short
git add <reviewed-files>
git commit -m "Prepare vX.Y.Z release"
git tag vX.Y.Z
git push origin master
git push origin vX.Y.Z
```

### 3. Automated Workflows Trigger

| Workflow | Trigger | What It Does |
|----------|---------|--------------|
| **Wheels** | `v*` tag push | Builds x86_64 + aarch64 wheels → TestPyPI → PyPI |
| **Publish to crates.io** | `v*` tag push | Dry-run → publish to crates.io |
| **Release** | `v*` tag push | Creates GitHub Release with auto-generated notes |

### 4. Monitor

```bash
# Watch the Wheels workflow (includes PyPI publish)
gh run watch --repo moeshawky/moesniper

# Check individual workflows
gh run list --repo moeshawky/moesniper --limit 5
```

---

## Workflow Files (in `.github/workflows/`)

| File | Purpose | Key Config |
|------|---------|------------|
| `wheels.yml` | PyPI + TestPyPI | `PROJECT: sniper`, CPython 3.10 stable ABI |
| `publish-cratesio.yml` | crates.io | `PROJECT: sniper`, `PUBLISH_FLAGS: "-p moesniper"` |
| `release.yml` | GitHub Release | Simple tag release, auto notes |

## Build Matrix (wheels.yml)

| OS | Runner | Target | manylinux | Output Tag |
|----|--------|--------|-----------|------------|
| Ubuntu 24.04 | `ubuntu-latest` | x86_64 | `auto` | `cp310-abi3-manylinux_*_x86_64` |
| Ubuntu 24.04 ARM | `ubuntu-24.04-arm` | aarch64 | `auto` | `cp310-abi3-manylinux_*_aarch64` |

> **CRITICAL**: aarch64 MUST use `manylinux: auto` (not `"off"`). PyPI rejects `linux_aarch64` platform tag.

---

## Troubleshooting

### Wheels show wrong version (e.g., 0.7.6 instead of 0.7.8)
**Cause**: `sniper-py/pyproject.toml` version not updated.
**Fix**: Update `sniper-py/pyproject.toml` `[project]` `version = "X.Y.Z"`

### PyPI rejects aarch64 wheel with "unsupported platform tag"
**Cause**: `manylinux: "off"` for aarch64 in `wheels.yml`.
**Fix**: Change to `manylinux: auto` for aarch64 matrix entry.

### crates.io publish fails
**Check**: `CARGO_REGISTRY_TOKEN` secret exists, `cratesio` environment configured.

### PyPI publish fails with 403/401
**Check**: Trusted Publishing configured on PyPI, `pypi` environment exists in GitHub.

---

## Manual Escape Hatches

If CI fails, you can publish manually:

```bash
# Build the local-platform wheel
cd sniper-py
maturin build --release --out dist

# Publish to PyPI
uv publish dist/*.whl

# Publish to crates.io
cd ..
cargo publish -p moesniper --locked

# GitHub Release
gh release create vX.Y.Z --generate-notes
```

---

*Last successful release: v0.7.12 (2026-06-19)*
