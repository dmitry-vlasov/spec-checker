use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use crate::extractors::ExtractedModule;

/// Directory-level spec defaults (loaded from `_defaults.yaml`)
///
/// Fields use `Option` to distinguish "not set" from "set to empty".
/// Scalar fields: child overrides parent.
/// List fields: child merges (union) with parent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpecDefaults {
    #[serde(default)]
    pub language: Option<String>,

    #[serde(default)]
    pub layer: Option<Layer>,

    #[serde(default)]
    pub context: Option<String>,

    #[serde(default)]
    pub stability: Option<Stability>,

    #[serde(default)]
    pub forbidden_deps: Option<Vec<String>>,

    #[serde(default)]
    pub external_deps: Option<Vec<String>>,

    #[serde(default)]
    pub forbidden_external: Option<Vec<String>>,
}

impl SpecDefaults {
    /// Merge two defaults: `self` is the parent, `child` overlays on top.
    /// Scalars: child wins if set. Lists: union of both.
    pub fn merge(&self, child: &SpecDefaults) -> SpecDefaults {
        SpecDefaults {
            language: child.language.clone().or_else(|| self.language.clone()),
            layer: child.layer.clone().or_else(|| self.layer.clone()),
            context: child.context.clone().or_else(|| self.context.clone()),
            stability: child.stability.or(self.stability),
            forbidden_deps: merge_optional_vecs(&self.forbidden_deps, &child.forbidden_deps),
            external_deps: merge_optional_vecs(&self.external_deps, &child.external_deps),
            forbidden_external: merge_optional_vecs(
                &self.forbidden_external,
                &child.forbidden_external,
            ),
        }
    }
}

/// Merge two optional Vec<String>: union of both, preserving order.
fn merge_optional_vecs(
    parent: &Option<Vec<String>>,
    child: &Option<Vec<String>>,
) -> Option<Vec<String>> {
    match (parent, child) {
        (None, None) => None,
        (Some(p), None) => Some(p.clone()),
        (None, Some(c)) => Some(c.clone()),
        (Some(p), Some(c)) => {
            let mut merged = p.clone();
            for item in c {
                if !merged.contains(item) {
                    merged.push(item.clone());
                }
            }
            Some(merged)
        }
    }
}

/// Compute the effective defaults for a spec file by walking up the directory
/// tree from `spec_dir` to `root_dir`, collecting `_defaults.yaml` at each level.
pub fn resolve_defaults(spec_dir: &Path, root_dir: &Path) -> SpecDefaults {
    // Collect defaults files from root down to spec_dir
    let mut chain: Vec<SpecDefaults> = Vec::new();

    // Build list of directories from root to spec_dir
    let mut dirs: Vec<&Path> = Vec::new();
    let mut current = spec_dir;
    loop {
        dirs.push(current);
        if current == root_dir {
            break;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent,
            _ => break,
        }
    }
    dirs.reverse(); // root first

    for dir in dirs {
        let defaults_path = dir.join("_defaults.yaml");
        if defaults_path.is_file() {
            if let Ok(content) = std::fs::read_to_string(&defaults_path) {
                if let Ok(defaults) = serde_yaml::from_str::<SpecDefaults>(&content) {
                    chain.push(defaults);
                }
            }
        }
    }

    // Merge chain: root first, then each child overlays
    let mut effective = SpecDefaults::default();
    for defaults in &chain {
        effective = effective.merge(defaults);
    }
    effective
}

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
    pub exposes: HashMap<String, ExposeSpec>,

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

    /// Architectural layer (vertical stratification)
    #[serde(default)]
    pub layer: Option<Layer>,

    /// Bounded context (horizontal segmentation)
    #[serde(default)]
    pub context: Option<String>,

    /// Stability level (change frequency)
    #[serde(default)]
    pub stability: Option<Stability>,

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

/// Specification for an exposed entity (function, type, trait, etc.)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExposeSpec {
    /// Entity kind: "function", "struct", "enum", "trait", "type"
    /// If omitted, defaults to "function" for backwards compatibility.
    #[serde(default)]
    pub kind: Option<String>,

    /// Function signature (legacy, for backward compatibility)
    #[serde(default)]
    pub signature: Option<String>,

    /// Preconditions (requires) — functions
    #[serde(default)]
    pub requires: Vec<String>,

    /// Postconditions (ensures) — functions and types
    #[serde(default)]
    pub ensures: Vec<String>,

    /// Modifies clause (state changes) — functions
    #[serde(default)]
    pub modifies: Vec<String>,

    /// Events emitted — functions
    #[serde(default)]
    pub emits: Vec<String>,

    /// Allowed callers — functions
    #[serde(default)]
    pub callable_by: Vec<String>,

    /// Function visibility (public, external, internal, private)
    #[serde(default)]
    pub visibility: Option<String>,

    /// Type formula constraints
    #[serde(default)]
    pub type_constraints: Vec<String>,
}

