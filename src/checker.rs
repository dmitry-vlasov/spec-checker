use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::extractors::{get_extractor, ExtractedModule};
use crate::rules::{self, Rule, RulesConfig, Severity as RuleSeverity};
use crate::spec::{LayerConfig, ModuleSpec};
use crate::type_formula::{self, TypeEvalContext};

// ─── Spec-Type Constraint Model ──────────────────────────────────────────────

/// The kind of constraint in the spec-type system.
///
/// Each kind has its own compatibility relation and verification strategy.
/// This enum is extensible — new constraint kinds can be added as the
/// spec-type system grows (e.g., Protocol, Invariant, SmtVerified).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstraintKind {
    /// Entity existence and visibility (functions, types, variables exist and are public)
    Structural,
    /// Dependency constraints (allowed/forbidden module and external dependencies)
    Dependency,
    /// Architectural rules (layer direction, context isolation, stability)
    Architectural,
    /// Event existence and completeness
    Event,
    /// Type formula constraints (type-level properties via the DSL)
    TypeConstraint,
    /// Protocol constraints (state machine / call sequence rules)
    Protocol,
}

impl std::fmt::Display for ConstraintKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConstraintKind::Structural => write!(f, "structural"),
            ConstraintKind::Dependency => write!(f, "dependency"),
            ConstraintKind::Architectural => write!(f, "architectural"),
            ConstraintKind::Event => write!(f, "event"),
            ConstraintKind::TypeConstraint => write!(f, "type-constraint"),
            ConstraintKind::Protocol => write!(f, "protocol"),
        }
    }
}

/// The verification tier that decided a constraint result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerificationTier {
    /// Decided by direct syntactic/structural matching
    Syntactic,
    /// Decided by the rules engine expression evaluator
    RulesEngine,
    /// Decided by the type formula evaluator
    TypeFormula,
}

impl std::fmt::Display for VerificationTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerificationTier::Syntactic => write!(f, "syntactic"),
            VerificationTier::RulesEngine => write!(f, "rules-engine"),
            VerificationTier::TypeFormula => write!(f, "type-formula"),
        }
    }
}

/// Severity of a constraint check result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstraintSeverity {
    Error,
    Warning,
}

/// A single constraint check result within the spec-type framework.
#[derive(Debug, Clone)]
pub struct ConstraintResult {
    /// What kind of constraint was checked
    pub kind: ConstraintKind,
    /// How the result was determined
    pub tier: VerificationTier,
    /// Error or warning
    pub severity: ConstraintSeverity,
    /// Human-readable message
    pub message: String,
}

/// Result of checking a spec against implementation
#[derive(Debug, Default)]
pub struct CheckResult {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    /// Typed constraint results for programmatic access
    pub constraint_results: Vec<ConstraintResult>,
}

impl CheckResult {
    pub fn error(&mut self, msg: impl Into<String>) {
        self.errors.push(msg.into());
    }

    pub fn warning(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }

    /// Record a constraint violation with full spec-type metadata
    fn constraint_error(
        &mut self,
        kind: ConstraintKind,
        tier: VerificationTier,
        msg: impl Into<String>,
    ) {
        let message = msg.into();
        self.constraint_results.push(ConstraintResult {
            kind,
            tier,
            severity: ConstraintSeverity::Error,
            message: message.clone(),
        });
        self.errors.push(message);
    }

    /// Record a constraint warning with full spec-type metadata
    fn constraint_warning(
        &mut self,
        kind: ConstraintKind,
        tier: VerificationTier,
        msg: impl Into<String>,
    ) {
        let message = msg.into();
        self.constraint_results.push(ConstraintResult {
            kind,
            tier,
            severity: ConstraintSeverity::Warning,
            message: message.clone(),
        });
        self.warnings.push(message);
    }

    /// Returns true if there are no errors (used by tests)
    #[cfg(test)]
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Specification checker
pub struct SpecChecker {
    source_root: PathBuf,
    /// Map of module source_path to spec (built from all specs)
    spec_map: HashMap<String, ModuleSpec>,
    /// Active rules (built-in + custom)
    rules: Vec<Rule>,
    /// Layer dependency configuration
    layer_config: LayerConfig,
}

impl SpecChecker {
    pub fn new(source_root: PathBuf) -> Self {
        Self {
            source_root,
            spec_map: HashMap::new(),
            rules: rules::builtin_rules(),
            layer_config: LayerConfig::builtin(),
        }
    }

    /// Build spec map from all specs for cross-module checking
    pub fn with_specs(mut self, specs: &[ModuleSpec]) -> Self {
        for spec in specs {
            if let Some(source_path) = &spec.source_path {
                self.spec_map.insert(source_path.clone(), spec.clone());
            }
        }
        self
    }

    /// Configure custom rules and layer definitions
    pub fn with_rules_config(mut self, config: &RulesConfig) -> Self {
        // Remove disabled built-in rules
        self.rules
            .retain(|r| !config.disable_builtin.contains(&r.name));

        // Add custom rules
        self.rules.extend(config.rules.clone());

        // Apply custom layer config if provided
        if let Some(ref layers_input) = config.layers {
            self.layer_config = layers_input.to_layer_config();
        }

        self
    }

    /// Check a spec against its implementation.
    ///
    /// This is the main entry point for spec-type verification. It collects
    /// all constraints from the spec and checks each one using the appropriate
    /// verification strategy for its kind.
    pub fn check(&self, spec: &ModuleSpec) -> Result<CheckResult> {
        let mut result = CheckResult::default();

        // Find the source file
        let source_path = self.find_source_file(spec)?;

        if source_path.is_none() {
            result.constraint_error(
                ConstraintKind::Structural,
                VerificationTier::Syntactic,
                format!(
                    "Could not find source file for module '{}'. Set source_path in spec.",
                    spec.module
                ),
            );
            return Ok(result);
        }

        let source_path = source_path.unwrap();
        let language = spec.language.as_deref().unwrap_or("unknown");

        // Extract implementation details
        let extractor = match get_extractor(language) {
            Ok(e) => e,
            Err(_) => {
                result.warning(format!(
                    "No extractor for language '{}'. Skipping implementation checks.",
                    language
                ));
                return Ok(result);
            }
        };

        let extracted = match extractor.extract(&source_path) {
            Ok(e) => e,
            Err(e) => {
                result.constraint_error(
                    ConstraintKind::Structural,
                    VerificationTier::Syntactic,
                    format!(
                        "Failed to extract from {}: {}",
                        source_path.display(),
                        e
                    ),
                );
                return Ok(result);
            }
        };

        // Run all constraint checks, grouped by kind
        self.check_exposes(spec, &extracted, &mut result);
        self.check_dependencies(spec, &extracted, &mut result);
        self.check_forbidden_deps(spec, &extracted, &mut result);
        self.check_architectural_constraints(spec, &mut result);
        self.check_events(spec, &extracted, &mut result);
        self.check_type_constraints(spec, &extracted, &mut result);
        self.check_state_constraints(spec, &extracted, &mut result);
        self.check_protocol(spec, &extracted, &mut result);

        Ok(result)
    }

