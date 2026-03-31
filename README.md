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
description: "Cross-chain token bridge handling deposits and signature-verified withdrawals"
language: solidity
source_path: contracts/Bridge.sol
source_hash: a1b2c3d4e5f6a7b8  # SHA-256 truncated to 16 hex chars

exposes:
  BridgeState:
    kind: type
    description: "Tracks deposited balances and admin configuration"
    type_constraints:
      - "is_product(Self)"
      - "has_field(Self, deposited)"
      - "has_field(Self, admin)"

  Bridge.deposit:
    kind: function
    description: "Accepts a token deposit, updating the bridge's balance tracking"
    type_constraints:
      - "equals(param(token), str)"
      - "fallible(return)"
    requires: [amount > 0, token in allowed_tokens]
    ensures: [deposited increases by amount]

  Bridge.withdraw:
    kind: function
    description: "Processes a withdrawal after verifying the cryptographic signature"
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

### Descriptions

Any entity (module, subsystem, function, type) can have an optional `description` field — a concise natural-language summary of its purpose. Descriptions are not used in formal verification (Tiers 1–2) but are included as context in LLM behavioral checks (Tier 3), improving the quality of invariant verification.

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

## Spec Staleness Detection

Each spec file can store a `source_hash` — a truncated SHA-256 hash (16 hex chars) of the source file content at the time the spec was last generated or updated:

```yaml
source_path: src/checker.rs
source_hash: 6d752c371c15af4a
```

- **`spec-checker init`** computes and stores the hash automatically
- **`spec-checker check`** compares the stored hash against the current file content; mismatch produces a warning:
  ```
  ⚠ [structural|syntactic] Specs may be stale for 'checker': source file has changed since specs were last updated
  ```
- **Backward compatible**: specs without `source_hash` skip the staleness check silently

This is particularly important for behavioral specs (requires/ensures/invariants), which are expensive to regenerate via LLM. The hash check is effectively free — just a file read and SHA-256.

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

Every check result shows its constraint kind and verification tier. Modules with issues display their source file path:

```
Checking: bridge (src/bridge.rs)
  ✗ [dependency|syntactic] Forbidden dependency: 'bridge' imports 'test_utils'
  ⚠ [architectural|rules-engine] Layer violation: infrastructure → domain

Composition Checks
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

# Exclude patterns (persisted by --exclude flag, used by both init and check)
exclude:
  - tools/**
  - tests/**

llm:
  # Anthropic (default)
  endpoint: https://api.anthropic.com
  model: claude-haiku-4-5-20251001
  api_key: sk-ant-...          # optional, can use ANTHROPIC_API_KEY env var
  check: off                    # default mode: off | dry-run | cached-only | full

  # Or use a local LLM as primary (Ollama, vLLM, LM Studio, etc.)
  # endpoint: http://localhost:11434/v1
  # model: qwen3.5:9b
  # check: full

  # Hybrid mode: local LLM for dry-run previews, cloud for full verification
  # When 'local' is configured and 'check: dry-run', the local LLM runs
  # the checks instead of just printing token estimates. Results are cached,
  # so subsequent 'cached-only' runs use them instantly.
  local:
    endpoint: http://localhost:11434/v1
    model: qwen3.5:9b

rules: rules.yaml              # default rules file
```

**Resolution order** (each overrides the previous):
1. Built-in defaults (endpoint=anthropic, model=haiku, check=off)
2. `.spec-checker.yaml` in project root
3. Environment variables (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `LLM_API_KEY`)
4. CLI flags (`--llm-endpoint`, `--llm-model`, `--llm-check`, `--llm-api-key`)

**Provider auto-detection**: `anthropic.com` in URL → Anthropic API format; everything else → OpenAI-compatible format (Ollama, vLLM, LM Studio, LocalAI, llama.cpp server, etc.). For local LLMs, the API key is optional.

**Hybrid mode**: Configure a `local` section under `llm` to use a local LLM for `dry-run` preview checks. This gives fast, free previews before committing to cloud API costs. Recommended local models: `qwen3.5:9b` (best reasoning) or `qwen2.5-coder:7b` (code-focused).

## Usage

```bash
# Check all specs (default: ./specs dir, current dir as source root)
spec-checker check .

# Check with explicit paths
spec-checker check ./specs -s .

# Check a single module
spec-checker check ./specs/checker.spec.yaml -s .

# Verbose output (show every module, not just errors/warnings)
spec-checker check . -v

# With custom rules
spec-checker check ./specs -s . -r rules.yaml

# With behavioral checks (dry-run, uses config file defaults)
spec-checker check ./specs -s . --llm-check dry-run

# With local LLM (overrides config)
spec-checker check ./specs -s . --llm-check full \
  --llm-endpoint http://localhost:11434/v1 --llm-model llama3.1

# With explicit Anthropic API key
ANTHROPIC_API_KEY=sk-ant-... spec-checker check ./specs -s . --llm-check full

# Generate lean spec skeleton from existing code
spec-checker init --language rust ./src/main.rs

# Generate specs for entire directory
spec-checker init --language rust ./src/ -o ./specs/

# AI-enriched init: adds descriptions, curates API, forbidden deps, layer
spec-checker init --ai ./src/
spec-checker init --ai --llm-provider anthropic ./src/main.rs

# Refine specs with behavioral contracts (requires/ensures/modifies/invariants)
spec-checker refine --ai
spec-checker refine --ai --file src/checker.rs

# Exclude directories from init (persisted to .spec-checker.yaml)
spec-checker init --language flow9 . -o ./specs --exclude 'tools/**' --exclude 'tests/**'

# Diff: show spec vs implementation discrepancies
spec-checker diff ./specs/main.spec.yaml ./src/main.rs

# Install Claude Code skills (spec-refine + spec-checker by default)
spec-checker init-skill

# Install globally
spec-checker init-skill --global

# Install a specific skill
spec-checker init-skill --only flow9
spec-checker init-skill --only spec-checker
```

