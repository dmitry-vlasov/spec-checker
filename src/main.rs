use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::*;
use std::path::PathBuf;

mod behavioral;
mod checker;
mod extractors;
mod rules;
mod smt;
mod spec;
mod type_formula;
mod types;

use checker::SpecChecker;
use spec::{ModuleSpec, resolve_defaults};

const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (", env!("GIT_HASH"), ")"
);

#[derive(Parser)]
#[command(name = "spec-checker")]
#[command(about = "Structural and behavioral specification checker")]
#[command(version = VERSION)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Check specs against implementation
    Check {
        /// Path to spec file(s) or directory
        #[arg(default_value = "./specs")]
        path: PathBuf,

        /// Path to source code root
        #[arg(short, long, default_value = ".")]
        source: PathBuf,

        /// Output format (text, json)
        #[arg(short, long, default_value = "text")]
        format: String,

        /// Rules configuration file (YAML)
        #[arg(short, long)]
        rules: Option<PathBuf>,

        /// LLM behavioral check mode: off, dry-run, cached-only, full
        #[arg(long)]
        llm_check: Option<String>,

        /// LLM model to use for behavioral checks
        #[arg(long)]
        llm_model: Option<String>,

        /// LLM API endpoint URL (default: Anthropic; set for local LLMs)
        #[arg(long)]
        llm_endpoint: Option<String>,

        /// LLM API key (overrides config and env vars)
        #[arg(long)]
        llm_api_key: Option<String>,

        /// Named LLM provider from config file (overrides default)
        #[arg(long)]
        llm_provider: Option<String>,

        /// Path to config file (default: .spec-checker.yaml)
        #[arg(long)]
        config: Option<PathBuf>,
    },

    /// Generate spec skeleton from existing code (file or directory)
    Init {
        /// Source file or directory to analyze
        source: PathBuf,

        /// Language (solidity, rust, flow9, typescript)
        #[arg(short, long)]
        language: Option<String>,

        /// Output spec file or directory path
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Show diff between spec and implementation
    Diff {
        /// Spec file
        spec: PathBuf,

        /// Source file
        source: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Check {
            path,
            source,
            format,
            rules,
            llm_check,
            llm_model,
            llm_endpoint,
            llm_api_key,
            llm_provider,
            config,
        } => {
            let llm_config = load_llm_config(
                config.as_ref(),
                llm_check.as_deref(),
                llm_model.as_deref(),
                llm_endpoint.as_deref(),
                llm_api_key.as_deref(),
                llm_provider.as_deref(),
            );
            cmd_check(&path, &source, &format, rules.as_ref(), &llm_config)
        }
        Commands::Init {
            source,
            language,
            output,
        } => cmd_init(&source, language.as_deref(), output.as_ref()),
        Commands::Diff { spec, source } => cmd_diff(&spec, &source),
    }
}

