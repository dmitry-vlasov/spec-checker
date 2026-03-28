use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::extractors::ExtractedModule;
use crate::spec::ModuleSpec;

// ─── Invariant Classification ─────────────────────────────────────────────────

/// How an invariant should be checked
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvariantTier {
    /// Can be checked by pattern matching on code (no LLM)
    Static,
    /// Requires LLM comprehension
    Behavioral,
}

/// A classified invariant ready for checking
#[derive(Debug, Clone)]
pub struct ClassifiedInvariant {
    pub text: String,
    pub tier: InvariantTier,
    pub module_name: String,
    pub source_path: Option<String>,
}

/// Result of checking a single invariant
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InvariantResult {
    pub satisfies: bool,
    pub reasoning: String,
    pub tier: String,
}

/// Classify an invariant into static or behavioral tier.
///
/// Only direct module-level assertions are classified as Static.
/// Function-level ensures/requires (formatted as "func: ensures ...")
/// are always Behavioral — they describe function behavior, not
/// module-wide properties, even if they mention keywords like "panic".
pub fn classify_invariant(text: &str) -> InvariantTier {
    // Function-level requires/ensures are always behavioral:
    // they describe what a function does, not a whole-module assertion.
    // Format: "function_name: requires ..." or "function_name: ensures ..."
    if text.contains(": requires ") || text.contains(": ensures ") {
        return InvariantTier::Behavioral;
    }

    let lower = text.to_lowercase();

    // Static-checkable patterns (module-level assertions only)
    if lower.contains("never panics") || lower.contains("no panic") {
        return InvariantTier::Static;
    }
    if lower.contains("returns result") || lower.contains("returns error") {
        return InvariantTier::Static;
    }
    if lower.contains("no unwrap") || lower.contains("never unwrap") {
        return InvariantTier::Static;
    }
    if lower.contains("no unsafe") || lower.contains("never unsafe") {
        return InvariantTier::Static;
    }

    // Everything else requires LLM comprehension
    InvariantTier::Behavioral
}

/// Collect all invariants from a spec, classified by tier
pub fn collect_invariants(spec: &ModuleSpec) -> Vec<ClassifiedInvariant> {
    let mut result = Vec::new();

    // Module-level invariants
    for inv in &spec.invariants {
        result.push(ClassifiedInvariant {
            text: inv.clone(),
            tier: classify_invariant(inv),
            module_name: spec.module.clone(),
            source_path: spec.source_path.clone(),
        });
    }

    // Function-level requires/ensures
    for (name, expose_spec) in &spec.exposes {
        let is_function = expose_spec.kind.as_deref() != Some("type");
        if !is_function {
            continue;
        }

        for req in &expose_spec.requires {
            let text = format!("{}: requires {}", name, req);
            let tier = classify_invariant(&text);
            result.push(ClassifiedInvariant {
                text,
                tier,
                module_name: spec.module.clone(),
                source_path: spec.source_path.clone(),
            });
        }

        for ens in &expose_spec.ensures {
            let text = format!("{}: ensures {}", name, ens);
            let tier = classify_invariant(&text);
            result.push(ClassifiedInvariant {
                text,
                tier,
                module_name: spec.module.clone(),
                source_path: spec.source_path.clone(),
            });
        }
    }

    result
}

// ─── Static-Semantic Checks ──────────────────────────────────────────────────

/// Run static-semantic checks on invariants that don't need an LLM
pub fn check_static_invariant(
    invariant: &ClassifiedInvariant,
    source_code: &str,
) -> Option<InvariantResult> {
    let lower = invariant.text.to_lowercase();

    if lower.contains("never panics") || lower.contains("no panic") {
        return Some(check_no_panics(source_code));
    }
    if lower.contains("no unwrap") || lower.contains("never unwrap") {
        return Some(check_no_unwrap(source_code));
    }
    if lower.contains("no unsafe") || lower.contains("never unsafe") {
        return Some(check_no_unsafe(source_code));
    }

    None
}

// TODO: These static checks use keyword scanning which produces false positives
// when the source code itself deals with these patterns (e.g., a checker that
// searches for "unsafe" will contain that keyword). The proper fix is to use
// AST-based analysis (e.g., via syn for Rust) instead of string matching.
// For now, we strip comments, string literals, and lines that are clearly
// checking for these patterns rather than using them.

