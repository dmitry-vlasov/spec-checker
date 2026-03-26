# Spec Checker

A specification checker for codebases, built on the **spec-type** framework — a unified type system that extends classical types from expressions to modules, subsystems, and systems.

## Purpose

This tool verifies that implementation code conforms to declarative specifications. In AI-heavy development workflows, the spec becomes the human-controlled contract while AI generates the implementation.

```
Human intent → Spec (human-sized) → Checker → Code (AI-sized)
```

## The Spec-Type Concept

Traditional type systems verify that `f(x)` is well-formed by checking that `x` fits `f`'s input contract. Spec-checker generalizes this to every granularity level:

| Level | "Type" Concept | Composition Check |
|-------|---------------|-------------------|
| **Expressions** | Types | Type checking |
| **Functions** | Type constraints, requires/ensures | Contract satisfaction |
| **Modules** | Exposes, dependencies, state, protocol | Signature + protocol matching |
| **Subsystems** | Delegated interfaces, invariants | Interface + boundary checking |

A **spec-type** is an open set of constraints attached to an entity. Each constraint has a *kind* (structural, dependency, architectural, type-constraint, protocol, event) and is verified through a **cascade**:

1. **Syntactic matching** — direct structural comparison. Certain.
2. **SMT solver (z3)** — encode as formulas, get definitive answer. Certain.
3. **LLM-assisted** — binary yes/no question. Clearly labeled as "LLM-verified, not formally proven."

Each step either decides or passes to the next level. Output shows which tier verified each result.

## Key Benefits

1. **Spec as Prompt Context**: AI gets type constraints, conditions, allowed imports → better generation
2. **Safe Regeneration**: Can rewrite from scratch; checker ensures same interface/deps
3. **Review Efficiency**: Review 10-line spec change, not 500-line code change
4. **Prevent Drift**: Shortcuts violate spec → CI fails → architecture preserved
5. **Multi-Agent Coordination**: Specs define contracts between agents working on different modules
6. **Composition Verification**: Cross-module checks catch integration bugs without integration tests

## Spec Format

Unified YAML format covering structural, type, protocol, and behavioral specifications:

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

  defaultBridge:
    kind: variable

depends_on:
  - TokenRegistry
  - SignerRegistry

forbidden_deps:
  - TestUtils

# State ownership tracking
owns_state: [tokenBalances, withdrawalNonces]
reads_state: [allowlist]          # external state this module reads
modifies: [registry]              # external state this module modifies

# Event specifications
emits: [Deposit, Withdraw]
subscribes: [SystemShutdown]

# Protocol: valid call sequences
protocol:
  states: [uninitialized, active, paused, closed]
  initial: uninitialized
  terminal: [closed]
  transitions:
    - { from: uninitialized, call: initialize, to: active }
    - { from: active, call: deposit, to: active }
    - { from: active, call: withdraw, to: active }
    - { from: active, call: pause, to: paused }
    - { from: paused, call: resume, to: active }
    - { from: active, call: close, to: closed }
    - { from: paused, call: close, to: closed }
  balanced_pairs:
    - [pause, resume]

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
- `kind: variable` — global constants and static variables

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

## Protocol Specifications

Protocols define valid call sequences as state machines:

```yaml
protocol:
  states: [disconnected, connected, closed]
  initial: disconnected
  terminal: [closed]
  transitions:
    - { from: disconnected, call: connect, to: connected }
    - { from: connected, call: send, to: connected }
    - { from: connected, call: disconnect, to: closed }
  balanced_pairs:
    - [connect, disconnect]
```

The checker verifies:
- **State consistency**: initial/terminal states are in the states list, transitions reference valid states
- **Function existence**: all transition functions exist in the implementation
- **Reachability**: warns about unreachable states and dead-end non-terminal states
- **Balanced pairs**: static analysis counts open/close calls in source code and warns about mismatches
- **Cross-module protocols**: when module A depends on module B with a protocol, verifies that subscribed events correspond to protocol transitions

## State Ownership

Track which modules own, read, or modify shared state:

```yaml
owns_state: [balances, nonces]     # this module owns these
reads_state: [config]               # reads external state
modifies: [registry]                # modifies external state
```

The checker verifies:
- Listed state variables exist in the implementation
- Warns about unlisted state variables
- **Cross-module**: `reads_state`/`modifies` references must match some module's `owns_state`
- **Ownership conflicts**: errors if two modules both claim to own the same state (also verified by SMT solver when z3 is available)

## Subsystem Specifications

Group modules into higher-level units with `*.subsystem.yaml` files:

```yaml
subsystem: PaymentService
modules:
  - src/bridge.rs
  - src/registry.rs

exposes:
  deposit:
    delegates_to: bridge.deposit
    requires: [amount > 0]
    ensures: [balance increases]

invariants:
  - "total_deposited >= total_withdrawn"

depends_on: [AuthService]
forbidden_deps: [TestUtils]

layer: application
context: payments
stability: stable
```

The checker verifies:
- All listed modules have matching specs
- Delegated interface targets exist in member modules' exposes
- Warns about internal dependencies that leak outside the subsystem
- Enforces forbidden subsystem dependencies

## Composition Checking

When multiple specs are loaded, the checker automatically runs cross-module verification:

