# Spec Format Reference

Unified YAML format covering structural, type, protocol, and behavioral specifications.

## Full Example

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

forbidden_deps:
  - TestUtils

# State ownership tracking
owns_state: [tokenBalances, withdrawalNonces]
reads_state: [allowlist]
modifies: [registry]

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

## Entity Naming

Methods are qualified with their parent type using dot notation (language-agnostic):
- `Bridge.deposit` — method `deposit` on type `Bridge`
- `get_extractor` — standalone function (no qualifier)

## Entity Kinds

Each entry in `exposes` has a `kind` field:
- `kind: function` — functions and methods
- `kind: type` — structs, enums, traits, interfaces
- `kind: variable` — global constants and static variables

## Descriptions

Any entity (module, subsystem, function, type) can have an optional `description` field — a concise natural-language summary of its purpose. Descriptions are included as context in LLM behavioral checks (Tier 3), improving the quality of invariant verification.

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

## Hierarchical Defaults

Use `_defaults.yaml` files to set directory-level defaults with per-file overrides:

```
specs/
  _defaults.yaml              # root defaults: language, context, stability
  checker.spec.yaml            # inherits root defaults
  main.spec.yaml               # overrides context=cli, stability=normal
  extractors/
    _defaults.yaml             # layer=infrastructure, extra forbidden_deps
    mod.spec.yaml              # inherits both root + extractors defaults
```

**Inheritance rules:**
- **Scalar fields** (language, layer, context, stability): child overrides parent
- **List fields** (forbidden_deps, external_deps, forbidden_external): child merges (union) with parent
- Per-file specs override defaults — explicitly set fields are never overwritten

## Staleness Detection

Each spec can store a `source_hash` — a truncated SHA-256 hash (16 hex chars) of the source file. `spec-checker check` warns when the hash doesn't match the current file. `spec-checker init` computes the hash automatically.

## State Ownership

Track which modules own, read, or modify shared state:

```yaml
owns_state: [balances, nonces]     # this module owns these
reads_state: [config]               # reads external state
modifies: [registry]                # modifies external state
```

The checker verifies:
- Listed state variables exist in the implementation
- **Cross-module**: `reads_state`/`modifies` references must match some module's `owns_state`
- **Ownership conflicts**: errors if two modules both claim to own the same state (also verified by SMT when z3 is available)
