// AI-assisted spec initialization: enriches a lean mechanical skeleton
// with descriptions, API curation, forbidden deps, invariants, and layer.

use crate::behavioral::{LlmConfig, LlmProvider};
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
    /// Non-obvious invariants a developer should know
    pub invariants: Vec<String>,
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

fn build_enrichment_prompt(
    module_name: &str,
    source_code: &str,
    entity_names: &[String],
    language: &str,
    provider: &LlmProvider,
) -> String {
    let no_think = match provider {
        LlmProvider::OpenAICompatible => " /no_think",
        LlmProvider::Anthropic => "",
    };

    let entities_list = entity_names.join(", ");

    format!(
        r#"You are analyzing a {language} source module to generate a specification.

## Module: {module_name}
## Public entities found: {entities_list}

## Source code:
```
{source_code}
```

## Instructions:
Analyze this module and produce a JSON object with these fields:

1. "description": One sentence describing what this module is FOR (its purpose/role), not what it contains.

2. "api_entities": Array of objects {{"name": "...", "description": "..."}} — only include entities that form the module's INTENDED public API contract. Exclude internal helpers that happen to be public. Each description should be one short sentence about the entity's role. For types, only include them if they are central to the module's contract.

3. "forbidden_deps": Array of objects {{"dep": "...", "reason": "..."}} — dependencies this module should NEVER have, based on its architectural role. Think about separation of concerns. Only suggest deps that would be a clear architectural violation.

4. "invariants": Array of strings — non-obvious properties that a developer modifying this code should know. Do NOT state things obvious from the type system or function signatures. Focus on: error handling contracts, performance assumptions, ordering requirements, safety properties, or design decisions that aren't self-evident.

5. "layer": One of "infrastructure", "domain", "application", "interface", or null if unclear. Based on the module's role: infrastructure=external I/O, domain=core business logic, application=orchestration, interface=user-facing.

Respond with ONLY a JSON object, no other text.{no_think}"#
    )
}

/// Call the LLM to enrich a lean spec skeleton with AI-generated content.
pub async fn ai_enrich_spec(
    spec: &ModuleSpec,
    source_code: &str,
    config: &LlmConfig,
) -> anyhow::Result<SpecEnrichment> {
    let language = spec.language.as_deref().unwrap_or("unknown");
    let entity_names: Vec<String> = spec.exposes.keys().cloned().collect();
    let prompt = build_enrichment_prompt(
        &spec.module,
        source_code,
        &entity_names,
        language,
        &config.provider,
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

    spec.invariants = enrichment.invariants;

    if let Some(layer) = enrichment.layer {
        spec.layer = Some(crate::spec::Layer::new(&layer));
    }
}
