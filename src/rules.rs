use serde::{Deserialize, Serialize};

use crate::spec::{LayerConfig, ModuleSpec, Stability};

/// A custom architectural rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// Rule name for error messages
    pub name: String,

    /// Description of what the rule enforces
    #[serde(default)]
    pub description: Option<String>,

    /// Condition for when this rule applies (default: always)
    /// Expression evaluated against (source, target) module pair
    #[serde(default = "default_when")]
    pub when: String,

    /// Requirement that must hold when 'when' is true
    /// Expression evaluated against (source, target) module pair
    pub require: String,

    /// Severity: error (default) or warning
    #[serde(default)]
    pub severity: Severity,
}

fn default_when() -> String {
    "true".to_string()
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    #[default]
    Error,
    Warning,
}

/// Project-level rules configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RulesConfig {
    /// Custom rules
    #[serde(default)]
    pub rules: Vec<Rule>,

    /// Disable built-in rules
    #[serde(default)]
    pub disable_builtin: Vec<String>,

    /// Custom layer definitions (overrides the built-in 4-layer model)
    #[serde(default)]
    pub layers: Option<LayersInput>,
}

/// Custom layer configuration input.
///
/// Supports two formats:
/// - **Linear list**: each layer can depend on all layers below it
///   ```yaml
///   layers:
///     - presentation
///     - application
///     - domain
///     - infrastructure
///   ```
/// - **Explicit DAG**: each layer lists its allowed dependencies
///   ```yaml
///   layers:
///     presentation:
///       can_depend_on: [application, domain, infrastructure]
///     domain:
///       can_depend_on: [infrastructure]
///   ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LayersInput {
    Linear(Vec<String>),
    Dag(std::collections::HashMap<String, LayerDepsInput>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerDepsInput {
    pub can_depend_on: Vec<String>,
}

impl LayersInput {
    /// Convert to a LayerConfig
    pub fn to_layer_config(&self) -> LayerConfig {
        match self {
            LayersInput::Linear(layers) => LayerConfig::from_linear(layers),
            LayersInput::Dag(dag) => {
                let converted: std::collections::HashMap<String, Vec<String>> = dag
                    .iter()
                    .map(|(k, v)| (k.clone(), v.can_depend_on.clone()))
                    .collect();
                LayerConfig::from_dag(&converted)
            }
        }
    }
}

/// Context for evaluating rule expressions
pub struct EvalContext<'a> {
    pub source: &'a ModuleSpec,
    pub target: &'a ModuleSpec,
    pub layer_config: &'a LayerConfig,
}

/// Evaluate a rule expression
pub fn evaluate(expr: &str, ctx: &EvalContext) -> Result<bool, String> {
    let expr = expr.trim();

    // Handle boolean literals
    if expr == "true" {
        return Ok(true);
    }
    if expr == "false" {
        return Ok(false);
    }

    // Handle OR expressions
    if let Some((left, right)) = split_binary_op(expr, " OR ") {
        return Ok(evaluate(left, ctx)? || evaluate(right, ctx)?);
    }
    if let Some((left, right)) = split_binary_op(expr, " or ") {
        return Ok(evaluate(left, ctx)? || evaluate(right, ctx)?);
    }
    if let Some((left, right)) = split_binary_op(expr, "||") {
        return Ok(evaluate(left, ctx)? || evaluate(right, ctx)?);
    }

    // Handle AND expressions
    if let Some((left, right)) = split_binary_op(expr, " AND ") {
        return Ok(evaluate(left, ctx)? && evaluate(right, ctx)?);
    }
    if let Some((left, right)) = split_binary_op(expr, " and ") {
        return Ok(evaluate(left, ctx)? && evaluate(right, ctx)?);
    }
    if let Some((left, right)) = split_binary_op(expr, "&&") {
        return Ok(evaluate(left, ctx)? && evaluate(right, ctx)?);
    }

    // Handle NOT expressions
    if let Some(inner) = expr
        .strip_prefix("NOT ")
        .or_else(|| expr.strip_prefix("not "))
        .or_else(|| expr.strip_prefix("!"))
    {
        return Ok(!evaluate(inner.trim(), ctx)?);
    }

    // Handle equality comparisons
    if let Some((left, right)) = split_binary_op(expr, "==") {
        let left_val = eval_value(left.trim(), ctx)?;
        let right_val = eval_value(right.trim(), ctx)?;
        return Ok(left_val == right_val);
    }
    if let Some((left, right)) = split_binary_op(expr, "!=") {
        let left_val = eval_value(left.trim(), ctx)?;
        let right_val = eval_value(right.trim(), ctx)?;
        return Ok(left_val != right_val);
    }

    // Handle method calls
    if expr.contains(".can_depend_on(") {
        return eval_can_depend_on(expr, ctx);
    }

    // Handle exists checks
    if expr.contains(".exists") {
        return eval_exists(expr, ctx);
    }

    // Try as a simple truthy check
    let val = eval_value(expr, ctx)?;
    Ok(!val.is_empty() && val != "none" && val != "null")
}

