use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::PathBuf;

use super::{ExtractedModule, Extractor};
use super::flow9_ast::{ArgType, Decl, Flow9Type, FunArg};
use super::flow9_parser;
use crate::types::{
    FieldInfo, FunctionInfo, ParamInfo, TypeInfo, TypeKind, TypeRepr, VariantInfo,
    Visibility as TypeVisibility,
};

pub struct Flow9Extractor;

impl Flow9Extractor {
    pub fn new() -> Self {
        Self
    }
}

impl Extractor for Flow9Extractor {
    fn extract(&self, path: &PathBuf) -> Result<ExtractedModule> {
        let content =
            std::fs::read_to_string(path).context("Failed to read flow9 source file")?;
        parse_flow9(&content, path)
    }
}

/// Convert a Flow9Type AST node to a TypeRepr
fn flow9_type_to_type_repr(t: &Flow9Type) -> TypeRepr {
    match t {
        Flow9Type::Int => TypeRepr::Named("int".into()),
        Flow9Type::Double => TypeRepr::Named("double".into()),
        Flow9Type::Bool => TypeRepr::Named("bool".into()),
        Flow9Type::Str => TypeRepr::Named("string".into()),
        Flow9Type::Flow => TypeRepr::Named("flow".into()),
        Flow9Type::NativeType => TypeRepr::Named("native".into()),
        Flow9Type::Void => TypeRepr::Unit,
        Flow9Type::Named(name) => TypeRepr::Named(name.clone()),
        Flow9Type::TypeVar(tv) => TypeRepr::Named(tv.clone()),
        Flow9Type::Array(inner) => TypeRepr::Applied(
            Box::new(TypeRepr::Named("Array".into())),
            vec![flow9_type_to_type_repr(inner)],
        ),
        Flow9Type::Ref(inner) => TypeRepr::Applied(
            Box::new(TypeRepr::Named("ref".into())),
            vec![flow9_type_to_type_repr(inner)],
        ),
        Flow9Type::FnType(params, ret) => {
            let param_reprs: Vec<TypeRepr> = params.iter().map(flow9_type_to_type_repr).collect();
            TypeRepr::FnPointer {
                params: param_reprs,
                ret: Box::new(flow9_type_to_type_repr(ret)),
            }
        }
        Flow9Type::Parameterized(name, args) => {
            let arg_reprs: Vec<TypeRepr> = args.iter().map(flow9_type_to_type_repr).collect();
            TypeRepr::Applied(Box::new(TypeRepr::Named(name.clone())), arg_reprs)
        }
    }
}

/// Format a Flow9Type as a string (for signature strings)
fn flow9_type_to_string(t: &Flow9Type) -> String {
    match t {
        Flow9Type::Int => "int".into(),
        Flow9Type::Double => "double".into(),
        Flow9Type::Bool => "bool".into(),
        Flow9Type::Str => "string".into(),
        Flow9Type::Flow => "flow".into(),
        Flow9Type::NativeType => "native".into(),
        Flow9Type::Void => "void".into(),
        Flow9Type::Named(name) => name.clone(),
        Flow9Type::TypeVar(tv) => tv.clone(),
        Flow9Type::Array(inner) => format!("[{}]", flow9_type_to_string(inner)),
        Flow9Type::Ref(inner) => format!("ref {}", flow9_type_to_string(inner)),
        Flow9Type::FnType(params, ret) => {
            let ps: Vec<String> = params.iter().map(flow9_type_to_string).collect();
            format!("({}) -> {}", ps.join(", "), flow9_type_to_string(ret))
        }
        Flow9Type::Parameterized(name, args) => {
            let as_: Vec<String> = args.iter().map(flow9_type_to_string).collect();
            format!("{}<{}>", name, as_.join(", "))
        }
    }
}

/// Build a signature string from argtypes and return type
fn build_sig_from_argtypes(arg_types: &[ArgType], return_type: &Flow9Type) -> String {
    let params: Vec<String> = arg_types
        .iter()
        .map(|at| match &at.name {
            Some(n) => format!("{} : {}", n, flow9_type_to_string(&at.type_)),
            None => flow9_type_to_string(&at.type_),
        })
        .collect();
    format!("({}) -> {}", params.join(", "), flow9_type_to_string(return_type))
}

