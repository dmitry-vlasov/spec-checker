# Spec Init

You are tasked with initializing specifications for a project that uses **spec-checker**. You will create lean, intent-focused spec files from source code.

## Arguments

$ARGUMENTS — Optional: a source file or directory path (e.g., `src/` or `src/checker.rs`). If omitted, process the entire project.

## Step 1: Generate lean skeletons

Run `spec-checker init <source>` (without `--ai`) to create mechanical skeletons:

```bash
spec-checker init .
```

This produces `.spec.yaml` files with just entity names and kinds — no descriptions, no invariants.

## Step 2: Get processing order

Run `spec-checker toposort ./specs` to get the dependency-ordered list of spec files. Process leaf dependencies first so you can use their specs as context for downstream modules.

If a single file was provided as argument, only process that file's spec.

## Step 3: Enrich each spec

For each spec file (in dependency order):

1. **Read the source file** (from `source_path` in the spec)
2. **Read the lean skeleton spec**
3. **Read already-enriched dependency specs** for context on the module's role
4. **Enrich the spec** with:

### Module description
One sentence describing what this module is FOR (its purpose/role), not what it contains.

### API curation
Review the `exposes` list. Remove internal helpers that happen to be public — keep only entities that form the module's **intended public API contract**. Add a one-line `description` to each retained entity.

### Forbidden dependencies
Add `forbidden_deps` — dependencies this module should NEVER have, based on its architectural role. Think separation of concerns. Only suggest deps that would be a clear violation. Add a YAML comment with the reason:
```yaml
forbidden_deps:
  - clap       # CLI concerns belong in main
  - syn        # AST parsing belongs in extractors
```

### Layer classification
Set `layer` to one of: `infrastructure`, `domain`, `application`, `interface`.
- infrastructure = external I/O, persistence, third-party integrations
- domain = core business logic, data structures
- application = orchestration, coordination
- interface = user-facing CLI, API endpoints

5. **Update source_hash**:
   ```bash
   sha256sum <source-file> | cut -c1-16
   ```
6. **Write the enriched spec** back
7. **Report progress** — tell the user which file you completed

## Step 4: Validate

Run `spec-checker check .` and fix any errors.

## What NOT to do

- Do NOT add `type_constraints` that mirror struct fields or function signatures — these are derivable from code
- Do NOT add `depends_on` or `external_deps` — derivable from source
- Do NOT add `invariants`, `requires`, `ensures`, `modifies` — these are behavioral, handled by `/spec-refine`
- Do NOT restate what the type system already guarantees

## Quality guidelines

- **Intent over structure** — specs describe what the module is FOR, not what it contains
- **Curate aggressively** — 5 well-chosen API entities beat 20 that mirror `pub` items
- **Be specific** — "Orchestrates verification passes against specs" is better than "Main checker module"
- **Use domain language** — match terminology from the code