/// Architectural layer — a user-defined string (e.g. "infrastructure", "domain").
///
/// Layer names are always lowercased for consistent comparison.
/// Dependency rules between layers are defined in `LayerConfig`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Layer(pub String);

impl Layer {
    #[allow(dead_code)]
    pub fn new(name: &str) -> Self {
        Layer(name.to_lowercase())
    }
}

/// Configurable layer dependency rules (a DAG).
///
/// Each layer maps to the set of layers it is allowed to depend on.
/// Same-layer dependencies are always allowed.
/// Unknown layers (not in config) are allowed by default.
#[derive(Debug, Clone)]
pub struct LayerConfig {
    allowed_deps: HashMap<String, std::collections::HashSet<String>>,
}

impl LayerConfig {
    /// The built-in 4-layer model (default when no custom config).
    pub fn builtin() -> Self {
        use std::collections::HashSet;
        let mut allowed: HashMap<String, HashSet<String>> = HashMap::new();

        allowed.insert("infrastructure".into(), HashSet::new());
        allowed.insert(
            "domain".into(),
            ["infrastructure"].into_iter().map(String::from).collect(),
        );
        allowed.insert(
            "application".into(),
            ["domain", "infrastructure"]
                .into_iter()
                .map(String::from)
                .collect(),
        );
        allowed.insert(
            "interface".into(),
            ["application", "domain", "infrastructure"]
                .into_iter()
                .map(String::from)
                .collect(),
        );

        LayerConfig { allowed_deps: allowed }
    }

    /// Build from a linear list: each layer can depend on all layers below it.
    pub fn from_linear(layers: &[String]) -> Self {
        use std::collections::HashSet;
        let mut allowed = HashMap::new();
        for (i, layer) in layers.iter().enumerate() {
            let deps: HashSet<String> = layers[i + 1..].iter().map(|s| s.to_lowercase()).collect();
            allowed.insert(layer.to_lowercase(), deps);
        }
        LayerConfig { allowed_deps: allowed }
    }

    /// Build from explicit DAG: each layer lists its allowed dependencies.
    pub fn from_dag(dag: &HashMap<String, Vec<String>>) -> Self {
        let mut allowed = HashMap::new();
        for (layer, deps) in dag {
            allowed.insert(
                layer.to_lowercase(),
                deps.iter().map(|s| s.to_lowercase()).collect(),
            );
        }
        LayerConfig { allowed_deps: allowed }
    }

    /// Check if `source` layer can depend on `target` layer.
    pub fn can_depend_on(&self, source: &str, target: &str) -> bool {
        let source = source.to_lowercase();
        let target = target.to_lowercase();
        // Same layer is always OK
        if source == target {
            return true;
        }
        // Check config; unknown layers are allowed by default
        self.allowed_deps
            .get(&source)
            .map(|deps| deps.contains(&target))
            .unwrap_or(true)
    }
}

/// Stability levels (stable modules shouldn't depend on volatile ones)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stability {
    /// Core abstractions, rarely change
    Stable,
    /// Normal modules
    Normal,
    /// Features, frequently change
    Volatile,
}

impl Stability {
    /// Check if this stability level can depend on another.
    /// Stable modules should not depend on volatile ones.
    pub fn can_depend_on(&self, other: &Stability) -> bool {
        use Stability::*;
        match (self, other) {
            // Stable can only depend on stable
            (Stable, Stable) => true,
            (Stable, _) => false,
            // Normal can depend on stable or normal
            (Normal, Volatile) => false,
            (Normal, _) => true,
            // Volatile can depend on anything
            (Volatile, _) => true,
        }
    }
}

