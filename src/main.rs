use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::*;
use std::collections::HashMap;
use std::path::PathBuf;

mod ai_init;
mod behavioral;
mod checker;
mod dependency;
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

        /// Only check the current project, skip dependency checks
        #[arg(long)]
        shallow: bool,

        /// Check a specific dependency project by name
        #[arg(long)]
        project: Option<String>,

        /// Verbose output: show every module check, not just errors/warnings
        #[arg(short, long)]
        verbose: bool,
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

        /// Glob patterns to exclude (can be repeated, e.g. --exclude 'tools/**' --exclude 'tests/**')
        #[arg(short, long)]
        exclude: Vec<String>,

        /// Use AI to enrich specs with descriptions, invariants, and API curation
        #[arg(long)]
        ai: bool,

        /// LLM provider from config (for --ai)
        #[arg(long)]
        llm_provider: Option<String>,
    },

    /// Show diff between spec and implementation
    Diff {
        /// Spec file
        spec: PathBuf,

        /// Source file
        source: PathBuf,
    },

    /// Print spec files in topological order (dependencies first)
    Toposort {
        /// Path to spec directory
        #[arg(default_value = "./specs")]
        path: PathBuf,

        /// Include all transitive dependency projects
        #[arg(long)]
        all: bool,
    },

    /// Show the dependency graph
    Deps {
        /// Path to project root
        #[arg(default_value = ".")]
        path: PathBuf,
    },

    /// Install Claude Code skills (default skills if no name given; "all" for everything)
    InitSkill {
        /// Install globally (~/.claude/commands/) instead of project-local (.claude/commands/)
        #[arg(long)]
        global: bool,

        /// Skill name to install (e.g. spec-refine, flow9, or "all")
        skill: Option<String>,
    },
}

fn main() -> Result<()> {
    let start = std::time::Instant::now();
    let cli = Cli::parse();

    let result = match cli.command {
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
            shallow,
            project,
            verbose,
        } => {
            let llm_config = load_llm_config(
                config.as_ref(),
                llm_check.as_deref(),
                llm_model.as_deref(),
                llm_endpoint.as_deref(),
                llm_api_key.as_deref(),
                llm_provider.as_deref(),
            );
            cmd_check(&path, &source, &format, rules.as_ref(), &llm_config, shallow, project.as_deref(), verbose)
        }
        Commands::Init {
            source,
            language,
            output,
            exclude,
            ai,
            llm_provider,
        } => {
            let llm_config = if ai {
                Some(load_llm_config(None, Some("full"), None, None, None, llm_provider.as_deref()))
            } else {
                None
            };
            cmd_init(&source, language.as_deref(), output.as_ref(), &exclude, llm_config.as_ref())
        }
        Commands::Diff { spec, source } => cmd_diff(&spec, &source),
        Commands::Toposort { path, all } => cmd_toposort(&path, all),
        Commands::Deps { path } => cmd_deps(&path),
        Commands::InitSkill { global, skill } => cmd_init_skill(global, skill.as_deref()),
    };

    let elapsed = start.elapsed();
    if elapsed.as_secs() >= 1 {
        eprintln!("\nCompleted in {:.1}s", elapsed.as_secs_f64());
    }

    result
}

