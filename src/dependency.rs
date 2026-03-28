//! Multi-project dependency graph resolution.
//!
//! Each project declares dependencies in `.spec-checker.yaml`:
//! ```yaml
//! name: my-project
//! dependencies:
//!   lib-a:
//!     path: ../lib-a
//! ```
//!
//! The resolver walks dependencies transitively, detects cycles,
//! deduplicates diamonds by canonical path, and prefixes specs
//! with `project_name::`.

use anyhow::{bail, Context, Result};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use crate::spec::{ModuleSpec, SubsystemSpec};

/// A resolved project node in the dependency graph.
#[derive(Debug, Clone)]
pub struct ResolvedProject {
    /// Canonical project name (from `name:` in config)
    pub name: String,
    /// Canonical filesystem root
    pub root: PathBuf,
    /// Loaded module specs (un-prefixed, local to this project)
    pub specs: Vec<ModuleSpec>,
    /// Loaded subsystem specs
    pub subsystems: Vec<SubsystemSpec>,
    /// Explicit public module list from config (None = use subsystems or all-public fallback)
    pub public_modules: Option<Vec<String>>,
    /// Direct dependency names (must match keys in the graph)
    pub dependency_names: Vec<String>,
}

impl ResolvedProject {
    /// Check whether a module (by source_path or module name) is public.
    ///
    /// Resolution order:
    /// 1. If `public_modules` is set in config, use that list.
    /// 2. If subsystems exist, the union of all subsystem `modules` is public.
    /// 3. Otherwise, everything is public.
    pub fn is_module_public(&self, module_ref: &str) -> bool {
        // 1. Explicit public_modules list
        if let Some(ref public) = self.public_modules {
            return public.iter().any(|m| {
                m == module_ref || extract_module_name(m) == module_ref
            });
        }

        // 2. Subsystem-based: union of all subsystem member modules
        if !self.subsystems.is_empty() {
            return self.subsystems.iter().any(|sub| {
                sub.modules.iter().any(|m| {
                    m == module_ref || extract_module_name(m) == module_ref
                })
            });
        }

        // 3. Fallback: all public
        true
    }
}

/// The full resolved dependency graph.
#[derive(Debug)]
pub struct DependencyGraph {
    /// Projects in topological order (dependencies first, root project last).
    pub projects: Vec<ResolvedProject>,
    /// name → index into `projects`
    pub index: HashMap<String, usize>,
}

impl DependencyGraph {
    /// Build the dependency graph starting from a root project directory.
    ///
    /// Performs transitive resolution, cycle detection, and diamond
    /// deduplication by canonical filesystem path.
    pub fn resolve(root: &Path) -> Result<Self> {
        let root = std::fs::canonicalize(root)
            .with_context(|| format!("cannot canonicalize project root: {}", root.display()))?;

        // canonical_path → (name, index in `projects`)
        let mut visited: HashMap<PathBuf, usize> = HashMap::new();
        // Cycle detection: paths currently on the DFS stack
        let mut in_stack: HashSet<PathBuf> = HashSet::new();

        let mut projects: Vec<ResolvedProject> = Vec::new();

        Self::resolve_recursive(
            &root,
            None, // root project: name comes from its own config
            &mut projects,
            &mut visited,
            &mut in_stack,
        )?;

        // Build index
        let index: HashMap<String, usize> = projects
            .iter()
            .enumerate()
            .map(|(i, p)| (p.name.clone(), i))
            .collect();

        // Topological sort (dependencies first)
        let sorted = Self::toposort(&projects, &index)?;

        Ok(DependencyGraph {
            projects: sorted,
            index: projects
                .iter()
                .enumerate()
                .map(|(i, p)| (p.name.clone(), i))
                .collect(),
        })
    }