fn cmd_check(
    spec_path: &PathBuf,
    source_root: &PathBuf,
    _format: &str,
    rules_path: Option<&PathBuf>,
    llm_config: &behavioral::LlmConfig,
) -> Result<()> {
    println!("{}", "Spec Checker".bold().cyan());
    println!("{}", "=".repeat(40));
    println!();

    let specs = load_specs(spec_path)?;

    if specs.is_empty() {
        println!("{}", "No spec files found.".yellow());
        return Ok(());
    }

    // Build checker with specs and optional rules config
    let mut checker = SpecChecker::new(source_root.clone()).with_specs(&specs);

    if let Some(rules_file) = rules_path {
        let rules_content = std::fs::read_to_string(rules_file)?;
        let rules_config: rules::RulesConfig = serde_yaml::from_str(&rules_content)?;
        checker = checker.with_rules_config(&rules_config);
        println!(
            "{} Loaded {} custom rule(s)",
            "ℹ".blue(),
            rules_config.rules.len()
        );
        if !rules_config.disable_builtin.is_empty() {
            println!(
                "{} Disabled built-in: {:?}",
                "ℹ".blue(),
                rules_config.disable_builtin
            );
        }
        println!();
    }

    let checker = checker;
    let mut total_errors = 0;
    let mut total_warnings = 0;

    for spec in &specs {
        println!("{} {}", "Checking:".bold(), spec.module.cyan());

        let result = checker.check(spec)?;

        for cr in &result.constraint_results {
            let tag = format!("[{}|{}]", cr.kind, cr.tier);
            match cr.severity {
                checker::ConstraintSeverity::Error => {
                    println!("  {} {} {}", "✗".red(), tag.dimmed(), cr.message);
                    total_errors += 1;
                }
                checker::ConstraintSeverity::Warning => {
                    println!("  {} {} {}", "⚠".yellow(), tag.dimmed(), cr.message);
                    total_warnings += 1;
                }
            }
        }

        if result.constraint_results.is_empty() {
            println!("  {} All checks passed", "✓".green());
        }

        println!();
    }

    // ── Composition checks (cross-module) ──────────────────────────────────
    if specs.len() > 1 {
        let composition_result = checker.check_composition(&specs);

        if !composition_result.constraint_results.is_empty() {
            println!("{}", "Composition Checks".bold().cyan());

            for cr in &composition_result.constraint_results {
                let tag = format!("[{}|{}]", cr.kind, cr.tier);
                match cr.severity {
                    checker::ConstraintSeverity::Error => {
                        println!("  {} {} {}", "✗".red(), tag.dimmed(), cr.message);
                        total_errors += 1;
                    }
                    checker::ConstraintSeverity::Warning => {
                        println!("  {} {} {}", "⚠".yellow(), tag.dimmed(), cr.message);
                        total_warnings += 1;
                    }
                }
            }

            println!();
        }
    }

    // ── Subsystem checks ──────────────────────────────────────────────────
    let subsystem_specs = load_subsystem_specs(spec_path);
    if !subsystem_specs.is_empty() {
        println!("{}", "Subsystem Checks".bold().cyan());

        for subsystem in &subsystem_specs {
            println!("{} {}", "Checking subsystem:".bold(), subsystem.subsystem.cyan());

            let sub_result = checker.check_subsystem(subsystem, &specs);

            for cr in &sub_result.constraint_results {
                let tag = format!("[{}|{}]", cr.kind, cr.tier);
                match cr.severity {
                    checker::ConstraintSeverity::Error => {
                        println!("  {} {} {}", "✗".red(), tag.dimmed(), cr.message);
                        total_errors += 1;
                    }
                    checker::ConstraintSeverity::Warning => {
                        println!("  {} {} {}", "⚠".yellow(), tag.dimmed(), cr.message);
                        total_warnings += 1;
                    }
                }
            }

            if sub_result.constraint_results.is_empty() {
                println!("  {} All checks passed", "✓".green());
            }

            println!();
        }
    }

    // ── Behavioral checks ──────────────────────────────────────────────────
    if llm_config.check_mode != behavioral::LlmCheckMode::Off {
        println!();
        println!("{}", "Behavioral Checks".bold().cyan());
        println!("{}", "-".repeat(40));

        let cache_dir = source_root.join(".spec-cache");

        let rt = tokio::runtime::Runtime::new()?;
        let mut behavioral_total = behavioral::BehavioralSummary::default();

        for spec in &specs {
            // Read source file for behavioral analysis
            let source_code = if let Some(sp) = &spec.source_path {
                let full_path = source_root.join(sp);
                std::fs::read_to_string(&full_path).unwrap_or_default()
            } else {
                String::new()
            };

            if source_code.is_empty() {
                continue;
            }

            let summary = rt.block_on(behavioral::check_behavioral(
                spec,
                &source_code,
                llm_config,
                &cache_dir,
            ));

            if summary.static_passed + summary.static_failed + summary.llm_passed
                + summary.llm_failed + summary.skipped
                > 0
            {
                println!("{} {}", "Module:".bold(), spec.module.cyan());
                if summary.static_passed > 0 {
                    println!(
                        "  {} {} static invariant(s) satisfied",
                        "✓".green(),
                        summary.static_passed
                    );
                }
                if summary.static_failed > 0 {
                    println!(
                        "  {} {} static invariant(s) failed",
                        "✗".red(),
                        summary.static_failed
                    );
                }
                if summary.llm_passed > 0 {
                    println!(
                        "  {} {} behavioral invariant(s) satisfied (LLM-verified{})",
                        "✓".green(),
                        summary.llm_passed,
                        if summary.cached > 0 {
                            format!(", {} cached", summary.cached)
                        } else {
                            String::new()
                        }
                    );
                }
                if summary.llm_failed > 0 {
                    println!(
                        "  {} {} behavioral invariant(s) failed (LLM-verified)",
                        "✗".red(),
                        summary.llm_failed
                    );
                }
                if summary.skipped > 0 {
                    println!(
                        "  {} {} invariant(s) skipped",
                        "○".blue(),
                        summary.skipped
                    );
                }
                for (inv, reasoning) in &summary.failures {
                    println!("    {} {}: {}", "→".red(), inv, reasoning);
                }
            }

            behavioral_total.static_passed += summary.static_passed;
            behavioral_total.static_failed += summary.static_failed;
            behavioral_total.llm_passed += summary.llm_passed;
            behavioral_total.llm_failed += summary.llm_failed;
            behavioral_total.cached += summary.cached;
            behavioral_total.skipped += summary.skipped;
            behavioral_total.failures.extend(summary.failures);
        }

        println!();
        total_errors += behavioral_total.static_failed + behavioral_total.llm_failed;

        if llm_config.check_mode == behavioral::LlmCheckMode::DryRun {
            println!(
                "{} Behavioral checks in dry-run mode (no LLM calls made)",
                "ℹ".blue()
            );
        }
    }

    println!("{}", "=".repeat(40));

    if total_errors > 0 {
        println!(
            "{} {} error(s), {} warning(s)",
            "FAILED:".red().bold(),
            total_errors,
            total_warnings
        );
        std::process::exit(1);
    } else if total_warnings > 0 {
        println!(
            "{} {} warning(s)",
            "PASSED:".yellow().bold(),
            total_warnings
        );
    } else {
        println!("{} All specs validated", "PASSED:".green().bold());
    }

    Ok(())
}

