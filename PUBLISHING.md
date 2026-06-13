# Publishing moesniper — Success Log & Reference

> This document records the exact steps that successfully published moesniper v0.7.8 to PyPI, TestPyPI, crates.io, and GitHub Releases. Use this as the canonical reference for future releases.

---

## Quick Reference

| Registry | Package | Version | Status |
|----------|---------|---------|--------|
| **PyPI** | `moesniper` | 0.7.8 | ✅ Published |
| **TestPyPI** | `moesniper` | 0.7.8 | ✅ Published |
| **crates.io** | `moesniper` | 0.7.8 | ✅ Published |
| **GitHub Release** | `moesniper` | v0.7.8 | ✅ Created |

**Wheels built:**
- `moesniper-0.7.8-cp312-cp312-manylinux_2_17_x86_64.manylinux2014_x86_64.whl`
- `moesniper-0.7.8-cp312-cp312-manylinux_2_17_aarch64.manylinux2014_aarch64.whl`

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
# version = "0.7.8"

# 2. sniper-py/Cargo.toml
# version = "0.7.8"

# 3. sniper-py/pyproject.toml (CRITICAL - maturin reads this!)
# version = "0.7.8"
```

> **⚠️ GOTCHA**: Maturin reads version from `sniper-py/pyproject.toml` `[project]` section, NOT from Cargo.toml. Forgetting this produces wheels with the old version.

---

## Release Procedure

### 1. Prepare Changes
```bash
# Ensure all tests pass
cargo test --workspace

# Ensure clippy clean
cargo clippy --all-targets -- -D warnings

# Update CHANGELOG.md with [Unreleased] section
```

### 2. Commit & Tag
```bash
git add -A
git commit -m "chore: bump version to X.Y.Z"
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
| `wheels.yml` | PyPI + TestPyPI | `PROJECT: sniper`, `PYTHON_VERSION: "3.12"` |
| `publish-cratesio.yml` | crates.io | `PROJECT: sniper`, `PUBLISH_FLAGS: ""` |
| `release.yml` | GitHub Release | Simple tag release, auto notes |

**All three are copies of master templates at `/workspace/.github/workflows/`.**

---

## Build Matrix (wheels.yml)

| OS | Runner | Target | manylinux | Output Tag |
|----|--------|--------|-----------|------------|
| Ubuntu 24.04 | `ubuntu-latest` | x86_64 | `auto` | `manylinux_2_17_x86_64` |
| Ubuntu 24.04 ARM | `ubuntu-24.04-arm` | aarch64 | `auto` | `manylinux_2_17_aarch64` |

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
# Build wheels locally (requires maturin, docker for cross-compile)
/workspace/.github/scripts/build-wheels.sh sniper          # x86_64
/workspace/.github/scripts/build-wheels.sh sniper --target aarch64  # ARM64

# Publish to PyPI
uv publish /workspace/sniper/sniper-py/dist/*.whl

# Publish to crates.io
cd /workspace/sniper && cargo publish

# GitHub Release
gh release create vX.Y.Z --generate-notes
```

---

## Related Files

- `/workspace/PUBLISHING.md` — Master template for all workspace projects
- `/workspace/.github/scripts/build-wheels.sh` — Unified wheel builder
- `/workspace/.github/workflows/` — Master workflow templates

---

*Last successful release: v0.7.8 (2026-06-13)*