    /// Recursive DFS resolver. Returns the index of the resolved project.
    fn resolve_recursive(
        dir: &Path,
        expected_name: Option<&str>,
        projects: &mut Vec<ResolvedProject>,
        visited: &mut HashMap<PathBuf, usize>,
        in_stack: &mut HashSet<PathBuf>,
    ) -> Result<usize> {
        let canonical = std::fs::canonicalize(dir)
            .with_context(|| format!("cannot canonicalize: {}", dir.display()))?;

        // Diamond dedup: already resolved this path
        if let Some(&idx) = visited.get(&canonical) {
            if let Some(expected) = expected_name {
                if projects[idx].name != expected {
                    bail!(
                        "Diamond dependency conflict: '{}' at {} is also referenced as '{}'",
                        projects[idx].name,
                        canonical.display(),
                        expected,
                    );
                }
            }
            return Ok(idx);
        }

        // Cycle detection
        if !in_stack.insert(canonical.clone()) {
            bail!("Dependency cycle detected involving: {}", canonical.display());
        }

        // Load this project's config
        let config = crate::load_project_config_at(&canonical);

        // Determine canonical name
        let name = config
            .name
            .clone()
            .unwrap_or_else(|| {
                canonical
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "unnamed".to_string())
            });

        // Verify name matches what consumer expects
        if let Some(expected) = expected_name {
            if name != expected {
                bail!(
                    "Dependency name mismatch: consumer expects '{}' but {} declares name '{}'",
                    expected,
                    canonical.display(),
                    name,
                );
            }
        }

        // Reserve slot for this project
        let idx = projects.len();
        projects.push(ResolvedProject {
            name: name.clone(),
            root: canonical.clone(),
            specs: Vec::new(),
            subsystems: Vec::new(),
            public_modules: config.public_modules.clone(),
            dependency_names: config.dependencies.keys().cloned().collect(),
        });
        visited.insert(canonical.clone(), idx);

        // Recurse into dependencies
        for (dep_name, dep_entry) in &config.dependencies {
            let dep_path = if dep_entry.path.is_absolute() {
                dep_entry.path.clone()
            } else {
                canonical.join(&dep_entry.path)
            };

            Self::resolve_recursive(
                &dep_path,
                Some(dep_name),
                projects,
                visited,
                in_stack,
            )?;
        }

        // Load specs and subsystems for this project
        let spec_dir = canonical.join("specs");
        if spec_dir.is_dir() {
            projects[idx].specs = load_specs_from_dir(&spec_dir)?;
            projects[idx].subsystems = load_subsystems_from_dir(&spec_dir);
        }

