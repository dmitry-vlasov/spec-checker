use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::PathBuf;

use super::{ExtractedModule, Extractor};
use crate::types::{
    FieldInfo, FunctionInfo, GenericParam, ParamInfo, TypeInfo, TypeKind, TypeRepr, VariantInfo,
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

fn parse_flow9(content: &str, path: &PathBuf) -> Result<ExtractedModule> {
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

    // Extract imports
    let import_re = regex::Regex::new(r"import\s+([\w/]+)\s*;")?;
    for cap in import_re.captures_iter(content) {
        module.imports.push(cap[1].to_string());
    }

    // Find export block boundaries
    let exported_names = extract_exported_names(content);

    // Extract union definitions (Name ::= Variant1, Variant2;)
    let union_re = regex::Regex::new(r"(\w+)\s*(?:<[^>]*>)?\s*::=\s*([^;]+);")?;
    for cap in union_re.captures_iter(content) {
        let name = cap[1].to_string();
        let variants_str = cap[2].to_string();
        let variant_names: Vec<String> = variants_str
            .split(',')
            .map(|s| {
                // Strip type params: "Some<?>" -> "Some"
                let trimmed = s.trim();
                if let Some(pos) = trimmed.find('<') {
                    trimmed[..pos].to_string()
                } else {
                    trimmed.to_string()
                }
            })
            .filter(|s| !s.is_empty())
            .collect();

        let variants: Vec<VariantInfo> = variant_names
            .iter()
            .map(|vn| VariantInfo {
                name: vn.clone(),
                fields: vec![], // variant fields are on the struct definitions
            })
            .collect();

        let ti = TypeInfo {
            name: name.clone(),
            kind: TypeKind::Enum, // unions map to sum types
            generics: vec![],
            fields: vec![],
            variants,
            trait_impls: vec![],
            derives: vec![],
        };
        module.type_definitions.insert(name.clone(), ti);

        if exported_names.contains(&name) {
            module.public_functions.push(name); // types go in public list for visibility
        }
    }

    // Extract struct definitions: Name(field : type, ...); or Name : (field : type, ...);
    // Pattern 1: Name(field : type, field2 : type2);
    let struct_re1 =
        regex::Regex::new(r"(?m)^\s*([A-Z]\w*)\s*\(([^)]*)\)\s*;")?;
    // Pattern 2: Name : (field : type, field2 : type2);
    let struct_re2 =
        regex::Regex::new(r"(?m)^\s*([A-Z]\w*)\s*:\s*\(([^)]*)\)\s*;")?;

    for re in &[&struct_re1, &struct_re2] {
        for cap in re.captures_iter(content) {
            let name = cap[1].to_string();
            let fields_str = cap[2].to_string();

            // Skip if this looks like a function type declaration (has ->)
            if fields_str.contains("->") {
                continue;
            }

            // Don't overwrite union definitions
            if module.type_definitions.contains_key(&name) {
                // But update fields on existing union variant entry if needed
                continue;
            }

            let fields = parse_struct_fields(&fields_str);

            let ti = TypeInfo {
                name: name.clone(),
                kind: TypeKind::Struct,
                generics: vec![],
                fields,
                variants: vec![],
                trait_impls: vec![],
                derives: vec![],
            };
            module.type_definitions.insert(name.clone(), ti);

            if exported_names.contains(&name) && !module.public_functions.contains(&name) {
                module.public_functions.push(name);
            }
        }
    }

    // Extract function type declarations from export block: name : (types) -> rettype;
    let fn_type_re =
        regex::Regex::new(r"(\w+)\s*:\s*\(([^)]*)\)\s*->\s*([^;]+)\s*;")?;
    for cap in fn_type_re.captures_iter(content) {
        let name = cap[1].to_string();
        let params_str = cap[2].to_string();
        let ret_str = cap[3].trim().to_string();

        // Skip struct-like declarations (name starts with uppercase and no ->)
        if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
            continue;
        }

        let sig = format!("({}) -> {}", params_str.trim(), ret_str);
        module
            .function_signatures
            .insert(name.clone(), sig.clone());

        let fi = build_function_info(&name, &params_str, Some(&ret_str));
        module.function_info.insert(name.clone(), fi);

        if exported_names.contains(&name) {
            if !module.public_functions.contains(&name) {
                module.public_functions.push(name);
            }
        } else if !module.private_functions.contains(&name) {
            module.private_functions.push(name);
        }
    }

    // Extract function definitions: name(args) -> type { body }
    // or name(args) { body }
    let fn_def_re =
        regex::Regex::new(r"(?m)^(\w+)\s*\(([^)]*)\)\s*(?:->\s*(\S+)\s*)?\{")?;
    for cap in fn_def_re.captures_iter(content) {
        let name = cap[1].to_string();

        // Skip keywords and struct-like names
        if name == "if"
            || name == "switch"
            || name == "export"
            || name == "import"
            || name == "native"
        {
            continue;
        }
        // Skip struct constructors (uppercase)
        if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
            continue;
        }

        let params_str = cap[2].to_string();
        let ret_str = cap.get(3).map(|m| m.as_str().trim().to_string());

        // Only add if we don't already have a type declaration for this
        if !module.function_signatures.contains_key(&name) {
            let sig = match &ret_str {
                Some(ret) => format!("({}) -> {}", params_str.trim(), ret),
                None => format!("({})", params_str.trim()),
            };
            module.function_signatures.insert(name.clone(), sig);
        }

        if !module.function_info.contains_key(&name) {
            let fi = build_function_info(&name, &params_str, ret_str.as_deref());
            module.function_info.insert(name.clone(), fi);
        }

        // Determine visibility
        if exported_names.contains(&name) {
            if !module.public_functions.contains(&name) {
                module.public_functions.push(name);
            }
        } else if !module.private_functions.contains(&name)
            && !module.public_functions.contains(&name)
        {
            module.private_functions.push(name);
        }
    }

    // Extract native declarations: native name : (types) -> type = binding;
    let native_re =
        regex::Regex::new(r"native\s+(\w+)\s*:\s*\(([^)]*)\)\s*->\s*([^=;]+)\s*=")?;
    for cap in native_re.captures_iter(content) {
        let name = cap[1].to_string();
        let params_str = cap[2].to_string();
        let ret_str = cap[3].trim().to_string();

        let sig = format!("({}) -> {}", params_str.trim(), ret_str);
        module.function_signatures.insert(name.clone(), sig);

        let fi = build_function_info(&name, &params_str, Some(&ret_str));
        module.function_info.insert(name.clone(), fi);

        if exported_names.contains(&name) {
            if !module.public_functions.contains(&name) {
                module.public_functions.push(name);
            }
        }
    }

    Ok(module)
}

