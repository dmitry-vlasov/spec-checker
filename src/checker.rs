use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::extractors::{get_extractor, ExtractedModule};
use crate::rules::{self, Rule, RulesConfig, Severity as RuleSeverity};
use crate::spec::{LayerConfig, ModuleSpec};

/// Result of checking a spec against implementation
#[derive(Debug, Default)]
pub struct CheckResult {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl CheckResult {
    pub fn error(&mut self, msg: impl Into<String>) {
        self.errors.push(msg.into());
    }

    pub fn warning(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
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

    /// Check a spec against its implementation
    pub fn check(&self, spec: &ModuleSpec) -> Result<CheckResult> {
        let mut result = CheckResult::default();

        // Find the source file
        let source_path = self.find_source_file(spec)?;

        if source_path.is_none() {
            result.error(format!(
                "Could not find source file for module '{}'. Set source_path in spec.",
                spec.module
            ));
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
                result.error(format!(
                    "Failed to extract from {}: {}",
                    source_path.display(),
                    e
                ));
                return Ok(result);
            }
        };

        // Run all checks
        self.check_exposes(spec, &extracted, &mut result);
        self.check_internal(spec, &extracted, &mut result);
        self.check_dependencies(spec, &extracted, &mut result);
        self.check_forbidden_deps(spec, &extracted, &mut result);
        self.check_architectural_constraints(spec, &mut result);
        self.check_events(spec, &extracted, &mut result);

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
                result.error(format!(
                    "Event '{}' is specified in emits but not found in implementation",
                    event
                ));
            }
        }

        // Check for events defined in implementation but not in spec
        if !spec.emits.is_empty() {
            for event in &extracted.events {
                if !spec.emits.contains(event) {
                    result.warning(format!(
                        "Event '{}' is defined in implementation but not in emits spec",
                        event
                    ));
                }
            }
        }

        // Note: subscribes checking would require analyzing function bodies
        // to see what events are being listened to - this is more complex
        // and language-specific, so we skip it for now
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
                match violation.severity {
                    RuleSeverity::Error => {
                        result.error(format!("[{}] {}", violation.rule_name, violation.message));
                    }
                    RuleSeverity::Warning => {
                        result.warning(format!("[{}] {}", violation.rule_name, violation.message));
                    }
                }
            }
        }
    }

    /// Check that all exposed functions exist in implementation
    fn check_exposes(
        &self,
        spec: &ModuleSpec,
        extracted: &ExtractedModule,
        result: &mut CheckResult,
    ) {
        for (name, func_spec) in &spec.exposes {
            if !extracted.public_functions.contains(name) {
                if extracted.private_functions.contains(name) {
                    result.error(format!(
                        "Function '{}' is specified as exposed but is private/internal in implementation",
                        name
                    ));
                } else {
                    result.error(format!(
                        "Function '{}' is specified as exposed but not found in implementation",
                        name
                    ));
                }
            } else {
                // Check signature if specified
                if let Some(spec_sig) = &func_spec.signature {
                    if let Some(impl_sig) = extracted.function_signatures.get(name) {
                        // Normalize signatures for comparison (remove whitespace)
                        let spec_normalized: String =
                            spec_sig.chars().filter(|c| !c.is_whitespace()).collect();
                        let impl_normalized: String =
                            impl_sig.chars().filter(|c| !c.is_whitespace()).collect();

                        if spec_normalized != impl_normalized {
                            result.warning(format!(
                                "Function '{}' signature mismatch:\n  spec: {}\n  impl: {}",
                                name, spec_sig, impl_sig
                            ));
                        }
                    }
                }
            }
        }
    }

    /// Check that internal functions are not exposed
    fn check_internal(
        &self,
        spec: &ModuleSpec,
        extracted: &ExtractedModule,
        result: &mut CheckResult,
    ) {
        for name in &spec.internal {
            if extracted.public_functions.contains(name) {
                result.error(format!(
                    "Function '{}' is specified as internal but is public in implementation",
                    name
                ));
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
                result.warning(format!(
                    "Import '{}' not in depends_on or external_deps",
                    import
                ));
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
                    result.error(format!(
                        "Forbidden dependency: '{}' imports '{}' which matches forbidden '{}'",
                        spec.module, import, forbidden
                    ));
                }
            }

            for forbidden in &spec.forbidden_external {
                if import.contains(forbidden) {
                    result.error(format!(
                        "Forbidden external dependency: '{}' imports '{}' which matches forbidden '{}'",
                        spec.module, import, forbidden
                    ));
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
    use crate::spec::{FunctionSpec, Layer, Stability};
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn create_test_spec() -> ModuleSpec {
        let mut exposes = HashMap::new();
        exposes.insert("deposit".to_string(), FunctionSpec::default());
        exposes.insert("withdraw".to_string(), FunctionSpec::default());

        ModuleSpec {
            module: "Bridge".to_string(),
            language: Some("solidity".to_string()),
            source_path: Some("Bridge.sol".to_string()),
            exposes,
            internal: vec!["_verifySignature".to_string()],
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
    fn test_check_internal_exposed() {
        let dir = TempDir::new().unwrap();

        // Create a source file where internal function is public
        let content = r#"
contract Bridge {
    function deposit(address token, uint256 amount) public {}
    function withdraw(address token, uint256 amount) public {}
    function _verifySignature(bytes memory sig) public {} // Should be internal!
}
"#;
        std::fs::write(dir.path().join("Bridge.sol"), content).unwrap();

        let checker = SpecChecker::new(dir.path().to_path_buf());
        let spec = create_test_spec();

        let result = checker.check(&spec).unwrap();

        assert!(!result.is_ok());
        assert!(result.errors.iter().any(|e| e.contains("_verifySignature")));
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
}
