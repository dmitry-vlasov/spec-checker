# Spec Checker

A specification checker for codebases. In AI-heavy development, the spec is the human-controlled contract while AI generates the implementation.

```
Human intent → Spec (human-sized) → Checker → Code (AI-sized)
```

## Install

```bash
cargo install --path .

# Optional: SMT-based verification
apt install z3  # or: brew install z3
```

## Quick Start

```bash
# 1. Generate spec skeletons from source code
spec-checker init .

# 2. Enrich with AI (choose one)
spec-checker init --ai .          # via LLM API
# or: /spec-init                  # via Claude Code

# 3. Add behavioral contracts (choose one)
spec-checker refine --ai          # via LLM API
# or: /spec-refine                # via Claude Code

# 4. Validate specs against code
spec-checker check .
```

## What It Checks

Specs are verified through a three-tier cascade:

1. **Syntactic** — entity existence, dependencies, protocols, state ownership
2. **SMT solver** — formal verification of ownership constraints (when z3 available)
3. **LLM-verified** — behavioral invariants via AI (opt-in, cached)

```
Checking: bridge (src/bridge.rs)
  ✗ [dependency|syntactic] Forbidden dependency: 'bridge' imports 'test_utils'
  ⚠ [architectural|rules-engine] Layer violation: infrastructure → domain
  ✓ [architectural|smt] State ownership consistency verified
```

## Spec Generation: 2x2 Matrix

|  | **Init** (scaffold) | **Refine** (behavioral contracts) |
|---|---|---|
| **CLI** (`--ai`) | `spec-checker init --ai` | `spec-checker refine --ai` |
| **Claude Code** | `/spec-init` | `/spec-refine` |

- **Init** creates: description, API curation, forbidden deps, layer
- **Refine** adds: requires, ensures, modifies, invariants

Both process files in dependency order for cross-module context.

## Supported Languages

| Language | Extractor | Status |
|----------|-----------|--------|
| Rust | syn AST | Working |
| Solidity | solc AST + regex fallback | Working |
| Flow9 | Recursive descent parser | Working |

## Claude Code Skills

```bash
spec-checker init-skill    # install skills into .claude/commands/
```

| Skill | Usage | Purpose |
|-------|-------|---------|
| `/spec-init` | `/spec-init [dir]` | Init specs using Claude Code |
| `/spec-refine` | `/spec-refine [file]` | Add behavioral contracts |
| `/spec-checker` | `/spec-checker <mode>` | AI guidance map (orient, ask, plan, guard, impact) |

## Documentation

- [Spec Format Reference](docs/spec-format.md) — YAML format, entity kinds, subsystems, hierarchical defaults
- [Type Formula DSL](docs/type-formulas.md) — type expressions, predicates, constraints
- [Verification](docs/verification.md) — three-tier cascade, protocols, composition checks
- [Architecture](docs/architecture.md) — layers, contexts, stability, rule DSL, configuration
- [Claude Code Skills](docs/skills.md) — skill details and modes

## License

Apache-2.0