/// Build a signature string from funargs and optional return type
fn build_sig_from_funargs(args: &[FunArg], return_type: Option<&Flow9Type>) -> String {
    let params: Vec<String> = args
        .iter()
        .map(|fa| match &fa.type_annotation {
            Some(t) => format!("{} : {}", fa.name, flow9_type_to_string(t)),
            None => fa.name.clone(),
        })
        .collect();
    match return_type {
        Some(rt) => format!("({}) -> {}", params.join(", "), flow9_type_to_string(rt)),
        None => format!("({})", params.join(", ")),
    }
}

/// Build FunctionInfo from funargs and optional return type
fn build_fi_from_funargs(
    name: &str,
    args: &[FunArg],
    return_type: Option<&Flow9Type>,
) -> FunctionInfo {
    let params: Vec<ParamInfo> = args
        .iter()
        .map(|fa| ParamInfo {
            name: Some(fa.name.clone()),
            type_repr: fa
                .type_annotation
                .as_ref()
                .map(flow9_type_to_type_repr)
                .unwrap_or(TypeRepr::Infer),
            is_receiver: false,
        })
        .collect();
    FunctionInfo {
        name: name.to_string(),
        params,
        return_type: return_type.map(flow9_type_to_type_repr),
        generics: vec![],
    }
}

/// Build FunctionInfo from argtypes and return type
fn build_fi_from_argtypes(
    name: &str,
    arg_types: &[ArgType],
    return_type: &Flow9Type,
) -> FunctionInfo {
    let params: Vec<ParamInfo> = arg_types
        .iter()
        .map(|at| ParamInfo {
            name: at.name.clone(),
            type_repr: flow9_type_to_type_repr(&at.type_),
            is_receiver: false,
        })
        .collect();
    FunctionInfo {
        name: name.to_string(),
        params,
        return_type: Some(flow9_type_to_type_repr(return_type)),
        generics: vec![],
    }
}