/// Split expression on binary operator, respecting parentheses
fn split_binary_op<'a>(expr: &'a str, op: &str) -> Option<(&'a str, &'a str)> {
    let mut depth = 0;
    let mut i = 0;
    let bytes = expr.as_bytes();

    while i < expr.len() {
        if bytes[i] == b'(' {
            depth += 1;
        } else if bytes[i] == b')' {
            depth -= 1;
        } else if depth == 0 && expr[i..].starts_with(op) {
            return Some((&expr[..i], &expr[i + op.len()..]));
        }
        i += 1;
    }
    None
}

/// Evaluate a value expression
fn eval_value(expr: &str, ctx: &EvalContext) -> Result<String, String> {
    let expr = expr.trim();

    // String literals
    if (expr.starts_with('"') && expr.ends_with('"'))
        || (expr.starts_with('\'') && expr.ends_with('\''))
    {
        return Ok(expr[1..expr.len() - 1].to_string());
    }

    // Stability literals
    if let Some(stability) = parse_stability_literal(expr) {
        return Ok(format!("{:?}", stability).to_lowercase());
    }

    // Property access: source.X or target.X
    if let Some(prop) = expr.strip_prefix("source.") {
        return eval_module_property(ctx.source, prop);
    }
    if let Some(prop) = expr.strip_prefix("target.") {
        return eval_module_property(ctx.target, prop);
    }

    // Return as literal
    Ok(expr.to_string())
}

fn eval_module_property(module: &ModuleSpec, prop: &str) -> Result<String, String> {
    match prop {
        "module" | "name" => Ok(module.module.clone()),
        "language" => Ok(module.language.clone().unwrap_or_default()),
        "source_path" | "path" => Ok(module.source_path.clone().unwrap_or_default()),
        "layer" => Ok(module
            .layer
            .as_ref()
            .map(|l| l.0.clone())
            .unwrap_or_default()),
        "context" => Ok(module.context.clone().unwrap_or_default()),
        "stability" => Ok(module
            .stability
            .map(|s| format!("{:?}", s).to_lowercase())
            .unwrap_or_default()),
        _ => Err(format!("Unknown property: {}", prop)),
    }
}

fn parse_stability_literal(s: &str) -> Option<Stability> {
    match s.to_lowercase().as_str() {
        "stable" | "stability::stable" => Some(Stability::Stable),
        "normal" | "stability::normal" => Some(Stability::Normal),
        "volatile" | "stability::volatile" => Some(Stability::Volatile),
        _ => None,
    }
}

fn eval_can_depend_on(expr: &str, ctx: &EvalContext) -> Result<bool, String> {
    // Parse: source.layer.can_depend_on(target.layer) or similar
    if expr.contains("layer.can_depend_on") {
        let source_layer = &ctx.source.layer;
        let target_layer = &ctx.target.layer;

        match (source_layer, target_layer) {
            (Some(s), Some(t)) => Ok(ctx.layer_config.can_depend_on(&s.0, &t.0)),
            _ => Ok(true), // If either is unspecified, allow
        }
    } else if expr.contains("stability.can_depend_on") {
        let source_stab = ctx.source.stability;
        let target_stab = ctx.target.stability;

        match (source_stab, target_stab) {
            (Some(s), Some(t)) => Ok(s.can_depend_on(&t)),
            _ => Ok(true), // If either is unspecified, allow
        }
    } else {
        Err(format!("Unknown can_depend_on expression: {}", expr))
    }
}

fn eval_exists(expr: &str, ctx: &EvalContext) -> Result<bool, String> {
    if expr.starts_with("source.") {
        let prop = expr
            .strip_prefix("source.")
            .unwrap()
            .strip_suffix(".exists")
            .unwrap_or(expr);
        let val = eval_module_property(ctx.source, prop)?;
        Ok(!val.is_empty())
    } else if expr.starts_with("target.") {
        let prop = expr
            .strip_prefix("target.")
            .unwrap()
            .strip_suffix(".exists")
            .unwrap_or(expr);
        let val = eval_module_property(ctx.target, prop)?;
        Ok(!val.is_empty())
    } else {
        Err(format!("Unknown exists expression: {}", expr))
    }
}

