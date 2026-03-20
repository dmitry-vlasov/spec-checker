use anyhow::{Context, Result};
use std::path::PathBuf;

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

        parse_rust_regex(&content, path)
    }
}

fn parse_rust_regex(content: &str, path: &PathBuf) -> Result<ExtractedModule> {
    let mut module = ExtractedModule {
        language: "rust".to_string(),
        source_path: Some(path.to_string_lossy().to_string()),
        ..Default::default()
    };

    // Extract module name from path or mod declaration
    let mod_re = regex::Regex::new(r"(?:pub\s+)?mod\s+(\w+)")?;
    if let Some(cap) = mod_re.captures(content) {
        module.name = cap[1].to_string();
    } else {
        module.name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_string();
    }

    // Extract public functions
    let pub_fn_re =
        regex::Regex::new(r"pub(?:\s+(?:async\s+)?)?fn\s+(\w+)\s*(?:<[^>]*>)?\s*\(([^)]*)\)")?;

    for cap in pub_fn_re.captures_iter(content) {
        let name = cap[1].to_string();
        let params = cap[2].to_string();

        module.public_functions.push(name.clone());
        module
            .function_signatures
            .insert(name, format!("({})", params.trim()));
    }

    // Extract private functions (fn without pub)
    let priv_fn_re =
        regex::Regex::new(r"(?:^|\n)\s*(?:async\s+)?fn\s+(\w+)\s*(?:<[^>]*>)?\s*\(([^)]*)\)")?;

    for cap in priv_fn_re.captures_iter(content) {
        let name = cap[1].to_string();
        // Skip if already found as public
        if !module.public_functions.contains(&name) {
            module.private_functions.push(name);
        }
    }

    // Extract use statements (imports) - module-level only, skip test code
    // Match "use" at start of line (module-level imports)
    let use_re = regex::Regex::new(r"(?m)^use\s+([^;\n]+);")?;

    // Only extract imports before #[cfg(test)] to skip test dependencies
    let main_content = content.split("#[cfg(test)]").next().unwrap_or(content);

    for cap in use_re.captures_iter(main_content) {
        let import = cap[1].trim().to_string();
        // Skip prelude, std primitives, and internal super/crate imports
        if !import.starts_with("std::prelude")
            && !import.starts_with("super::")
            && !import.starts_with("crate::")
        {
            module.imports.push(import);
        }
    }

    // Extract struct definitions as "state"
    let struct_re = regex::Regex::new(r"(?:pub\s+)?struct\s+(\w+)")?;
    for cap in struct_re.captures_iter(content) {
        module.state_variables.push(cap[1].to_string());
    }

    // Extract trait implementations
    let impl_re = regex::Regex::new(r"impl(?:<[^>]*>)?\s+(\w+)")?;
    for cap in impl_re.captures_iter(content) {
        let name = cap[1].to_string();
        if !module.modifiers.contains(&name) {
            module.modifiers.push(name);
        }
    }

    Ok(module)
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
use crate::types::Config;
use super::utils::helper;

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

    pub async fn deposit(&mut self, token: &str, amount: u64) -> Result<(), Error> {
        // deposit logic
        Ok(())
    }

    pub fn withdraw(&mut self, token: &str, amount: u64, signature: &[u8]) -> Result<(), Error> {
        self.verify_signature(signature)?;
        self.execute_transfer(token, amount)?;
        Ok(())
    }

    fn verify_signature(&self, signature: &[u8]) -> Result<(), Error> {
        Ok(())
    }

    fn execute_transfer(&mut self, token: &str, amount: u64) -> Result<(), Error> {
        Ok(())
    }
}

struct Error;
"#;

        let mut file = NamedTempFile::new().unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let extractor = RustExtractor::new();
        let module = extractor.extract(&file.path().to_path_buf()).unwrap();

        assert_eq!(module.language, "rust");

        assert!(module.public_functions.contains(&"new".to_string()));
        assert!(module.public_functions.contains(&"deposit".to_string()));
        assert!(module.public_functions.contains(&"withdraw".to_string()));

        assert!(module
            .private_functions
            .contains(&"verify_signature".to_string()));
        assert!(module
            .private_functions
            .contains(&"execute_transfer".to_string()));

        assert!(module.imports.iter().any(|i| i.contains("HashMap")));
        // crate:: and super:: imports are filtered out as internal
        // assert!(module.imports.iter().any(|i| i.contains("Config")));

        assert!(module.state_variables.contains(&"Bridge".to_string()));
    }
}