// ─── Config File ─────────────────────────────────────────────────────────────

/// Config file structure (.spec-checker.yaml)
#[derive(Debug, Default, serde::Deserialize)]
struct ProjectConfig {
    #[serde(default)]
    llm: LlmFileConfig,
    #[serde(default)]
    rules: Option<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct LlmFileConfig {
    /// Global check mode (off, dry-run, cached-only, full)
    #[serde(default)]
    check: Option<String>,
    /// Default provider name (must match a key in `providers`)
    #[serde(default)]
    default: Option<String>,
    /// Named LLM provider configurations
    #[serde(default)]
    providers: std::collections::HashMap<String, ProviderFileConfig>,

    // Legacy flat fields (backward compatibility)
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    cooldown_secs: Option<u64>,
    #[serde(default)]
    context_size: Option<u32>,
    #[serde(default)]
    local: Option<LegacyLocalFileConfig>,
}

/// Per-provider configuration in the `providers` map.
#[derive(Debug, Default, Clone, serde::Deserialize)]
struct ProviderFileConfig {
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    cooldown_secs: Option<u64>,
    #[serde(default)]
    context_size: Option<u32>,
}

/// Legacy local LLM sub-config (accepted but ignored for backward compatibility).
#[derive(Debug, Default, serde::Deserialize)]
#[allow(dead_code)]
struct LegacyLocalFileConfig {
    endpoint: String,
    model: String,
}

/// Load and resolve LlmConfig from: defaults → config file → env vars → CLI flags.
fn load_llm_config(
    config_path: Option<&PathBuf>,
    cli_check: Option<&str>,
    cli_model: Option<&str>,
    cli_endpoint: Option<&str>,
    cli_api_key: Option<&str>,
    cli_provider: Option<&str>,
) -> behavioral::LlmConfig {
    let mut config = behavioral::LlmConfig::default();

    // 1. Load config file
    let file_config = load_project_config(config_path);
    let llm = &file_config.llm;

    // 2. Resolve provider config: new format (providers map) or legacy (flat fields)
    if !llm.providers.is_empty() {
        // New format: pick the selected provider
        let provider_name = cli_provider
            .map(|s| s.to_string())
            .or_else(|| llm.default.clone());

        let selected = if let Some(ref name) = provider_name {
            match llm.providers.get(name) {
                Some(p) => p.clone(),
                None => {
                    let available: Vec<&str> =
                        llm.providers.keys().map(|s| s.as_str()).collect();
                    eprintln!(
                        "Warning: provider '{}' not found in config. Available: {}",
                        name,
                        available.join(", ")
                    );
                    ProviderFileConfig::default()
                }
            }
        } else {
            // No default set and no CLI override — pick first alphabetically
            if let Some((name, p)) = llm.providers.iter().min_by_key(|(k, _)| k.as_str()) {
                eprintln!(
                    "Warning: no default provider set, using '{}'",
                    name
                );
                p.clone()
            } else {
                ProviderFileConfig::default()
            }
        };

        if let Some(endpoint) = selected.endpoint {
            config.endpoint = endpoint;
        }
        if let Some(model) = selected.model {
            config.model = model;
        }
        if let Some(api_key) = selected.api_key {
            config.api_key = Some(api_key);
        }
        if let Some(cooldown) = selected.cooldown_secs {
            config.cooldown_secs = cooldown;
        }
        if let Some(ctx) = selected.context_size {
            config.context_size = ctx;
        }
    } else {
        // Legacy flat format
        if let Some(ref endpoint) = llm.endpoint {
            config.endpoint = endpoint.clone();
        }
        if let Some(ref model) = llm.model {
            config.model = model.clone();
        }
        if let Some(ref api_key) = llm.api_key {
            config.api_key = Some(api_key.clone());
        }
        if let Some(cooldown) = llm.cooldown_secs {
            config.cooldown_secs = cooldown;
        }
        if let Some(ctx) = llm.context_size {
            config.context_size = ctx;
        }
    }

    // 3. Apply global check mode from config
    if let Some(ref check) = llm.check {
        if let Ok(mode) = check.parse() {
            config.check_mode = mode;
        }
    }

    // 4. CLI flags override everything
    if let Some(endpoint) = cli_endpoint {
        config.endpoint = endpoint.to_string();
    }
    if let Some(model) = cli_model {
        config.model = model.to_string();
    }
    if let Some(api_key) = cli_api_key {
        config.api_key = Some(api_key.to_string());
    }
    if let Some(check) = cli_check {
        if let Ok(mode) = check.parse() {
            config.check_mode = mode;
        }
    }

    // 5. Detect provider from endpoint
    config.provider = behavioral::LlmConfig::detect_provider(&config.endpoint);

    // 6. Resolve API key from env vars if not set
    config.resolve_api_key();

    config
}

fn load_project_config(config_path: Option<&PathBuf>) -> ProjectConfig {
    // Try explicit path first, then default location
    let paths_to_try: Vec<PathBuf> = if let Some(p) = config_path {
        vec![p.clone()]
    } else {
        vec![
            PathBuf::from(".spec-checker.yaml"),
            PathBuf::from(".spec-checker.yml"),
        ]
    };

    for path in paths_to_try {
        if path.is_file() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                match serde_yaml::from_str::<ProjectConfig>(&content) {
                    Ok(config) => return config,
                    Err(e) => {
                        eprintln!("Warning: failed to parse {}: {}", path.display(), e);
                    }
                }
            }
        }
    }

    ProjectConfig::default()
}

