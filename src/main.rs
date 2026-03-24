use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::*;
use std::path::PathBuf;

mod checker;
mod extractors;
mod rules;
mod spec;

use checker::SpecChecker;
use spec::{ModuleSpec, resolve_defaults};

#[derive(Parser)]
#[command(name = "spec-checker")]
#[command(about = "Structural and behavioral specification checker")]
#[command(version)]
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
    },

    /// Generate spec skeleton from existing code
    Init {
        /// Source file to analyze
        source: PathBuf,

        /// Language (solidity, rust, typescript)
        #[arg(short, long)]
        language: Option<String>,

        /// Output spec file path
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
        } => cmd_check(&path, &source, &format, rules.as_ref()),
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
            "ts" | "tsx" => "typescript",
            "js" | "jsx" => "javascript",
            _ => ext,
        })
        .map(String::from)
}
