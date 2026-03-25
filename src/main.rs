use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::*;
use std::path::PathBuf;

mod behavioral;
mod checker;
mod extractors;
mod rules;
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
        #[arg(long, default_value = "off")]
        llm_check: String,

        /// LLM model to use for behavioral checks
        #[arg(long, default_value = "claude-haiku-4-5-20251001")]
        llm_model: String,
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
        } => cmd_check(&path, &source, &format, rules.as_ref(), &llm_check, &llm_model),
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
    llm_check_mode: &str,
    llm_model: &str,
) -> Result<()> {
    let llm_mode: behavioral::LlmCheckMode = llm_check_mode
        .parse()
        .map_err(|e: String| anyhow::anyhow!(e))?;
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

        for error in &result.errors {
            println!("  {} {}", "✗".red(), error);
            total_errors += 1;
        }

        for warning in &result.warnings {
            println!("  {} {}", "⚠".yellow(), warning);
            total_warnings += 1;
        }

        if result.errors.is_empty() && result.warnings.is_empty() {
            println!("  {} All checks passed", "✓".green());
        }

        println!();
    }

    // ── Behavioral checks ──────────────────────────────────────────────────
    if llm_mode != behavioral::LlmCheckMode::Off {
        println!();
        println!("{}", "Behavioral Checks".bold().cyan());
        println!("{}", "-".repeat(40));

        let cache_dir = source_root.join(".spec-cache");
        let api_key = std::env::var("ANTHROPIC_API_KEY").ok();

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
                &llm_mode,
                &cache_dir,
                api_key.as_deref(),
                llm_model,
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

        if llm_mode == behavioral::LlmCheckMode::DryRun {
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
