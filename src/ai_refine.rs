// AI-assisted spec refinement: adds behavioral contracts
// (requires, ensures, modifies, invariants) to existing specs.

use crate::behavioral::{LlmConfig, LlmProvider};
use crate::ai_init::summarize_extracted;
use crate::extractors::ExtractedModule;
use crate::spec::ModuleSpec;

/// AI-generated behavioral refinements for a module spec.
#[derive(Debug, serde::Deserialize)]
pub struct SpecRefinement {
    /// Per-entity behavioral contracts
    pub entities: Vec<EntityRefinement>,
    /// Module-level invariants
    pub invariants: Vec<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct EntityRefinement {
    pub name: String,
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub ensures: Vec<String>,
    #[serde(default)]
    pub modifies: Vec<String>,
}

const MAX_RETRIES: u32 = 5;
const INITIAL_BACKOFF_MS: u64 = 2000;

/// Format existing spec as compact YAML-like context for the prompt.
fn format_existing_spec(spec: &ModuleSpec) -> String {
    let mut lines = Vec::new();
    lines.push(format!("module: {}", spec.module));
    if let Some(desc) = &spec.description {
        lines.push(format!("description: \"{}\"", desc));
    }
    if let Some(layer) = &spec.layer {
        lines.push(format!("layer: {}", layer.0));
    }
    lines.push("exposes:".to_string());
    for (name, expose) in &spec.exposes {
        let kind = expose.kind.as_deref().unwrap_or("unknown");
        let desc = expose.description.as_deref().unwrap_or("");
        lines.push(format!("  {} ({}): {}", name, kind, desc));
    }
    if !spec.invariants.is_empty() {
        lines.push("invariants:".to_string());
        for inv in &spec.invariants {
            lines.push(format!("  - {}", inv));
        }
    }
    lines.join("\n")
}

/// Format already-refined dependency specs as context.
pub fn format_dep_context(dep_specs: &[&ModuleSpec]) -> String {
    let mut parts = Vec::new();
    for spec in dep_specs {
        let mut lines = Vec::new();
        let desc = spec.description.as_deref().unwrap_or(&spec.module);
        lines.push(format!("- {}: {}", spec.module, desc));
        for (name, expose) in &spec.exposes {
            if expose.kind.as_deref() == Some("function") {
                let mut contract = Vec::new();
                if !expose.requires.is_empty() {
                    contract.push(format!("requires: {}", expose.requires.join(", ")));
                }
                if !expose.ensures.is_empty() {
                    contract.push(format!("ensures: {}", expose.ensures.join(", ")));
                }
                if contract.is_empty() {
                    if let Some(d) = &expose.description {
                        lines.push(format!("  - {}: {}", name, d));
                    }
                } else {
                    lines.push(format!("  - {}: {}", name, contract.join("; ")));
                }
            }
        }
        parts.push(lines.join("\n"));
    }
    parts.join("\n")
}

fn build_refine_prompt(
    spec: &ModuleSpec,
    source_code: &str,
    dep_context: &str,
    provider: &LlmProvider,
    is_summary: bool,
) -> String {
    let no_think = match provider {
        LlmProvider::OpenAICompatible => " /no_think",
        LlmProvider::Anthropic => "",
    };

    let existing_spec = format_existing_spec(spec);

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

    let entity_names: Vec<&str> = spec.exposes.iter()
        .filter(|(_, e)| e.kind.as_deref() == Some("function"))
        .map(|(name, _)| name.as_str())
        .collect();
    let functions_list = entity_names.join(", ");

    format!(
        r#"You are adding behavioral contracts to an existing specification.

## Existing spec:
{existing_spec}

## Functions to specify: {functions_list}
{dep_section}
{code_label}
```
{source_code}
```

## Instructions:
Read the source code and produce a JSON object with behavioral contracts:

1. "entities": Array of objects for each function that has meaningful contracts:
   {{
     "name": "function_name",
     "requires": ["precondition 1", ...],
     "ensures": ["postcondition 1", ...],
     "modifies": ["state 1", ...]
   }}

   Guidelines for contracts:
   - "requires": Preconditions callers must satisfy. Examples: "amount > 0", "connection is open", "input is not empty". Skip if the function accepts any valid input.
   - "ensures": Postconditions guaranteed on return. Examples: "returns sorted list", "emits DepositEvent", "file handle is closed on error". Focus on what's NOT obvious from the return type.
   - "modifies": State the function changes. Examples: "self.balance", "global registry". Skip for pure functions.

2. "invariants": Array of strings — module-level properties that always hold. Focus on:
   - Error handling: "all public functions return Result, never panic"
   - Safety: "no unsafe code", "thread-safe"
   - Ordering: "events emitted after state update"
   - Design decisions: "cache keys are deterministic (SHA256)"

Do NOT:
- Restate type signatures ("takes a u64 parameter")
- State the obvious ("returns a Result" when the type says so)
- Write vague specs ("handles errors correctly")

Respond with ONLY a JSON object, no other text.{no_think}"#
    )
}

/// Call the LLM to generate behavioral refinements for a spec.
pub async fn ai_refine_spec(
    spec: &ModuleSpec,
    source_code: &str,
    extracted: &ExtractedModule,
    config: &LlmConfig,
    dep_context: &str,
) -> anyhow::Result<SpecRefinement> {
    let (code_content, is_summary) = match config.provider {
        LlmProvider::Anthropic => (source_code.to_string(), false),
        LlmProvider::OpenAICompatible => {
            let dep_tokens = dep_context.len() / 3;
            let max_chars = ((config.context_size as usize).saturating_sub(2500 + dep_tokens)) * 3;
            (summarize_extracted(extracted, max_chars), true)
        }
    };

    let prompt = build_refine_prompt(spec, &code_content, dep_context, &config.provider, is_summary);

    let api_key = config.api_key.as_deref().unwrap_or("");
    let client = reqwest::Client::new();
    let url = config.api_url();
    let max_tokens = match config.provider {
        LlmProvider::Anthropic => 4096,
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

        return parse_refine_response(resp, &config.provider).await;
    }

    anyhow::bail!(
        "LLM API rate limited after {} retries: {}",
        MAX_RETRIES,
        last_error
    )
}

async fn parse_refine_response(
    resp: reqwest::Response,
    provider: &LlmProvider,
) -> anyhow::Result<SpecRefinement> {
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
        let inner = text.trim_start_matches("```json").trim_start_matches("```");
        inner.trim_end_matches("```").trim()
    } else {
        text
    };

    let refinement: SpecRefinement = serde_json::from_str(text).map_err(|e| {
        anyhow::anyhow!(
            "Failed to parse LLM refinement response: {}. Raw: {}",
            e,
            text
        )
    })?;

    Ok(refinement)
}

/// Apply refinements to a spec. Never removes existing fields, only adds.
pub fn apply_refinement(spec: &mut ModuleSpec, refinement: SpecRefinement) {
    for entity in refinement.entities {
        if let Some(expose) = spec.exposes.get_mut(&entity.name) {
            for req in entity.requires {
                if !expose.requires.contains(&req) {
                    expose.requires.push(req);
                }
            }
            for ens in entity.ensures {
                if !expose.ensures.contains(&ens) {
                    expose.ensures.push(ens);
                }
            }
            for modif in entity.modifies {
                if !expose.modifies.contains(&modif) {
                    expose.modifies.push(modif);
                }
            }
        }
    }

    for inv in refinement.invariants {
        if !spec.invariants.contains(&inv) {
            spec.invariants.push(inv);
        }
    }
}