## Spec Generation: 2x2 Matrix

Spec generation has two dimensions — **what** to do and **how** to do it:

|  | **Init** (scaffold from scratch) | **Refine** (add behavioral contracts) |
|---|---|---|
| **CLI** (`--ai`, LLM API) | `spec-checker init --ai` | `spec-checker refine --ai` |
| **Claude Code** (skill) | `/spec-init` | `/spec-refine` |

### Init: structural scaffolding

Creates specs from source code: module description, API curation, forbidden deps, layer classification.

- `spec-checker init --ai` — batch, automated, uses configured LLM provider. Good for CI or when Claude Code isn't available.
- `/spec-init` — interactive, uses Claude Code's intelligence. Higher quality, no API key needed.

### Refine: behavioral contracts

Deepens existing specs with per-function requires/ensures/modifies and module-level invariants.

- `spec-checker refine --ai` — batch, automated, uses configured LLM provider.
- `/spec-refine` — interactive, uses Claude Code. Best quality for complex behavioral contracts.

Both process files in dependency order so downstream specs can reference upstream contracts.

### Recommended workflow

```bash
# 1. Generate lean skeletons
spec-checker init .

# 2. Enrich with AI (choose one)
spec-checker init --ai .          # via LLM API
# or: /spec-init                  # via Claude Code

# 3. Add behavioral contracts (choose one)
spec-checker refine --ai          # via LLM API
# or: /spec-refine                # via Claude Code

# 4. Validate
spec-checker check .
```

## Claude Code Skills

Spec-checker ships with Claude Code skills that can be installed into any project via `spec-checker init-skill`. Skills are markdown prompt files placed in `.claude/commands/`.

| Skill | Installed by default | Usage | Purpose |
|-------|---------------------|-------|---------|
| `spec-init` | Yes | `/spec-init [dir]` | Init specs using Claude Code (no API key needed) |
| `spec-refine` | Yes | `/spec-refine [file]` | Refine specs with per-function behavioral contracts |
| `spec-checker` | Yes | `/spec-checker <mode>` | AI guidance map: use specs for reasoning, planning, refactoring, and Q&A |
| `flow9` | No (`--only flow9`) | `/flow9` | Flow9 language reference for the AI agent |

```bash
# Install default skills (spec-init + spec-refine + spec-checker)
spec-checker init-skill

# Install all skills including language-specific ones
spec-checker init-skill --only all

# Install globally (available in all projects)
spec-checker init-skill --global
```

### spec-init

The `spec-init` skill scaffolds specs using Claude Code. It runs `spec-checker init` to create lean skeletons, then enriches each one with descriptions, API curation, forbidden deps, and layer classification. Processes in dependency order for cross-module context.

### spec-refine

The `spec-refine` skill deepens existing specs with per-function behavioral contracts (`requires`, `ensures`, `modifies`, `invariants`). Processes in dependency order so each module's contracts can reference upstream guarantees.

```bash
# Fill specs for the whole project
/spec-refine

# Fill specs for a single file
/spec-refine src/checker.rs
```

### spec-checker (AI Guidance Skill)

The `spec-checker` skill turns specs into an **AI guidance map** — a compressed semantic model of the codebase that an AI agent consults before reading source code.

### Layered resolution

The skill follows a layered resolution principle, stopping as soon as it has enough information:

1. **Spec descriptions + dependency graph** (~1500 tokens for a whole project)
2. **Contracts** — requires/ensures/modifies/invariants of specific entities
3. **Source code** — only when specs are insufficient

### Modes

| Mode | Usage | Purpose |
|------|-------|---------|
| `help` | `/spec-checker help` | Show available modes |
| `orient` | `/spec-checker orient` | Read all specs, build mental model of the codebase |
| `ask` | `/spec-checker ask <question>` | Answer questions from specs first, fall through to source if needed |
| `plan` | `/spec-checker plan <feature>` | Spec-first development: design in specs, then implement |
| `guard` | `/spec-checker guard` | Snapshot contracts before refactoring, verify preservation after |
| `impact` | `/spec-checker impact <module>` | Trace dependency graph to show blast radius of changes |
| `check` | `/spec-checker check` | Run spec-checker, interpret results, offer to fix staleness |
| `bootstrap` | `/spec-checker bootstrap` | One-command setup for new projects |

### Use cases

- **Refactoring**: use `guard` to ensure semantics don't change
- **New features**: use `plan` to design specs first, then implement to satisfy them
- **Q&A**: use `ask` to get answers from specs without reading source
- **Onboarding**: use `orient` to understand a codebase in seconds

## Self-Verification

The spec-checker verifies its own structure:

```bash
$ spec-checker check .

Spec Checker
========================================

========================================
PASSED: All 14 specs validated
```

By default, only modules with errors or warnings are shown. Use `-v` for verbose output:

```bash
$ spec-checker check . -v

Spec Checker
========================================

Checking: behavioral (src/behavioral.rs)
  ✓ All checks passed

Checking: checker (src/checker.rs)
  ✓ All checks passed

...

========================================
PASSED: All 14 specs validated
```

## License

Apache-2.0