/// Built-in rules
pub fn builtin_rules() -> Vec<Rule> {
    vec![
        Rule {
            name: "layer-direction".to_string(),
            description: Some("Higher layers can depend on lower layers, not vice versa".to_string()),
            when: "source.layer.exists AND target.layer.exists".to_string(),
            require: "source.layer.can_depend_on(target.layer)".to_string(),
            severity: Severity::Error,
        },
        Rule {
            name: "context-isolation".to_string(),
            description: Some("Cross-context dependencies must go through Interface layer".to_string()),
            when: "source.context.exists AND target.context.exists AND source.context != target.context".to_string(),
            require: "source.layer == interface OR target.layer == interface".to_string(),
            severity: Severity::Error,
        },
        Rule {
            name: "stability-direction".to_string(),
            description: Some("Stable modules cannot depend on less stable modules".to_string()),
            when: "source.stability.exists AND target.stability.exists".to_string(),
            require: "source.stability.can_depend_on(target.stability)".to_string(),
            severity: Severity::Error,
        },
    ]
}

/// Check a dependency against all rules
pub fn check_dependency(
    source: &ModuleSpec,
    target: &ModuleSpec,
    rules: &[Rule],
    layer_config: &LayerConfig,
) -> Vec<RuleViolation> {
    let ctx = EvalContext {
        source,
        target,
        layer_config,
    };
    let mut violations = Vec::new();

    for rule in rules {
        // Check if rule applies
        let applies = match evaluate(&rule.when, &ctx) {
            Ok(v) => v,
            Err(e) => {
                violations.push(RuleViolation {
                    rule_name: rule.name.clone(),
                    message: format!("Error evaluating 'when': {}", e),
                    severity: Severity::Error,
                });
                continue;
            }
        };

        if !applies {
            continue;
        }

        // Check requirement
        let satisfied = match evaluate(&rule.require, &ctx) {
            Ok(v) => v,
            Err(e) => {
                violations.push(RuleViolation {
                    rule_name: rule.name.clone(),
                    message: format!("Error evaluating 'require': {}", e),
                    severity: Severity::Error,
                });
                continue;
            }
        };

        if !satisfied {
            let source_path = source.source_path.as_deref().unwrap_or(&source.module);
            let target_path = target.source_path.as_deref().unwrap_or(&target.module);

            violations.push(RuleViolation {
                rule_name: rule.name.clone(),
                message: format!(
                    "'{}' -> '{}': {}",
                    source_path,
                    target_path,
                    rule.description.as_deref().unwrap_or(&rule.name)
                ),
                severity: rule.severity,
            });
        }
    }

    violations
}

#[derive(Debug)]
pub struct RuleViolation {
    pub rule_name: String,
    pub message: String,
    pub severity: Severity,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::Layer;

    fn make_spec(
        name: &str,
        layer: Option<Layer>,
        context: Option<&str>,
        stability: Option<Stability>,
    ) -> ModuleSpec {
        ModuleSpec {
            module: name.to_string(),
            source_path: Some(format!("src/{}.rs", name)),
            layer,
            context: context.map(String::from),
            stability,
            ..Default::default()
        }
    }

    fn builtin_layer_config() -> LayerConfig {
        LayerConfig::builtin()
    }

    #[test]
    fn test_evaluate_literals() {
        let source = make_spec("a", None, None, None);
        let target = make_spec("b", None, None, None);
        let lc = builtin_layer_config();
        let ctx = EvalContext {
            source: &source,
            target: &target,
            layer_config: &lc,
        };

        assert!(evaluate("true", &ctx).unwrap());
        assert!(!evaluate("false", &ctx).unwrap());
    }

    #[test]
    fn test_evaluate_property_access() {
        let source = make_spec(
            "payments",
            Some(Layer::new("domain")),
            Some("payments"),
            Some(Stability::Stable),
        );
        let target = make_spec(
            "users",
            Some(Layer::new("domain")),
            Some("users"),
            Some(Stability::Volatile),
        );
        let lc = builtin_layer_config();
        let ctx = EvalContext {
            source: &source,
            target: &target,
            layer_config: &lc,
        };

        assert_eq!(eval_value("source.module", &ctx).unwrap(), "payments");
        assert_eq!(eval_value("target.context", &ctx).unwrap(), "users");
        assert_eq!(eval_value("source.layer", &ctx).unwrap(), "domain");
        assert_eq!(eval_value("target.stability", &ctx).unwrap(), "volatile");
    }

    #[test]
    fn test_evaluate_comparisons() {
        let source = make_spec("a", Some(Layer::new("domain")), Some("payments"), None);
        let target = make_spec("b", Some(Layer::new("domain")), Some("users"), None);
        let lc = builtin_layer_config();
        let ctx = EvalContext {
            source: &source,
            target: &target,
            layer_config: &lc,
        };

        assert!(evaluate("source.layer == target.layer", &ctx).unwrap());
        assert!(evaluate("source.context != target.context", &ctx).unwrap());
        assert!(evaluate("source.layer == domain", &ctx).unwrap());
    }

