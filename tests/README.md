# Test Suite

Tests are organized by layer, running fastest and most critical first.

```
tests/
├── smoke.rs              # Binary exists, core commands work, permissions preserved
├── unit_edge_cases.rs    # Edge cases: empty files, unicode, long lines, EOF boundaries
├── property_invariants.rs # Property-based: encode/decode roundtrip, undo reversibility
├── regression/           # Golden-file regression against known outputs
│   ├── golden_tests.rs
│   └── golden/
├── integration.rs        # End-to-end: concurrency, lock behavior, full workflows
├── enterprise_security.rs # Path traversal, symlink attacks, lock integrity, audit
├── boundary_tests.rs     # Cross-component boundary: splice, undo, manifest interactions
```

## Running

```bash
cargo test                          # All tests
cargo test --test smoke             # Smoke only (fastest)
cargo test --test unit_edge_cases   # Edge cases
cargo test --test property_invariants  # Property-based
cargo test --test golden_tests      # Golden file regression
cargo test --test integration       # Integration
cargo test --test enterprise_security  # Security
cargo test --test boundary_tests   # Cross-boundary
```

## Coverage

| Layer | What It Shields |
|-------|-----------------|
| Smoke | Binary integrity, basic commands |
| Security | Path safety, lock hygiene, atomic writes |
| Edge cases | Encoding boundary, file limits, character sets |
| Semantic | Invariants, roundtrip guarantees |
| Error handling | Invalid inputs, corrupted state |
| Integration | Concurrency, multi-operation correctness |
| Regression | Golden output stability |
| Performance | Benchmarks in `benches/` |

## Adding Tests

1. Choose the matching test file based on the layer
2. Name: `test_<category>_<description>`
3. If adding a new test file, update this README
