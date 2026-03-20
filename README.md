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
module Bridge:
  language: Solidity
  exposes:
    deposit:
      signature: (address token, uint256 amount, bytes32 receiver)
      requires: [amount > 0, token in allowed_tokens]
      ensures: [deposited' == deposited + amount]
    withdraw:
      requires: [valid_signature, not already_executed]
      ensures: [withdrawn' <= deposited]
  internal: [_verifySignature, _executeTransfer]
  depends_on: [TokenRegistry, SignerRegistry]
  forbidden_deps: [TestUtils]
  invariants:
    - forall token: withdrawn[token] <= deposited[token]
```

## Property Categories

### Phase 1 (MVP)
- **exposes** - public functions that must exist
- **internal** - private functions (shouldn't be exposed)
- **depends_on** - allowed module dependencies
- **forbidden_deps** - banned dependencies

### Phase 2
- **layer** - infrastructure/domain/application/interface
- **events** - emits/subscribes

### Phase 3
- **state ownership** - owns_state, reads_state, modifies
- **access control** - callable_by, roles

## Supported Languages

- **Solidity**: Uses `solc --ast-json` for extraction
- **Rust**: Uses `cargo metadata` + `syn` crate
- **TypeScript**: Uses `tsc` API
- **Fallback**: `tree-sitter` for unsupported languages

## Installation

```bash
cargo install --path .
```

## Usage

```bash
# Check all specs in a directory
spec-checker check ./specs

# Check a single module
spec-checker check ./specs/Bridge.spec.yaml

# Generate spec skeleton from existing code
spec-checker init --language solidity ./contracts/Bridge.sol

# Diff: show spec vs implementation discrepancies
spec-checker diff ./specs/Bridge.spec.yaml ./contracts/Bridge.sol
```

## Development Phases

### MVP (Weeks 1-2)
- [ ] Structural checker for Solidity (`solc --ast-json`)
- [ ] YAML spec parser
- [ ] CLI with `check`, `init`, `diff` commands
- [ ] GitHub Actions integration

### Phase 2 (Weeks 3-4)
- [ ] Rust support (`cargo metadata` + `syn`)
- [ ] Layer violation detection
- [ ] Event emission checking

### Phase 3 (Month 2)
- [ ] Behavioral checker (runtime assertions)
- [ ] Fuzz test generation from requires/ensures
- [ ] TypeScript support

## License

Apache-2.0