fn cmd_check(
    spec_path: &PathBuf,
    source_root: &PathBuf,
    _format: &str,
    rules_path: Option<&PathBuf>,
    llm_config: &behavioral::LlmConfig,
    shallow: bool,
    project_filter: Option<&str>,
    verbose: bool,
) -> Result<()> {
    println!("{}", "Spec Checker".bold().cyan());
    println!("{}", "=".repeat(40));
    println!();

    // ── Dependency graph resolution ─────────────────────────────────────────
    let config = load_project_config_at(source_root);
    let has_deps = !config.dependencies.is_empty();
    let dep_graph = if has_deps && !shallow {
        match dependency::DependencyGraph::resolve(source_root) {
            Ok(graph) => {
                println!(
                    "{} Resolved {} project(s) in dependency graph",
                    "ℹ".blue(),
                    graph.projects.len()
                );
                for proj in graph.dependencies() {
                    println!("  {} {} ({})", "·".dimmed(), proj.name.cyan(), proj.root.display());
                }
                println!();
                Some(graph)
            }
            Err(e) => {
                eprintln!("{} Failed to resolve dependencies: {}", "✗".red(), e);
                return Err(e);
            }
        }
    } else {
        None
    };

    // ── If --project is set, check that specific dependency in isolation ────
    if let Some(proj_name) = project_filter {
        if let Some(ref graph) = dep_graph {
            if let Some(proj) = graph.get(proj_name) {
                println!("{} Checking project '{}' in isolation", "ℹ".blue(), proj_name);
                println!();
                let proj_checker = SpecChecker::new(proj.root.clone()).with_specs(&proj.specs);
                return run_single_project_check(&proj.specs, &proj_checker, &proj.root, llm_config);
            } else {
                anyhow::bail!("Project '{}' not found in dependency graph", proj_name);
            }
        } else {
            anyhow::bail!("--project requires dependencies to be configured (and --shallow must not be set)");
        }
    }

    // ── Phase 1: Check dependency projects internally ───────────────────────
    let mut dep_errors: HashMap<String, usize> = HashMap::new();
    if let Some(ref graph) = dep_graph {
        for proj in graph.dependencies() {
            println!("{} {}", "Checking dependency:".bold(), proj.name.cyan());

            let proj_checker = SpecChecker::new(proj.root.clone()).with_specs(&proj.specs);
            let mut proj_errors = 0;

            for spec in &proj.specs {
                let result = proj_checker.check(spec)?;
                let loc = spec.source_path.as_deref().unwrap_or("?");
                for cr in &result.constraint_results {
                    let tag = format!("[{}|{}]", cr.kind, cr.tier);
                    match cr.severity {
                        checker::ConstraintSeverity::Error => {
                            println!("  {} {} {}::{} ({}): {}", "✗".red(), tag.dimmed(), proj.name, spec.module, loc, cr.message);
                            proj_errors += 1;
                        }
                        checker::ConstraintSeverity::Warning => {
                            println!("  {} {} {}::{} ({}): {}", "⚠".yellow(), tag.dimmed(), proj.name, spec.module, loc, cr.message);
                        }
                    }
                }
            }

            if proj_errors > 0 {
                dep_errors.insert(proj.name.clone(), proj_errors);
                println!(
                    "  {} {} error(s) in dependency '{}'",
                    "✗".red(),
                    proj_errors,
                    proj.name
                );
            } else {
                println!("  {} All checks passed", "✓".green());
            }
            println!();
        }

        if !dep_errors.is_empty() {
            println!(
                "{} {} dependency project(s) have internal errors — boundary checks will be skipped for them",
                "⚠".yellow(),
                dep_errors.len()
            );
            println!();
        }
    }

    // ── Load and check the root project ─────────────────────────────────────
    let all_specs = load_specs(spec_path)?;

    if all_specs.is_empty() {
        println!("{}", "No spec files found.".yellow());
        return Ok(());
    }

    // Filter out specs matching exclude patterns from config
    let specs = if config.exclude.is_empty() {
        all_specs
    } else {
        let canonical_source = source_root.canonicalize().unwrap_or_else(|_| source_root.clone());
        let matchers: Vec<glob::Pattern> = config.exclude.iter()
            .filter_map(|pat| {
                let full = format!("{}/{}", canonical_source.display(), pat);
                glob::Pattern::new(&full).ok()
            })
            .collect();
        let before = all_specs.len();
        let filtered: Vec<_> = all_specs.into_iter().filter(|s| {
            if let Some(ref sp) = s.source_path {
                let full = canonical_source.join(sp);
                let canonical = full.canonicalize().unwrap_or(full);
                !matchers.iter().any(|p| p.matches_path(&canonical))
            } else {
                true
            }
        }).collect();
        let excluded = before - filtered.len();
        if excluded > 0 {
            println!("{} Excluded {} spec(s) matching config exclude patterns", "ℹ".blue(), excluded);
        }
        filtered
    };

    // Build checker with specs and optional rules config
    let mut checker = SpecChecker::new(source_root.clone()).with_specs(&specs);

    // Attach external specs from dependencies for cross-project reference resolution
    if let Some(ref graph) = dep_graph {
        checker = checker.with_dependency_graph(graph);
    }

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
    let mut total_passed = 0;

    for spec in &specs {
        let result = checker.check(spec)?;
        let loc = spec.source_path.as_deref().unwrap_or("?");

        if result.constraint_results.is_empty() {
            total_passed += 1;
            if verbose {
                println!("{} {} ({})", "Checking:".bold(), spec.module.cyan(), loc.dimmed());
                println!("  {} All checks passed", "✓".green());
                println!();
            }
        } else {
            println!("{} {} ({})", "Checking:".bold(), spec.module.cyan(), loc.dimmed());
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
            println!();
        }
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
        let mut subsystem_header_printed = false;

        for subsystem in &subsystem_specs {
            let sub_result = checker.check_subsystem(subsystem, &specs);

            if sub_result.constraint_results.is_empty() {
                total_passed += 1;
                if verbose {
                    if !subsystem_header_printed {
                        println!("{}", "Subsystem Checks".bold().cyan());
                        subsystem_header_printed = true;
                    }
                    println!("{} {}", "Checking subsystem:".bold(), subsystem.subsystem.cyan());
                    println!("  {} All checks passed", "✓".green());
                    println!();
                }
            } else {
                if !subsystem_header_printed {
                    println!("{}", "Subsystem Checks".bold().cyan());
                    subsystem_header_printed = true;
                }
                println!("{} {}", "Checking subsystem:".bold(), subsystem.subsystem.cyan());
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
                println!();
            }
        }
    }

    // ── Phase 2: Boundary checks (cross-project) ─────────────────────────
    if let Some(ref graph) = dep_graph {
        let boundary_result = checker.check_cross_project_boundaries(graph, &dep_errors);

        if !boundary_result.constraint_results.is_empty() {
            println!("{}", "Cross-Project Boundary Checks".bold().cyan());

            for cr in &boundary_result.constraint_results {
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
            "{} {} passed, {} error(s), {} warning(s)",
            "FAILED:".red().bold(),
            total_passed,
            total_errors,
            total_warnings
        );
        std::process::exit(1);
    } else if total_warnings > 0 {
        println!(
            "{} {} passed, {} warning(s)",
            "PASSED:".yellow().bold(),
            total_passed,
            total_warnings
        );
    } else {
        println!("{} All {} specs validated", "PASSED:".green().bold(), total_passed);
    }

    Ok(())
}

// ─── Config File ─────────────────────────────────────────────────────────────

/// Config file structure (.spec-checker.yaml)
#[derive(Debug, Default, serde::Deserialize)]
struct ProjectConfig {
    /// Canonical project name (used by consumers in `::` references)
    #[serde(default)]
    name: Option<String>,
    /// Direct dependencies: name -> { path: "..." }
    #[serde(default)]
    dependencies: HashMap<String, DependencyEntry>,
    /// Lightweight public module list (alternative to subsystem `exposes`)
    #[serde(default)]
    public_modules: Option<Vec<String>>,
    /// Glob patterns to exclude from init and check
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    llm: LlmFileConfig,
    #[serde(default)]
    #[allow(dead_code)]
    rules: Option<String>,
}

/// A dependency entry in the project config
#[derive(Debug, Clone, serde::Deserialize)]
struct DependencyEntry {
    /// Filesystem path to the dependency root (absolute or relative)
    path: PathBuf,
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
    #[allow(dead_code)]
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
    let paths_to_try: Vec<PathBuf> = if let Some(p) = config_path {
        vec![p.clone()]
    } else {
        vec![
            PathBuf::from(".spec-checker.yaml"),
            PathBuf::from(".spec-checker.yml"),
        ]
    };

    load_project_config_from_paths(&paths_to_try)
}

/// Load project config searching in a specific directory (used by dependency resolver)
fn load_project_config_at(dir: &std::path::Path) -> ProjectConfig {
    let paths_to_try = vec![
        dir.join(".spec-checker.yaml"),
        dir.join(".spec-checker.yml"),
    ];
    load_project_config_from_paths(&paths_to_try)
}

fn load_project_config_from_paths(paths_to_try: &[PathBuf]) -> ProjectConfig {
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

/// Save exclude patterns to .spec-checker.yaml, preserving other fields.
/// Ensure .spec-checker.yaml exists with at least a `name` field.
/// Optionally updates `exclude` patterns. Preserves all existing fields.
fn ensure_project_config(dir: &PathBuf, excludes: Option<&[String]>) -> Result<()> {
    let config_path = dir.join(".spec-checker.yaml");
    let is_new = !config_path.is_file();

    // Read existing config as a generic YAML value to preserve all fields
    let mut doc: serde_yaml::Value = if !is_new {
        let content = std::fs::read_to_string(&config_path)?;
        serde_yaml::from_str(&content).unwrap_or(serde_yaml::Value::Mapping(Default::default()))
    } else {
        serde_yaml::Value::Mapping(Default::default())
    };

    let mapping = doc.as_mapping_mut().ok_or_else(|| anyhow::anyhow!("Config is not a YAML mapping"))?;

    // Ensure `name` field exists (derive from directory name)
    let name_key = serde_yaml::Value::String("name".to_string());
    if !mapping.contains_key(&name_key) {
        let canonical = dir.canonicalize().unwrap_or_else(|_| dir.clone());
        let dir_name = canonical
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".to_string());
        mapping.insert(name_key, serde_yaml::Value::String(dir_name));
    }

    // Update exclude patterns if provided
    if let Some(excludes) = excludes {
        let exclude_val: Vec<serde_yaml::Value> = excludes.iter().map(|s| serde_yaml::Value::String(s.clone())).collect();
        mapping.insert(
            serde_yaml::Value::String("exclude".to_string()),
            serde_yaml::Value::Sequence(exclude_val),
        );
    }

    // Check if anything changed before writing
    let new_yaml = serde_yaml::to_string(&doc)?;
    let old_yaml = if !is_new {
        std::fs::read_to_string(&config_path).ok()
    } else {
        None
    };
    let changed = old_yaml.as_deref() != Some(&new_yaml);

    if changed {
        std::fs::write(&config_path, &new_yaml)?;
        if is_new {
            println!("{} Created {}", "ℹ".blue(), config_path.display());
        } else {
            println!("{} Updated {}", "ℹ".blue(), config_path.display());
        }
    }

    Ok(())
}

/// Run checks on a single project (used by --project flag)
fn run_single_project_check(
    specs: &[ModuleSpec],
    checker: &SpecChecker,
    source_root: &PathBuf,
    llm_config: &behavioral::LlmConfig,
) -> Result<()> {
    let mut total_errors = 0;
    let mut total_warnings = 0;
    let mut total_passed = 0;

    for spec in specs {
        let result = checker.check(spec)?;
        let loc = spec.source_path.as_deref().unwrap_or("?");

        if result.constraint_results.is_empty() {
            total_passed += 1;
        } else {
            println!("{} {} ({})", "Checking:".bold(), spec.module.cyan(), loc.dimmed());
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
            println!();
        }
    }

    if specs.len() > 1 {
        let composition_result = checker.check_composition(specs);
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
    }

    // Ignore LLM config for dependency projects (rules are project-local)
    let _ = (source_root, llm_config);

    println!("{}", "=".repeat(40));
    if total_errors > 0 {
        println!(
            "{} {} passed, {} error(s), {} warning(s)",
            "FAILED:".red().bold(),
            total_passed,
            total_errors,
            total_warnings
        );
        std::process::exit(1);
    } else {
        println!("{} All {} specs validated", "PASSED:".green().bold(), total_passed);
    }
    Ok(())
}

fn cmd_deps(project_root: &PathBuf) -> Result<()> {
    println!("{}", "Dependency Graph".bold().cyan());
    println!("{}", "=".repeat(40));
    println!();

    let config = load_project_config_at(project_root);
    let name = config.name.as_deref().unwrap_or("(unnamed)");

    if config.dependencies.is_empty() {
        println!("{} {} has no dependencies", "ℹ".blue(), name);
        return Ok(());
    }

    let graph = dependency::DependencyGraph::resolve(project_root)?;

    // Print tree
    println!("{} (root)", graph.root_project().name.bold());
    let deps = graph.root_project().dependency_names.clone();
    for (i, dep_name) in deps.iter().enumerate() {
        let is_last = i == deps.len() - 1;
        let prefix = if is_last { "└── " } else { "├── " };
        if let Some(proj) = graph.get(dep_name) {
            println!("{}{} ({})", prefix, proj.name.cyan(), proj.root.display());
            print_dep_tree(&graph, proj, if is_last { "    " } else { "│   " });
        }
    }

    println!();
    println!(
        "{} {} project(s) total",
        "ℹ".blue(),
        graph.projects.len()
    );

    Ok(())
}

fn print_dep_tree(graph: &dependency::DependencyGraph, proj: &dependency::ResolvedProject, indent: &str) {
    for (i, dep_name) in proj.dependency_names.iter().enumerate() {
        let is_last = i == proj.dependency_names.len() - 1;
        let prefix = if is_last { "└── " } else { "├── " };
        if let Some(dep) = graph.get(dep_name) {
            println!("{}{}{} ({})", indent, prefix, dep.name.cyan(), dep.root.display());
            let child_indent = format!("{}{}", indent, if is_last { "    " } else { "│   " });
            print_dep_tree(graph, dep, &child_indent);
        }
    }
}

fn cmd_init(source: &PathBuf, language: Option<&str>, output: Option<&PathBuf>, exclude: &[String], llm_config: Option<&behavioral::LlmConfig>) -> Result<()> {
    if source.is_dir() {
        return cmd_init_dir(source, language, output, exclude, llm_config);
    }

    let lang = language
        .map(String::from)
        .or_else(|| detect_language(source))
        .ok_or_else(|| anyhow::anyhow!("Could not detect language. Use --language flag."))?;

    println!("{} {} ({})", "Analyzing:".bold(), source.display(), lang);

    let extractor = extractors::get_extractor(&lang)?;
    let extracted = extractor.extract(source)?;

    let mut spec = ModuleSpec::from_extracted(&extracted);
    spec.source_hash = Some(spec::compute_source_hash(source)?);

    // AI enrichment if requested
    if let Some(config) = llm_config {
        let source_code = std::fs::read_to_string(source)?;
        println!("  {} Enriching with AI...", "⚙".cyan());
        let rt = tokio::runtime::Runtime::new()?;
        let start = std::time::Instant::now();
        match rt.block_on(ai_init::ai_enrich_spec(&spec, &source_code, &extracted, config, "")) {
            Ok(enrichment) => {
                ai_init::apply_enrichment(&mut spec, enrichment);
                println!("  {} AI enrichment applied ({:.1}s)", "✓".green(), start.elapsed().as_secs_f64());
            }
            Err(e) => {
                eprintln!("  {} AI enrichment failed: {} — using lean skeleton ({:.1}s)", "⚠".yellow(), e, start.elapsed().as_secs_f64());
            }
        }
    }

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
    exclude: &[String],
    llm_config: Option<&behavioral::LlmConfig>,
) -> Result<()> {
    // Determine file extension to scan for
    let extensions: Vec<&str> = match language {
        Some("flow9") | Some("flow") => vec!["flow"],
        Some("rust") | Some("rs") => vec!["rs"],
        Some("solidity") | Some("sol") => vec!["sol"],
        Some(lang) => anyhow::bail!("Unsupported language: {}", lang),
        None => vec!["flow", "rs", "sol"],
    };

    // Merge CLI excludes with existing config excludes
    let existing_config = load_project_config_at(source_dir);
    let all_excludes: Vec<String> = if exclude.is_empty() {
        existing_config.exclude.clone()
    } else {
        let mut merged = existing_config.exclude.clone();
        for pat in exclude {
            if !merged.contains(pat) {
                merged.push(pat.clone());
            }
        }
        merged
    };

    // Ensure .spec-checker.yaml exists with name; update exclude if CLI provided new ones
    let exclude_update = if exclude.is_empty() { None } else { Some(all_excludes.as_slice()) };
    ensure_project_config(source_dir, exclude_update)?;

    // Compile exclude patterns (resolved relative to source_dir)
    let canonical_source = source_dir.canonicalize().unwrap_or_else(|_| source_dir.clone());
    let exclude_matchers: Vec<glob::Pattern> = all_excludes
        .iter()
        .filter_map(|pat| {
            let full = format!("{}/{}", canonical_source.display(), pat);
            glob::Pattern::new(&full).ok()
        })
        .collect();

    // Determine output directory
    let out_dir = output.cloned().unwrap_or_else(|| PathBuf::from("./specs"));
    std::fs::create_dir_all(&out_dir)?;

    // Collect all source files first (for progress reporting)
    let mut source_files: Vec<PathBuf> = Vec::new();
    let mut skipped = 0;

    for ext in &extensions {
        let pattern = format!("{}/**/*.{}", source_dir.display(), ext);
        for entry in glob::glob(&pattern)? {
            let entry = entry?;
            if !exclude_matchers.is_empty() {
                let canonical_entry = entry.canonicalize().unwrap_or_else(|_| entry.clone());
                if exclude_matchers.iter().any(|p| p.matches_path(&canonical_entry)) {
                    skipped += 1;
                    continue;
                }
            }
            source_files.push(entry);
        }
    }

    // Read all source files upfront (needed for toposort when --ai)
    let mut file_sources: Vec<(PathBuf, String)> = Vec::new();
    for entry in &source_files {
        let source_code = std::fs::read_to_string(entry).unwrap_or_default();
        file_sources.push((entry.clone(), source_code));
    }

    // When using AI, process in dependency order so we can pass context
    let processing_order: Vec<usize> = if llm_config.is_some() {
        let order = ai_init::toposort_files(&file_sources);
        eprintln!("  {} Processing in dependency order", "ℹ".cyan());
        order
    } else {
        (0..file_sources.len()).collect()
    };

    // Create tokio runtime once (only needed for AI enrichment)
    let rt = llm_config.map(|_| tokio::runtime::Runtime::new()).transpose()?;

    let mut count = 0;
    let total = file_sources.len();
    // Accumulated enrichments keyed by module name (for dep context)
    let mut enrichments: HashMap<String, ai_init::SpecEnrichment> = HashMap::new();

    for (step, &idx) in processing_order.iter().enumerate() {
        let (ref entry, ref source_code) = file_sources[idx];

        let lang = detect_language(entry)
            .or_else(|| language.map(String::from))
            .unwrap_or_default();

        if lang.is_empty() {
            continue;
        }

        let extractor = match extractors::get_extractor(&lang) {
            Ok(e) => e,
            Err(_) => continue,
        };

        let extracted = match extractor.extract(entry) {
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

        let mut spec = ModuleSpec::from_extracted(&extracted);
        spec.source_hash = Some(spec::compute_source_hash(entry)?);

        // AI enrichment if requested
        if let (Some(config), Some(rt)) = (llm_config, rt.as_ref()) {
            if !source_code.is_empty() {
                // Build dep context from already-processed dependencies
                let internal_deps = ai_init::extract_internal_deps(source_code);
                let dep_enrichments: Vec<&ai_init::SpecEnrichment> = internal_deps.iter()
                    .filter_map(|dep| enrichments.get(dep))
                    .collect();
                let dep_context = ai_init::format_dep_context(&dep_enrichments);

                let dep_info = if dep_enrichments.is_empty() {
                    String::new()
                } else {
                    format!(" (deps: {})", internal_deps.iter()
                        .filter(|d| enrichments.contains_key(*d))
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", "))
                };

                eprint!("  [{}/{}] {} Enriching {}{}...",
                    step + 1, total, "⚙".cyan(), entry.display(), dep_info);
                let start = std::time::Instant::now();
                match rt.block_on(ai_init::ai_enrich_spec(&spec, source_code, &extracted, config, &dep_context)) {
                    Ok(enrichment) => {
                        // Store enrichment for downstream deps before applying
                        let module_name = spec.module.clone();
                        ai_init::apply_enrichment(&mut spec, enrichment);
                        // Re-extract enrichment from the applied spec for context
                        let stored = ai_init::SpecEnrichment {
                            description: spec.description.clone().unwrap_or_default(),
                            api_entities: spec.exposes.iter()
                                .map(|(name, e)| ai_init::ApiEntity {
                                    name: name.clone(),
                                    description: e.description.clone().unwrap_or_default(),
                                })
                                .collect(),
                            forbidden_deps: Vec::new(),
                            invariants: spec.invariants.clone(),
                            layer: spec.layer.as_ref().map(|l| l.0.clone()),
                        };
                        enrichments.insert(module_name, stored);
                        eprintln!(" {} ({:.1}s)", "done".green(), start.elapsed().as_secs_f64());
                    }
                    Err(e) => {
                        eprintln!(" {} {} ({:.1}s)", "failed:".yellow(), e, start.elapsed().as_secs_f64());
                    }
                }
            }
        }

        let yaml = serde_yaml::to_string(&spec)?;

        // Build output path: mirror directory structure
        let relative = entry
            .strip_prefix(source_dir)
            .unwrap_or(entry);
        let spec_name = relative.with_extension("spec.yaml");
        let spec_path = out_dir.join(&spec_name);

        if let Some(parent) = spec_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&spec_path, &yaml)?;
        println!("  {} {}", "✓".green(), spec_path.display());
        count += 1;
    }

    if skipped > 0 {
        println!(
            "\n{} Generated {} spec file(s) in {} ({} file(s) excluded)",
            "Done:".green().bold(),
            count,
            out_dir.display(),
            skipped
        );
    } else {
        println!(
            "\n{} Generated {} spec file(s) in {}",
            "Done:".green().bold(),
            count,
            out_dir.display()
        );
    }

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

fn cmd_toposort(spec_path: &PathBuf, all: bool) -> Result<()> {
    // If --all, resolve the full dependency graph and print projects + modules
    if all {
        let graph = dependency::DependencyGraph::resolve(&PathBuf::from("."))?;
        for proj in &graph.projects {
            println!("{} {}", "project:".bold(), proj.name.cyan());
            for spec in &proj.specs {
                if let Some(ref sp) = spec.source_path {
                    println!("  {}", sp);
                } else {
                    println!("  {}", spec.module);
                }
            }
        }
        return Ok(());
    }

    let specs = load_specs(spec_path)?;

    if specs.is_empty() {
        println!("{}", "No spec files found.".yellow());
        return Ok(());
    }

    // Build a map: source_path -> spec (for dependency resolution)
    // Also map module name -> source_path for flexible matching
    let mut path_to_idx: HashMap<String, usize> = HashMap::new();
    let mut module_to_idx: HashMap<String, usize> = HashMap::new();

    for (i, spec) in specs.iter().enumerate() {
        if let Some(ref sp) = spec.source_path {
            path_to_idx.insert(sp.clone(), i);
        }
        module_to_idx.insert(spec.module.clone(), i);
    }

    let n = specs.len();
    // Build adjacency list: edges[i] = list of indices that i depends on
    let mut edges: Vec<Vec<usize>> = vec![Vec::new(); n];

    for (i, spec) in specs.iter().enumerate() {
        for dep in &spec.depends_on {
            // Try to resolve dependency: first as source_path, then as module name
            if let Some(&j) = path_to_idx.get(dep) {
                edges[i].push(j);
            } else if let Some(&j) = module_to_idx.get(dep) {
                edges[i].push(j);
            }
            // External/unresolved deps are silently skipped
        }
    }

    // Kahn's algorithm for topological sort
    let mut in_degree = vec![0usize; n];
    for deps in &edges {
        for &j in deps {
            in_degree[j] += 1;
        }
    }

    let mut queue: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    for i in 0..n {
        if in_degree[i] == 0 {
            queue.push_back(i);
        }
    }

    let mut order: Vec<usize> = Vec::with_capacity(n);
    while let Some(i) = queue.pop_front() {
        order.push(i);
        for &j in &edges[i] {
            in_degree[j] -= 1;
            if in_degree[j] == 0 {
                queue.push_back(j);
            }
        }
    }

    // Detect cycles: if order doesn't contain all nodes
    if order.len() < n {
        let in_cycle: Vec<&str> = (0..n)
            .filter(|i| in_degree[*i] > 0)
            .map(|i| specs[i].module.as_str())
            .collect();
        eprintln!(
            "{} Dependency cycle detected among: {}",
            "⚠".yellow(),
            in_cycle.join(", ")
        );
        eprintln!("Appending cyclic modules in arbitrary order.");
        for i in 0..n {
            if in_degree[i] > 0 {
                order.push(i);
            }
        }
    }

    // Reverse: we want dependencies first (leaves first)
    // Kahn's gives us dependents first (nodes with no incoming edges = things nothing depends on)
    // Actually, let me reconsider. In our graph, edges[i] = what i depends on.
    // in_degree[j] counts how many things depend on j.
    // Kahn's removes nodes with in_degree 0 first = nodes nothing depends on = leaves/consumers.
    // We want the opposite: dependencies first.
    // So we reverse the output.
    order.reverse();

    for i in &order {
        let spec = &specs[*i];
        if let Some(ref sp) = spec.source_path {
            println!("{}", sp);
        } else {
            println!("{}", spec.module);
        }
    }

    Ok(())
}

fn cmd_init_skill(global: bool, skill: Option<&str>) -> Result<()> {
    let all_skills: &[(&str, &str, &[&str], bool)] = &[
        (
            "spec-refine",
            include_str!("../skills/spec-refine.md"),
            &[
                "  /spec-refine                      Refine specs with behavioral contracts",
                "  /spec-refine src/foo.rs            Refine specs for a single file",
            ],
            true, // installed by default
        ),
        (
            "spec-checker",
            include_str!("../skills/spec-checker.md"),
            &[
                "  /spec-checker help                  Show available modes",
                "  /spec-checker orient                Build mental model from specs",
                "  /spec-checker ask <question>         Answer questions from specs first",
                "  /spec-checker plan <feature>         Spec-first feature development",
                "  /spec-checker guard                 Guard refactoring against semantic drift",
                "  /spec-checker impact <module>        Show blast radius of changes",
                "  /spec-checker check                 Run checks and interpret results",
                "  /spec-checker bootstrap             One-command setup for new projects",
            ],
            true, // installed by default
        ),
        (
            "flow9",
            include_str!("../skills/flow9.md"),
            &[
                "  /flow9                             Flow9 language reference for the agent",
            ],
            false, // only installed with --only flow9
        ),
    ];

    let target_dir = if global {
        let home = std::env::var("HOME")
            .map_err(|_| anyhow::anyhow!("Could not determine HOME directory"))?;
        PathBuf::from(home).join(".claude").join("commands")
    } else {
        PathBuf::from(".claude").join("commands")
    };

    std::fs::create_dir_all(&target_dir)?;

    let install_all = skill == Some("all");

    let mut installed = 0;
    for (name, content, usage_lines, default) in all_skills {
        match skill {
            None => {
                if !default {
                    continue;
                }
            }
            Some("all") => {} // install everything
            Some(s) => {
                if s != *name {
                    continue;
                }
            }
        }

        let target_file = target_dir.join(format!("{}.md", name));
        let verb = if target_file.exists() { "Updated" } else { "Installed" };
        std::fs::write(&target_file, content)?;
        println!("{} {} skill: {}", "✓".green(), verb, target_file.display());
        for line in *usage_lines {
            println!("{}", line.cyan());
        }
        installed += 1;
    }

    if installed == 0 && !install_all {
        let available: Vec<&str> = all_skills.iter().map(|(n, _, _, _)| *n).collect();
        println!(
            "{} No matching skill '{}'. Available: {}, all",
            "⚠".yellow(),
            skill.unwrap_or(""),
            available.join(", ")
        );
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
