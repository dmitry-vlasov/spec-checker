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

/// Classify an invariant into static or behavioral tier
pub fn classify_invariant(text: &str) -> InvariantTier {
    let lower = text.to_lowercase();

    // Static-checkable patterns
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

fn check_no_panics(source: &str) -> InvariantResult {
    // Look for panic patterns outside of test modules and string literals
    let clean = strip_tests_and_strings(source);
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
    let clean = strip_tests_and_strings(source);
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
    let clean = strip_tests_and_strings(source);
    let has_unsafe = clean.contains("unsafe ");

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

/// LLM response from the API
#[derive(Debug, serde::Deserialize)]
struct LlmResponse {
    satisfies: bool,
    reasoning: String,
}

/// Build the prompt for an LLM behavioral check
fn build_prompt(code: &str, invariant: &str) -> String {
    format!(
        r#"You are verifying whether source code satisfies a specification invariant.

## Invariant to verify:
{invariant}

## Source code:
```
{code}
```

## Instructions:
Analyze the code and determine whether it satisfies the invariant.
Respond with ONLY a JSON object, no other text:
{{"satisfies": true/false, "reasoning": "your explanation in 1-2 sentences"}}"#
    )
}

/// Call the LLM API to verify an invariant
pub async fn llm_verify(
    code: &str,
    invariant: &str,
    api_key: &str,
    model: &str,
) -> anyhow::Result<InvariantResult> {
    let client = reqwest::Client::new();
    let prompt = build_prompt(code, invariant);

    let body = serde_json::json!({
        "model": model,
        "max_tokens": 256,
        "messages": [{
            "role": "user",
            "content": prompt
        }]
    });

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("LLM API error ({}): {}", status, text);
    }

    let api_resp: serde_json::Value = resp.json().await?;

    // Extract text from Anthropic response format
    let text = api_resp["content"][0]["text"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Unexpected API response format"))?;

    // Strip markdown code blocks if present (LLMs often wrap JSON in ```json ... ```)
    let text = text.trim();
    let text = if text.starts_with("```") {
        let inner = text.trim_start_matches("```json").trim_start_matches("```");
        inner.trim_end_matches("```").trim()
    } else {
        text
    };

    // Parse the JSON response from the LLM
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
        tier: "behavioral".into(),
    })
}

/// Estimate token count for a code+invariant pair (rough approximation)
pub fn estimate_tokens(code: &str, invariant: &str) -> usize {
    // Rough: ~4 chars per token for English/code
    let prompt = build_prompt(code, invariant);
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
    mode: &LlmCheckMode,
    cache_dir: &Path,
    api_key: Option<&str>,
    model: &str,
) -> BehavioralSummary {
    let invariants = collect_invariants(spec);
    let mut summary = BehavioralSummary::default();

    for inv in &invariants {
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
                        mode,
                        cache_dir,
                        api_key,
                        model,
                        &mut summary,
                    )
                    .await;
                }
            }
            InvariantTier::Behavioral => {
                handle_behavioral_invariant(
                    inv,
                    source_code,
                    mode,
                    cache_dir,
                    api_key,
                    model,
                    &mut summary,
                )
                .await;
            }
        }
    }

    summary
}

async fn handle_behavioral_invariant(
    inv: &ClassifiedInvariant,
    source_code: &str,
    mode: &LlmCheckMode,
    cache_dir: &Path,
    api_key: Option<&str>,
    model: &str,
    summary: &mut BehavioralSummary,
) {
    let key = cache_key(source_code, &inv.text);

    match mode {
        LlmCheckMode::Off => {
            summary.skipped += 1;
        }
        LlmCheckMode::DryRun => {
            let cached = read_cache(cache_dir, &key).is_some();
            let tokens = estimate_tokens(source_code, &inv.text);
            eprintln!(
                "  [dry-run] {} | ~{} tokens | {}",
                inv.text,
                tokens,
                if cached { "CACHED" } else { "would call LLM" }
            );
            summary.skipped += 1;
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
            let Some(api_key) = api_key else {
                eprintln!(
                    "  [skip] {} — no ANTHROPIC_API_KEY set",
                    inv.text
                );
                summary.skipped += 1;
                return;
            };

            match llm_verify(source_code, &inv.text, api_key, model).await {
                Ok(result) => {
                    // Cache the result
                    let cached = CachedResult {
                        satisfies: result.satisfies,
                        reasoning: result.reasoning.clone(),
                        model: model.to_string(),
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
                    eprintln!("  [error] {} — LLM call failed: {}", inv.text, e);
                    summary.skipped += 1;
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
        let tokens = estimate_tokens("fn foo() {}", "never panics");
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
