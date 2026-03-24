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

## Hierarchical Specs

For larger projects, per-file specs don't scale. Use `_defaults.yaml` files to set directory-level defaults with per-file overrides:

```
specs/
  _defaults.yaml              # root defaults: language, context, stability
  checker.spec.yaml            # inherits root defaults
  main.spec.yaml               # overrides context=cli, stability=normal
  extractors/
    _defaults.yaml             # layer=infrastructure, extra forbidden_deps
    mod.spec.yaml              # inherits both root + extractors defaults
    rust.spec.yaml             # inherits both, adds own forbidden_deps
    solidity.spec.yaml
```

**`_defaults.yaml` example:**
```yaml
language: rust
context: core
stability: stable
forbidden_deps:
  - clap
```

**Inheritance rules:**
- **Scalar fields** (language, layer, context, stability): child overrides parent
- **List fields** (forbidden_deps, external_deps, forbidden_external): child merges (union) with parent
- Per-file specs override defaults — explicitly set fields are never overwritten

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
- **context** - bounded contexts for horizontal isolation
- **stability** - stable/normal/volatile (stable can't depend on volatile)
- **events** - emits (checked), subscribes (spec only)

### Phase 3
- **state ownership** - owns_state, reads_state, modifies
- **access control** - callable_by, roles

## Architectural Concepts

The spec-checker enforces three orthogonal architectural constraints:

### Layer (Vertical Stratification)

The default 4-layer model:

```
Interface → Application → Domain → Infrastructure
```

- **Infrastructure**: Cannot depend on Domain/Application/Interface
- **Domain**: Can only depend on Infrastructure
- **Application**: Can depend on Domain and Infrastructure
- **Interface**: Can depend on anything

#### Custom Layers

Define your own layers in `rules.yaml`. Two formats:

**Linear hierarchy** (each layer can depend on all layers below):
```yaml
layers:
  - presentation
  - application
  - domain
  - persistence
```

**Explicit DAG** (full control over allowed dependencies):
```yaml
layers:
  api:
    can_depend_on: [service, model]
  service:
    can_depend_on: [model, repository]
  repository:
    can_depend_on: [model]
  model:
    can_depend_on: []
```

When no `layers` config is provided, the built-in 4-layer model is used.

### Context (Horizontal Segmentation)

Bounded contexts isolate different functional areas:

```yaml
context: payments   # or: users, bridge, verifier
```

**Rule**: Cross-context dependencies must go through the Interface layer.

```
✗ Context violation: 'src/payments.rs' (context: payments) cannot directly 
  depend on 'src/users.rs' (context: users). Cross-context dependencies 
  must go through Interface layer.
```

### Stability (Change Frequency)

```yaml
stability: stable   # or: normal, volatile
```

- **Stable**: Core abstractions, rarely change
- **Normal**: Standard modules
- **Volatile**: Features, frequently change

**Rule**: Stable modules cannot depend on less stable ones.

```
✗ Stability violation: 'src/core.rs' (Stable) cannot depend on 
  'src/feature.rs' (Volatile). Stable modules cannot depend on less stable ones.
```

## Rule DSL

Define custom architectural rules in a `rules.yaml` file:

```yaml
rules:
  - name: no-test-depends-prod
    description: "Test modules should not depend on production code"
    when: "source.context == test"
    require: "target.context == test OR target.stability == stable"
    severity: warning

  - name: core-must-be-stable  
    description: "Core modules must have stable stability"
    when: "source.context == core"
    require: "source.stability == stable"
    severity: error

# Disable built-in rules if needed
disable_builtin:
  - layer-direction
```

Load custom rules:
```bash
spec-checker check ./specs -s . --rules rules.yaml
```

### DSL Reference

**Properties** (source.X or target.X):
- `module`, `name`: module name
- `path`, `source_path`: file path  
- `layer`: infrastructure | domain | application | interface
- `context`: bounded context name
- `stability`: stable | normal | volatile

**Operators**:
- `==`, `!=`: equality
- `AND`, `OR`, `NOT` (or `&&`, `||`, `!`)

**Methods**:
- `source.layer.can_depend_on(target.layer)`
- `source.stability.can_depend_on(target.stability)`
- `source.X.exists`: check if property has value

## Example Violations

Layer:
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
- [x] Events checking (emits)
- [x] Rule DSL for custom architectural constraints
- [x] Hierarchical specs (`_defaults.yaml` with directory-level inheritance)
- [x] Configurable layers (user-defined layer names with linear or DAG dependencies)

### In Progress
- [ ] Subscribes checking (requires function body analysis)
- [ ] More DSL functions (starts_with, contains, etc.)

### Planned
- [ ] GitHub Actions integration
- [ ] TypeScript support
- [ ] Behavioral checker (runtime assertions)

## License

Apache-2.0