fn parse_flow9(raw_content: &str, path: &PathBuf) -> Result<ExtractedModule> {
    let content = strip_comments(raw_content);
    let mut module = ExtractedModule {
        name: path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string(),
        language: "flow9".to_string(),
        source_path: Some(path.to_string_lossy().to_string()),
        ..Default::default()
    };

    let parsed = flow9_parser::parse_flow9_source(&content)
        .map_err(|e| anyhow::anyhow!("Parse error: {}", e))?;

    // Collect exported names
    let exported_names: HashSet<String> = parsed.exports.iter().cloned().collect();

    // Process all declarations
    for decl in &parsed.declarations {
        match decl {
            Decl::Import(path) => {
                if !module.imports.contains(path) {
                    module.imports.push(path.clone());
                }
            }
            Decl::Forbid(_) => {
                // forbids are tracked via parsed.forbids
            }
            Decl::Union {
                name,
                type_params: _,
                variants,
            } => {
                let variant_infos: Vec<VariantInfo> = variants
                    .iter()
                    .map(|tn| VariantInfo {
                        name: tn.name.clone(),
                        fields: vec![],
                    })
                    .collect();

                let ti = TypeInfo {
                    name: name.clone(),
                    kind: TypeKind::Enum,
                    generics: vec![],
                    fields: vec![],
                    variants: variant_infos,
                    trait_impls: vec![],
                    derives: vec![],
                };
                module.type_definitions.insert(name.clone(), ti);

                if exported_names.contains(name) && !module.public_functions.contains(name) {
                    module.public_functions.push(name.clone());
                }
            }
            Decl::StructDecl {
                name,
                fields,
                exported,
            } => {
                // Don't overwrite union definitions
                if module.type_definitions.contains_key(name) {
                    continue;
                }

                let field_infos: Vec<FieldInfo> = fields
                    .iter()
                    .map(|fa| FieldInfo {
                        name: fa.name.clone(),
                        type_repr: fa
                            .type_annotation
                            .as_ref()
                            .map(flow9_type_to_type_repr)
                            .unwrap_or(TypeRepr::Infer),
                        visibility: TypeVisibility::Public,
                    })
                    .collect();

                let ti = TypeInfo {
                    name: name.clone(),
                    kind: TypeKind::Struct,
                    generics: vec![],
                    fields: field_infos,
                    variants: vec![],
                    trait_impls: vec![],
                    derives: vec![],
                };
                module.type_definitions.insert(name.clone(), ti);

                if (*exported || exported_names.contains(name))
                    && !module.public_functions.contains(name)
                {
                    module.public_functions.push(name.clone());
                }
            }
            Decl::FunctionDecl {
                name,
                arg_types,
                return_type,
                exported,
            } => {
                // Skip struct-like declarations (uppercase first char)
                if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                    continue;
                }

                let sig = build_sig_from_argtypes(arg_types, return_type);
                module.function_signatures.insert(name.clone(), sig);

                let fi = build_fi_from_argtypes(name, arg_types, return_type);
                module.function_info.insert(name.clone(), fi);

                if *exported || exported_names.contains(name) {
                    if !module.public_functions.contains(name) {
                        module.public_functions.push(name.clone());
                    }
                } else if !module.private_functions.contains(name) {
                    module.private_functions.push(name.clone());
                }
            }
            Decl::Function {
                name,
                args,
                return_type,
                exported,
                ..
            } => {
                // Skip keywords and struct constructors
                if name == "if"
                    || name == "switch"
                    || name == "export"
                    || name == "import"
                    || name == "native"
                {
                    continue;
                }
                if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                    // This is a struct constructor call in the function position.
                    // Check if it should be treated as a struct definition.
                    if !module.type_definitions.contains_key(name) {
                        let field_infos: Vec<FieldInfo> = args
                            .iter()
                            .map(|fa| FieldInfo {
                                name: fa.name.clone(),
                                type_repr: fa
                                    .type_annotation
                                    .as_ref()
                                    .map(flow9_type_to_type_repr)
                                    .unwrap_or(TypeRepr::Infer),
                                visibility: TypeVisibility::Public,
                            })
                            .collect();

                        let ti = TypeInfo {
                            name: name.clone(),
                            kind: TypeKind::Struct,
                            generics: vec![],
                            fields: field_infos,
                            variants: vec![],
                            trait_impls: vec![],
                            derives: vec![],
                        };
                        module.type_definitions.insert(name.clone(), ti);
                    }
                    if (*exported || exported_names.contains(name))
                        && !module.public_functions.contains(name)
                    {
                        module.public_functions.push(name.clone());
                    }
                    continue;
                }

                // Only add signature/info if we don't already have a type declaration
                if !module.function_signatures.contains_key(name) {
                    let sig = build_sig_from_funargs(args, return_type.as_ref());
                    module.function_signatures.insert(name.clone(), sig);
                }

                if !module.function_info.contains_key(name) {
                    let fi = build_fi_from_funargs(name, args, return_type.as_ref());
                    module.function_info.insert(name.clone(), fi);
                }

                if *exported || exported_names.contains(name) {
                    if !module.public_functions.contains(name) {
                        module.public_functions.push(name.clone());
                    }
                } else if !module.private_functions.contains(name)
                    && !module.public_functions.contains(name)
                {
                    module.private_functions.push(name.clone());
                }
            }
            Decl::Native {
                name,
                type_sig,
                ..
            } => {
                // Extract params and return type from the native's type signature
                match type_sig {
                    Flow9Type::FnType(params, ret) => {
                        let params_str: Vec<String> =
                            params.iter().map(flow9_type_to_string).collect();
                        let ret_str = flow9_type_to_string(ret);
                        let sig = format!("({}) -> {}", params_str.join(", "), ret_str);
                        module.function_signatures.insert(name.clone(), sig);

                        let param_infos: Vec<ParamInfo> = params
                            .iter()
                            .map(|p| ParamInfo {
                                name: None,
                                type_repr: flow9_type_to_type_repr(p),
                                is_receiver: false,
                            })
                            .collect();
                        let fi = FunctionInfo {
                            name: name.clone(),
                            params: param_infos,
                            return_type: Some(flow9_type_to_type_repr(ret)),
                            generics: vec![],
                        };
                        module.function_info.insert(name.clone(), fi);
                    }
                    _ => {
                        // Non-function native — just record it
                        let sig = flow9_type_to_string(type_sig);
                        module.function_signatures.insert(name.clone(), sig);
                    }
                }

                if exported_names.contains(name) && !module.public_functions.contains(name) {
                    module.public_functions.push(name.clone());
                }
            }
            Decl::Assign {
                name, exported, ..
            } => {
                // Exported assigns that aren't known functions/types are state variables
                if *exported || exported_names.contains(name) {
                    if !module.function_signatures.contains_key(name)
                        && !module.function_info.contains_key(name)
                        && !module.type_definitions.contains_key(name)
                        && !module
                            .type_definitions
                            .values()
                            .any(|ti| {
                                ti.fields.iter().any(|f| f.name == *name)
                                    || ti.variants.iter().any(|v| v.name == *name)
                            })
                        && !module.state_variables.contains(name)
                    {
                        module.state_variables.push(name.clone());
                    }
                }
            }
            Decl::VarDecl {
                name,
                type_annotation: _,
                exported,
                ..
            } => {
                if *exported || exported_names.contains(name) {
                    if !module.state_variables.contains(name) {
                        module.state_variables.push(name.clone());
                    }
                }
            }
        }
    }

    Ok(module)
}