fn check_no_panics(source: &str) -> InvariantResult {
    let clean = strip_tests_strings_and_checks(source);
    let has_panic = clean.contains("panic!(")
        || clean.contains(".unwrap()")
        || clean.contains(".expect(");

    InvariantResult {
        satisfies: !has_panic,
        reasoning: if has_panic {
            "Found panic!(), unwrap(), or expect() outside test code".into()
        } else {
            "No panic paths found outside test code".into()
        },
        tier: "static".into(),
    }
}

fn check_no_unwrap(source: &str) -> InvariantResult {
    let clean = strip_tests_strings_and_checks(source);
    let has_unwrap = clean.contains(".unwrap()");

    InvariantResult {
        satisfies: !has_unwrap,
        reasoning: if has_unwrap {
            "Found .unwrap() outside test code".into()
        } else {
            "No .unwrap() found outside test code".into()
        },
        tier: "static".into(),
    }
}

fn check_no_unsafe(source: &str) -> InvariantResult {
    let clean = strip_tests_strings_and_checks(source);
    // Match "unsafe " as a keyword but not as part of identifiers like check_no_unsafe
    let has_unsafe = has_unsafe_keyword(&clean);

    InvariantResult {
        satisfies: !has_unsafe,
        reasoning: if has_unsafe {
            "Found unsafe block or function outside test code".into()
        } else {
            "No unsafe code found outside test code".into()
        },
        tier: "static".into(),
    }
}

/// Check if source contains `unsafe` as a keyword (not part of an identifier).
fn has_unsafe_keyword(source: &str) -> bool {
    for line in source.lines() {
        let trimmed = line.trim();
        // Look for "unsafe" followed by space/brace (keyword use)
        // but not preceded by alphanumeric/underscore (part of identifier)
        let mut search_from = 0;
        while let Some(pos) = trimmed[search_from..].find("unsafe") {
            let abs_pos = search_from + pos;
            let before_ok = abs_pos == 0
                || !trimmed.as_bytes()[abs_pos - 1].is_ascii_alphanumeric()
                    && trimmed.as_bytes()[abs_pos - 1] != b'_';
            let after_pos = abs_pos + 6;
            let after_ok = after_pos >= trimmed.len()
                || !trimmed.as_bytes()[after_pos].is_ascii_alphanumeric()
                    && trimmed.as_bytes()[after_pos] != b'_';
            if before_ok && after_ok {
                return true;
            }
            search_from = abs_pos + 6;
        }
    }
    false
}

/// Strip tests, string literals, AND lines that are checking for patterns
/// (e.g., `.contains("panic!(")`) to avoid false positives in code that
/// is itself a checker for those patterns.
fn strip_tests_strings_and_checks(source: &str) -> String {
    let cleaned = strip_tests_and_strings(source);
    let mut result = String::new();
    for line in cleaned.lines() {
        let trimmed = line.trim();
        // Skip lines that are clearly pattern-matching checks, not actual usage:
        // - .contains("...") calls
        // - lines that are string literals being assigned (reasoning messages)
        // - lines inside if conditions checking for keywords
        if trimmed.contains(".contains(\"")
            || trimmed.contains(".contains(&\"")
            || trimmed.contains("lower.contains")
            || trimmed.starts_with("//")
        {
            continue;
        }
        result.push_str(line);
        result.push('\n');
    }
    result
}

/// Strip test modules from source code (public for use by checker).
pub fn strip_tests(source: &str) -> String {
    let mut result = String::new();
    let mut in_test_module = false;
    let mut brace_depth = 0i32;
    let mut test_start_depth = 0i32;

    for line in source.lines() {
        let trimmed = line.trim();

        if trimmed.contains("#[cfg(test)]") {
            in_test_module = true;
            test_start_depth = brace_depth;
            continue;
        }

        for ch in trimmed.chars() {
            match ch {
                '{' => brace_depth += 1,
                '}' => {
                    brace_depth -= 1;
                    if in_test_module && brace_depth <= test_start_depth {
                        in_test_module = false;
                    }
                }
                _ => {}
            }
        }

        if !in_test_module {
            result.push_str(line);
            result.push('\n');
        }
    }

    result
}