- **Event subscribe/emit matching**: every subscription must have a matching emitter
- **Contract compatibility**: when a consumer's `requires` references a provider's function, checks that the provider's `ensures` cover the requirement
- **State ownership consistency**: SMT-verified mutual exclusion of state ownership (when z3 is available)
- **Cross-module protocol compatibility**: verifies event subscriptions align with provider's protocol transitions

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

## Verification Cascade

Every constraint check goes through a cascade of increasingly powerful checkers:

### Tier 1: Syntactic (always runs)
- Entity existence (functions, types, variables)
- Dependency matching (allowed/forbidden)
- Event existence and completeness
- Protocol state machine validation
- State ownership conflicts
- Balanced pair call counting
- Static invariant checks ("no panics", "no unsafe", "no unwrap")

**Note on static invariant checks**: The current static checks use keyword scanning (searching for patterns like `unwrap()`, `unsafe` in source text). This approach can produce false positives when the source code itself is a checker that searches for those patterns. The checker attempts to filter out `.contains("...")` lines and similar meta-patterns, but this is inherently imprecise. A proper solution would use AST-based analysis (e.g., via `syn` for Rust). This is a known limitation.

### Tier 2: SMT Solver (when z3 is available)
- State ownership mutual exclusion (formally verified)
- Numeric constraint satisfiability
- Logical implication checking

Install z3 for SMT verification: `apt install z3` / `brew install z3`. If z3 is not found, these checks are silently skipped.

### Tier 3: LLM-Verified (opt-in)
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

Rate-limited requests (HTTP 429) are automatically retried with exponential backoff (up to 5 retries: 2s, 4s, 8s, 16s, 32s).

**Important**: LLM verification is not formal proof. Results are clearly labeled as "LLM-verified".

### Output Tags

Every check result shows its constraint kind and verification tier:

```
  ✗ [dependency|syntactic] Forbidden dependency: 'bridge' imports 'test_utils'
  ⚠ [architectural|rules-engine] Layer violation: infrastructure → domain
  ✓ [architectural|smt] State ownership consistency verified
```

## Supported Languages

| Language | Extractor | Status |
|----------|-----------|--------|
| Rust | syn AST (full type extraction) | Working |
| Solidity | solc AST + regex fallback | Working |
| Flow9 | Recursive descent parser (full top-level grammar) | Working |
| TypeScript | - | Planned |

## Installation

```bash
cargo install --path .
```

Optional: install z3 for SMT-based verification:
```bash
# Ubuntu/Debian
apt install z3

# macOS
brew install z3
```

## Configuration

Create `.spec-checker.yaml` in your project root to set defaults:

```yaml
# .spec-checker.yaml
llm:
  # Anthropic (default)
  endpoint: https://api.anthropic.com
  model: claude-haiku-4-5-20251001
  api_key: sk-ant-...          # optional, can use ANTHROPIC_API_KEY env var
  check: off                    # default mode: off | dry-run | cached-only | full

  # Or use a local LLM (Ollama, vLLM, LM Studio, etc.)
  # endpoint: http://localhost:11434/v1
  # model: llama3.1
  # check: full

rules: rules.yaml              # default rules file
```

**Resolution order** (each overrides the previous):
1. Built-in defaults (endpoint=anthropic, model=haiku, check=off)
2. `.spec-checker.yaml` in project root
3. Environment variables (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `LLM_API_KEY`)
4. CLI flags (`--llm-endpoint`, `--llm-model`, `--llm-check`, `--llm-api-key`)

The provider is auto-detected from the endpoint URL: `anthropic.com` uses the Anthropic API format, everything else uses the OpenAI-compatible format (works with Ollama, vLLM, LM Studio, LocalAI, llama.cpp server, etc.).

For local LLMs, the API key is optional — most local servers don't require one.

## Usage

```bash
# Check all specs in a directory
spec-checker check ./specs -s .

# Check a single module
spec-checker check ./specs/checker.spec.yaml -s .

# With custom rules
spec-checker check ./specs -s . -r rules.yaml

# With behavioral checks (dry-run, uses config file defaults)
spec-checker check ./specs -s . --llm-check dry-run

# With local LLM (overrides config)
spec-checker check ./specs -s . --llm-check full \
  --llm-endpoint http://localhost:11434/v1 --llm-model llama3.1

# With explicit Anthropic API key
ANTHROPIC_API_KEY=sk-ant-... spec-checker check ./specs -s . --llm-check full

# Generate spec skeleton from existing code (with protocol detection)
spec-checker init --language rust ./src/main.rs

# Generate specs for entire directory
spec-checker init --language rust ./src/ -o ./specs/

# Diff: show spec vs implementation discrepancies
spec-checker diff ./specs/main.spec.yaml ./src/main.rs
```

## Self-Verification

The spec-checker verifies its own structure (10 modules, 1 subsystem, ~120 type constraints, ~30 invariants):

```bash
$ spec-checker check ./specs -s .

Spec Checker
========================================

Checking: checker
  ✓ All checks passed

Checking: spec
  ✓ All checks passed

...

Subsystem Checks
Checking subsystem: CoreVerification
  ✓ All checks passed

========================================
PASSED: All specs validated
```

## License

Apache-2.0
