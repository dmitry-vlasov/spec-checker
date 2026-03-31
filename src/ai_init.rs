// AI-assisted spec initialization: enriches a lean mechanical skeleton
// with descriptions, API curation, forbidden deps, and layer.

use crate::behavioral::{LlmConfig, LlmProvider};
use crate::extractors::ExtractedModule;
use crate::spec::ModuleSpec;

/// AI-generated enrichments for a module spec.
#[derive(Debug, serde::Deserialize)]
pub struct SpecEnrichment {
    /// One-sentence module description
    pub description: String,
    /// Names of exposed entities that form the intended public API
    /// (subset of the mechanical extraction — drops internal helpers)
    pub api_entities: Vec<ApiEntity>,
    /// Suggested forbidden dependencies with reasons
    pub forbidden_deps: Vec<ForbiddenDep>,
    /// Suggested architectural layer
    pub layer: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct ApiEntity {
    pub name: String,
    pub description: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct ForbiddenDep {
    pub dep: String,
    #[allow(dead_code)]
    pub reason: String,
}

/// Maximum number of retries for rate-limited requests
const MAX_RETRIES: u32 = 5;
const INITIAL_BACKOFF_MS: u64 = 2000;

/// Build a compact summary of a module from extracted metadata,
/// fitting within a character budget. Prioritizes: type definitions,
/// public function signatures, then private signatures. Drops whole
/// entities rather than cutting mid-definition.
pub fn summarize_extracted(extracted: &ExtractedModule, max_chars: usize) -> String {
    // Build blocks in priority order. Each block is a self-contained chunk.
    let mut blocks: Vec<(u8, String)> = Vec::new(); // (priority, content) — lower = more important

    // Priority 0: type definitions (compact, give structural understanding)
    for (name, info) in &extracted.type_definitions {
        let kind = match info.kind {
            crate::types::TypeKind::Struct => "struct",
            crate::types::TypeKind::Enum => "enum",
            crate::types::TypeKind::Trait => "trait",
            crate::types::TypeKind::TypeAlias => "type",
        };
        let mut def = format!("pub {} {}", kind, name);
        if !info.generics.is_empty() {
            let gs: Vec<&str> = info.generics.iter().map(|g| g.name.as_str()).collect();
            def.push_str(&format!("<{}>", gs.join(", ")));
        }
        if !info.fields.is_empty() {
            let fields: Vec<String> = info.fields.iter()
                .map(|f| format!("  {}: {}", f.name, f.type_repr))
                .collect();
            def.push_str(&format!(" {{\n{}\n}}", fields.join(",\n")));
        } else if !info.variants.is_empty() {
            let variants: Vec<String> = info.variants.iter()
                .map(|v| {
                    if v.fields.is_empty() {
                        format!("  {}", v.name)
                    } else {
                        let vf: Vec<String> = v.fields.iter()
                            .map(|f| format!("{}: {}", f.name, f.type_repr))
                            .collect();
                        format!("  {}({})", v.name, vf.join(", "))
                    }
                })
                .collect();
            def.push_str(&format!(" {{\n{}\n}}", variants.join(",\n")));
        }
        blocks.push((0, def));
    }

    // Priority 1: public function signatures
    for (name, sig) in &extracted.function_signatures {
        if extracted.public_functions.contains(name) {
            blocks.push((1, format!("pub {}", sig)));
        }
    }

    // Priority 2: private function signatures
    for (name, sig) in &extracted.function_signatures {
        if !extracted.public_functions.contains(name) {
            blocks.push((2, sig.clone()));
        }
    }

    // Priority 3: imports
    if !extracted.imports.is_empty() {
        let imports: Vec<String> = extracted.imports.iter()
            .map(|imp| format!("//   {}", imp))
            .collect();
        blocks.push((3, format!("// Imports:\n{}", imports.join("\n"))));
    }

    // Priority 4: state variables & events (small, low priority)
    if !extracted.state_variables.is_empty() {
        let vars: Vec<String> = extracted.state_variables.iter()
            .map(|v| format!("//   {}", v))
            .collect();
        blocks.push((4, format!("// State variables:\n{}", vars.join("\n"))));
    }
    if !extracted.events.is_empty() {
        let evts: Vec<String> = extracted.events.iter()
            .map(|e| format!("//   {}", e))
            .collect();
        blocks.push((4, format!("// Events:\n{}", evts.join("\n"))));
    }

    // Sort by priority (stable — preserves insertion order within same priority)
    blocks.sort_by_key(|(prio, _)| *prio);

    // Greedily add blocks until budget is exhausted
    let mut result = Vec::new();
    let mut remaining = max_chars;
    let mut dropped = 0usize;

    for (_prio, block) in &blocks {
        let cost = block.len() + 1; // +1 for newline separator
        if cost <= remaining {
            result.push(block.as_str());
            remaining -= cost;
        } else {
            dropped += 1;
        }
    }

    if dropped > 0 {
        result.push("// ... (some definitions omitted to fit context window)");
    }

    result.join("\n")
}

/// Extract internal module dependencies from source code.
/// Scans for `use crate::module` and `crate::module::` patterns.
/// Returns module names (e.g., ["spec", "behavioral", "extractors"]).
pub fn extract_internal_deps(source_code: &str) -> Vec<String> {
    let mut deps = Vec::new();
    for line in source_code.lines() {
        let trimmed = line.trim();
        // Match: use crate::module_name
        if let Some(rest) = trimmed.strip_prefix("use crate::") {
            if let Some(module) = rest.split(|c: char| c == ':' || c == ';' || c == '{').next() {
                let module = module.trim();
                if !module.is_empty() && !deps.contains(&module.to_string()) {
                    deps.push(module.to_string());
                }
            }
        }
        // Match: crate::module_name:: in code (not just use statements)
        // This catches inline references like crate::types::TypeKind
        for part in trimmed.split("crate::").skip(1) {
            if let Some(module) = part.split(|c: char| c == ':' || c == ';' || c == '(' || c == ')' || c == ',' || c == ' ').next() {
                let module = module.trim();
                if !module.is_empty() && !deps.contains(&module.to_string()) {
                    deps.push(module.to_string());
                }
            }
        }
    }
    deps
}

/// Topologically sort files based on internal dependencies.
/// Returns indices into the input slice, deps-first order.
/// Files without deps come first. Cycles are broken arbitrarily.
pub fn toposort_files(
    files: &[(std::path::PathBuf, String)], // (path, source_code)
) -> Vec<usize> {
    use std::collections::{HashMap, VecDeque};

    // Build module_name -> index mapping from file paths
    let mut name_to_idx: HashMap<String, usize> = HashMap::new();
    for (i, (path, _)) in files.iter().enumerate() {
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            name_to_idx.insert(stem.to_string(), i);
        }
        // Also map by parent/stem for nested modules (e.g., extractors/mod -> extractors)
        if let Some(parent) = path.parent() {
            if let Some(parent_name) = parent.file_name().and_then(|s| s.to_str()) {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if stem == "mod" {
                        name_to_idx.insert(parent_name.to_string(), i);
                    }
                }
            }
        }
    }

