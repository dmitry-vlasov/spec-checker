# Spec Checker

A structural and behavioral specification checker for codebases.

## Purpose

This tool verifies that implementation code conforms to declarative specifications. In AI-heavy development workflows, the spec becomes the human-controlled contract while AI generates the implementation.

```
Human intent → Spec (human-sized) → Checker → Code (AI-sized)
```

## Key Benefits

1. **Spec as Prompt Context**: AI gets type constraints, conditions, allowed imports → better generation
2. **Safe Regeneration**: Can rewrite from scratch; checker ensures same interface/deps
3. **Review Efficiency**: Review 10-line spec change, not 500-line code change
4. **Prevent Drift**: Shortcuts violate spec → CI fails → architecture preserved
5. **Multi-Agent Coordination**: Specs define contracts between agents working on different modules

## Spec Format

Unified YAML format covering structural, type, and behavioral specifications:

```yaml
module: Bridge
language: solidity
source_path: contracts/Bridge.sol

exposes:
  BridgeState:
    kind: type
    type_constraints:
      - "is_product(Self)"
      - "has_field(Self, deposited)"
      - "has_field(Self, admin)"

  Bridge.deposit:
    kind: function
    type_constraints:
      - "equals(param(token), str)"
      - "fallible(return)"
    requires: [amount > 0, token in allowed_tokens]
    ensures: [deposited increases by amount]

  Bridge.withdraw:
    kind: function
    type_constraints:
      - "fallible(return)"
    requires: [valid_signature, not already_executed]
    ensures: [withdrawn <= deposited]

depends_on:
  - TokenRegistry
  - SignerRegistry

forbidden_deps:
  - TestUtils

invariants:
  - "forall token: withdrawn[token] <= deposited[token]"
```

### Entity Naming

Methods are qualified with their parent type using dot notation (language-agnostic):
- `Bridge.deposit` — method `deposit` on type `Bridge`
- `get_extractor` — standalone function (no qualifier)

### Entity Kinds

Each entry in `exposes` has a `kind` field:
- `kind: function` — functions and methods
- `kind: type` — structs, enums, traits, interfaces

## Type Formula DSL

Abstract type constraints expressed as formulas, not structural duplication:

```yaml
exposes:
  CheckResult:
    kind: type
    type_constraints:
      - "is_product(Self)"
      - "has_field(Self, errors)"
      - "equals(con(field(errors)), Vec)"
      - "cloneable"

  SpecChecker.check:
    kind: function
    type_constraints:
      - "equals(param(spec), ModuleSpec)"
      - "fallible(return)"
      - "equals(con(return), Result)"
```

### Type Expressions

| Expression | Meaning |
|-----------|---------|
| `Self` | The type being specified |
| `field(name)` | Type of a struct field |
| `param(n)` or `param(name)` | Function parameter type |
| `return` | Function return type |
| `con(T)` | Type constructor (strip args): `con(Vec<String>)` = `Vec` |
| `arg(T, n)` | n-th type argument: `arg(Result<X, E>, 0)` = `X` |
| `apply(F, A, ...)` | Apply constructor to args |
| `function(A, B, ..., R)` | Function type (last = return) |
| `domain(F)` / `codomain(F)` | Parameter/return of function type |
| `sum(A, B, ...)` / `product(A, B, ...)` | Algebraic types |

### Predicates

| Predicate | Meaning |
|----------|---------|
| `equals(T1, T2)` | Type equality (strips references automatically) |
| `matches(T, pattern)` | Pattern match with `_` wildcards |
| `is_subtype(T, Super)` | Subtyping (trait impl, interface) |
| `is_sum` / `is_product` / `is_function` | Kind checks |
| `has_field(T, name)` | Struct has field |
| `has_method(T, name)` | Type has method |
| `has_variant(T, name)` | Enum has variant |
| `cloneable` / `serializable` / `send` / `sync` | Property checks |
| `fallible` | Returns Result or Option |
| `and` / `or` / `not` / `implies` | Logical connectives |

Type comparisons are **language-agnostic**: `equals` strips references (`&`, `&mut`) so specs express type dependencies, not language-specific passing conventions.

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
```

**Inheritance rules:**
- **Scalar fields** (language, layer, context, stability): child overrides parent
- **List fields** (forbidden_deps, external_deps, forbidden_external): child merges (union) with parent
- Per-file specs override defaults — explicitly set fields are never overwritten

## Architectural Concepts

The spec-checker enforces three orthogonal architectural constraints:

### Layer (Vertical Stratification)

The default 4-layer model:

```
Interface → Application → Domain → Infrastructure
```

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

### Context (Horizontal Segmentation)

Bounded contexts isolate different functional areas:

```yaml
context: payments   # or: users, bridge, verifier
```

**Rule**: Cross-context dependencies must go through the Interface layer.

### Stability (Change Frequency)

```yaml
stability: stable   # or: normal, volatile
```

**Rule**: Stable modules cannot depend on less stable ones.

## Rule DSL

Define custom architectural rules in a `rules.yaml` file:

```yaml
rules:
  - name: no-test-depends-prod
    description: "Test modules should not depend on production code"
    when: "source.context == test"
    require: "target.context == test OR target.stability == stable"
    severity: warning

# Disable built-in rules if needed
disable_builtin:
  - layer-direction
```

## Behavioral Checker

Invariants, requires, and ensures clauses are verified through a tiered approach:

### Tier 1: Static-Semantic (no LLM)
Pattern-based checks run automatically:
- "never panics" → scans for `unwrap()`, `panic!()`, `expect()`
- "no unsafe" → scans for `unsafe` blocks

### Tier 2: LLM-Verified
Invariants requiring code comprehension are sent to an LLM with hash-based caching:

```bash
# Preview what would be checked (zero cost)
spec-checker check ./specs -s . --llm-check dry-run

# Use only cached results (for CI)
spec-checker check ./specs -s . --llm-check cached-only

# Full verification (calls LLM for uncached invariants)
ANTHROPIC_API_KEY=sk-ant-... spec-checker check ./specs -s . --llm-check full
```

Results are cached in `.spec-cache/` keyed by `sha256(code + invariant)`. Unchanged code = zero token spend.

**Important**: LLM verification is not formal proof. Results are clearly labeled as "LLM-verified".

## Supported Languages

| Language | Extractor | Status |
|----------|-----------|--------|
| Rust | syn AST (full type extraction) | Working |
| Solidity | solc AST + regex fallback | Working |
| Flow9 | Regex (exports, structs, unions, functions) | Working |
| TypeScript | - | Planned |

## Installation

```bash
cargo install --path .
```

## Usage

```bash
# Check all specs in a directory
spec-checker check ./specs -s .

# Check a single module
spec-checker check ./specs/checker.spec.yaml -s .

# With behavioral checks (dry-run)
spec-checker check ./specs -s . --llm-check dry-run

# Generate spec skeleton from existing code
spec-checker init --language rust ./src/main.rs

# Diff: show spec vs implementation discrepancies
spec-checker diff ./specs/main.spec.yaml ./src/main.rs
```

## Self-Verification

The spec-checker verifies its own structure (9 modules, ~100 type constraints, ~30 invariants):

```bash
$ spec-checker check ./specs -s .

Spec Checker
========================================

Checking: checker
  ✓ All checks passed

Checking: extractors
  ✓ All checks passed

...

========================================
PASSED: All specs validated
```

## License

Apache-2.0
