# Spec Checker

A structural and behavioral specification checker for codebases.

## Purpose

This tool verifies that implementation code conforms to declarative specifications. In AI-heavy development workflows, the spec becomes the human-controlled contract while AI generates the implementation.

```
Human intent → Spec (human-sized) → Checker → Code (AI-sized)
```

## Key Benefits

1. **Spec as Prompt Context**: AI gets exact signature, conditions, allowed imports → better generation
2. **Safe Regeneration**: Can rewrite from scratch; checker ensures same interface/deps
3. **Review Efficiency**: Review 10-line spec change, not 500-line code change
4. **Prevent Drift**: Shortcuts violate spec → CI fails → architecture preserved
5. **Multi-Agent Coordination**: Specs define contracts between agents working on different modules

## Spec Format

Unified YAML format for both structural and behavioral specifications:

```yaml
module: Bridge
language: solidity
source_path: contracts/Bridge.sol

exposes:
  deposit:
    signature: "(address token, uint256 amount, bytes32 receiver)"
    requires: [amount > 0, token in allowed_tokens]
    ensures: [deposited' == deposited + amount]
  withdraw:
    requires: [valid_signature, not already_executed]
    ensures: [withdrawn' <= deposited]

internal:
  - _verifySignature
  - _executeTransfer

depends_on:
  - TokenRegistry
  - SignerRegistry

forbidden_deps:
  - TestUtils

invariants:
  - "forall token: withdrawn[token] <= deposited[token]"
```

## Property Categories

### Phase 1 (MVP) ✅
- **exposes** - public functions that must exist
- **internal** - private functions (shouldn't be exposed)
- **depends_on** - allowed module dependencies
- **forbidden_deps** - banned dependencies

### Phase 2 ✅
- **layer** - infrastructure/domain/application/interface (with violation detection)
- **events** - emits/subscribes (spec only, not yet checked)

### Phase 3
- **state ownership** - owns_state, reads_state, modifies
- **access control** - callable_by, roles

## Layer Rules

Architectural layers enforce dependency direction:

```
Interface → Application → Domain → Infrastructure
```

- **Infrastructure**: Cannot depend on Domain/Application/Interface
- **Domain**: Can only depend on Infrastructure
- **Application**: Can depend on Domain and Infrastructure
- **Interface**: Can depend on anything

Example violation:
```
✗ Layer violation: 'src/infra.rs' (Infrastructure) cannot depend on 'src/domain.rs' (Domain)
```

## Supported Languages

| Language | Extractor | Status |
|----------|-----------|--------|
| Solidity | Regex (solc AST fallback) | ✅ Working |
| Rust | Regex | ✅ Working |
| TypeScript | - | Planned |

## Installation

```bash
cargo install --path .
```

## Usage

```bash
# Check all specs in a directory
spec-checker check ./specs

# Check a single module against source root
spec-checker check ./specs/main.spec.yaml --source ./src

# Generate spec skeleton from existing code
spec-checker init --language rust ./src/main.rs

# Diff: show spec vs implementation discrepancies
spec-checker diff ./specs/main.spec.yaml ./src/main.rs
```

## Self-Verification

The spec-checker verifies its own structure:

```bash
$ spec-checker check ./specs -s .

Spec Checker
========================================

Checking: main
  ✓ All checks passed

Checking: checker
  ⚠ Function 'check' signature mismatch...

Checking: extractors
  ✓ All checks passed

========================================
PASSED: 11 warning(s)
```

## Development Status

### Done
- [x] YAML spec parser
- [x] CLI with `check`, `init`, `diff` commands
- [x] Solidity extractor (solc AST + regex fallback)
- [x] Rust extractor (syn AST)
- [x] Self-specification in `specs/`
- [x] Layer violation detection
- [x] Filter test dependencies from warnings
- [x] Return type extraction in signatures

### In Progress
- [ ] Events checking (emits/subscribes)

### Planned
- [ ] GitHub Actions integration
- [ ] TypeScript support
- [ ] Behavioral checker (runtime assertions)

## License

Apache-2.0
