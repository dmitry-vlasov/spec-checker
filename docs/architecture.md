# Architectural Constraints

The spec-checker enforces three orthogonal architectural constraints.

## Layer (Vertical Stratification)

The default 4-layer model:

```
Presentation → Application → Domain → Infrastructure
```

Higher layers depend on lower layers, never the reverse. **Presentation** is the user-facing entry point (CLI, HTTP handlers, API endpoints). **Application** orchestrates domain logic and infrastructure. **Domain** holds core business rules and data structures. **Infrastructure** handles external I/O (databases, file systems, third-party APIs).

Note: this is *not* the same as an OOP "interface" (a public contract). A presentation-layer module is a top-level entry point, not a leaf abstraction.

### Custom Layers

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

## Context (Horizontal Segmentation)

Bounded contexts isolate different functional areas:

```yaml
context: payments   # or: users, bridge, verifier
```

**Rule**: Cross-context dependencies must go through the Presentation layer.

## Stability (Change Frequency)

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

## Configuration

Create `.spec-checker.yaml` in your project root:

```yaml
exclude:
  - tools/**
  - tests/**

llm:
  check: off                    # off | dry-run | cached-only | full
  default: anthropic
  providers:
    anthropic:
      endpoint: https://api.anthropic.com
      model: claude-haiku-4-5-20251001
    local:
      endpoint: http://localhost:11434/v1
      model: qwen3.5:9b

rules: rules.yaml
```

**Resolution order**: built-in defaults → `.spec-checker.yaml` → env vars → CLI flags.

**Provider auto-detection**: `anthropic.com` in URL → Anthropic format; everything else → OpenAI-compatible (Ollama, vLLM, LM Studio, etc.).