fn cmd_init(source: &PathBuf, language: Option<&str>, output: Option<&PathBuf>) -> Result<()> {
    if source.is_dir() {
        return cmd_init_dir(source, language, output);
    }

    let lang = language
        .map(String::from)
        .or_else(|| detect_language(source))
        .ok_or_else(|| anyhow::anyhow!("Could not detect language. Use --language flag."))?;

    println!("{} {} ({})", "Analyzing:".bold(), source.display(), lang);

    let extractor = extractors::get_extractor(&lang)?;
    let extracted = extractor.extract(source)?;

    let spec = ModuleSpec::from_extracted(&extracted);
    let yaml = serde_yaml::to_string(&spec)?;

    if let Some(out_path) = output {
        std::fs::write(out_path, &yaml)?;
        println!("{} {}", "Wrote spec to:".green(), out_path.display());
    } else {
        println!();
        println!("{}", yaml);
    }

    Ok(())
}

fn cmd_init_dir(
    source_dir: &PathBuf,
    language: Option<&str>,
    output: Option<&PathBuf>,
) -> Result<()> {
    // Determine file extension to scan for
    let extensions: Vec<&str> = match language {
        Some("flow9") | Some("flow") => vec!["flow"],
        Some("rust") | Some("rs") => vec!["rs"],
        Some("solidity") | Some("sol") => vec!["sol"],
        Some(lang) => anyhow::bail!("Unsupported language: {}", lang),
        None => vec!["flow", "rs", "sol"],
    };

    // Determine output directory
    let out_dir = output.cloned().unwrap_or_else(|| PathBuf::from("./specs"));
    std::fs::create_dir_all(&out_dir)?;

    let mut count = 0;

    for ext in &extensions {
        let pattern = format!("{}/**/*.{}", source_dir.display(), ext);
        for entry in glob::glob(&pattern)? {
            let entry = entry?;

            let lang = detect_language(&entry)
                .or_else(|| language.map(String::from))
                .unwrap_or_default();

            if lang.is_empty() {
                continue;
            }

            let extractor = match extractors::get_extractor(&lang) {
                Ok(e) => e,
                Err(_) => continue,
            };

            let extracted = match extractor.extract(&entry) {
                Ok(e) => e,
                Err(e) => {
                    eprintln!(
                        "  {} {} — {}",
                        "⚠".yellow(),
                        entry.display(),
                        e
                    );
                    continue;
                }
            };

            // Skip files with nothing to spec
            if extracted.public_functions.is_empty() && extracted.type_definitions.is_empty() {
                continue;
            }

            let spec = ModuleSpec::from_extracted(&extracted);
            let yaml = serde_yaml::to_string(&spec)?;

            // Build output path: mirror directory structure
            let relative = entry
                .strip_prefix(source_dir)
                .unwrap_or(&entry);
            let spec_name = relative.with_extension("spec.yaml");
            let spec_path = out_dir.join(&spec_name);

            if let Some(parent) = spec_path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            std::fs::write(&spec_path, &yaml)?;
            println!("  {} {}", "✓".green(), spec_path.display());
            count += 1;
        }
    }

    println!(
        "\n{} Generated {} spec file(s) in {}",
        "Done:".green().bold(),
        count,
        out_dir.display()
    );

    Ok(())
}