    let n = files.len();
    let mut edges: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut in_degree = vec![0usize; n];

    for (i, (_, source)) in files.iter().enumerate() {
        let deps = extract_internal_deps(source);
        for dep in &deps {
            if let Some(&j) = name_to_idx.get(dep) {
                if j != i {
                    edges[j].push(i); // j must come before i
                    in_degree[i] += 1;
                }
            }
        }
    }

    // Kahn's algorithm
    let mut queue: VecDeque<usize> = VecDeque::new();
    for i in 0..n {
        if in_degree[i] == 0 {
            queue.push_back(i);
        }
    }

    let mut order = Vec::with_capacity(n);
    while let Some(i) = queue.pop_front() {
        order.push(i);
        for &j in &edges[i] {
            in_degree[j] -= 1;
            if in_degree[j] == 0 {
                queue.push_back(j);
            }
        }
    }

    // Append any remaining (cycles) in original order
    if order.len() < n {
        for i in 0..n {
            if !order.contains(&i) {
                order.push(i);
            }
        }
    }

    order
}

/// Format already-generated specs as compact context for the LLM prompt.
/// Returns a string like:
///   Dependency `spec`: Core data structures for module specs...
///   - ModuleSpec: Complete specification for a single module
///   - Invariants: Layer dependencies are acyclic; ...
pub fn format_dep_context(dep_specs: &[&SpecEnrichment]) -> String {
    let mut parts = Vec::new();
    for spec in dep_specs {
        let mut lines = vec![format!("- {}", spec.description)];
        // Just entity names and descriptions, very compact
        for ent in &spec.api_entities {
            lines.push(format!("  - {}: {}", ent.name, ent.description));
        }
        parts.push(lines.join("\n"));
    }
    parts.join("\n")
}

