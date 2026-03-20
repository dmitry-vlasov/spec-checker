use anyhow::{Context, Result};
use quote::ToTokens;
use std::path::PathBuf;
use syn::{visit::Visit, File, Item, ItemFn, UseTree, Visibility};

use super::{ExtractedModule, Extractor};

pub struct RustExtractor;

impl RustExtractor {
    pub fn new() -> Self {
        Self
    }
}

impl Extractor for RustExtractor {
    fn extract(&self, path: &PathBuf) -> Result<ExtractedModule> {
        let content = std::fs::read_to_string(path).context("Failed to read Rust source file")?;

        let syntax: File = syn::parse_file(&content).context("Failed to parse Rust source file")?;

        let mut visitor = RustVisitor::new(path);
        visitor.visit_file(&syntax);

        Ok(visitor.into_module())
    }
}

/// AST visitor that extracts module information
struct RustVisitor {
    module: ExtractedModule,
    in_test_module: bool,
}

impl RustVisitor {
    fn new(path: &PathBuf) -> Self {
        Self {
            module: ExtractedModule {
                name: path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string(),
                language: "rust".to_string(),
                source_path: Some(path.to_string_lossy().to_string()),
                ..Default::default()
            },
            in_test_module: false,
        }
    }

    fn into_module(self) -> ExtractedModule {
        self.module
    }

    fn is_public(vis: &Visibility) -> bool {
        matches!(vis, Visibility::Public(_))
    }

    /// Flatten a UseTree into a list of import paths
    fn flatten_use_tree(prefix: &str, tree: &UseTree) -> Vec<String> {
        match tree {
            UseTree::Path(path) => {
                let new_prefix = if prefix.is_empty() {
                    path.ident.to_string()
                } else {
                    format!("{}::{}", prefix, path.ident)
                };
                Self::flatten_use_tree(&new_prefix, &path.tree)
            }
            UseTree::Name(name) => {
                let full_path = if prefix.is_empty() {
                    name.ident.to_string()
                } else {
                    format!("{}::{}", prefix, name.ident)
                };
                vec![full_path]
            }
            UseTree::Rename(rename) => {
                let full_path = if prefix.is_empty() {
                    rename.ident.to_string()
                } else {
                    format!("{}::{}", prefix, rename.ident)
                };
                vec![full_path]
            }
            UseTree::Glob(_) => {
                vec![format!("{}::*", prefix)]
            }
            UseTree::Group(group) => {
                let mut results = Vec::new();
                for item in &group.items {
                    results.extend(Self::flatten_use_tree(prefix, item));
                }
                results
            }
        }
    }

    /// Get the root crate of an import path
    fn get_root_crate(path: &str) -> Option<String> {
        let parts: Vec<&str> = path.split("::").collect();
        if parts.is_empty() {
            return None;
        }

        let root = parts[0];

        // Skip internal imports
        if root == "crate" || root == "super" || root == "self" {
            return None;
        }

        // Skip std library
        if root == "std" || root == "core" || root == "alloc" {
            return None;
        }

        Some(root.to_string())
    }
}

impl<'ast> Visit<'ast> for RustVisitor {
    fn visit_item(&mut self, item: &'ast Item) {
        // Skip items in test modules
        if self.in_test_module {
            return;
        }

        match item {
            Item::Use(item_use) => {
                let paths = Self::flatten_use_tree("", &item_use.tree);
                for path in paths {
                    if let Some(root) = Self::get_root_crate(&path) {
                        if !self.module.imports.contains(&root) {
                            self.module.imports.push(root);
                        }
                    }
                }
            }
            Item::Fn(item_fn) => {
                let name = item_fn.sig.ident.to_string();
                let sig = self.format_fn_signature(item_fn);

                self.module.function_signatures.insert(name.clone(), sig);

                if Self::is_public(&item_fn.vis) {
                    self.module.public_functions.push(name);
                } else {
                    self.module.private_functions.push(name);
                }
            }
            Item::Struct(item_struct) => {
                self.module
                    .state_variables
                    .push(item_struct.ident.to_string());
            }
            Item::Mod(item_mod) => {
                // Check if this is a test module
                let is_test = item_mod.attrs.iter().any(|attr| {
                    attr.path().segments.iter().any(|seg| seg.ident == "cfg")
                        && attr.to_token_stream().to_string().contains("test")
                });

                if is_test {
                    // Don't visit test module contents
                    return;
                }

                // Visit the module's content
                if let Some((_, items)) = &item_mod.content {
                    for item in items {
                        self.visit_item(item);
                    }
                }
            }
            Item::Impl(item_impl) => {
                // Extract methods from impl blocks
                for impl_item in &item_impl.items {
                    if let syn::ImplItem::Fn(method) = impl_item {
                        let name = method.sig.ident.to_string();
                        let sig = self.format_method_signature(method);

                        self.module.function_signatures.insert(name.clone(), sig);

                        if Self::is_public(&method.vis) {
                            if !self.module.public_functions.contains(&name) {
                                self.module.public_functions.push(name);
                            }
                        } else if !self.module.private_functions.contains(&name)
                            && !self.module.public_functions.contains(&name)
                        {
                            self.module.private_functions.push(name);
                        }
                    }
                }
            }
            _ => {
                // Visit children for other item types
                syn::visit::visit_item(self, item);
            }
        }
    }
}