    #[test]
    fn test_evaluate_logical_ops() {
        let source = make_spec("a", Some(Layer::new("interface")), Some("x"), None);
        let target = make_spec("b", Some(Layer::new("domain")), Some("y"), None);
        let lc = builtin_layer_config();
        let ctx = EvalContext {
            source: &source,
            target: &target,
            layer_config: &lc,
        };

        assert!(evaluate(
            "source.layer == interface OR target.layer == interface",
            &ctx
        )
        .unwrap());
        assert!(!evaluate("source.layer == domain AND target.layer == domain", &ctx).unwrap());
        assert!(evaluate("NOT source.layer == domain", &ctx).unwrap());
    }

    #[test]
    fn test_evaluate_can_depend_on() {
        let source = make_spec(
            "a",
            Some(Layer::new("domain")),
            None,
            Some(Stability::Stable),
        );
        let target_infra = make_spec(
            "b",
            Some(Layer::new("infrastructure")),
            None,
            Some(Stability::Stable),
        );
        let target_app = make_spec(
            "c",
            Some(Layer::new("application")),
            None,
            Some(Stability::Volatile),
        );

        let lc = builtin_layer_config();

        let ctx1 = EvalContext {
            source: &source,
            target: &target_infra,
            layer_config: &lc,
        };
        assert!(evaluate("source.layer.can_depend_on(target.layer)", &ctx1).unwrap());

        let ctx2 = EvalContext {
            source: &source,
            target: &target_app,
            layer_config: &lc,
        };
        assert!(!evaluate("source.layer.can_depend_on(target.layer)", &ctx2).unwrap());

        // Stability
        let ctx3 = EvalContext {
            source: &source,
            target: &target_app,
            layer_config: &lc,
        };
        assert!(!evaluate("source.stability.can_depend_on(target.stability)", &ctx3).unwrap());
    }

    #[test]
    fn test_evaluate_can_depend_on_custom_layers() {
        let layers = vec![
            "ui".to_string(),
            "logic".to_string(),
            "data".to_string(),
        ];
        let lc = LayerConfig::from_linear(&layers);

        let source = make_spec("a", Some(Layer::new("data")), None, None);
        let target = make_spec("b", Some(Layer::new("ui")), None, None);

        let ctx = EvalContext {
            source: &source,
            target: &target,
            layer_config: &lc,
        };
        // data cannot depend on ui (ui is above data)
        assert!(!evaluate("source.layer.can_depend_on(target.layer)", &ctx).unwrap());

        // Reverse: ui can depend on data
        let ctx2 = EvalContext {
            source: &target,
            target: &source,
            layer_config: &lc,
        };
        assert!(evaluate("source.layer.can_depend_on(target.layer)", &ctx2).unwrap());
    }

    #[test]
    fn test_builtin_rules() {
        let rules = builtin_rules();
        assert_eq!(rules.len(), 3);
        assert!(rules.iter().any(|r| r.name == "layer-direction"));
        assert!(rules.iter().any(|r| r.name == "context-isolation"));
        assert!(rules.iter().any(|r| r.name == "stability-direction"));
    }

    #[test]
    fn test_check_dependency_layer_violation() {
        let source = make_spec("infra", Some(Layer::new("infrastructure")), None, None);
        let target = make_spec("domain", Some(Layer::new("domain")), None, None);
        let rules = builtin_rules();
        let lc = builtin_layer_config();

        let violations = check_dependency(&source, &target, &rules, &lc);
        assert!(!violations.is_empty());
        assert!(violations.iter().any(|v| v.rule_name == "layer-direction"));
    }

    #[test]
    fn test_check_dependency_context_violation() {
        let source = make_spec("payments", Some(Layer::new("domain")), Some("payments"), None);
        let target = make_spec("users", Some(Layer::new("domain")), Some("users"), None);
        let rules = builtin_rules();
        let lc = builtin_layer_config();

        let violations = check_dependency(&source, &target, &rules, &lc);
        assert!(violations
            .iter()
            .any(|v| v.rule_name == "context-isolation"));
    }

    #[test]
    fn test_custom_rule() {
        let rule = Rule {
            name: "no-test-to-prod".to_string(),
            description: Some("Test modules cannot depend on production code".to_string()),
            when: "true".to_string(),
            require: "source.module != target.module".to_string(),
            severity: Severity::Warning,
        };

        let source = make_spec("test_a", None, None, None);
        let target = make_spec("test_a", None, None, None);
        let lc = builtin_layer_config();

        let violations = check_dependency(&source, &target, &[rule], &lc);
        assert!(!violations.is_empty());
    }
}