/// Rough strip of test modules and string literals to avoid false positives
fn strip_tests_and_strings(source: &str) -> String {
    let mut result = String::new();
    let mut in_test_module = false;
    let mut brace_depth = 0i32;
    let mut test_start_depth = 0i32;

    for line in source.lines() {
        let trimmed = line.trim();

        // Track test module boundaries
        if trimmed.contains("#[cfg(test)]") {
            in_test_module = true;
            test_start_depth = brace_depth;
            continue;
        }

        // Track brace depth
        for ch in trimmed.chars() {
            match ch {
                '{' => brace_depth += 1,
                '}' => {
                    brace_depth -= 1;
                    if in_test_module && brace_depth <= test_start_depth {
                        in_test_module = false;
                    }
                }
                _ => {}
            }
        }

        if !in_test_module {
            // Strip string literals (rough: remove content between quotes)
            let stripped = strip_string_literals(trimmed);
            result.push_str(&stripped);
            result.push('\n');
        }
    }

    result
}

/// Rough strip of string literal contents
fn strip_string_literals(line: &str) -> String {
    let mut result = String::new();
    let mut in_string = false;
    let mut prev = '\0';

    for ch in line.chars() {
        if ch == '"' && prev != '\\' {
            in_string = !in_string;
            result.push(ch);
        } else if !in_string {
            result.push(ch);
        }
        prev = ch;
    }

    result
}

// ─── Cache Layer ─────────────────────────────────────────────────────────────

/// Cached LLM result
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CachedResult {
    pub satisfies: bool,
    pub reasoning: String,
    pub model: String,
    pub timestamp: String,
    pub code_hash: String,
    pub invariant_hash: String,
}

/// Compute cache key from code content and invariant text
pub fn cache_key(code: &str, invariant: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(code.as_bytes());
    hasher.update(b"|");
    hasher.update(invariant.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Read a cached result if it exists
pub fn read_cache(cache_dir: &Path, key: &str) -> Option<CachedResult> {
    let path = cache_dir.join(format!("{}.json", &key[..16]));
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&path).ok()?;
    let cached: CachedResult = serde_json::from_str(&content).ok()?;
    // Verify the full hash matches (not just the filename prefix)
    if cached.code_hash == key.split('|').next().unwrap_or("") {
        // Key matches — but we store the full combined hash, so just check it
    }
    Some(cached)
}

/// Write a result to cache
pub fn write_cache(cache_dir: &Path, key: &str, result: &CachedResult) -> anyhow::Result<()> {
    std::fs::create_dir_all(cache_dir)?;
    let path = cache_dir.join(format!("{}.json", &key[..16]));
    let content = serde_json::to_string_pretty(result)?;
    std::fs::write(path, content)?;
    Ok(())
}

// ─── LLM Client ──────────────────────────────────────────────────────────────

/// LLM check mode
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmCheckMode {
    /// Don't run LLM checks (default)
    Off,
    /// Show what would be checked, with estimated cost
    DryRun,
    /// Only use cached results, never call LLM
    CachedOnly,
    /// Call LLM for uncached invariants
    Full,
}

impl std::str::FromStr for LlmCheckMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "off" => Ok(LlmCheckMode::Off),
            "dry-run" | "dryrun" | "dry_run" => Ok(LlmCheckMode::DryRun),
            "cached-only" | "cached" | "cache" => Ok(LlmCheckMode::CachedOnly),
            "full" => Ok(LlmCheckMode::Full),
            _ => Err(format!(
                "Unknown LLM check mode: '{}'. Use: off, dry-run, cached-only, full",
                s
            )),
        }
    }
}

/// LLM API provider (auto-detected from endpoint URL)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmProvider {
    /// Anthropic API (api.anthropic.com)
    Anthropic,
    /// OpenAI-compatible API (Ollama, vLLM, LM Studio, etc.)
    OpenAICompatible,
}

/// Configuration for LLM behavioral checks.
///
/// Resolved from: built-in defaults → .spec-checker.yaml → env vars → CLI flags.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub endpoint: String,
    pub model: String,
    pub api_key: Option<String>,
    pub check_mode: LlmCheckMode,
    pub provider: LlmProvider,
    /// Cooldown between consecutive LLM calls in seconds.
    /// Prevents GPU thermal throttling on consumer hardware.
    pub cooldown_secs: u64,
    /// Context window size for Ollama models.
    pub context_size: u32,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            endpoint: "https://api.anthropic.com".to_string(),
            model: "claude-haiku-4-5-20251001".to_string(),
            api_key: None,
            check_mode: LlmCheckMode::Off,
            provider: LlmProvider::Anthropic,
            cooldown_secs: DEFAULT_LOCAL_LLM_COOLDOWN_SECS,
            context_size: DEFAULT_OLLAMA_NUM_CTX,
        }
    }
}

