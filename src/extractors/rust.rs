use anyhow::{Context, Result};
use quote::ToTokens;
use std::path::PathBuf;
use syn::{visit::Visit, File, Item, ItemFn, UseTree, Visibility};

use super::{ExtractedModule, Extractor};
use crate::types::{
    FieldInfo, FunctionInfo, GenericParam, ParamInfo, TypeInfo, TypeKind, TypeRepr, VariantInfo,
    Visibility as TypeVisibility,
};

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
                let fi = Self::build_function_info(&item_fn.sig);

                self.module.function_signatures.insert(name.clone(), sig);
                self.module.function_info.insert(name.clone(), fi);

                if Self::is_public(&item_fn.vis) {
                    self.module.public_functions.push(name);
                } else {
                    self.module.private_functions.push(name);
                }
            }
            Item::Struct(item_struct) => {
                let name = item_struct.ident.to_string();
                self.module.state_variables.push(name.clone());

                let fields: Vec<FieldInfo> = match &item_struct.fields {
                    syn::Fields::Named(named) => named
                        .named
                        .iter()
                        .map(|f| FieldInfo {
                            name: f.ident.as_ref().map(|i| i.to_string()).unwrap_or_default(),
                            type_repr: Self::syn_type_to_repr(&f.ty),
                            visibility: if Self::is_public(&f.vis) {
                                TypeVisibility::Public
                            } else {
                                TypeVisibility::Private
                            },
                        })
                        .collect(),
                    syn::Fields::Unnamed(unnamed) => unnamed
                        .unnamed
                        .iter()
                        .enumerate()
                        .map(|(i, f)| FieldInfo {
                            name: i.to_string(),
                            type_repr: Self::syn_type_to_repr(&f.ty),
                            visibility: if Self::is_public(&f.vis) {
                                TypeVisibility::Public
                            } else {
                                TypeVisibility::Private
                            },
                        })
                        .collect(),
                    syn::Fields::Unit => vec![],
                };

                let ti = TypeInfo {
                    name: name.clone(),
                    kind: TypeKind::Struct,
                    generics: Self::extract_generics(&item_struct.generics),
                    fields,
                    variants: vec![],
                    trait_impls: vec![],
                    derives: Self::extract_derives(&item_struct.attrs),
                };
                self.module.type_definitions.insert(name, ti);
            }
            Item::Enum(item_enum) => {
                let name = item_enum.ident.to_string();
                self.module.state_variables.push(name.clone());

                let variants: Vec<VariantInfo> = item_enum
                    .variants
                    .iter()
                    .map(|v| {
                        let fields: Vec<FieldInfo> = match &v.fields {
                            syn::Fields::Named(named) => named
                                .named
                                .iter()
                                .map(|f| FieldInfo {
                                    name: f
                                        .ident
                                        .as_ref()
                                        .map(|i| i.to_string())
                                        .unwrap_or_default(),
                                    type_repr: Self::syn_type_to_repr(&f.ty),
                                    visibility: TypeVisibility::Public,
                                })
                                .collect(),
                            syn::Fields::Unnamed(unnamed) => unnamed
                                .unnamed
                                .iter()
                                .enumerate()
                                .map(|(i, f)| FieldInfo {
                                    name: i.to_string(),
                                    type_repr: Self::syn_type_to_repr(&f.ty),
                                    visibility: TypeVisibility::Public,
                                })
                                .collect(),
                            syn::Fields::Unit => vec![],
                        };
                        VariantInfo {
                            name: v.ident.to_string(),
                            fields,
                        }
                    })
                    .collect();

                let ti = TypeInfo {
                    name: name.clone(),
                    kind: TypeKind::Enum,
                    generics: Self::extract_generics(&item_enum.generics),
                    fields: vec![],
                    variants,
                    trait_impls: vec![],
                    derives: Self::extract_derives(&item_enum.attrs),
                };
                self.module.type_definitions.insert(name, ti);
            }
            Item::Trait(item_trait) => {
                let name = item_trait.ident.to_string();
                let ti = TypeInfo {
                    name: name.clone(),
                    kind: TypeKind::Trait,
                    generics: Self::extract_generics(&item_trait.generics),
                    fields: vec![],
                    variants: vec![],
                    trait_impls: item_trait
                        .supertraits
                        .iter()
                        .map(|b| quote::quote!(#b).to_string())
                        .collect(),
                    derives: vec![],
                };
                self.module.type_definitions.insert(name, ti);
            }
            Item::Type(item_type) => {
                let name = item_type.ident.to_string();
                let ti = TypeInfo {
                    name: name.clone(),
                    kind: TypeKind::TypeAlias,
                    generics: Self::extract_generics(&item_type.generics),
                    fields: vec![],
                    variants: vec![],
                    trait_impls: vec![],
                    derives: vec![],
                };
                self.module.type_definitions.insert(name, ti);
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
                // Record trait implementations: `impl Trait for Type`
                if let Some((_, trait_path, _)) = &item_impl.trait_ {
                    let trait_name = trait_path
                        .segments
                        .last()
                        .map(|s| s.ident.to_string())
                        .unwrap_or_default();
                    // Get the type being implemented
                    if let syn::Type::Path(type_path) = item_impl.self_ty.as_ref() {
                        let type_name = type_path
                            .path
                            .segments
                            .last()
                            .map(|s| s.ident.to_string())
                            .unwrap_or_default();
                        if let Some(ti) = self.module.type_definitions.get_mut(&type_name) {
                            if !ti.trait_impls.contains(&trait_name) {
                                ti.trait_impls.push(trait_name);
                            }
                        }
                    }
                }

                // Extract the impl target type name for qualified method names
                let impl_type_name = if let syn::Type::Path(type_path) = item_impl.self_ty.as_ref() {
                    type_path
                        .path
                        .segments
                        .last()
                        .map(|s| s.ident.to_string())
                } else {
                    None
                };

                // Extract methods from impl blocks
                for impl_item in &item_impl.items {
                    if let syn::ImplItem::Fn(method) = impl_item {
                        let bare_name = method.sig.ident.to_string();
                        let qualified_name = match &impl_type_name {
                            Some(type_name) => format!("{}.{}", type_name, bare_name),
                            None => bare_name,
                        };
                        let sig = self.format_method_signature(method);
                        let fi = Self::build_function_info(&method.sig);

                        self.module.function_signatures.insert(qualified_name.clone(), sig);
                        self.module.function_info.insert(qualified_name.clone(), fi);

                        if Self::is_public(&method.vis) {
                            if !self.module.public_functions.contains(&qualified_name) {
                                self.module.public_functions.push(qualified_name);
                            }
                        } else if !self.module.private_functions.contains(&qualified_name)
                            && !self.module.public_functions.contains(&qualified_name)
                        {
                            self.module.private_functions.push(qualified_name);
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
        Self::format_signature(&item_fn.sig)
    }

    fn format_method_signature(&self, method: &syn::ImplItemFn) -> String {
        Self::format_signature(&method.sig)
    }

    /// Convert a `syn::Type` to our `TypeRepr`
    fn syn_type_to_repr(ty: &syn::Type) -> TypeRepr {
        match ty {
            syn::Type::Path(type_path) => {
                // Get the last segment (ignoring path qualifiers like std::collections::)
                let segments: Vec<_> = type_path.path.segments.iter().collect();
                if segments.is_empty() {
                    return TypeRepr::Named("()".into());
                }
                let last = segments.last().unwrap();
                let name = if segments.len() > 1 {
                    // Use full path for qualified types
                    type_path
                        .path
                        .segments
                        .iter()
                        .map(|s| s.ident.to_string())
                        .collect::<Vec<_>>()
                        .join("::")
                } else {
                    last.ident.to_string()
                };

                match &last.arguments {
                    syn::PathArguments::None => TypeRepr::Named(name),
                    syn::PathArguments::AngleBracketed(args) => {
                        let type_args: Vec<TypeRepr> = args
                            .args
                            .iter()
                            .filter_map(|arg| match arg {
                                syn::GenericArgument::Type(t) => Some(Self::syn_type_to_repr(t)),
                                _ => None,
                            })
                            .collect();
                        if type_args.is_empty() {
                            TypeRepr::Named(name)
                        } else {
                            TypeRepr::Applied(Box::new(TypeRepr::Named(name)), type_args)
                        }
                    }
                    syn::PathArguments::Parenthesized(args) => {
                        let params: Vec<TypeRepr> =
                            args.inputs.iter().map(|t| Self::syn_type_to_repr(t)).collect();
                        let ret = match &args.output {
                            syn::ReturnType::Default => TypeRepr::Unit,
                            syn::ReturnType::Type(_, ty) => Self::syn_type_to_repr(ty),
                        };
                        TypeRepr::FnPointer {
                            params,
                            ret: Box::new(ret),
                        }
                    }
                }
            }
            syn::Type::Reference(type_ref) => TypeRepr::Reference {
                mutable: type_ref.mutability.is_some(),
                inner: Box::new(Self::syn_type_to_repr(&type_ref.elem)),
            },
            syn::Type::Tuple(type_tuple) => {
                if type_tuple.elems.is_empty() {
                    TypeRepr::Unit
                } else {
                    TypeRepr::Tuple(
                        type_tuple.elems.iter().map(|t| Self::syn_type_to_repr(t)).collect(),
                    )
                }
            }
            syn::Type::Slice(type_slice) => {
                TypeRepr::Slice(Box::new(Self::syn_type_to_repr(&type_slice.elem)))
            }
            syn::Type::Array(type_array) => {
                let size = match &type_array.len {
                    syn::Expr::Lit(expr_lit) => match &expr_lit.lit {
                        syn::Lit::Int(lit_int) => lit_int.base10_parse::<usize>().unwrap_or(0),
                        _ => 0,
                    },
                    _ => 0,
                };
                TypeRepr::Array(Box::new(Self::syn_type_to_repr(&type_array.elem)), size)
            }
            syn::Type::BareFn(type_fn) => {
                let params: Vec<TypeRepr> = type_fn
                    .inputs
                    .iter()
                    .map(|arg| Self::syn_type_to_repr(&arg.ty))
                    .collect();
                let ret = match &type_fn.output {
                    syn::ReturnType::Default => TypeRepr::Unit,
                    syn::ReturnType::Type(_, ty) => Self::syn_type_to_repr(ty),
                };
                TypeRepr::FnPointer {
                    params,
                    ret: Box::new(ret),
                }
            }
            syn::Type::Paren(type_paren) => Self::syn_type_to_repr(&type_paren.elem),
            syn::Type::Infer(_) => TypeRepr::Infer,
            // Fallback: use token stream as a named type
            _ => TypeRepr::Named(quote::quote!(#ty).to_string()),
        }
    }

    /// Extract generic parameters from syn generics
    fn extract_generics(generics: &syn::Generics) -> Vec<GenericParam> {
        generics
            .params
            .iter()
            .filter_map(|param| match param {
                syn::GenericParam::Type(tp) => {
                    let bounds: Vec<String> = tp
                        .bounds
                        .iter()
                        .map(|b| quote::quote!(#b).to_string())
                        .collect();
                    Some(GenericParam {
                        name: tp.ident.to_string(),
                        bounds,
                    })
                }
                _ => None,
            })
            .collect()
    }

    /// Extract derive macros from attributes
    fn extract_derives(attrs: &[syn::Attribute]) -> Vec<String> {
        let mut derives = Vec::new();
        for attr in attrs {
            if attr.path().is_ident("derive") {
                if let Ok(nested) = attr.parse_args_with(
                    syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated,
                ) {
                    for path in nested {
                        if let Some(ident) = path.get_ident() {
                            derives.push(ident.to_string());
                        } else if let Some(last) = path.segments.last() {
                            derives.push(last.ident.to_string());
                        }
                    }
                }
            }
        }
        derives
    }

    /// Build FunctionInfo from a syn::Signature
    fn build_function_info(sig: &syn::Signature) -> FunctionInfo {
        let params: Vec<ParamInfo> = sig
            .inputs
            .iter()
            .map(|arg| match arg {
                syn::FnArg::Receiver(r) => {
                    let type_repr = if r.reference.is_some() {
                        TypeRepr::Reference {
                            mutable: r.mutability.is_some(),
                            inner: Box::new(TypeRepr::Named("Self".into())),
                        }
                    } else {
                        TypeRepr::Named("Self".into())
                    };
                    ParamInfo {
                        name: Some("self".into()),
                        type_repr,
                        is_receiver: true,
                    }
                }
                syn::FnArg::Typed(pat_type) => {
                    let name = match pat_type.pat.as_ref() {
                        syn::Pat::Ident(pi) => Some(pi.ident.to_string()),
                        _ => None,
                    };
                    ParamInfo {
                        name,
                        type_repr: Self::syn_type_to_repr(&pat_type.ty),
                        is_receiver: false,
                    }
                }
            })
            .collect();

        let return_type = match &sig.output {
            syn::ReturnType::Default => None,
            syn::ReturnType::Type(_, ty) => Some(Self::syn_type_to_repr(ty)),
        };

        FunctionInfo {
            name: sig.ident.to_string(),
            params,
            return_type,
            generics: Self::extract_generics(&sig.generics),
        }
    }

    fn format_signature(sig: &syn::Signature) -> String {
        let params: Vec<String> = sig
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

        let params_str = format!("({})", params.join(", "));

        // Extract return type
        let return_type = match &sig.output {
            syn::ReturnType::Default => None,
            syn::ReturnType::Type(_, ty) => Some(quote::quote!(#ty).to_string()),
        };

        match return_type {
            Some(ret) => format!("{} -> {}", params_str, ret),
            None => params_str,
        }
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

        // Public functions (qualified with impl type)
        assert!(module.public_functions.contains(&"Bridge.new".to_string()));
        assert!(module.public_functions.contains(&"Bridge.deposit".to_string()));
        assert!(module.public_functions.contains(&"Bridge.withdraw".to_string()));

        // Private functions (qualified with impl type)
        assert!(module
            .private_functions
            .contains(&"Bridge.verify_signature".to_string()));
        assert!(module
            .private_functions
            .contains(&"Bridge.execute_transfer".to_string()));

        // Imports - now extracts root crates from nested imports
        assert!(module.imports.contains(&"alloy".to_string()));
        assert!(module.imports.contains(&"anyhow".to_string()));
        // std is filtered out
        assert!(!module.imports.iter().any(|i| i.starts_with("std")));

        // Test imports should be filtered out
        assert!(!module.imports.contains(&"test_utils".to_string()));

        // Check return type extraction (qualified names)
        let new_sig = module.function_signatures.get("Bridge.new").unwrap();
        assert!(
            new_sig.contains("-> Self"),
            "Expected '-> Self' in signature but got: {}",
            new_sig
        );

        let deposit_sig = module.function_signatures.get("Bridge.deposit").unwrap();
        assert!(
            deposit_sig.contains("-> Result"),
            "Expected '-> Result' in signature but got: {}",
            deposit_sig
        );

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