fn build_enrichment_prompt(
    module_name: &str,
    code_content: &str,
    entity_names: &[String],
    language: &str,
    provider: &LlmProvider,
    is_summary: bool,
    dep_context: &str,
) -> String {
    let no_think = match provider {
        LlmProvider::OpenAICompatible => " /no_think",
        LlmProvider::Anthropic => "",
    };

    let entities_list = entity_names.join(", ");

    let code_label = if is_summary {
        "## Module signatures and structure (summary):"
    } else {
        "## Source code:"
    };

    let dep_section = if dep_context.is_empty() {
        String::new()
    } else {
        format!("\n## Dependencies (already specified):\n{}\n", dep_context)
    };

    format!(
        r#"You are analyzing a {language} source module to generate a specification.

## Module: {module_name}
## Public entities found: {entities_list}
{dep_section}
{code_label}
```
{code_content}
```

## Instructions:
Analyze this module and produce a JSON object with these fields:

1. "description": One sentence describing what this module is FOR (its purpose/role), not what it contains.

2. "api_entities": Array of objects {{"name": "...", "description": "..."}} — only include entities that form the module's INTENDED public API contract. Exclude internal helpers that happen to be public. Each description should be one short sentence about the entity's role. For types, only include them if they are central to the module's contract.

3. "forbidden_deps": Array of objects {{"dep": "...", "reason": "..."}} — dependencies this module should NEVER have, based on its architectural role. Think about separation of concerns. Only suggest deps that would be a clear architectural violation.

4. "layer": One of "infrastructure", "domain", "application", "interface", or null if unclear. Based on the module's role: infrastructure=external I/O, domain=core business logic, application=orchestration, interface=user-facing.

Respond with ONLY a JSON object, no other text.{no_think}"#
    )
}