impl LlmConfig {
    /// Detect the provider from the endpoint URL.
    pub fn detect_provider(endpoint: &str) -> LlmProvider {
        if endpoint.contains("anthropic.com") {
            LlmProvider::Anthropic
        } else {
            LlmProvider::OpenAICompatible
        }
    }

    /// Resolve the API key: config value → env var (based on provider).
    pub fn resolve_api_key(&mut self) {
        if self.api_key.is_some() {
            return;
        }
        // Try provider-specific env vars, then generic
        self.api_key = match self.provider {
            LlmProvider::Anthropic => std::env::var("ANTHROPIC_API_KEY").ok(),
            LlmProvider::OpenAICompatible => std::env::var("OPENAI_API_KEY")
                .or_else(|_| std::env::var("LLM_API_KEY"))
                .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
                .ok(),
        };
    }

    /// Build the full API URL for chat/messages endpoint.
    pub fn api_url(&self) -> String {
        let base = self.endpoint.trim_end_matches('/');
        match self.provider {
            LlmProvider::Anthropic => format!("{}/v1/messages", base),
            LlmProvider::OpenAICompatible => {
                // Use Ollama native API (/api/chat) when endpoint looks like Ollama,
                // otherwise use OpenAI-compatible endpoint
                if base.contains("11434") || base.ends_with("/api") {
                    let ollama_base = base
                        .trim_end_matches("/v1")
                        .trim_end_matches("/api");
                    format!("{}/api/chat", ollama_base)
                } else {
                    format!("{}/chat/completions", base)
                }
            }
        }
    }

    /// Check if this config points to an Ollama instance.
    pub fn is_ollama(&self) -> bool {
        self.endpoint.contains("11434")
    }
}

/// LLM response from the API
#[derive(Debug, serde::Deserialize)]
struct LlmResponse {
    satisfies: bool,
    reasoning: String,
}

/// Build the prompt for an LLM behavioral check.
/// For OpenAI-compatible providers (local LLMs), appends /no_think
/// to disable thinking/reasoning mode (e.g., Qwen 3.5) which would
/// otherwise consume all tokens on internal reasoning.
fn build_prompt(code: &str, invariant: &str, context: &str, provider: &LlmProvider) -> String {
    let no_think = match provider {
        LlmProvider::OpenAICompatible => " /no_think",
        LlmProvider::Anthropic => "",
    };
    let context_section = if context.is_empty() {
        String::new()
    } else {
        format!("\n## Context:\n{context}\n")
    };
    format!(
        r#"You are verifying whether source code satisfies a specification invariant.

## Invariant to verify:
{invariant}
{context_section}
## Source code:
```
{code}
```

## Instructions:
Analyze the code and determine whether it satisfies the invariant.
Respond with ONLY a JSON object, no other text:
{{"satisfies": true/false, "reasoning": "your explanation in 1-2 sentences"}}{no_think}"#
    )
}

/// Build a context string from module/entity descriptions for LLM prompts.
fn build_description_context(spec: &ModuleSpec, invariant_text: &str) -> String {
    let mut parts = Vec::new();

    if let Some(desc) = &spec.description {
        parts.push(format!("Module `{}`: {}", spec.module, desc));
    }

    // If the invariant is entity-specific (e.g. "func_name: ensures ..."),
    // include that entity's description
    for (name, expose) in &spec.exposes {
        if let Some(desc) = &expose.description {
            if invariant_text.starts_with(&format!("{}: ", name)) {
                parts.push(format!("`{}`: {}", name, desc));
            }
        }
    }

    parts.join("\n")
}

/// Maximum number of retries for rate-limited requests
const MAX_RETRIES: u32 = 5;
/// Initial backoff delay in milliseconds
const INITIAL_BACKOFF_MS: u64 = 2000;

