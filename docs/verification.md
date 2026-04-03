# Verification

## Three-Tier Cascade

Every constraint check goes through a cascade of increasingly powerful checkers:

### Tier 1: Syntactic (always runs)
- Entity existence (functions, types, variables)
- Dependency matching (allowed/forbidden)
- Event existence and completeness
- Protocol state machine validation
- State ownership conflicts
- Balanced pair call counting
- Static invariant checks ("no panics", "no unsafe", "no unwrap")

**Note**: Static checks use keyword scanning, which can produce false positives when code itself searches for those patterns. A known limitation.

### Tier 2: SMT Solver (when z3 is available)
- State ownership mutual exclusion (formally verified)
- Numeric constraint satisfiability
- Logical implication checking

Install z3: `apt install z3` / `brew install z3`. If not found, these checks are silently skipped.

### Tier 3: LLM-Verified (opt-in)
Invariants requiring code comprehension are sent to an LLM with hash-based caching:

```bash
spec-checker check ./specs -s . --llm-check dry-run       # preview (zero cost)
spec-checker check ./specs -s . --llm-check cached-only    # CI-safe
spec-checker check ./specs -s . --llm-check full            # calls LLM
```

Results cached in `.spec-cache/` keyed by `sha256(code + invariant)`. Unchanged code = zero token spend. Rate limits are retried with exponential backoff.

**Important**: LLM verification is not formal proof. Results are labeled as "LLM-verified".

### Output Tags

Every result shows its constraint kind and verification tier:

```
Checking: bridge (src/bridge.rs)
  ✗ [dependency|syntactic] Forbidden dependency: 'bridge' imports 'test_utils'
  ⚠ [architectural|rules-engine] Layer violation: infrastructure → domain

Composition Checks
  ✓ [architectural|smt] State ownership consistency verified
```

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

The checker verifies: state consistency, function existence, reachability, balanced pairs, and cross-module protocol compatibility.

## Composition Checking

When multiple specs are loaded, the checker automatically runs cross-module verification:

- **Event subscribe/emit matching**: every subscription must have a matching emitter
- **Contract compatibility**: consumer's `requires` vs provider's `ensures`
- **State ownership consistency**: SMT-verified mutual exclusion (when z3 is available)
- **Cross-module protocol compatibility**: event subscriptions align with protocol transitions
