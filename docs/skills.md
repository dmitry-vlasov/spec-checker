# Claude Code Skills

Spec-checker ships with Claude Code skills — markdown prompt files installed into `.claude/commands/`.

```bash
spec-checker init-skill              # install default skills
spec-checker init-skill --global     # install globally
spec-checker init-skill --only all   # install all including language-specific
```

## Available Skills

| Skill | Default | Usage | Purpose |
|-------|---------|-------|---------|
| `spec-init` | Yes | `/spec-init [dir]` | Init specs using Claude Code |
| `spec-refine` | Yes | `/spec-refine [file]` | Add behavioral contracts |
| `spec-checker` | Yes | `/spec-checker <mode>` | AI guidance map |
| `flow9` | No | `/flow9` | Flow9 language reference |

## spec-init

Scaffolds specs using Claude Code. Runs `spec-checker init` for lean skeletons, then enriches each with descriptions, API curation, forbidden deps, and layer classification. Processes in dependency order.

## spec-refine

Deepens existing specs with per-function behavioral contracts (`requires`, `ensures`, `modifies`, `invariants`). Processes in dependency order so each module's contracts can reference upstream guarantees.

```bash
/spec-refine                    # whole project
/spec-refine src/checker.rs     # single file
```

## spec-checker (AI Guidance)

Turns specs into an **AI guidance map** — a compressed semantic model consulted before reading source code.

**Layered resolution** — stops as soon as it has enough information:
1. Spec descriptions + dependency graph (~1500 tokens for a whole project)
2. Contracts — requires/ensures/modifies/invariants of specific entities
3. Source code — only when specs are insufficient

**Modes:**

| Mode | Usage | Purpose |
|------|-------|---------|
| `orient` | `/spec-checker orient` | Build mental model from specs |
| `ask` | `/spec-checker ask <question>` | Answer from specs first |
| `plan` | `/spec-checker plan <feature>` | Spec-first development |
| `guard` | `/spec-checker guard` | Guard refactoring semantics |
| `impact` | `/spec-checker impact <module>` | Show blast radius |
| `check` | `/spec-checker check` | Run checks, interpret results |
| `bootstrap` | `/spec-checker bootstrap` | One-command setup |
