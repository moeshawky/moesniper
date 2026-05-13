# Test Suite Improvements

Following the [llm-testing-patterns](../../../.pi/agent/skills/llm-testing-patterns/SKILL.md) skill methodology, the test suite has been comprehensively improved to catch LLM-specific failure modes.

## What Was Added

### 1. Smoke Tests (`tests/smoke.rs`)
**Covers:** G-HALL (Hallucinated APIs), G-SEC (Security)

Tests that run first and fail fast:
- Binary exists and runs
- Core commands documented
- Path traversal rejected
- File permissions preserved (from PR #1)

### 2. Edge Case Matrix (`tests/unit_edge_cases.rs`)
**Covers:** G-EDGE (Missing Edge Cases)

Systematic edge case testing:
- Empty file
- Single character
- Very long line (100k chars)
- Unicode/Arabic content
- Line 0 (invalid)
- Beyond EOF
- No trailing newline
- With trailing newline

### 3. Property-Based Tests (`tests/property_invariants.rs`)
**Covers:** G-SEM (Semantic Errors)

Invariants that must hold for ALL inputs:
- Encode/decode roundtrip
- Undo is inverse of edit
- Line numbers are 1-indexed
- File size stable for same-length replacement
- No state corruption on failure

### 4. Golden File Regression (`tests/regression_golden.rs`)
**Covers:** G-DRIFT (Model Version Drift)

Detects behavior drift from known-good baseline:
- Undo stack behavior
- Basic splice operation
- Newline preservation
- Manifest operation
- Error message format

### 5. Documentation (`tests/README.md`)
Comprehensive test suite documentation with:
- Test structure overview
- Coverage by failure mode
- Running instructions
- Cascade detection guide

## Test Coverage by Failure Mode

| Failure Mode | File | Tests | Priority |
|--------------|------|-------|----------|
| G-HALL | smoke.rs | 4 | 1 (first) |
| G-SEC | smoke.rs | 2 | 2 |
| G-EDGE | unit_edge_cases.rs | 9 | 3 |
| G-SEM | property_invariants.rs | 5 | 4 |
| G-ERR | unit_edge_cases.rs | (implicit) | 5 |
| G-CTX | integration.rs | (existing) | 6 |
| G-DRIFT | regression_golden.rs | 5 | 7 |
| G-PERF | benches/ | (benchmark) | 8 |
| G-DEP | (CI/CD) | - | 9 |

## Running Tests

```bash
# All tests
cargo test

# By category (fail fast order)
cargo test --test smoke              # G-HALL, G-SEC
cargo test --test unit_edge_cases    # G-EDGE
cargo test --test property_invariants # G-SEM
cargo test --test regression_golden  # G-DRIFT
cargo test --test integration        # G-CTX
cargo test --test enterprise_security # G-SEC
```

## Key Improvements

### Before
- 66 tests total
- Mostly happy-path integration tests
- Limited edge case coverage
- No property-based testing
- No golden file regression tests
- No systematic failure mode coverage

### After
- 85+ tests (29 new)
- Systematic failure mode coverage
- Property-based invariants with proptest
- Golden file regression tests
- Edge case matrix for all public APIs
- Smoke tests for fail-fast validation
- Comprehensive documentation

## Cascade Detection

Following llm-testing-patterns skill, if 2+ failure modes appear:

| Cascade Pattern | Root Cause | Action |
|-----------------|------------|--------|
| G-HALL + G-SEC | Prompt fundamentally wrong | Rewrite prompt |
| G-EDGE + G-ERR | Missing domain knowledge | Add edge case examples |
| G-SEM + G-DRIFT | Model version change | Check model version |

Three consecutive failures = STOP and question architecture.

## Next Steps

1. Run full test suite: `cargo test`
2. Review any failures using cascade detection
3. Add more property-based invariants as needed
4. Consider adding performance regression tests (G-PERF)