        in_stack.remove(&canonical);
        Ok(idx)
    }

    /// Topological sort: dependencies first, root project last.
    fn toposort(
        projects: &[ResolvedProject],
        index: &HashMap<String, usize>,
    ) -> Result<Vec<ResolvedProject>> {
        let n = projects.len();
        let mut in_degree = vec![0usize; n];
        let mut edges: Vec<Vec<usize>> = vec![Vec::new(); n];

        for (i, proj) in projects.iter().enumerate() {
            for dep_name in &proj.dependency_names {
                if let Some(&j) = index.get(dep_name) {
                    edges[i].push(j);
                    in_degree[j] += 1;
                }
            }
        }

        // Kahn's algorithm
        // in_degree[j] = how many projects depend on j
        // We remove nodes with in_degree 0 first (= consumers / leaves)
        // Then reverse to get dependencies first
        let mut queue: VecDeque<usize> = VecDeque::new();
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

        if order.len() < n {
            let in_cycle: Vec<&str> = (0..n)
                .filter(|i| in_degree[*i] > 0)
                .map(|i| projects[i].name.as_str())
                .collect();
            bail!("Dependency cycle among projects: {}", in_cycle.join(", "));
        }

        // Reverse: dependencies first
        order.reverse();
        Ok(order.into_iter().map(|i| projects[i].clone()).collect())
    }

    /// Get a project by name
    pub fn get(&self, name: &str) -> Option<&ResolvedProject> {
        self.index.get(name).map(|&i| &self.projects[i])
    }

    /// Get the root project (last in topological order)
    pub fn root_project(&self) -> &ResolvedProject {
        self.projects.last().expect("dependency graph is non-empty")
    }

    /// Parse a cross-project reference like "russell-lib::parser" into (project, local_ref).
    /// Returns None for local references (no `::` delimiter).
    pub fn parse_cross_ref(reference: &str) -> Option<(&str, &str)> {
        reference.split_once("::")
    }

    /// Iterate all dependency projects (excluding the root)
    pub fn dependencies(&self) -> &[ResolvedProject] {
        if self.projects.len() > 1 {
            &self.projects[..self.projects.len() - 1]
        } else {
            &[]
        }
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Extract a bare module name from a source path: "src/checker.rs" → "checker"
fn extract_module_name(path: &str) -> String {
    let path = path.trim_end_matches(".rs");
    if path.ends_with("/mod") {
        return path
            .trim_end_matches("/mod")
            .rsplit('/')
            .next()
            .unwrap_or(path)
            .to_string();
    }
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// Load module specs from a directory (reuses the same glob logic as main.rs)
fn load_specs_from_dir(spec_dir: &Path) -> Result<Vec<ModuleSpec>> {
    use crate::spec::resolve_defaults;

    let mut specs = Vec::new();
    let root = spec_dir;

    for ext in &["yaml", "yml"] {
        let pattern = format!("{}/**/*.spec.{}", spec_dir.display(), ext);
        for entry in glob::glob(&pattern)? {
            let entry = entry?;
            let content = std::fs::read_to_string(&entry)?;
            let mut spec: ModuleSpec = serde_yaml::from_str(&content)
                .with_context(|| format!("failed to parse {}", entry.display()))?;

            if let Some(dir) = entry.parent() {
                let defaults = resolve_defaults(dir, root);
                spec.apply_defaults(&defaults);
            }
            specs.push(spec);
        }
    }

    Ok(specs)
}

/// Load subsystem specs from a directory
fn load_subsystems_from_dir(spec_dir: &Path) -> Vec<SubsystemSpec> {
    let mut subsystems = Vec::new();
    let pattern = format!("{}/**/*.subsystem.yaml", spec_dir.display());

    if let Ok(entries) = glob::glob(&pattern) {
        for entry in entries.flatten() {
            if let Ok(content) = std::fs::read_to_string(&entry) {
                match serde_yaml::from_str::<SubsystemSpec>(&content) {
                    Ok(sub) => subsystems.push(sub),
                    Err(e) => {
                        eprintln!("  Warning: failed to parse {}: {}", entry.display(), e);
                    }
                }
            }
        }
    }

    subsystems
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_config(dir: &Path, name: &str, deps: &[(&str, &str)]) {
        let mut yaml = format!("name: {}\n", name);
        if !deps.is_empty() {
            yaml.push_str("dependencies:\n");
            for (dep_name, dep_path) in deps {
                yaml.push_str(&format!("  {}:\n    path: {}\n", dep_name, dep_path));
            }
        }
        fs::write(dir.join(".spec-checker.yaml"), yaml).unwrap();
    }

    fn write_spec(dir: &Path, module: &str) {
        let spec_dir = dir.join("specs");
        fs::create_dir_all(&spec_dir).unwrap();
        let yaml = format!("module: {}\nsource_path: src/{}.rs\n", module, module);
        fs::write(spec_dir.join(format!("{}.spec.yaml", module)), yaml).unwrap();
    }

    #[test]
    fn test_single_project_no_deps() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_config(root, "my-project", &[]);
        write_spec(root, "main");

        let graph = DependencyGraph::resolve(root).unwrap();
        assert_eq!(graph.projects.len(), 1);
        assert_eq!(graph.root_project().name, "my-project");
        assert_eq!(graph.root_project().specs.len(), 1);
    }

    #[test]
    fn test_linear_dependency() {
        let tmp = TempDir::new().unwrap();
        let lib_dir = tmp.path().join("lib-a");
        let root_dir = tmp.path().join("root");
        fs::create_dir_all(&lib_dir).unwrap();
        fs::create_dir_all(&root_dir).unwrap();

        write_config(&lib_dir, "lib-a", &[]);
        write_spec(&lib_dir, "parser");

        write_config(&root_dir, "my-project", &[("lib-a", "../lib-a")]);
        write_spec(&root_dir, "main");

        let graph = DependencyGraph::resolve(&root_dir).unwrap();
        assert_eq!(graph.projects.len(), 2);
        // Dependencies first in topological order
        assert_eq!(graph.projects[0].name, "lib-a");
        assert_eq!(graph.projects[1].name, "my-project");
    }

    #[test]
    fn test_diamond_dedup() {
        let tmp = TempDir::new().unwrap();
        let shared = tmp.path().join("shared");
        let lib_a = tmp.path().join("lib-a");
        let root = tmp.path().join("root");
        fs::create_dir_all(&shared).unwrap();
        fs::create_dir_all(&lib_a).unwrap();
        fs::create_dir_all(&root).unwrap();

        write_config(&shared, "shared", &[]);
        write_spec(&shared, "utils");

        write_config(&lib_a, "lib-a", &[("shared", "../shared")]);
        write_spec(&lib_a, "parser");

        write_config(
            &root,
            "my-project",
            &[("lib-a", "../lib-a"), ("shared", "../shared")],
        );
        write_spec(&root, "main");

        let graph = DependencyGraph::resolve(&root).unwrap();
        // shared is only loaded once despite being referenced by both root and lib-a
        assert_eq!(graph.projects.len(), 3);
        let names: Vec<&str> = graph.projects.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"shared"));
        assert!(names.contains(&"lib-a"));
        assert!(names.contains(&"my-project"));
    }

    #[test]
    fn test_cycle_detected() {
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();

        write_config(&a, "a", &[("b", "../b")]);
        write_config(&b, "b", &[("a", "../a")]);

        let result = DependencyGraph::resolve(&a);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("cycle") || err.contains("Cycle"),
            "Expected cycle error, got: {}",
            err
        );
    }

    #[test]
    fn test_name_mismatch_error() {
        let tmp = TempDir::new().unwrap();
        let lib = tmp.path().join("lib");
        let root = tmp.path().join("root");
        fs::create_dir_all(&lib).unwrap();
        fs::create_dir_all(&root).unwrap();

        write_config(&lib, "actual-name", &[]);
        write_config(&root, "root", &[("wrong-name", "../lib")]);

        let result = DependencyGraph::resolve(&root);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("mismatch"));
    }

    #[test]
    fn test_public_modules_explicit() {
        let proj = ResolvedProject {
            name: "test".to_string(),
            root: PathBuf::from("/tmp/test"),
            specs: vec![],
            subsystems: vec![],
            public_modules: Some(vec!["src/parser.rs".to_string()]),
            dependency_names: vec![],
        };

        assert!(proj.is_module_public("src/parser.rs"));
        assert!(proj.is_module_public("parser")); // by module name
        assert!(!proj.is_module_public("src/internal.rs"));
    }

    #[test]
    fn test_parse_cross_ref() {
        assert_eq!(
            DependencyGraph::parse_cross_ref("lib-a::parser"),
            Some(("lib-a", "parser"))
        );
        assert_eq!(
            DependencyGraph::parse_cross_ref("lib-a::parser.Parser.parse"),
            Some(("lib-a", "parser.Parser.parse"))
        );
        assert_eq!(DependencyGraph::parse_cross_ref("local_module"), None);
    }

    #[test]
    fn test_fallback_all_public() {
        let proj = ResolvedProject {
            name: "test".to_string(),
            root: PathBuf::from("/tmp/test"),
            specs: vec![],
            subsystems: vec![],
            public_modules: None,
            dependency_names: vec![],
        };
        assert!(proj.is_module_public("anything"));
    }
}