    /// Check that specified events exist in implementation
    fn check_events(
        &self,
        spec: &ModuleSpec,
        extracted: &ExtractedModule,
        result: &mut CheckResult,
    ) {
        // Check emits - events the spec says should be defined
        for event in &spec.emits {
            if !extracted.events.contains(event) {
                result.constraint_error(
                    ConstraintKind::Event,
                    VerificationTier::Syntactic,
                    format!(
                        "Event '{}' is specified in emits but not found in implementation",
                        event
                    ),
                );
            }
        }

        // Check for events defined in implementation but not in spec
        if !spec.emits.is_empty() {
            for event in &extracted.events {
                if !spec.emits.contains(event) {
                    result.constraint_warning(
                        ConstraintKind::Event,
                        VerificationTier::Syntactic,
                        format!(
                            "Event '{}' is defined in implementation but not in emits spec",
                            event
                        ),
                    );
                }
            }
        }
    }

    /// Check state-related constraints: owns_state, reads_state, modifies
    fn check_state_constraints(
        &self,
        spec: &ModuleSpec,
        extracted: &ExtractedModule,
        result: &mut CheckResult,
    ) {
        // Check owns_state: each listed state variable must exist in this module
        for state_var in &spec.owns_state {
            if !extracted.state_variables.contains(state_var) {
                result.constraint_error(
                    ConstraintKind::Structural,
                    VerificationTier::Syntactic,
                    format!(
                        "State variable '{}' is listed in owns_state but not found in implementation",
                        state_var
                    ),
                );
            }
        }

        // Check for state variables in implementation not listed in owns_state
        if !spec.owns_state.is_empty() {
            for state_var in &extracted.state_variables {
                if !spec.owns_state.contains(state_var) {
                    result.constraint_warning(
                        ConstraintKind::Structural,
                        VerificationTier::Syntactic,
                        format!(
                            "State variable '{}' exists in implementation but not listed in owns_state",
                            state_var
                        ),
                    );
                }
            }
        }

        // Check reads_state: referenced state must exist in some other module's owns_state
        for state_ref in &spec.reads_state {
            if !self.state_exists_in_other_module(state_ref, spec) {
                result.constraint_warning(
                    ConstraintKind::Dependency,
                    VerificationTier::Syntactic,
                    format!(
                        "reads_state '{}' not found in any other module's owns_state",
                        state_ref
                    ),
                );
            }
        }

        // Check modifies: referenced state must exist in some other module's owns_state
        for state_ref in &spec.modifies {
            if !self.state_exists_in_other_module(state_ref, spec) {
                result.constraint_warning(
                    ConstraintKind::Dependency,
                    VerificationTier::Syntactic,
                    format!(
                        "modifies '{}' not found in any other module's owns_state",
                        state_ref
                    ),
                );
            }
        }

        // Cross-module: check for state ownership conflicts
        self.check_state_ownership_conflicts(spec, result);
    }

    /// Check if a state variable is owned by some module other than the given one
    fn state_exists_in_other_module(&self, state_var: &str, current_spec: &ModuleSpec) -> bool {
        // If no other specs are loaded, skip the check
        if self.spec_map.is_empty() {
            return true;
        }
        for (_, other_spec) in &self.spec_map {
            if other_spec.module == current_spec.module {
                continue;
            }
            if other_spec.owns_state.contains(&state_var.to_string()) {
                return true;
            }
        }
        false
    }

    /// Detect cross-module state ownership conflicts:
    /// if two modules both claim to own the same state variable, that's an error.
    fn check_state_ownership_conflicts(
        &self,
        spec: &ModuleSpec,
        result: &mut CheckResult,
    ) {
        for state_var in &spec.owns_state {
            for (_, other_spec) in &self.spec_map {
                if other_spec.module == spec.module {
                    continue;
                }
                if other_spec.owns_state.contains(state_var) {
                    result.constraint_error(
                        ConstraintKind::Architectural,
                        VerificationTier::Syntactic,
                        format!(
                            "State ownership conflict: '{}' is owned by both '{}' and '{}'",
                            state_var, spec.module, other_spec.module
                        ),
                    );
                }
            }
        }
    }

    /// Check protocol constraints: validate the state machine definition and
    /// verify that all referenced functions exist in the implementation.
    fn check_protocol(
        &self,
        spec: &ModuleSpec,
        extracted: &ExtractedModule,
        result: &mut CheckResult,
    ) {
        let protocol = match &spec.protocol {
            Some(p) => p,
            None => return,
        };

        // Validate: initial state must be in states list
        if !protocol.states.is_empty() && !protocol.states.contains(&protocol.initial) {
            result.constraint_error(
                ConstraintKind::Protocol,
                VerificationTier::Syntactic,
                format!(
                    "Protocol initial state '{}' is not in states list",
                    protocol.initial
                ),
            );
        }

        // Validate: terminal states must be in states list
        for terminal in &protocol.terminal {
            if !protocol.states.is_empty() && !protocol.states.contains(terminal) {
                result.constraint_error(
                    ConstraintKind::Protocol,
                    VerificationTier::Syntactic,
                    format!(
                        "Protocol terminal state '{}' is not in states list",
                        terminal
                    ),
                );
            }
        }

        // Validate transitions
        let all_functions: Vec<&String> = extracted
            .public_functions
            .iter()
            .chain(extracted.private_functions.iter())
            .collect();

        for transition in &protocol.transitions {
            // from/to states must be in states list
            if !protocol.states.is_empty() {
                if !protocol.states.contains(&transition.from) {
                    result.constraint_error(
                        ConstraintKind::Protocol,
                        VerificationTier::Syntactic,
                        format!(
                            "Protocol transition references unknown state '{}'",
                            transition.from
                        ),
                    );
                }
                if !protocol.states.contains(&transition.to) {
                    result.constraint_error(
                        ConstraintKind::Protocol,
                        VerificationTier::Syntactic,
                        format!(
                            "Protocol transition references unknown state '{}'",
                            transition.to
                        ),
                    );
                }
            }

            // The called function must exist in implementation
            if !all_functions.iter().any(|f| **f == transition.call) {
                result.constraint_error(
                    ConstraintKind::Protocol,
                    VerificationTier::Syntactic,
                    format!(
                        "Protocol transition references function '{}' not found in implementation",
                        transition.call
                    ),
                );
            }
        }

        // Validate balanced pairs: both functions must exist
        for pair in &protocol.balanced_pairs {
            for func in pair {
                if !all_functions.iter().any(|f| **f == *func) {
                    result.constraint_error(
                        ConstraintKind::Protocol,
                        VerificationTier::Syntactic,
                        format!(
                            "Balanced pair references function '{}' not found in implementation",
                            func
                        ),
                    );
                }
            }
        }

        // Check for unreachable states: every non-initial state must be
        // reachable as a 'to' target of some transition
        if !protocol.states.is_empty() {
            let reachable_states: std::collections::HashSet<&String> = protocol
                .transitions
                .iter()
                .map(|t| &t.to)
                .chain(std::iter::once(&protocol.initial))
                .collect();

            for state in &protocol.states {
                if !reachable_states.contains(state) {
                    result.constraint_warning(
                        ConstraintKind::Protocol,
                        VerificationTier::Syntactic,
                        format!(
                            "Protocol state '{}' is unreachable (no transition leads to it)",
                            state
                        ),
                    );
                }
            }
        }

        // Check for dead-end non-terminal states: every non-terminal state
        // should have at least one outgoing transition
        if !protocol.states.is_empty() {
            let states_with_outgoing: std::collections::HashSet<&String> =
                protocol.transitions.iter().map(|t| &t.from).collect();

            for state in &protocol.states {
                if !protocol.terminal.contains(state)
                    && !states_with_outgoing.contains(state)
                {
                    result.constraint_warning(
                        ConstraintKind::Protocol,
                        VerificationTier::Syntactic,
                        format!(
                            "Protocol state '{}' is a dead end (no outgoing transitions and not terminal)",
                            state
                        ),
                    );
                }
            }
        }
    }

