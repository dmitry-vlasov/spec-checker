use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::extractors::ExtractedModule;

/// A module specification in the unified YAML format
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModuleSpec {
    /// Module name
    pub module: String,

    /// Programming language (solidity, rust, typescript)
    #[serde(default)]
    pub language: Option<String>,

    /// Path to source file(s) relative to source root
    #[serde(default)]
    pub source_path: Option<String>,

    /// Publicly exposed functions with their contracts
    #[serde(default)]
    pub exposes: HashMap<String, FunctionSpec>,

    /// Internal/private functions (should not be exposed)
    #[serde(default)]
    pub internal: Vec<String>,

    /// Allowed module dependencies
    #[serde(default)]
    pub depends_on: Vec<String>,

    /// Forbidden dependencies
    #[serde(default)]
    pub forbidden_deps: Vec<String>,

    /// External package dependencies
    #[serde(default)]
    pub external_deps: Vec<String>,

    /// Forbidden external packages
    #[serde(default)]
    pub forbidden_external: Vec<String>,

    /// Architectural layer
    #[serde(default)]
    pub layer: Option<Layer>,

    /// Module-level invariants
    #[serde(default)]
    pub invariants: Vec<String>,

    /// Events this module emits
    #[serde(default)]
    pub emits: Vec<String>,

    /// Events this module subscribes to
    #[serde(default)]
    pub subscribes: Vec<String>,

    /// State variables owned by this module
    #[serde(default)]
    pub owns_state: Vec<String>,

    /// External state this module reads
    #[serde(default)]
    pub reads_state: Vec<String>,

    /// External state this module modifies
    #[serde(default)]
    pub modifies: Vec<String>,

    /// Who can call this module
    #[serde(default)]
    pub callable_by: Vec<String>,

    /// Role-based access control
    #[serde(default)]
    pub roles: Vec<String>,
}

/// Function-level specification (Design by Contract)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FunctionSpec {
    /// Function signature
    #[serde(default)]
    pub signature: Option<String>,

    /// Preconditions (requires)
    #[serde(default)]
    pub requires: Vec<String>,

    /// Postconditions (ensures)
    #[serde(default)]
    pub ensures: Vec<String>,

    /// Modifies clause (state changes)
    #[serde(default)]
    pub modifies: Vec<String>,

    /// Events emitted
    #[serde(default)]
    pub emits: Vec<String>,

    /// Allowed callers
    #[serde(default)]
    pub callable_by: Vec<String>,

    /// Function visibility (public, external, internal, private)
    #[serde(default)]
    pub visibility: Option<String>,
}

/// Architectural layers (lower cannot import higher)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    Infrastructure,
    Domain,
    Application,
    Interface,
}

impl Layer {
    /// Check if this layer can depend on another layer.
    /// Used for architectural validation (Phase 2).
    #[allow(dead_code)]
    pub fn can_depend_on(&self, other: &Layer) -> bool {
        use Layer::*;
        match (self, other) {
            // Same layer is always OK
            (a, b) if a == b => true,
            // Interface can depend on anything
            (Interface, _) => true,
            // Application can depend on domain and infrastructure
            (Application, Domain | Infrastructure) => true,
            // Domain can only depend on infrastructure
            (Domain, Infrastructure) => true,
            // Infrastructure cannot depend on higher layers
            (Infrastructure, _) => false,
            // Everything else is forbidden
            _ => false,
        }
    }
}

impl ModuleSpec {
    /// Create a spec from extracted module information
    pub fn from_extracted(extracted: &ExtractedModule) -> Self {
        let mut exposes = HashMap::new();

        for func in &extracted.public_functions {
            let func_spec = FunctionSpec {
                signature: extracted.function_signatures.get(func).cloned(),
                visibility: Some("public".to_string()),
                ..Default::default()
            };
            exposes.insert(func.clone(), func_spec);
        }

        Self {
            module: extracted.name.clone(),
            language: Some(extracted.language.clone()),
            source_path: extracted.source_path.clone(),
            exposes,
            internal: extracted.private_functions.clone(),
            depends_on: Vec::new(),
            forbidden_deps: Vec::new(),
            external_deps: extracted.imports.clone(),
            forbidden_external: Vec::new(),
            layer: None,
            invariants: Vec::new(),
            emits: extracted.events.clone(),
            subscribes: Vec::new(),
            owns_state: extracted.state_variables.clone(),
            reads_state: Vec::new(),
            modifies: Vec::new(),
            callable_by: Vec::new(),
            roles: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layer_dependencies() {
        use Layer::*;

        // Infrastructure can only depend on itself
        assert!(Infrastructure.can_depend_on(&Infrastructure));
        assert!(!Infrastructure.can_depend_on(&Domain));
        assert!(!Infrastructure.can_depend_on(&Application));
        assert!(!Infrastructure.can_depend_on(&Interface));

        // Domain can depend on itself and infrastructure
        assert!(Domain.can_depend_on(&Infrastructure));
        assert!(Domain.can_depend_on(&Domain));
        assert!(!Domain.can_depend_on(&Application));
        assert!(!Domain.can_depend_on(&Interface));

        // Application can depend on domain and infrastructure
        assert!(Application.can_depend_on(&Infrastructure));
        assert!(Application.can_depend_on(&Domain));
        assert!(Application.can_depend_on(&Application));
        assert!(!Application.can_depend_on(&Interface));

        // Interface can depend on anything
        assert!(Interface.can_depend_on(&Infrastructure));
        assert!(Interface.can_depend_on(&Domain));
        assert!(Interface.can_depend_on(&Application));
        assert!(Interface.can_depend_on(&Interface));
    }

    #[test]
    fn test_spec_deserialization() {
        let yaml = r#"
module: Bridge
language: solidity
exposes:
  deposit:
    signature: "(address token, uint256 amount, bytes32 receiver)"
    requires:
      - amount > 0
      - token in allowed_tokens
    ensures:
      - deposited' == deposited + amount
  withdraw:
    requires:
      - valid_signature
    ensures:
      - withdrawn' <= deposited
internal:
  - _verifySignature
  - _executeTransfer
depends_on:
  - TokenRegistry
  - SignerRegistry
forbidden_deps:
  - TestUtils
invariants:
  - "forall token: withdrawn[token] <= deposited[token]"
"#;

        let spec: ModuleSpec = serde_yaml::from_str(yaml).unwrap();

        assert_eq!(spec.module, "Bridge");
        assert_eq!(spec.language, Some("solidity".to_string()));
        assert_eq!(spec.exposes.len(), 2);
        assert!(spec.exposes.contains_key("deposit"));
        assert!(spec.exposes.contains_key("withdraw"));
        assert_eq!(spec.internal.len(), 2);
        assert_eq!(spec.depends_on.len(), 2);
        assert_eq!(spec.forbidden_deps.len(), 1);
        assert_eq!(spec.invariants.len(), 1);
    }
}