impl RustVisitor {
    fn format_fn_signature(&self, item_fn: &ItemFn) -> String {
        let params: Vec<String> = item_fn
            .sig
            .inputs
            .iter()
            .map(|arg| match arg {
                syn::FnArg::Receiver(r) => {
                    if r.reference.is_some() {
                        if r.mutability.is_some() {
                            "&mut self".to_string()
                        } else {
                            "&self".to_string()
                        }
                    } else {
                        "self".to_string()
                    }
                }
                syn::FnArg::Typed(pat_type) => {
                    format!("{}", quote::quote!(#pat_type))
                }
            })
            .collect();

        format!("({})", params.join(", "))
    }

    fn format_method_signature(&self, method: &syn::ImplItemFn) -> String {
        let params: Vec<String> = method
            .sig
            .inputs
            .iter()
            .map(|arg| match arg {
                syn::FnArg::Receiver(r) => {
                    if r.reference.is_some() {
                        if r.mutability.is_some() {
                            "&mut self".to_string()
                        } else {
                            "&self".to_string()
                        }
                    } else {
                        "self".to_string()
                    }
                }
                syn::FnArg::Typed(pat_type) => {
                    format!("{}", quote::quote!(#pat_type))
                }
            })
            .collect();

        format!("({})", params.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_extract_rust_module() {
        let content = r#"
use std::collections::HashMap;
use alloy::{
    network::EthereumWallet,
    primitives::{Address, Bytes},
    providers::{Provider, ProviderBuilder},
};
use anyhow::Result;

pub struct Bridge {
    deposited: HashMap<String, u64>,
    admin: String,
}

impl Bridge {
    pub fn new(admin: String) -> Self {
        Self {
            deposited: HashMap::new(),
            admin,
        }
    }

    pub async fn deposit(&mut self, token: &str, amount: u64) -> Result<()> {
        Ok(())
    }

    pub fn withdraw(&mut self, token: &str, amount: u64, signature: &[u8]) -> Result<()> {
        self.verify_signature(signature)?;
        self.execute_transfer(token, amount)?;
        Ok(())
    }

    fn verify_signature(&self, signature: &[u8]) -> Result<()> {
        Ok(())
    }

    fn execute_transfer(&mut self, token: &str, amount: u64) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_utils::mock_stuff;

    #[test]
    fn test_something() {}
}
"#;

        let mut file = NamedTempFile::new().unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let extractor = RustExtractor::new();
        let module = extractor.extract(&file.path().to_path_buf()).unwrap();

        assert_eq!(module.language, "rust");

        // Public functions
        assert!(module.public_functions.contains(&"new".to_string()));
        assert!(module.public_functions.contains(&"deposit".to_string()));
        assert!(module.public_functions.contains(&"withdraw".to_string()));

        // Private functions
        assert!(module
            .private_functions
            .contains(&"verify_signature".to_string()));
        assert!(module
            .private_functions
            .contains(&"execute_transfer".to_string()));

        // Imports - now extracts root crates from nested imports
        assert!(module.imports.contains(&"alloy".to_string()));
        assert!(module.imports.contains(&"anyhow".to_string()));
        // std is filtered out
        assert!(!module.imports.iter().any(|i| i.starts_with("std")));

        // Test imports should be filtered out
        assert!(!module.imports.contains(&"test_utils".to_string()));

        // Structs
        assert!(module.state_variables.contains(&"Bridge".to_string()));
    }

    #[test]
    fn test_flatten_use_tree() {
        // Test nested use parsing
        let content = r#"
use alloy::{
    primitives::{Address, Bytes},
    providers::Provider,
};
"#;
        let syntax: syn::File = syn::parse_file(content).unwrap();

        if let syn::Item::Use(item_use) = &syntax.items[0] {
            let paths = RustVisitor::flatten_use_tree("", &item_use.tree);
            assert!(paths.contains(&"alloy::primitives::Address".to_string()));
            assert!(paths.contains(&"alloy::primitives::Bytes".to_string()));
            assert!(paths.contains(&"alloy::providers::Provider".to_string()));
        } else {
            panic!("Expected use item");
        }
    }
}