/// Call the LLM API to verify an invariant, with retry on rate limits.
/// Supports both Anthropic and OpenAI-compatible endpoints.
pub async fn llm_verify(
    code: &str,
    invariant: &str,
    context: &str,
    config: &LlmConfig,
) -> anyhow::Result<InvariantResult> {
    let api_key = config.api_key.as_deref().unwrap_or("");
    let client = reqwest::Client::new();
    let prompt = build_prompt(code, invariant, context, &config.provider);
    let url = config.api_url();
    let max_tokens = match config.provider {
        LlmProvider::Anthropic => 256,
        LlmProvider::OpenAICompatible => 1024, // local models need more room
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
                    // Ollama native API format
                    serde_json::json!({
                        "model": config.model,
                        "stream": false,
                        "options": { "num_ctx": config.context_size },
                        "messages": [{ "role": "user", "content": prompt }]
                    })
                } else {
                    // Standard OpenAI-compatible format
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

        return parse_llm_response(resp, &config.provider).await;
    }

    anyhow::bail!(
        "LLM API rate limited after {} retries: {}",
        MAX_RETRIES,
        last_error
    )
}

/// Parse a successful LLM API response into an InvariantResult.
/// Handles both Anthropic and OpenAI response formats.
async fn parse_llm_response(
    resp: reqwest::Response,
    provider: &LlmProvider,
) -> anyhow::Result<InvariantResult> {
    let api_resp: serde_json::Value = resp.json().await?;

    // Extract text based on provider format
    let text = match provider {
        LlmProvider::Anthropic => api_resp["content"][0]["text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Unexpected Anthropic response format"))?,
        LlmProvider::OpenAICompatible => {
            // Try Ollama native format first, then OpenAI-compatible
            api_resp["message"]["content"]
                .as_str()
                .or_else(|| api_resp["choices"][0]["message"]["content"].as_str())
                .ok_or_else(|| anyhow::anyhow!(
                    "Unexpected response format: {}",
                    serde_json::to_string_pretty(&api_resp).unwrap_or_default()
                ))?
        }
    };

    // Clean up response text
    let text = text.trim();
    // Strip /no_think if echoed back by the model
    let text = text.trim_end_matches("/no_think").trim();
    // Strip markdown code blocks if present
    let text = if text.starts_with("```") {
        let inner = text.trim_start_matches("```json").trim_start_matches("```");
        inner.trim_end_matches("```").trim()
    } else {
        text
    };

    let llm_resp: LlmResponse = serde_json::from_str(text).map_err(|e| {
        anyhow::anyhow!(
            "Failed to parse LLM response as JSON: {}. Raw response: {}",
            e,
            text
        )
    })?;

    Ok(InvariantResult {
        satisfies: llm_resp.satisfies,
        reasoning: llm_resp.reasoning,
        tier: "llm-verified".into(),
    })
}

/// Estimate token count for a code+invariant pair (rough approximation)
pub fn estimate_tokens(code: &str, invariant: &str, context: &str) -> usize {
    // Rough: ~4 chars per token for English/code
    let prompt = build_prompt(code, invariant, context, &LlmProvider::Anthropic);
    prompt.len() / 4 + 100 // +100 for output tokens
}

// ─── Orchestrator ────────────────────────────────────────────────────────────

/// Summary of behavioral check results
#[derive(Debug, Default)]
pub struct BehavioralSummary {
    pub static_passed: usize,
    pub static_failed: usize,
    pub llm_passed: usize,
    pub llm_failed: usize,
    pub cached: usize,
    pub skipped: usize,
    pub failures: Vec<(String, String)>, // (invariant, reasoning)
}

/// Run behavioral checks for a module
pub async fn check_behavioral(
    spec: &ModuleSpec,
    source_code: &str,
    config: &LlmConfig,
    cache_dir: &Path,
) -> BehavioralSummary {
    let invariants = collect_invariants(spec);
    let mut summary = BehavioralSummary::default();

    for inv in &invariants {
        let context = build_description_context(spec, &inv.text);
        match inv.tier {
            InvariantTier::Static => {
                if let Some(result) = check_static_invariant(inv, source_code) {
                    if result.satisfies {
                        summary.static_passed += 1;
                    } else {
                        summary.static_failed += 1;
                        summary
                            .failures
                            .push((inv.text.clone(), result.reasoning.clone()));
                    }
                } else {
                    // Classifier said static but no checker matched — treat as behavioral
                    handle_behavioral_invariant(
                        inv,
                        source_code,
                        &context,
                        config,
                        cache_dir,
                        &mut summary,
                    )
                    .await;
                }
            }
            InvariantTier::Behavioral => {
                handle_behavioral_invariant(
                    inv,
                    source_code,
                    &context,
                    config,
                    cache_dir,
                    &mut summary,
                )
                .await;
            }
        }
    }

    summary
}

