use anyhow::Result;
use std::path::PathBuf;

use crate::extractors::{get_extractor, ExtractedModule};
use crate::spec::ModuleSpec;

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
}

impl SpecChecker {
    pub fn new(source_root: PathBuf) -> Self {
        Self { source_root }
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

        Ok(result)
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
    use crate::spec::FunctionSpec;
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
}