fn cmd_diff(spec_path: &PathBuf, source_path: &PathBuf) -> Result<()> {
    println!("{}", "Spec vs Implementation Diff".bold().cyan());
    println!("{}", "=".repeat(40));
    println!();

    let spec = load_spec(spec_path)?;
    let lang = spec
        .language
        .clone()
        .or_else(|| detect_language(source_path))
        .ok_or_else(|| anyhow::anyhow!("Could not detect language"))?;

    let extractor = extractors::get_extractor(&lang)?;
    let extracted = extractor.extract(source_path)?;

    // Compare exposes
    println!("{}", "Exposed Functions:".bold());
    for name in spec.exposes.keys() {
        if extracted.public_functions.contains(name) {
            println!("  {} {} (spec + impl)", "✓".green(), name);
        } else {
            println!("  {} {} (spec only - MISSING)", "✗".red(), name);
        }
    }
    for name in &extracted.public_functions {
        if !spec.exposes.contains_key(name) {
            println!("  {} {} (impl only - NOT IN SPEC)", "?".yellow(), name);
        }
    }

    println!();
    println!("{}", "Dependencies:".bold());
    for dep in &spec.depends_on {
        if extracted.imports.iter().any(|i| i.contains(dep)) {
            println!("  {} {} (allowed + used)", "✓".green(), dep);
        } else {
            println!("  {} {} (allowed, not used)", "○".blue(), dep);
        }
    }

    for import in &extracted.imports {
        let is_forbidden = spec.forbidden_deps.iter().any(|f| import.contains(f));
        let is_allowed = spec.depends_on.iter().any(|d| import.contains(d));

        if is_forbidden {
            println!("  {} {} (FORBIDDEN)", "✗".red(), import);
        } else if !is_allowed {
            println!("  {} {} (not in spec)", "?".yellow(), import);
        }
    }

    // State variables
    if !spec.owns_state.is_empty() || !extracted.state_variables.is_empty() {
        println!();
        println!("{}", "State Variables:".bold());
        for state in &spec.owns_state {
            if extracted.state_variables.contains(state) {
                println!("  {} {} (spec + impl)", "✓".green(), state);
            } else {
                println!("  {} {} (spec only - MISSING)", "✗".red(), state);
            }
        }
        for state in &extracted.state_variables {
            if !spec.owns_state.contains(state) {
                println!("  {} {} (impl only - NOT IN SPEC)", "?".yellow(), state);
            }
        }
    }

    // Events
    if !spec.emits.is_empty() || !extracted.events.is_empty() {
        println!();
        println!("{}", "Events:".bold());
        for event in &spec.emits {
            if extracted.events.contains(event) {
                println!("  {} {} (spec + impl)", "✓".green(), event);
            } else {
                println!("  {} {} (spec only - MISSING)", "✗".red(), event);
            }
        }
        for event in &extracted.events {
            if !spec.emits.contains(event) {
                println!("  {} {} (impl only - NOT IN SPEC)", "?".yellow(), event);
            }
        }
    }

    // Protocol
    if let Some(ref protocol) = spec.protocol {
        println!();
        println!("{}", "Protocol:".bold());
        println!(
            "  States: {} | Initial: {} | Terminal: {:?}",
            protocol.states.join(", "),
            protocol.initial,
            protocol.terminal
        );
        println!("  Transitions: {}", protocol.transitions.len());
        for t in &protocol.transitions {
            let fn_exists = extracted.public_functions.contains(&t.call)
                || extracted.function_info.contains_key(&t.call);
            let marker = if fn_exists { "✓".green() } else { "✗".red() };
            println!("    {} {} -> {}() -> {}", marker, t.from, t.call, t.to);
        }
        if !protocol.balanced_pairs.is_empty() {
            println!("  Balanced pairs:");
            for pair in &protocol.balanced_pairs {
                println!("    [{}, {}]", pair[0], pair[1]);
            }
        }
    }

    Ok(())
}