impl ModuleSpec {
    /// Apply directory-level defaults to this spec.
    /// Scalars: only fill in if the spec doesn't already set them.
    /// Lists: merge (union) defaults into the spec's lists.
    pub fn apply_defaults(&mut self, defaults: &SpecDefaults) {
        if self.language.is_none() {
            self.language = defaults.language.clone();
        }
        if self.layer.is_none() {
            self.layer = defaults.layer.clone();
        }
        if self.context.is_none() {
            self.context = defaults.context.clone();
        }
        if self.stability.is_none() {
            self.stability = defaults.stability;
        }
        if let Some(ref deps) = defaults.forbidden_deps {
            for dep in deps {
                if !self.forbidden_deps.contains(dep) {
                    self.forbidden_deps.push(dep.clone());
                }
            }
        }
        if let Some(ref deps) = defaults.external_deps {
            for dep in deps {
                if !self.external_deps.contains(dep) {
                    self.external_deps.push(dep.clone());
                }
            }
        }
        if let Some(ref deps) = defaults.forbidden_external {
            for dep in deps {
                if !self.forbidden_external.contains(dep) {
                    self.forbidden_external.push(dep.clone());
                }
            }
        }
    }

    /// Create a spec from extracted module information
    pub fn from_extracted(extracted: &ExtractedModule) -> Self {
        let mut exposes = HashMap::new();

        for func in &extracted.public_functions {
            let func_spec = ExposeSpec {
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
            depends_on: Vec::new(),
            forbidden_deps: Vec::new(),
            external_deps: extracted.imports.clone(),
            forbidden_external: Vec::new(),
            layer: None,
            context: None,
            stability: None,
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
    fn test_defaults_merge_scalars() {
        let parent = SpecDefaults {
            language: Some("rust".to_string()),
            layer: Some(Layer::new("domain")),
            context: Some("core".to_string()),
            stability: Some(Stability::Stable),
            ..Default::default()
        };
        let child = SpecDefaults {
            layer: Some(Layer::new("infrastructure")),
            context: Some("infra".to_string()),
            ..Default::default()
        };

        let merged = parent.merge(&child);
        assert_eq!(merged.language, Some("rust".to_string())); // parent (child None)
        assert_eq!(merged.layer, Some(Layer::new("infrastructure"))); // child overrides
        assert_eq!(merged.context, Some("infra".to_string())); // child overrides
        assert_eq!(merged.stability, Some(Stability::Stable)); // parent (child None)
    }

    #[test]
    fn test_defaults_merge_lists() {
        let parent = SpecDefaults {
            forbidden_deps: Some(vec!["clap".to_string(), "syn".to_string()]),
            ..Default::default()
        };
        let child = SpecDefaults {
            forbidden_deps: Some(vec!["syn".to_string(), "regex".to_string()]),
            ..Default::default()
        };

        let merged = parent.merge(&child);
        let forbidden = merged.forbidden_deps.unwrap();
        assert_eq!(forbidden.len(), 3); // clap, syn, regex (deduplicated)
        assert!(forbidden.contains(&"clap".to_string()));
        assert!(forbidden.contains(&"syn".to_string()));
        assert!(forbidden.contains(&"regex".to_string()));
    }

    #[test]
    fn test_apply_defaults_fills_missing() {
        let defaults = SpecDefaults {
            language: Some("rust".to_string()),
            layer: Some(Layer::new("domain")),
            context: Some("core".to_string()),
            stability: Some(Stability::Stable),
            forbidden_deps: Some(vec!["clap".to_string()]),
            ..Default::default()
        };

        let mut spec = ModuleSpec {
            module: "test".to_string(),
            // language, layer, context, stability all None
            forbidden_deps: vec!["syn".to_string()],
            ..Default::default()
        };

        spec.apply_defaults(&defaults);

        assert_eq!(spec.language, Some("rust".to_string()));
        assert_eq!(spec.layer, Some(Layer::new("domain")));
        assert_eq!(spec.context, Some("core".to_string()));
        assert_eq!(spec.stability, Some(Stability::Stable));
        // Lists merge
        assert!(spec.forbidden_deps.contains(&"clap".to_string()));
        assert!(spec.forbidden_deps.contains(&"syn".to_string()));
    }

    #[test]
    fn test_apply_defaults_no_override() {
        let defaults = SpecDefaults {
            language: Some("rust".to_string()),
            layer: Some(Layer::new("domain")),
            ..Default::default()
        };

        let mut spec = ModuleSpec {
            module: "test".to_string(),
            language: Some("solidity".to_string()),
            layer: Some(Layer::new("interface")),
            ..Default::default()
        };

        spec.apply_defaults(&defaults);

        // Spec's own values are preserved
        assert_eq!(spec.language, Some("solidity".to_string()));
        assert_eq!(spec.layer, Some(Layer::new("interface")));
    }

    #[test]
    fn test_resolve_defaults_chain() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();

        // Create root _defaults.yaml
        std::fs::write(
            root.join("_defaults.yaml"),
            "language: rust\nstability: stable\nforbidden_deps:\n  - clap\n",
        )
        .unwrap();

        // Create subdir with overriding _defaults.yaml
        let sub = root.join("extractors");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(
            sub.join("_defaults.yaml"),
            "layer: infrastructure\nforbidden_deps:\n  - serde_yaml\n",
        )
        .unwrap();

        let effective = resolve_defaults(&sub, root);
        assert_eq!(effective.language, Some("rust".to_string())); // from root
        assert_eq!(effective.stability, Some(Stability::Stable)); // from root
        assert_eq!(effective.layer, Some(Layer::new("infrastructure"))); // from subdir
        let forbidden = effective.forbidden_deps.unwrap();
        assert!(forbidden.contains(&"clap".to_string())); // from root
        assert!(forbidden.contains(&"serde_yaml".to_string())); // from subdir
    }

    #[test]
    fn test_builtin_layer_config() {
        let config = LayerConfig::builtin();

        // Infrastructure can only depend on itself
        assert!(config.can_depend_on("infrastructure", "infrastructure"));
        assert!(!config.can_depend_on("infrastructure", "domain"));
        assert!(!config.can_depend_on("infrastructure", "application"));
        assert!(!config.can_depend_on("infrastructure", "interface"));

        // Domain can depend on itself and infrastructure
        assert!(config.can_depend_on("domain", "infrastructure"));
        assert!(config.can_depend_on("domain", "domain"));
        assert!(!config.can_depend_on("domain", "application"));
        assert!(!config.can_depend_on("domain", "interface"));

        // Application can depend on domain and infrastructure
        assert!(config.can_depend_on("application", "infrastructure"));
        assert!(config.can_depend_on("application", "domain"));
        assert!(config.can_depend_on("application", "application"));
        assert!(!config.can_depend_on("application", "interface"));

        // Interface can depend on anything
        assert!(config.can_depend_on("interface", "infrastructure"));
        assert!(config.can_depend_on("interface", "domain"));
        assert!(config.can_depend_on("interface", "application"));
        assert!(config.can_depend_on("interface", "interface"));
    }

    #[test]
    fn test_custom_layer_config_linear() {
        let layers = vec![
            "presentation".to_string(),
            "business".to_string(),
            "data".to_string(),
        ];
        let config = LayerConfig::from_linear(&layers);

        assert!(config.can_depend_on("presentation", "business"));
        assert!(config.can_depend_on("presentation", "data"));
        assert!(config.can_depend_on("business", "data"));
        assert!(!config.can_depend_on("data", "business"));
        assert!(!config.can_depend_on("data", "presentation"));
        assert!(!config.can_depend_on("business", "presentation"));
    }

    #[test]
    fn test_custom_layer_config_dag() {
        // Diamond: A depends on B and C, B and C both depend on D
        let mut dag = HashMap::new();
        dag.insert("a".to_string(), vec!["b".to_string(), "c".to_string()]);
        dag.insert("b".to_string(), vec!["d".to_string()]);
        dag.insert("c".to_string(), vec!["d".to_string()]);
        dag.insert("d".to_string(), vec![]);

        let config = LayerConfig::from_dag(&dag);

        assert!(config.can_depend_on("a", "b"));
        assert!(config.can_depend_on("a", "c"));
        assert!(!config.can_depend_on("a", "d")); // not transitive — explicit only
        assert!(config.can_depend_on("b", "d"));
        assert!(!config.can_depend_on("d", "a"));
        assert!(!config.can_depend_on("d", "b"));
    }

    #[test]
    fn test_unknown_layers_allowed() {
        let config = LayerConfig::builtin();
        // Unknown layers not in config are allowed by default
        assert!(config.can_depend_on("custom", "anything"));
    }

    #[test]
    fn test_stability_dependencies() {
        use Stability::*;

        // Stable can only depend on stable
        assert!(Stable.can_depend_on(&Stable));
        assert!(!Stable.can_depend_on(&Normal));
        assert!(!Stable.can_depend_on(&Volatile));

        // Normal can depend on stable and normal
        assert!(Normal.can_depend_on(&Stable));
        assert!(Normal.can_depend_on(&Normal));
        assert!(!Normal.can_depend_on(&Volatile));

        // Volatile can depend on anything
        assert!(Volatile.can_depend_on(&Stable));
        assert!(Volatile.can_depend_on(&Normal));
        assert!(Volatile.can_depend_on(&Volatile));
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
        assert_eq!(spec.depends_on.len(), 2);
        assert_eq!(spec.forbidden_deps.len(), 1);
        assert_eq!(spec.invariants.len(), 1);
    }
}