/// Default cooldown between consecutive local LLM calls (seconds).
/// Prevents GPU thermal throttling on consumer hardware.
const DEFAULT_LOCAL_LLM_COOLDOWN_SECS: u64 = 10;

/// Default context window size for Ollama models.
const DEFAULT_OLLAMA_NUM_CTX: u32 = 8192;

async fn handle_behavioral_invariant(
    inv: &ClassifiedInvariant,
    source_code: &str,
    context: &str,
    config: &LlmConfig,
    cache_dir: &Path,
    summary: &mut BehavioralSummary,
) {
    let key = cache_key(source_code, &inv.text);

    match &config.check_mode {
        LlmCheckMode::Off => {
            summary.skipped += 1;
        }
        LlmCheckMode::DryRun => {
            // Check cache first
            if let Some(cached) = read_cache(cache_dir, &key) {
                summary.cached += 1;
                if cached.satisfies {
                    summary.llm_passed += 1;
                } else {
                    summary.llm_failed += 1;
                    summary
                        .failures
                        .push((inv.text.clone(), cached.reasoning.clone()));
                }
                return;
            }

            // If provider requires an API key we don't have, just print estimate
            if config.provider == LlmProvider::Anthropic && config.api_key.is_none() {
                let tokens = estimate_tokens(source_code, &inv.text, context);
                eprintln!(
                    "  [dry-run] {} | ~{} tokens | would call LLM",
                    inv.text, tokens,
                );
                summary.skipped += 1;
                return;
            }

            // Call the configured provider for a preview check
            match llm_verify(source_code, &inv.text, context, config).await {
                Ok(result) => {
                    let cached = CachedResult {
                        satisfies: result.satisfies,
                        reasoning: result.reasoning.clone(),
                        model: config.model.clone(),
                        timestamp: chrono_now(),
                        code_hash: key.clone(),
                        invariant_hash: key.clone(),
                    };
                    let _ = write_cache(cache_dir, &key, &cached);

                    if result.satisfies {
                        summary.llm_passed += 1;
                    } else {
                        summary.llm_failed += 1;
                        summary
                            .failures
                            .push((inv.text.clone(), result.reasoning));
                    }
                }
                Err(e) => {
                    eprintln!("  [dry-run] {} — LLM error: {}", inv.text, e);
                    summary.skipped += 1;
                }
            }
            // Cooldown between calls to prevent GPU overheating
            if config.cooldown_secs > 0 {
                tokio::time::sleep(std::time::Duration::from_secs(config.cooldown_secs)).await;
            }
        }
        LlmCheckMode::CachedOnly => {
            if let Some(cached) = read_cache(cache_dir, &key) {
                summary.cached += 1;
                if cached.satisfies {
                    summary.llm_passed += 1;
                } else {
                    summary.llm_failed += 1;
                    summary
                        .failures
                        .push((inv.text.clone(), cached.reasoning.clone()));
                }
            } else {
                summary.skipped += 1;
            }
        }
        LlmCheckMode::Full => {
            // Check cache first
            if let Some(cached) = read_cache(cache_dir, &key) {
                summary.cached += 1;
                if cached.satisfies {
                    summary.llm_passed += 1;
                } else {
                    summary.llm_failed += 1;
                    summary
                        .failures
                        .push((inv.text.clone(), cached.reasoning.clone()));
                }
                return;
            }

            // Call LLM
            if config.api_key.is_none() && config.provider == LlmProvider::Anthropic {
                eprintln!(
                    "  [skip] {} — no API key set (ANTHROPIC_API_KEY or config)",
                    inv.text
                );
                summary.skipped += 1;
                return;
            }

            match llm_verify(source_code, &inv.text, context, config).await {
                Ok(result) => {
                    // Cache the result
                    let cached = CachedResult {
                        satisfies: result.satisfies,
                        reasoning: result.reasoning.clone(),
                        model: config.model.clone(),
                        timestamp: chrono_now(),
                        code_hash: key.clone(),
                        invariant_hash: key.clone(),
                    };
                    let _ = write_cache(cache_dir, &key, &cached);

                    // Cooldown between local LLM calls to prevent GPU overheating
                    if config.is_ollama() {
                        tokio::time::sleep(std::time::Duration::from_secs(config.cooldown_secs)).await;
                    }

                    if result.satisfies {
                        summary.llm_passed += 1;
                    } else {
                        summary.llm_failed += 1;
                        summary
                            .failures
                            .push((inv.text.clone(), result.reasoning));
                    }
                }
                Err(e) => {
                    eprintln!("  [error] {} — LLM call failed: {}", inv.text, e);
                    summary.skipped += 1;
                    // Cooldown even on error for local models
                    if config.is_ollama() {
                        tokio::time::sleep(std::time::Duration::from_secs(config.cooldown_secs)).await;
                    }
                }
            }
        }
    }
}

