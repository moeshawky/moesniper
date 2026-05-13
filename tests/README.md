# Test Suite Documentation

This directory contains the improved test suite for moesniper, following the [llm-testing-patterns](../../../.pi/agent/skills/llm-testing-patterns/SKILL.md) methodology.

## Test Structure

Following AOP v3 screening order (fail fast):

```
tests/
├── smoke/              # G-HALL, G-SEC - Does it even run safely?
│   └── smoke.rs
├── unit_edge_cases.rs  # G-EDGE - Edge case matrix
├── property_invariants.rs  # G-SEM - Property-based testing with proptest
├── regression/         # G-DRIFT - Golden file regression tests
│   ├── golden_tests.rs
│   └── golden/
├── integration.rs      # G-CTX - Integration tests (existing, enhanced)
└── enterprise_security.rs  # G-SEC - Security tests (existing)
```

## Running Tests

```bash
# All tests
cargo test

# Smoke tests only (fail fast)
cargo test --test smoke

# Edge cases
cargo test --test unit_edge_cases

# Property-based (proptest)
cargo test --test property_invariants

# Regression (golden files)
cargo test --test golden_tests

# Security tests
cargo test --test enterprise_security
```

## Coverage by Failure Mode

| Failure Mode | Test File | Priority |
|--------------|-----------|----------|
| G-HALL (Hallucinated APIs) | smoke.rs | 1 (first) |
| G-SEC (Security) | smoke.rs, enterprise_security.rs | 2 |
| G-EDGE (Edge Cases) | unit_edge_cases.rs | 3 |
| G-SEM (Semantic) | property_invariants.rs | 4 |
| G-ERR (Error Handling) | unit_edge_cases.rs | 5 |
| G-CTX (Context) | integration.rs | 6 |
| G-DRIFT (Drift) | regression/golden_tests.rs | 7 |
| G-PERF (Performance) | benches/ | 8 |
| G-DEP (Dependencies) | (CI/CD) | 9 |

## Test Patterns Used

### 1. Smoke Tests (G-HALL, G-SEC)
- Binary exists and runs
- Core commands documented
- Path traversal rejected
- File permissions preserved

### 2. Edge Case Matrix (G-EDGE)
- Empty file
- Single character
- Very long line (100k chars)
- Unicode/Arabic content
- Null bytes
- Line 0 (invalid)
- Beyond EOF
- No trailing newline
- With trailing newline
- Special characters
- Multiple operations

### 3. Property-Based (G-SEM)
- Encode/decode roundtrip
- Undo is inverse of edit
- Line numbers are 1-indexed
- File size stable for same-length replacement
- No state corruption on failure

### 4. Golden File Regression (G-DRIFT)
- Undo stack behavior
- Basic splice operation
- Newline preservation
- Manifest operation
- Error message format

## Adding New Tests

1. **Identify failure mode** being tested (G-HALL through G-DRIFT)
2. **Choose test file** based on failure mode
3. **Follow naming convention**: `test_<failure_mode>_<description>`
4. **Include in PR**: Update this README if adding new failure mode coverage

## Cascade Detection

If 2+ failure modes appear in same test run:
- G-HALL + G-SEC: Prompt/design issue
- G-EDGE + G-ERR: Missing domain knowledge
- G-SEM + G-DRIFT: Model version change

See [llm-testing-patterns skill](../../../.pi/agent/skills/llm-testing-patterns/SKILL.md#cascade-detection) for escalation protocol.