fn load_specs(path: &PathBuf) -> Result<Vec<ModuleSpec>> {
    let mut specs = Vec::new();

    if path.is_file() {
        let mut spec = load_spec(path)?;
        // Apply defaults from the spec file's directory up to its parent
        if let Some(dir) = path.parent() {
            let root = dir; // single file: use its directory as root
            let defaults = resolve_defaults(dir, root);
            spec.apply_defaults(&defaults);
        }
        specs.push(spec);
    } else if path.is_dir() {
        let root = path.as_path();

        for entry in glob::glob(&format!("{}/**/*.spec.yaml", path.display()))? {
            let entry = entry?;
            let mut spec = load_spec(&entry)?;
            // Apply hierarchical defaults from root down to this spec's directory
            if let Some(dir) = entry.parent() {
                let defaults = resolve_defaults(dir, root);
                spec.apply_defaults(&defaults);
            }
            specs.push(spec);
        }
        for entry in glob::glob(&format!("{}/**/*.spec.yml", path.display()))? {
            let entry = entry?;
            let mut spec = load_spec(&entry)?;
            if let Some(dir) = entry.parent() {
                let defaults = resolve_defaults(dir, root);
                spec.apply_defaults(&defaults);
            }
            specs.push(spec);
        }
    }

    Ok(specs)
}

fn load_spec(path: &PathBuf) -> Result<ModuleSpec> {
    let content = std::fs::read_to_string(path)?;
    let spec: ModuleSpec = serde_yaml::from_str(&content)?;
    Ok(spec)
}

fn load_subsystem_specs(path: &PathBuf) -> Vec<spec::SubsystemSpec> {
    let mut subsystems = Vec::new();

    if path.is_dir() {
        let pattern = format!("{}/**/*.subsystem.yaml", path.display());
        if let Ok(entries) = glob::glob(&pattern) {
            for entry in entries.flatten() {
                if let Ok(content) = std::fs::read_to_string(&entry) {
                    match serde_yaml::from_str::<spec::SubsystemSpec>(&content) {
                        Ok(sub) => subsystems.push(sub),
                        Err(e) => {
                            eprintln!("  Warning: failed to parse {}: {}", entry.display(), e);
                        }
                    }
                }
            }
        }
    }

    subsystems
}

fn detect_language(path: &PathBuf) -> Option<String> {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| match ext {
            "sol" => "solidity",
            "rs" => "rust",
            "flow" => "flow9",
            "ts" | "tsx" => "typescript",
            "js" | "jsx" => "javascript",
            _ => ext,
        })
        .map(String::from)
}