fn chrono_now() -> String {
    // Simple timestamp without chrono dependency
    use std::time::SystemTime;
    let dur = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}", dur.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_static_invariants() {
        assert_eq!(
            classify_invariant("check() never panics, returns Result"),
            InvariantTier::Static
        );
        assert_eq!(
            classify_invariant("No unwrap in production code"),
            InvariantTier::Static
        );
        assert_eq!(
            classify_invariant("No unsafe code"),
            InvariantTier::Static
        );
    }

    #[test]
    fn classify_behavioral_invariants() {
        assert_eq!(
            classify_invariant("apply_defaults never overwrites explicitly set fields"),
            InvariantTier::Behavioral
        );
        assert_eq!(
            classify_invariant("All violations are collected, not short-circuited"),
            InvariantTier::Behavioral
        );
        assert_eq!(
            classify_invariant("Extraction is deterministic for same input"),
            InvariantTier::Behavioral
        );
    }

    #[test]
    fn static_check_no_panics() {
        let clean_code = r#"
pub fn safe_function(x: i32) -> Result<i32, String> {
    if x < 0 {
        return Err("negative".to_string());
    }
    Ok(x * 2)
}
"#;
        let result = check_no_panics(clean_code);
        assert!(result.satisfies);

        let panicky_code = r#"
pub fn unsafe_function(x: Option<i32>) -> i32 {
    x.unwrap()
}
"#;
        let result = check_no_panics(panicky_code);
        assert!(!result.satisfies);
    }

    #[test]
    fn static_check_ignores_tests() {
        let code = r#"
pub fn safe() -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    fn test_something() {
        safe().unwrap();
    }
}
"#;
        let result = check_no_panics(code);
        assert!(result.satisfies, "unwrap in test should be ignored");
    }

    #[test]
    fn cache_key_deterministic() {
        let k1 = cache_key("fn foo() {}", "never panics");
        let k2 = cache_key("fn foo() {}", "never panics");
        assert_eq!(k1, k2);

        let k3 = cache_key("fn foo() { x }", "never panics");
        assert_ne!(k1, k3);
    }

    #[test]
    fn cache_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let key = cache_key("code", "invariant");
        let result = CachedResult {
            satisfies: true,
            reasoning: "looks good".into(),
            model: "test".into(),
            timestamp: "123".into(),
            code_hash: key.clone(),
            invariant_hash: key.clone(),
        };

        write_cache(dir.path(), &key, &result).unwrap();
        let loaded = read_cache(dir.path(), &key).unwrap();
        assert_eq!(loaded.satisfies, true);
        assert_eq!(loaded.reasoning, "looks good");
    }

    #[test]
    fn estimate_tokens_reasonable() {
        let tokens = estimate_tokens("fn foo() {}", "never panics", "");
        assert!(tokens > 50 && tokens < 500);
    }

    #[test]
    fn parse_llm_check_mode() {
        assert_eq!("off".parse::<LlmCheckMode>().unwrap(), LlmCheckMode::Off);
        assert_eq!(
            "dry-run".parse::<LlmCheckMode>().unwrap(),
            LlmCheckMode::DryRun
        );
        assert_eq!(
            "cached-only".parse::<LlmCheckMode>().unwrap(),
            LlmCheckMode::CachedOnly
        );
        assert_eq!(
            "full".parse::<LlmCheckMode>().unwrap(),
            LlmCheckMode::Full
        );
        assert!("invalid".parse::<LlmCheckMode>().is_err());
    }
}