/// Extract names declared in the export { ... } block
fn extract_exported_names(content: &str) -> HashSet<String> {
    let mut names = HashSet::new();

    // Find export block
    let export_start = content.find("export");
    if export_start.is_none() {
        return names;
    }
    let export_start = export_start.unwrap();

    // Find the opening brace
    let brace_start = content[export_start..].find('{');
    if brace_start.is_none() {
        return names;
    }
    let brace_start = export_start + brace_start.unwrap();

    // Find matching closing brace
    let mut depth = 0;
    let mut brace_end = brace_start;
    for (i, ch) in content[brace_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    brace_end = brace_start + i;
                    break;
                }
            }
            _ => {}
        }
    }

    let export_block = &content[brace_start + 1..brace_end];

    // Extract names from export block
    // Patterns: name : type;  Name(fields);  Name ::= variants;  Name : (fields);
    let name_re = regex::Regex::new(r"(?m)^\s*(\w+)").unwrap();
    for cap in name_re.captures_iter(export_block) {
        let name = cap[1].to_string();
        // Skip keywords
        if name != "native" && name != "import" && name != "export" {
            names.insert(name);
        }
    }

    names
}

/// Parse struct fields from a string like "field1 : type1, field2 : type2"
fn parse_struct_fields(fields_str: &str) -> Vec<FieldInfo> {
    if fields_str.trim().is_empty() {
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