/// Strip // and /* */ comments from a string
fn strip_comments(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '"' {
            // String literal: copy verbatim until closing quote
            result.push(ch);
            while let Some(c) = chars.next() {
                result.push(c);
                if c == '\\' {
                    // Escaped character: copy next char too
                    if let Some(esc) = chars.next() {
                        result.push(esc);
                    }
                } else if c == '"' {
                    break;
                }
            }
        } else if ch == '/' {
            if chars.peek() == Some(&'/') {
                // Line comment: skip to end of line
                for c in chars.by_ref() {
                    if c == '\n' {
                        result.push('\n');
                        break;
                    }
                }
            } else if chars.peek() == Some(&'*') {
                // Block comment: skip to */
                chars.next(); // consume *
                while let Some(c) = chars.next() {
                    if c == '*' && chars.peek() == Some(&'/') {
                        chars.next();
                        break;
                    }
                }
            } else {
                result.push(ch);
            }
        } else {
            result.push(ch);
        }
    }
    result
}

#[allow(dead_code)]
/// Parse struct fields from a string like "field1 : type1, field2 : type2"
fn parse_struct_fields(fields_str: &str) -> Vec<FieldInfo> {
    // Strip comments first
    let fields_str = strip_comments(fields_str);
    let fields_str = fields_str.trim();
    if fields_str.is_empty() {
        return vec![];
    }

    let mut fields = Vec::new();
    // Split on commas respecting nesting
    let parts = split_respecting_nesting(fields_str, ',');

    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        // Pattern: name : type  or  mutable name : type
        let is_mutable = part.starts_with("mutable ");
        let part = if is_mutable {
            part.trim_start_matches("mutable ").trim()
        } else {
            part
        };

        if let Some(colon_pos) = part.find(':') {
            let name = part[..colon_pos].trim().to_string();
            let type_str = part[colon_pos + 1..].trim();
            let type_repr = parse_flow9_type(type_str);
            fields.push(FieldInfo {
                name,
                type_repr,
                visibility: TypeVisibility::Public,
            });
        } else {
            // Positional field without type annotation
            fields.push(FieldInfo {
                name: fields.len().to_string(),
                type_repr: TypeRepr::Infer,
                visibility: TypeVisibility::Public,
            });
        }
    }

    fields
}

#[allow(dead_code)]
/// Parse a flow9 type string into TypeRepr
fn parse_flow9_type(s: &str) -> TypeRepr {
    let s = s.trim();
    if s.is_empty() {
        return TypeRepr::Unit;
    }

    // Type variables: ?, ??, ???
    if s.chars().all(|c| c == '?') {
        return TypeRepr::Named(s.to_string());
    }

    // Void
    if s == "void" {
        return TypeRepr::Unit;
    }

    // Array type: [type]
    if s.starts_with('[') && s.ends_with(']') {
        let inner = parse_flow9_type(&s[1..s.len() - 1]);
        return TypeRepr::Applied(Box::new(TypeRepr::Named("Array".into())), vec![inner]);
    }

    // Reference type: ref type
    if s.starts_with("ref ") {
        let inner = parse_flow9_type(&s[4..]);
        return TypeRepr::Applied(Box::new(TypeRepr::Named("ref".into())), vec![inner]);
    }

    // Function type: (types) -> rettype
    if let Some(arrow_pos) = find_top_level_arrow(s) {
        let params_part = s[..arrow_pos].trim();
        let ret_part = s[arrow_pos + 2..].trim();

        let params = if params_part.starts_with('(') && params_part.ends_with(')') {
            let inner = &params_part[1..params_part.len() - 1];
            parse_type_list(inner)
        } else {
            vec![parse_flow9_type(params_part)]
        };

        let ret = parse_flow9_type(ret_part);
        return TypeRepr::FnPointer {
            params,
            ret: Box::new(ret),
        };
    }

    // Parameterized type: Name<Type1, Type2>
    if let Some(angle_pos) = s.find('<') {
        if s.ends_with('>') {
            let name = s[..angle_pos].trim();
            let args_str = &s[angle_pos + 1..s.len() - 1];
            let args = parse_type_list(args_str);
            return TypeRepr::Applied(Box::new(TypeRepr::Named(name.to_string())), args);
        }
    }

    // Simple named type
    TypeRepr::Named(s.to_string())
}