    /// Check type formula constraints for all exposed entities
    fn check_type_constraints(
        &self,
        spec: &ModuleSpec,
        extracted: &ExtractedModule,
        result: &mut CheckResult,
    ) {
        for (name, expose_spec) in &spec.exposes {
            if expose_spec.type_constraints.is_empty() {
                continue;
            }

            let kind = expose_spec.kind.as_deref();

            // Variables don't have type info yet — skip type constraint checking
            if kind == Some("variable") {
                continue;
            }

            let is_type = kind == Some("type");

            if is_type {
                // Type entity — look up in type_definitions
                let type_info = extracted.type_definitions.get(name);

                if type_info.is_none() && !expose_spec.type_constraints.is_empty() {
                    result.constraint_error(
                        ConstraintKind::TypeConstraint,
                        VerificationTier::Syntactic,
                        format!(
                            "Type '{}' is specified but not found in implementation",
                            name
                        ),
                    );
                    continue;
                }

                // Evaluate type constraints
                if let Some(ti) = type_info {
                    let ctx = TypeEvalContext {
                        self_type: Some(ti),
                        function: None,
                        type_defs: &extracted.type_definitions,
                        function_defs: &extracted.function_info,
                    };
                    self.eval_constraints(name, &expose_spec.type_constraints, &ctx, result);
                }
            } else {
                // Function entity — look up in function_info
                if expose_spec.type_constraints.is_empty() {
                    continue;
                }

                let func_info = extracted.function_info.get(name);
                if func_info.is_none() {
                    // Function existence is already checked by check_exposes
                    continue;
                }

                let fi = func_info.unwrap();
                let ctx = TypeEvalContext {
                    self_type: None,
                    function: Some(fi),
                    type_defs: &extracted.type_definitions,
                    function_defs: &extracted.function_info,
                };
                self.eval_constraints(name, &expose_spec.type_constraints, &ctx, result);
            }
        }
    }

    /// Evaluate a list of type constraint formulas against a context
    fn eval_constraints(
        &self,
        entity_name: &str,
        constraints: &[String],
        ctx: &TypeEvalContext,
        result: &mut CheckResult,
    ) {
        for constraint in constraints {
            match type_formula::parse_formula(constraint) {
                Ok(formula) => match type_formula::evaluate_formula(&formula, ctx) {
                    Ok(true) => {}
                    Ok(false) => {
                        result.constraint_error(
                            ConstraintKind::TypeConstraint,
                            VerificationTier::TypeFormula,
                            format!(
                                "'{}' violates type constraint: {}",
                                entity_name, constraint
                            ),
                        );
                    }
                    Err(e) => {
                        result.constraint_error(
                            ConstraintKind::TypeConstraint,
                            VerificationTier::TypeFormula,
                            format!(
                                "'{}' type constraint evaluation error for '{}': {}",
                                entity_name, constraint, e
                            ),
                        );
                    }
                },
                Err(e) => {
                    result.constraint_error(
                        ConstraintKind::TypeConstraint,
                        VerificationTier::TypeFormula,
                        format!(
                            "'{}' type constraint parse error for '{}': {}",
                            entity_name, constraint, e
                        ),
                    );
                }
            }
        }
    }

    /// Check all architectural constraints using rules engine
    fn check_architectural_constraints(&self, spec: &ModuleSpec, result: &mut CheckResult) {
        for dep_path in &spec.depends_on {
            let target_spec = match self.spec_map.get(dep_path) {
                Some(s) => s,
                None => continue, // External or unspecified dependency - skip
            };

            // Check all rules against this dependency
            let violations =
                rules::check_dependency(spec, target_spec, &self.rules, &self.layer_config);

            for violation in violations {
                let msg = format!("[{}] {}", violation.rule_name, violation.message);
                match violation.severity {
                    RuleSeverity::Error => {
                        result.constraint_error(
                            ConstraintKind::Architectural,
                            VerificationTier::RulesEngine,
                            msg,
                        );
                    }
                    RuleSeverity::Warning => {
                        result.constraint_warning(
                            ConstraintKind::Architectural,
                            VerificationTier::RulesEngine,
                            msg,
                        );
                    }
                }
            }
        }
    }