/// Call the LLM to enrich a lean spec skeleton with AI-generated content.
/// For local LLMs (OpenAI-compatible), sends a compact signature summary
/// instead of full source code to fit within context limits.
/// `dep_context` is a formatted summary of already-processed dependency specs.
pub async fn ai_enrich_spec(
    spec: &ModuleSpec,
    source_code: &str,
    extracted: &ExtractedModule,
    config: &LlmConfig,
    dep_context: &str,
) -> anyhow::Result<SpecEnrichment> {
    let language = spec.language.as_deref().unwrap_or("unknown");
    let entity_names: Vec<String> = spec.exposes.keys().cloned().collect();

    let (code_content, is_summary) = match config.provider {
        LlmProvider::Anthropic => (source_code.to_string(), false),
        LlmProvider::OpenAICompatible => {
            // Reserve ~2000 tokens for prompt template + response + dep context.
            // Rough estimate: 1 token ≈ 3.5 chars.
            let dep_tokens = dep_context.len() / 3;
            let max_chars = ((config.context_size as usize).saturating_sub(2000 + dep_tokens)) * 3;
            (summarize_extracted(extracted, max_chars), true)
        }
    };

    let prompt = build_enrichment_prompt(
        &spec.module,
        &code_content,
        &entity_names,
        language,
        &config.provider,
        is_summary,
        dep_context,
    );

    let api_key = config.api_key.as_deref().unwrap_or("");
    let client = reqwest::Client::new();
    let url = config.api_url();
    let max_tokens = match config.provider {
        LlmProvider::Anthropic => 2048,
        LlmProvider::OpenAICompatible => 4096,
    };

    let mut last_error = String::new();

    for attempt in 0..=MAX_RETRIES {
        if attempt > 0 {
            let delay = INITIAL_BACKOFF_MS * 2u64.pow(attempt - 1);
            eprintln!(
                "  [retry {}/{}] Rate limited, waiting {}s...",
                attempt, MAX_RETRIES, delay / 1000
            );
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        }

        let resp = match config.provider {
            LlmProvider::Anthropic => {
                let body = serde_json::json!({
                    "model": config.model,
                    "max_tokens": max_tokens,
                    "messages": [{ "role": "user", "content": prompt }]
                });
                client
                    .post(&url)
                    .header("x-api-key", api_key)
                    .header("anthropic-version", "2023-06-01")
                    .header("content-type", "application/json")
                    .json(&body)
                    .send()
                    .await?
            }
            LlmProvider::OpenAICompatible => {
                let body = if config.is_ollama() {
                    serde_json::json!({
                        "model": config.model,
                        "stream": false,
                        "options": { "num_ctx": config.context_size },
                        "messages": [{ "role": "user", "content": prompt }]
                    })
                } else {
                    serde_json::json!({
                        "model": config.model,
                        "max_tokens": max_tokens,
                        "messages": [{ "role": "user", "content": prompt }]
                    })
                };
                let mut req = client
                    .post(&url)
                    .header("content-type", "application/json");
                if !api_key.is_empty() {
                    req = req.header("authorization", format!("Bearer {}", api_key));
                }
                req.json(&body).send().await?
            }
        };

        if resp.status() == 429 {
            last_error = resp.text().await.unwrap_or_default();
            continue;
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("LLM API error ({}): {}", status, text);
        }

        return parse_enrichment_response(resp, &config.provider).await;
    }

    anyhow::bail!(
        "LLM API rate limited after {} retries: {}",
        MAX_RETRIES,
        last_error
    )
}

async fn parse_enrichment_response(
    resp: reqwest::Response,
    provider: &LlmProvider,
) -> anyhow::Result<SpecEnrichment> {
    let api_resp: serde_json::Value = resp.json().await?;

    let text = match provider {
        LlmProvider::Anthropic => api_resp["content"][0]["text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Unexpected Anthropic response format"))?,
        LlmProvider::OpenAICompatible => api_resp["message"]["content"]
            .as_str()
            .or_else(|| api_resp["choices"][0]["message"]["content"].as_str())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Unexpected response format: {}",
                    serde_json::to_string_pretty(&api_resp).unwrap_or_default()
                )
            })?,
    };

    let text = text.trim();
    let text = text.trim_end_matches("/no_think").trim();
    let text = if text.starts_with("```") {
        let inner = text
            .trim_start_matches("```json")
            .trim_start_matches("```");
        inner.trim_end_matches("```").trim()
    } else {
        text
    };

    let enrichment: SpecEnrichment = serde_json::from_str(text).map_err(|e| {
        anyhow::anyhow!(
            "Failed to parse LLM enrichment response: {}. Raw: {}",
            e,
            text
        )
    })?;

    Ok(enrichment)
}

/// Apply AI enrichments to a lean spec skeleton.
pub fn apply_enrichment(spec: &mut ModuleSpec, enrichment: SpecEnrichment) {
    spec.description = Some(enrichment.description);

    // Replace exposes with only the AI-curated API entities
    let mut new_exposes = std::collections::HashMap::new();
    for entity in &enrichment.api_entities {
        if let Some(mut existing) = spec.exposes.remove(&entity.name) {
            existing.description = Some(entity.description.clone());
            new_exposes.insert(entity.name.clone(), existing);
        } else {
            // AI suggested an entity not in the mechanical extraction — skip
        }
    }
    spec.exposes = new_exposes;

    spec.forbidden_deps = enrichment
        .forbidden_deps
        .into_iter()
        .map(|fd| fd.dep)
        .collect();

    if let Some(layer) = enrichment.layer {
        spec.layer = Some(crate::spec::Layer::new(&layer));
    }
}