#[allow(dead_code)]
/// Find the position of a top-level `->` (not inside parens/brackets/angles)
fn find_top_level_arrow(s: &str) -> Option<usize> {
    let chars: Vec<char> = s.chars().collect();
    let mut depth_paren = 0i32;
    let mut depth_angle = 0i32;
    let mut depth_bracket = 0i32;

    let mut i = 0;
    while i < chars.len().saturating_sub(1) {
        match chars[i] {
            '(' => depth_paren += 1,
            ')' => depth_paren -= 1,
            '<' => depth_angle += 1,
            '>' => depth_angle -= 1,
            '[' => depth_bracket += 1,
            ']' => depth_bracket -= 1,
            '-' if depth_paren == 0 && depth_angle == 0 && depth_bracket == 0 => {
                if chars.get(i + 1) == Some(&'>') {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

#[allow(dead_code)]
/// Parse a comma-separated list of types
fn parse_type_list(s: &str) -> Vec<TypeRepr> {
    let parts = split_respecting_nesting(s, ',');
    parts
        .iter()
        .map(|p| {
            let p = p.trim();
            // Handle named params: "name : type"
            if let Some(colon_pos) = p.find(':') {
                let after_colon = p[colon_pos + 1..].trim();
                parse_flow9_type(after_colon)
            } else {
                parse_flow9_type(p)
            }
        })
        .filter(|t| *t != TypeRepr::Unit || !s.trim().is_empty())
        .collect()
}

#[allow(dead_code)]
/// Split a string on a delimiter, respecting (), <>, [] nesting
fn split_respecting_nesting(s: &str, delim: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth_paren = 0i32;
    let mut depth_angle = 0i32;
    let mut depth_bracket = 0i32;

    for ch in s.chars() {
        match ch {
            '(' => {
                depth_paren += 1;
                current.push(ch);
            }
            ')' => {
                depth_paren -= 1;
                current.push(ch);
            }
            '<' => {
                depth_angle += 1;
                current.push(ch);
            }
            '>' => {
                depth_angle -= 1;
                current.push(ch);
            }
            '[' => {
                depth_bracket += 1;
                current.push(ch);
            }
            ']' => {
                depth_bracket -= 1;
                current.push(ch);
            }
            c if c == delim
                && depth_paren == 0
                && depth_angle == 0
                && depth_bracket == 0 =>
            {
                parts.push(current.clone());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

#[allow(dead_code)]
/// Build FunctionInfo from parsed parameter and return type strings
fn build_function_info(name: &str, params_str: &str, ret_str: Option<&str>) -> FunctionInfo {
    let params = if params_str.trim().is_empty() {
        vec![]
    } else {
        let parts = split_respecting_nesting(params_str, ',');
        parts
            .iter()
            .map(|p| {
                let p = p.trim();
                if let Some(colon_pos) = p.find(':') {
                    let param_name = p[..colon_pos].trim();
                    let type_str = p[colon_pos + 1..].trim();
                    ParamInfo {
                        name: Some(param_name.to_string()),
                        type_repr: parse_flow9_type(type_str),
                        is_receiver: false,
                    }
                } else {
                    ParamInfo {
                        name: None,
                        type_repr: parse_flow9_type(p),
                        is_receiver: false,
                    }
                }
            })
            .collect()
    };

    let return_type = ret_str.map(|r| parse_flow9_type(r));

    FunctionInfo {
        name: name.to_string(),
        params,
        return_type,
        generics: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_extract_maybe_module() {
        let content = r#"
import runtime;

export {
    Maybe<?> ::= None, Some<?>;
    None();
    Some(value : ?);

    isNone : (m : Maybe<?>) -> bool;
    isSome : (m : Maybe<?>) -> bool;
    either : (m : Maybe<?>, alternative: ?) -> ?;
}

isNone(m : Maybe<?>) -> bool {
    switch (m : Maybe) {
        None(): true;
        Some(__): false;
    }
}

isSome(m : Maybe<?>) -> bool {
    switch (m : Maybe) {
        None(): false;
        Some(__): true;
    }
}

either(m : Maybe<?>, alternative: ?) -> ? {
    switch (m : Maybe) {
        None(): alternative;
        Some(v): v;
    }
}

helperPrivate(x : int) -> int {
    x + 1;
}
"#;

        let mut file = NamedTempFile::with_suffix(".flow").unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let extractor = Flow9Extractor::new();
        let module = extractor.extract(&file.path().to_path_buf()).unwrap();

        assert_eq!(module.language, "flow9");

        // Imports
        assert!(module.imports.contains(&"runtime".to_string()));

        // Union type
        let maybe_type = module.type_definitions.get("Maybe").unwrap();
        assert_eq!(maybe_type.kind, TypeKind::Enum);
        assert!(maybe_type.variants.iter().any(|v| v.name == "None"));
        assert!(maybe_type.variants.iter().any(|v| v.name == "Some"));

        // Struct types (union variants)
        assert!(module.type_definitions.contains_key("None"));
        assert!(module.type_definitions.contains_key("Some"));

        // Public functions
        assert!(
            module.public_functions.contains(&"isNone".to_string()),
            "isNone should be public, got: {:?}",
            module.public_functions
        );
        assert!(module.public_functions.contains(&"isSome".to_string()));
        assert!(module.public_functions.contains(&"either".to_string()));

        // Private functions
        assert!(
            module
                .private_functions
                .contains(&"helperPrivate".to_string()),
            "helperPrivate should be private, got: {:?}",
            module.private_functions
        );

        // Function signatures
        let is_none_sig = module.function_signatures.get("isNone").unwrap();
        assert!(is_none_sig.contains("->"), "Signature: {}", is_none_sig);
        assert!(is_none_sig.contains("bool"), "Signature: {}", is_none_sig);
    }

    #[test]
    fn test_parse_flow9_types() {
        assert_eq!(parse_flow9_type("int"), TypeRepr::Named("int".into()));
        assert_eq!(parse_flow9_type("void"), TypeRepr::Unit);
        assert_eq!(parse_flow9_type("?"), TypeRepr::Named("?".into()));
        assert_eq!(parse_flow9_type("??"), TypeRepr::Named("??".into()));

        // Array
        assert_eq!(
            parse_flow9_type("[int]"),
            TypeRepr::Applied(
                Box::new(TypeRepr::Named("Array".into())),
                vec![TypeRepr::Named("int".into())]
            )
        );

        // Parameterized
        assert_eq!(
            parse_flow9_type("Maybe<int>"),
            TypeRepr::Applied(
                Box::new(TypeRepr::Named("Maybe".into())),
                vec![TypeRepr::Named("int".into())]
            )
        );

        // Function type
        let fn_type = parse_flow9_type("(int) -> bool");
        assert!(matches!(fn_type, TypeRepr::FnPointer { .. }));

        // Ref
        assert_eq!(
            parse_flow9_type("ref int"),
            TypeRepr::Applied(
                Box::new(TypeRepr::Named("ref".into())),
                vec![TypeRepr::Named("int".into())]
            )
        );
    }

    #[test]
    fn test_extract_struct_fields() {
        let fields = parse_struct_fields("text : string, style : [[string]]");
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "text");
        assert_eq!(fields[0].type_repr, TypeRepr::Named("string".into()));
        assert_eq!(fields[1].name, "style");
    }

    #[test]
    fn test_extract_native_functions() {
        let content = r#"
export {
    println : (?) -> void;
    strlen : (string) -> int;
}

native println : (?) -> void = Native.println;
native strlen : (string) -> int = Native.strlen;
"#;
        let mut file = NamedTempFile::with_suffix(".flow").unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let extractor = Flow9Extractor::new();
        let module = extractor.extract(&file.path().to_path_buf()).unwrap();

        assert!(module.public_functions.contains(&"println".to_string()));
        assert!(module.public_functions.contains(&"strlen".to_string()));
        assert!(module.function_signatures.contains_key("println"));
    }
}