    /// Check that all exposed entities exist in implementation
    fn check_exposes(
        &self,
        spec: &ModuleSpec,
        extracted: &ExtractedModule,
        result: &mut CheckResult,
    ) {
        for (name, expose_spec) in &spec.exposes {
            let kind = expose_spec.kind.as_deref();

            if kind == Some("type") {
                // Type entities are checked by check_type_constraints
                // Here we just verify they exist
                if !extracted.type_definitions.contains_key(name) {
                    result.constraint_error(
                        ConstraintKind::Structural,
                        VerificationTier::Syntactic,
                        format!(
                            "Type '{}' is specified as exposed but not found in implementation",
                            name
                        ),
                    );
                }
            } else if kind == Some("variable") {
                // Global variables — check in state_variables
                if !extracted.state_variables.contains(name) {
                    result.constraint_error(
                        ConstraintKind::Structural,
                        VerificationTier::Syntactic,
                        format!(
                            "Variable '{}' is specified as exposed but not found in implementation",
                            name
                        ),
                    );
                }
            } else {
                // Function entities
                if !extracted.public_functions.contains(name) {
                    if extracted.private_functions.contains(name) {
                        result.constraint_error(
                            ConstraintKind::Structural,
                            VerificationTier::Syntactic,
                            format!(
                                "Function '{}' is specified as exposed but is private/internal in implementation",
                                name
                            ),
                        );
                    } else {
                        result.constraint_error(
                            ConstraintKind::Structural,
                            VerificationTier::Syntactic,
                            format!(
                                "Function '{}' is specified as exposed but not found in implementation",
                                name
                            ),
                        );
                    }
                } else {
                    // Check signature if specified (legacy)
                    if let Some(spec_sig) = &expose_spec.signature {
                        if let Some(impl_sig) = extracted.function_signatures.get(name) {
                            let spec_normalized: String =
                                spec_sig.chars().filter(|c| !c.is_whitespace()).collect();
                            let impl_normalized: String =
                                impl_sig.chars().filter(|c| !c.is_whitespace()).collect();

                            if spec_normalized != impl_normalized {
                                result.constraint_warning(
                                    ConstraintKind::Structural,
                                    VerificationTier::Syntactic,
                                    format!(
                                        "Function '{}' signature mismatch:\n  spec: {}\n  impl: {}",
                                        name, spec_sig, impl_sig
                                    ),
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// Check that all imports are in allowed dependencies
    fn check_dependencies(
        &self,
        spec: &ModuleSpec,
        extracted: &ExtractedModule,
        result: &mut CheckResult,
    ) {
        if spec.depends_on.is_empty() {
            // No dependency spec - skip check
            return;
        }

        for import in &extracted.imports {
            // Check if import matches any dependency
            // depends_on can be full paths (src/checker.rs) or module names (checker)
            let is_allowed = spec.depends_on.iter().any(|dep| {
                // Extract module name from path: "src/extractors/mod.rs" -> "extractors"
                let dep_name = Self::extract_module_name(dep);
                import == &dep_name || import.contains(&dep_name)
            });
            let is_external = spec.external_deps.iter().any(|dep| import.contains(dep));
            let is_std = import.starts_with("std::")
                || import.starts_with("core::")
                || import.starts_with("alloc::");

            if !is_allowed && !is_external && !is_std {
                result.constraint_warning(
                    ConstraintKind::Dependency,
                    VerificationTier::Syntactic,
                    format!(
                        "Import '{}' not in depends_on or external_deps",
                        import
                    ),
                );
            }
        }
    }

    /// Extract module name from a file path
    /// "src/checker.rs" -> "checker"
    /// "src/extractors/mod.rs" -> "extractors"
    fn extract_module_name(path: &str) -> String {
        let path = path.trim_end_matches(".rs");

        // Handle mod.rs case: "src/extractors/mod" -> "extractors"
        if path.ends_with("/mod") {
            return path
                .trim_end_matches("/mod")
                .rsplit('/')
                .next()
                .unwrap_or(path)
                .to_string();
        }

        // Regular case: "src/checker" -> "checker"
        path.rsplit('/').next().unwrap_or(path).to_string()
    }

    /// Check for forbidden dependencies
    fn check_forbidden_deps(
        &self,
        spec: &ModuleSpec,
        extracted: &ExtractedModule,
        result: &mut CheckResult,
    ) {
        for import in &extracted.imports {
            for forbidden in &spec.forbidden_deps {
                if import.contains(forbidden) {
                    result.constraint_error(
                        ConstraintKind::Dependency,
                        VerificationTier::Syntactic,
                        format!(
                            "Forbidden dependency: '{}' imports '{}' which matches forbidden '{}'",
                            spec.module, import, forbidden
                        ),
                    );
                }
            }

            for forbidden in &spec.forbidden_external {
                if import.contains(forbidden) {
                    result.constraint_error(
                        ConstraintKind::Dependency,
                        VerificationTier::Syntactic,
                        format!(
                            "Forbidden external dependency: '{}' imports '{}' which matches forbidden '{}'",
                            spec.module, import, forbidden
                        ),
                    );
                }
            }
        }
    }

    /// Find the source file for a module
    fn find_source_file(&self, spec: &ModuleSpec) -> Result<Option<PathBuf>> {
        // If source_path is specified, use it
        if let Some(source_path) = &spec.source_path {
            let path = self.source_root.join(source_path);
            if path.exists() {
                return Ok(Some(path));
            }
            return Ok(None);
        }

        // Try common patterns based on language
        let name = &spec.module;
        let language = spec.language.as_deref().unwrap_or("unknown");

        let candidates: Vec<PathBuf> = match language {
            "solidity" => vec![
                self.source_root.join(format!("contracts/{}.sol", name)),
                self.source_root.join(format!("src/{}.sol", name)),
                self.source_root.join(format!("{}.sol", name)),
            ],
            "rust" => vec![
                self.source_root
                    .join(format!("src/{}.rs", name.to_lowercase())),
                self.source_root.join("src/lib.rs".to_string()),
                self.source_root.join("src/main.rs".to_string()),
            ],
            "typescript" => vec![
                self.source_root.join(format!("src/{}.ts", name)),
                self.source_root.join(format!("{}.ts", name)),
            ],
            _ => Vec::new(),
        };

        for candidate in candidates {
            if candidate.exists() {
                return Ok(Some(candidate));
            }
        }

        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{ExposeSpec, Layer, ProtocolSpec, Stability, Transition};
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn create_test_spec() -> ModuleSpec {
        let mut exposes = HashMap::new();
        exposes.insert("deposit".to_string(), ExposeSpec::default());
        exposes.insert("withdraw".to_string(), ExposeSpec::default());

        ModuleSpec {
            module: "Bridge".to_string(),
            language: Some("solidity".to_string()),
            source_path: Some("Bridge.sol".to_string()),
            exposes,
            depends_on: vec!["TokenRegistry".to_string()],
            forbidden_deps: vec!["TestUtils".to_string()],
            ..Default::default()
        }
    }

    #[test]
    fn test_check_missing_function() {
        let dir = TempDir::new().unwrap();

        // Create a source file missing the withdraw function
        let content = r#"
contract Bridge {
    function deposit(address token, uint256 amount) public {}
    function _verifySignature(bytes memory sig) internal {}
}
"#;
        std::fs::write(dir.path().join("Bridge.sol"), content).unwrap();

        let checker = SpecChecker::new(dir.path().to_path_buf());
        let spec = create_test_spec();

        let result = checker.check(&spec).unwrap();

        assert!(!result.is_ok());
        assert!(result.errors.iter().any(|e| e.contains("withdraw")));
    }

    #[test]
    fn test_check_forbidden_dep() {
        let dir = TempDir::new().unwrap();

        let content = r#"
import "./TestUtils.sol"; // Forbidden!
import "./TokenRegistry.sol";

contract Bridge {
    function deposit(address token, uint256 amount) public {}
    function withdraw(address token, uint256 amount) public {}
    function _verifySignature(bytes memory sig) internal {}
}
"#;
        std::fs::write(dir.path().join("Bridge.sol"), content).unwrap();

        let checker = SpecChecker::new(dir.path().to_path_buf());
        let spec = create_test_spec();

        let result = checker.check(&spec).unwrap();

        assert!(!result.is_ok());
        assert!(result
            .errors
            .iter()
            .any(|e| e.contains("TestUtils") && e.contains("forbidden")));
    }

    #[test]
    fn test_check_all_passing() {
        let dir = TempDir::new().unwrap();

        let content = r#"
import "./TokenRegistry.sol";

contract Bridge {
    function deposit(address token, uint256 amount) public {}
    function withdraw(address token, uint256 amount) public {}
    function _verifySignature(bytes memory sig) internal {}
}
"#;
        std::fs::write(dir.path().join("Bridge.sol"), content).unwrap();

        let checker = SpecChecker::new(dir.path().to_path_buf());
        let spec = create_test_spec();

        let result = checker.check(&spec).unwrap();

        assert!(
            result.is_ok(),
            "Expected no errors but got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_check_layer_violation() {
        let dir = TempDir::new().unwrap();

        // Create two source files
        let infra_content = r#"
pub fn infra_function() {}
"#;
        let domain_content = r#"
use infra;
pub fn domain_function() {}
"#;
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/infra.rs"), infra_content).unwrap();
        std::fs::write(dir.path().join("src/domain.rs"), domain_content).unwrap();

        // Infrastructure module that depends on domain (VIOLATION!)
        let infra_spec = ModuleSpec {
            module: "infra".to_string(),
            language: Some("rust".to_string()),
            source_path: Some("src/infra.rs".to_string()),
            layer: Some(Layer::new("infrastructure")),
            depends_on: vec!["src/domain.rs".to_string()], // Infrastructure depending on Domain!
            ..Default::default()
        };

        // Domain module
        let domain_spec = ModuleSpec {
            module: "domain".to_string(),
            language: Some("rust".to_string()),
            source_path: Some("src/domain.rs".to_string()),
            layer: Some(Layer::new("domain")),
            ..Default::default()
        };

        let specs = vec![infra_spec.clone(), domain_spec];
        let checker = SpecChecker::new(dir.path().to_path_buf()).with_specs(&specs);

        let result = checker.check(&infra_spec).unwrap();

        assert!(
            !result.is_ok(),
            "Expected layer violation error but got none"
        );
        assert!(
            result.errors.iter().any(|e| e.contains("layer-direction")),
            "Expected 'layer-direction' error but got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_check_layer_valid() {
        let dir = TempDir::new().unwrap();

        // Create source files
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/infra.rs"), "pub fn f() {}").unwrap();
        std::fs::write(dir.path().join("src/domain.rs"), "pub fn f() {}").unwrap();

        // Domain depending on Infrastructure (valid)
        let domain_spec = ModuleSpec {
            module: "domain".to_string(),
            language: Some("rust".to_string()),
            source_path: Some("src/domain.rs".to_string()),
            layer: Some(Layer::new("domain")),
            depends_on: vec!["src/infra.rs".to_string()],
            ..Default::default()
        };

        let infra_spec = ModuleSpec {
            module: "infra".to_string(),
            language: Some("rust".to_string()),
            source_path: Some("src/infra.rs".to_string()),
            layer: Some(Layer::new("infrastructure")),
            ..Default::default()
        };

        let specs = vec![domain_spec.clone(), infra_spec];
        let checker = SpecChecker::new(dir.path().to_path_buf()).with_specs(&specs);

        let result = checker.check(&domain_spec).unwrap();

        assert!(
            result.is_ok(),
            "Expected no errors but got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_check_events_missing() {
        let dir = TempDir::new().unwrap();

        // Solidity contract with only one event
        let content = r#"
contract Bridge {
    event Deposited(address token, uint256 amount);
    
    function deposit(address token, uint256 amount) public {}
}
"#;
        std::fs::write(dir.path().join("Bridge.sol"), content).unwrap();

        // Spec expects two events
        let spec = ModuleSpec {
            module: "Bridge".to_string(),
            language: Some("solidity".to_string()),
            source_path: Some("Bridge.sol".to_string()),
            emits: vec!["Deposited".to_string(), "Withdrawn".to_string()],
            ..Default::default()
        };

        let checker = SpecChecker::new(dir.path().to_path_buf());
        let result = checker.check(&spec).unwrap();

        assert!(!result.is_ok(), "Expected error for missing event");
        assert!(
            result.errors.iter().any(|e| e.contains("Withdrawn")),
            "Expected error about 'Withdrawn' event but got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_check_events_extra() {
        let dir = TempDir::new().unwrap();

        // Solidity contract with two events
        let content = r#"
contract Bridge {
    event Deposited(address token, uint256 amount);
    event Withdrawn(address token, uint256 amount);
    
    function deposit(address token, uint256 amount) public {}
}
"#;
        std::fs::write(dir.path().join("Bridge.sol"), content).unwrap();

        // Spec only lists one event
        let spec = ModuleSpec {
            module: "Bridge".to_string(),
            language: Some("solidity".to_string()),
            source_path: Some("Bridge.sol".to_string()),
            emits: vec!["Deposited".to_string()],
            ..Default::default()
        };

        let checker = SpecChecker::new(dir.path().to_path_buf());
        let result = checker.check(&spec).unwrap();

        // Should have warning for extra event
        assert!(
            result.warnings.iter().any(|w| w.contains("Withdrawn")),
            "Expected warning about 'Withdrawn' event not in spec but got: {:?}",
            result.warnings
        );
    }

    #[test]
    fn test_check_context_violation() {
        let dir = TempDir::new().unwrap();

        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/payments.rs"), "pub fn pay() {}").unwrap();
        std::fs::write(dir.path().join("src/users.rs"), "pub fn user() {}").unwrap();

        // Payments domain module trying to directly depend on Users domain module
        let payments_spec = ModuleSpec {
            module: "payments".to_string(),
            language: Some("rust".to_string()),
            source_path: Some("src/payments.rs".to_string()),
            layer: Some(Layer::new("domain")),
            context: Some("payments".to_string()),
            depends_on: vec!["src/users.rs".to_string()],
            ..Default::default()
        };

        let users_spec = ModuleSpec {
            module: "users".to_string(),
            language: Some("rust".to_string()),
            source_path: Some("src/users.rs".to_string()),
            layer: Some(Layer::new("domain")),
            context: Some("users".to_string()),
            ..Default::default()
        };

        let specs = vec![payments_spec.clone(), users_spec];
        let checker = SpecChecker::new(dir.path().to_path_buf()).with_specs(&specs);

        let result = checker.check(&payments_spec).unwrap();

        assert!(!result.is_ok(), "Expected context violation error");
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.contains("context-isolation")),
            "Expected 'context-isolation' error but got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_check_context_via_interface() {
        let dir = TempDir::new().unwrap();

        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/payments_api.rs"), "pub fn pay() {}").unwrap();
        std::fs::write(dir.path().join("src/users_api.rs"), "pub fn api() {}").unwrap();

        // Payments interface depending on Users interface (allowed - both at interface layer)
        let payments_api_spec = ModuleSpec {
            module: "payments_api".to_string(),
            language: Some("rust".to_string()),
            source_path: Some("src/payments_api.rs".to_string()),
            layer: Some(Layer::new("interface")),
            context: Some("payments".to_string()),
            depends_on: vec!["src/users_api.rs".to_string()],
            ..Default::default()
        };

        let users_api_spec = ModuleSpec {
            module: "users_api".to_string(),
            language: Some("rust".to_string()),
            source_path: Some("src/users_api.rs".to_string()),
            layer: Some(Layer::new("interface")),
            context: Some("users".to_string()),
            ..Default::default()
        };

        let specs = vec![payments_api_spec.clone(), users_api_spec];
        let checker = SpecChecker::new(dir.path().to_path_buf()).with_specs(&specs);

        let result = checker.check(&payments_api_spec).unwrap();

        assert!(
            result.is_ok(),
            "Cross-context via interface should be allowed but got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_check_stability_violation() {
        let dir = TempDir::new().unwrap();

        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/core.rs"), "pub fn core() {}").unwrap();
        std::fs::write(dir.path().join("src/feature.rs"), "pub fn feature() {}").unwrap();

        // Stable core depending on volatile feature (VIOLATION)
        let core_spec = ModuleSpec {
            module: "core".to_string(),
            language: Some("rust".to_string()),
            source_path: Some("src/core.rs".to_string()),
            stability: Some(Stability::Stable),
            depends_on: vec!["src/feature.rs".to_string()],
            ..Default::default()
        };

        let feature_spec = ModuleSpec {
            module: "feature".to_string(),
            language: Some("rust".to_string()),
            source_path: Some("src/feature.rs".to_string()),
            stability: Some(Stability::Volatile),
            ..Default::default()
        };

        let specs = vec![core_spec.clone(), feature_spec];
        let checker = SpecChecker::new(dir.path().to_path_buf()).with_specs(&specs);

        let result = checker.check(&core_spec).unwrap();

        assert!(!result.is_ok(), "Expected stability violation error");
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.contains("stability-direction")),
            "Expected 'stability-direction' error but got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_check_stability_valid() {
        let dir = TempDir::new().unwrap();

        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/feature.rs"), "pub fn feature() {}").unwrap();
        std::fs::write(dir.path().join("src/core.rs"), "pub fn core() {}").unwrap();

        // Volatile feature depending on stable core (allowed)
        let feature_spec = ModuleSpec {
            module: "feature".to_string(),
            language: Some("rust".to_string()),
            source_path: Some("src/feature.rs".to_string()),
            stability: Some(Stability::Volatile),
            depends_on: vec!["src/core.rs".to_string()],
            ..Default::default()
        };

        let core_spec = ModuleSpec {
            module: "core".to_string(),
            language: Some("rust".to_string()),
            source_path: Some("src/core.rs".to_string()),
            stability: Some(Stability::Stable),
            ..Default::default()
        };

        let specs = vec![feature_spec.clone(), core_spec];
        let checker = SpecChecker::new(dir.path().to_path_buf()).with_specs(&specs);

        let result = checker.check(&feature_spec).unwrap();

        assert!(
            result.is_ok(),
            "Volatile depending on stable should be allowed but got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_check_owns_state_missing() {
        let dir = TempDir::new().unwrap();

        // Solidity contract with one state variable
        let content = r#"
contract Bridge {
    uint256 public deposited;
    function deposit() public {}
}
"#;
        std::fs::write(dir.path().join("Bridge.sol"), content).unwrap();

        // Spec claims two state variables, but only one exists
        let spec = ModuleSpec {
            module: "Bridge".to_string(),
            language: Some("solidity".to_string()),
            source_path: Some("Bridge.sol".to_string()),
            owns_state: vec!["deposited".to_string(), "withdrawn".to_string()],
            ..Default::default()
        };

        let checker = SpecChecker::new(dir.path().to_path_buf());
        let result = checker.check(&spec).unwrap();

        assert!(!result.is_ok(), "Expected error for missing state variable");
        assert!(
            result.errors.iter().any(|e| e.contains("withdrawn") && e.contains("owns_state")),
            "Expected error about 'withdrawn' in owns_state but got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_check_owns_state_extra() {
        let dir = TempDir::new().unwrap();

        // Solidity contract with two state variables
        let content = r#"
contract Bridge {
    uint256 public deposited;
    uint256 public withdrawn;
    function deposit() public {}
}
"#;
        std::fs::write(dir.path().join("Bridge.sol"), content).unwrap();

        // Spec only lists one
        let spec = ModuleSpec {
            module: "Bridge".to_string(),
            language: Some("solidity".to_string()),
            source_path: Some("Bridge.sol".to_string()),
            owns_state: vec!["deposited".to_string()],
            ..Default::default()
        };

        let checker = SpecChecker::new(dir.path().to_path_buf());
        let result = checker.check(&spec).unwrap();

        assert!(
            result.warnings.iter().any(|w| w.contains("withdrawn") && w.contains("owns_state")),
            "Expected warning about 'withdrawn' not in owns_state but got: {:?}",
            result.warnings
        );
    }

    #[test]
    fn test_check_state_ownership_conflict() {
        let dir = TempDir::new().unwrap();

        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/bridge.rs"),
            "pub static BALANCES: () = ();\npub fn deposit() {}",
        ).unwrap();
        std::fs::write(
            dir.path().join("src/vault.rs"),
            "pub static BALANCES: () = ();\npub fn withdraw() {}",
        ).unwrap();

        // Both modules claim to own BALANCES
        let bridge_spec = ModuleSpec {
            module: "bridge".to_string(),
            language: Some("rust".to_string()),
            source_path: Some("src/bridge.rs".to_string()),
            owns_state: vec!["BALANCES".to_string()],
            ..Default::default()
        };

        let vault_spec = ModuleSpec {
            module: "vault".to_string(),
            language: Some("rust".to_string()),
            source_path: Some("src/vault.rs".to_string()),
            owns_state: vec!["BALANCES".to_string()],
            ..Default::default()
        };

        let specs = vec![bridge_spec.clone(), vault_spec];
        let checker = SpecChecker::new(dir.path().to_path_buf()).with_specs(&specs);

        let result = checker.check(&bridge_spec).unwrap();

        assert!(
            !result.is_ok(),
            "Expected state ownership conflict error"
        );
        assert!(
            result.errors.iter().any(|e| e.contains("BALANCES") && e.contains("ownership conflict")),
            "Expected ownership conflict error about BALANCES but got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_check_reads_state_valid() {
        let dir = TempDir::new().unwrap();

        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/reader.rs"), "pub fn read_data() {}").unwrap();
        std::fs::write(
            dir.path().join("src/owner.rs"),
            "pub static DATA: () = ();\npub fn write_data() {}",
        ).unwrap();

        let reader_spec = ModuleSpec {
            module: "reader".to_string(),
            language: Some("rust".to_string()),
            source_path: Some("src/reader.rs".to_string()),
            reads_state: vec!["DATA".to_string()],
            ..Default::default()
        };

        let owner_spec = ModuleSpec {
            module: "owner".to_string(),
            language: Some("rust".to_string()),
            source_path: Some("src/owner.rs".to_string()),
            owns_state: vec!["DATA".to_string()],
            ..Default::default()
        };

        let specs = vec![reader_spec.clone(), owner_spec];
        let checker = SpecChecker::new(dir.path().to_path_buf()).with_specs(&specs);

        let result = checker.check(&reader_spec).unwrap();

        // No warning about reads_state because DATA exists in owner's owns_state
        assert!(
            !result.warnings.iter().any(|w| w.contains("reads_state") && w.contains("DATA")),
            "Should not warn about DATA in reads_state when owner exists, but got: {:?}",
            result.warnings
        );
    }

    #[test]
    fn test_check_reads_state_orphan() {
        let dir = TempDir::new().unwrap();

        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/reader.rs"), "pub fn read_data() {}").unwrap();
        std::fs::write(dir.path().join("src/other.rs"), "pub fn other() {}").unwrap();

        // reader reads DATA, but no module owns it
        let reader_spec = ModuleSpec {
            module: "reader".to_string(),
            language: Some("rust".to_string()),
            source_path: Some("src/reader.rs".to_string()),
            reads_state: vec!["DATA".to_string()],
            ..Default::default()
        };

        let other_spec = ModuleSpec {
            module: "other".to_string(),
            language: Some("rust".to_string()),
            source_path: Some("src/other.rs".to_string()),
            ..Default::default()
        };

        let specs = vec![reader_spec.clone(), other_spec];
        let checker = SpecChecker::new(dir.path().to_path_buf()).with_specs(&specs);

        let result = checker.check(&reader_spec).unwrap();

        assert!(
            result.warnings.iter().any(|w| w.contains("reads_state") && w.contains("DATA")),
            "Expected warning about orphan reads_state DATA but got: {:?}",
            result.warnings
        );
    }

    #[test]
    fn test_check_modifies_orphan() {
        let dir = TempDir::new().unwrap();

        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/writer.rs"), "pub fn write_data() {}").unwrap();
        std::fs::write(dir.path().join("src/other.rs"), "pub fn other() {}").unwrap();

        let writer_spec = ModuleSpec {
            module: "writer".to_string(),
            language: Some("rust".to_string()),
            source_path: Some("src/writer.rs".to_string()),
            modifies: vec!["COUNTER".to_string()],
            ..Default::default()
        };

        let other_spec = ModuleSpec {
            module: "other".to_string(),
            language: Some("rust".to_string()),
            source_path: Some("src/other.rs".to_string()),
            ..Default::default()
        };

        let specs = vec![writer_spec.clone(), other_spec];
        let checker = SpecChecker::new(dir.path().to_path_buf()).with_specs(&specs);

        let result = checker.check(&writer_spec).unwrap();

        assert!(
            result.warnings.iter().any(|w| w.contains("modifies") && w.contains("COUNTER")),
            "Expected warning about orphan modifies COUNTER but got: {:?}",
            result.warnings
        );
    }

    #[test]
    fn test_check_constraint_results_populated() {
        let dir = TempDir::new().unwrap();

        let content = r#"
contract Bridge {
    function deposit(address token, uint256 amount) public {}
}
"#;
        std::fs::write(dir.path().join("Bridge.sol"), content).unwrap();

        let mut exposes = HashMap::new();
        exposes.insert("deposit".to_string(), ExposeSpec::default());
        exposes.insert("missing_fn".to_string(), ExposeSpec::default());

        let spec = ModuleSpec {
            module: "Bridge".to_string(),
            language: Some("solidity".to_string()),
            source_path: Some("Bridge.sol".to_string()),
            exposes,
            ..Default::default()
        };

        let checker = SpecChecker::new(dir.path().to_path_buf());
        let result = checker.check(&spec).unwrap();

        // Should have at least one constraint result for the missing function
        assert!(
            !result.constraint_results.is_empty(),
            "Expected constraint_results to be populated"
        );
        assert!(
            result.constraint_results.iter().any(|cr| cr.kind == ConstraintKind::Structural),
            "Expected a Structural constraint result"
        );
        assert!(
            result.constraint_results.iter().any(|cr| cr.tier == VerificationTier::Syntactic),
            "Expected a Syntactic tier result"
        );
    }

    // ── Protocol constraint tests ────────────────────────────────────────

    fn make_protocol_spec(protocol: ProtocolSpec) -> ModuleSpec {
        ModuleSpec {
            module: "StateMachine".to_string(),
            language: Some("rust".to_string()),
            source_path: Some("src/sm.rs".to_string()),
            protocol: Some(protocol),
            ..Default::default()
        }
    }

    fn write_rust_with_fns(dir: &TempDir, fns: &[&str]) {
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        let content: String = fns
            .iter()
            .map(|f| format!("pub fn {}() {{}}\n", f))
            .collect();
        std::fs::write(dir.path().join("src/sm.rs"), content).unwrap();
    }

    #[test]
    fn test_protocol_valid() {
        let dir = TempDir::new().unwrap();
        write_rust_with_fns(&dir, &["initialize", "process", "close"]);

        let spec = make_protocol_spec(ProtocolSpec {
            states: vec!["init".into(), "ready".into(), "closed".into()],
            initial: "init".into(),
            terminal: vec!["closed".into()],
            transitions: vec![
                Transition { from: "init".into(), call: "initialize".into(), to: "ready".into() },
                Transition { from: "ready".into(), call: "process".into(), to: "ready".into() },
                Transition { from: "ready".into(), call: "close".into(), to: "closed".into() },
            ],
            balanced_pairs: vec![],
        });

        let checker = SpecChecker::new(dir.path().to_path_buf());
        let result = checker.check(&spec).unwrap();

        assert!(
            result.is_ok(),
            "Valid protocol should pass but got: {:?}",
            result.errors
        );
        // No warnings about unreachable/dead-end states
        assert!(
            !result.warnings.iter().any(|w| w.contains("unreachable") || w.contains("dead end")),
            "No protocol warnings expected but got: {:?}",
            result.warnings
        );
    }

    #[test]
    fn test_protocol_initial_state_invalid() {
        let dir = TempDir::new().unwrap();
        write_rust_with_fns(&dir, &["start"]);

        let spec = make_protocol_spec(ProtocolSpec {
            states: vec!["ready".into()],
            initial: "nonexistent".into(),
            terminal: vec![],
            transitions: vec![
                Transition { from: "ready".into(), call: "start".into(), to: "ready".into() },
            ],
            balanced_pairs: vec![],
        });

        let checker = SpecChecker::new(dir.path().to_path_buf());
        let result = checker.check(&spec).unwrap();

        assert!(
            result.errors.iter().any(|e| e.contains("initial state") && e.contains("nonexistent")),
            "Expected error about invalid initial state but got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_protocol_unknown_state_in_transition() {
        let dir = TempDir::new().unwrap();
        write_rust_with_fns(&dir, &["go"]);

        let spec = make_protocol_spec(ProtocolSpec {
            states: vec!["init".into(), "ready".into()],
            initial: "init".into(),
            terminal: vec![],
            transitions: vec![
                Transition { from: "init".into(), call: "go".into(), to: "phantom".into() },
            ],
            balanced_pairs: vec![],
        });

        let checker = SpecChecker::new(dir.path().to_path_buf());
        let result = checker.check(&spec).unwrap();

        assert!(
            result.errors.iter().any(|e| e.contains("unknown state") && e.contains("phantom")),
            "Expected error about unknown state 'phantom' but got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_protocol_missing_function() {
        let dir = TempDir::new().unwrap();
        write_rust_with_fns(&dir, &["initialize"]);

        let spec = make_protocol_spec(ProtocolSpec {
            states: vec!["init".into(), "ready".into()],
            initial: "init".into(),
            terminal: vec![],
            transitions: vec![
                Transition { from: "init".into(), call: "initialize".into(), to: "ready".into() },
                Transition { from: "ready".into(), call: "missing_fn".into(), to: "ready".into() },
            ],
            balanced_pairs: vec![],
        });

        let checker = SpecChecker::new(dir.path().to_path_buf());
        let result = checker.check(&spec).unwrap();

        assert!(
            result.errors.iter().any(|e| e.contains("missing_fn") && e.contains("not found")),
            "Expected error about missing function but got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_protocol_unreachable_state() {
        let dir = TempDir::new().unwrap();
        write_rust_with_fns(&dir, &["go"]);

        let spec = make_protocol_spec(ProtocolSpec {
            states: vec!["init".into(), "ready".into(), "orphan".into()],
            initial: "init".into(),
            terminal: vec![],
            transitions: vec![
                Transition { from: "init".into(), call: "go".into(), to: "ready".into() },
            ],
            balanced_pairs: vec![],
        });

        let checker = SpecChecker::new(dir.path().to_path_buf());
        let result = checker.check(&spec).unwrap();

        assert!(
            result.warnings.iter().any(|w| w.contains("orphan") && w.contains("unreachable")),
            "Expected warning about unreachable state 'orphan' but got: {:?}",
            result.warnings
        );
    }

    #[test]
    fn test_protocol_dead_end_state() {
        let dir = TempDir::new().unwrap();
        write_rust_with_fns(&dir, &["go"]);

        let spec = make_protocol_spec(ProtocolSpec {
            states: vec!["init".into(), "stuck".into()],
            initial: "init".into(),
            terminal: vec![], // stuck is NOT terminal
            transitions: vec![
                Transition { from: "init".into(), call: "go".into(), to: "stuck".into() },
            ],
            balanced_pairs: vec![],
        });

        let checker = SpecChecker::new(dir.path().to_path_buf());
        let result = checker.check(&spec).unwrap();

        assert!(
            result.warnings.iter().any(|w| w.contains("stuck") && w.contains("dead end")),
            "Expected warning about dead-end state 'stuck' but got: {:?}",
            result.warnings
        );
    }

    #[test]
    fn test_protocol_balanced_pairs_valid() {
        let dir = TempDir::new().unwrap();
        write_rust_with_fns(&dir, &["open", "close", "process"]);

        let spec = make_protocol_spec(ProtocolSpec {
            states: vec!["init".into(), "opened".into(), "closed".into()],
            initial: "init".into(),
            terminal: vec!["closed".into()],
            transitions: vec![
                Transition { from: "init".into(), call: "open".into(), to: "opened".into() },
                Transition { from: "opened".into(), call: "process".into(), to: "opened".into() },
                Transition { from: "opened".into(), call: "close".into(), to: "closed".into() },
            ],
            balanced_pairs: vec![["open".into(), "close".into()]],
        });

        let checker = SpecChecker::new(dir.path().to_path_buf());
        let result = checker.check(&spec).unwrap();

        assert!(
            result.is_ok(),
            "Valid protocol with balanced pairs should pass but got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_protocol_balanced_pairs_missing_fn() {
        let dir = TempDir::new().unwrap();
        write_rust_with_fns(&dir, &["acquire"]);

        let spec = make_protocol_spec(ProtocolSpec {
            states: vec!["init".into()],
            initial: "init".into(),
            terminal: vec![],
            transitions: vec![],
            balanced_pairs: vec![["acquire".into(), "release".into()]],
        });

        let checker = SpecChecker::new(dir.path().to_path_buf());
        let result = checker.check(&spec).unwrap();

        assert!(
            result.errors.iter().any(|e| e.contains("release") && e.contains("Balanced pair")),
            "Expected error about missing balanced pair function but got: {:?}",
            result.errors
        );
    }

    #[test]
    fn test_protocol_yaml_deserialization() {
        let yaml = r#"
module: Connection
language: rust
source_path: src/conn.rs
protocol:
  states: [disconnected, connected, closed]
  initial: disconnected
  terminal: [closed]
  transitions:
    - from: disconnected
      call: connect
      to: connected
    - from: connected
      call: send
      to: connected
    - from: connected
      call: disconnect
      to: closed
  balanced_pairs:
    - [connect, disconnect]
"#;
        let spec: ModuleSpec = serde_yaml::from_str(yaml).unwrap();

        let protocol = spec.protocol.as_ref().unwrap();
        assert_eq!(protocol.states.len(), 3);
        assert_eq!(protocol.initial, "disconnected");
        assert_eq!(protocol.terminal, vec!["closed"]);
        assert_eq!(protocol.transitions.len(), 3);
        assert_eq!(protocol.balanced_pairs.len(), 1);
        assert_eq!(protocol.balanced_pairs[0], ["connect", "disconnect"]);
    }
}
