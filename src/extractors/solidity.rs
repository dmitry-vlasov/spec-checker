use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

use super::{ExtractedModule, Extractor};

pub struct SolidityExtractor;

impl SolidityExtractor {
    pub fn new() -> Self {
        Self
    }
}

impl Extractor for SolidityExtractor {
    fn extract(&self, path: &PathBuf) -> Result<ExtractedModule> {
        // Try to use solc with AST output
        let output = Command::new("solc")
            .args(["--ast-compact-json", path.to_str().unwrap()])
            .output();

        if let Ok(output) = output {
            if output.status.success() {
                // Parse the AST JSON
                let stdout = String::from_utf8_lossy(&output.stdout);
                return parse_solidity_ast(&stdout, path);
            }
        }

        // Fallback to regex-based extraction
        let content =
            std::fs::read_to_string(path).context("Failed to read Solidity source file")?;
        parse_solidity_regex(&content, path)
    }
}

fn parse_solidity_ast(_ast_output: &str, path: &PathBuf) -> Result<ExtractedModule> {
    // The AST output format from solc
    // For now, fall back to regex - full AST parsing is more complex
    let content = std::fs::read_to_string(path)?;
    parse_solidity_regex(&content, path)
}

fn parse_solidity_regex(content: &str, path: &PathBuf) -> Result<ExtractedModule> {
    let mut module = ExtractedModule {
        language: "solidity".to_string(),
        source_path: Some(path.to_string_lossy().to_string()),
        ..Default::default()
    };

    // Extract contract name
    let contract_re = regex::Regex::new(r"(?:contract|interface|library)\s+(\w+)")?;
    if let Some(cap) = contract_re.captures(content) {
        module.name = cap[1].to_string();
    } else {
        module.name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_string();
    }

    // Extract functions
    let func_re = regex::Regex::new(
        r"function\s+(\w+)\s*\(([^)]*)\)\s*((?:public|external|internal|private|view|pure|payable|virtual|override|\s)*)",
    )?;

    for cap in func_re.captures_iter(content) {
        let name = cap[1].to_string();
        let params = cap[2].to_string();
        let modifiers = cap[3].to_string();

        let signature = format!("({})", params.trim());
        module.function_signatures.insert(name.clone(), signature);

        if modifiers.contains("public") || modifiers.contains("external") {
            module.public_functions.push(name);
        } else if modifiers.contains("private") || modifiers.contains("internal") {
            module.private_functions.push(name);
        } else {
            // Default to public for interface functions
            if content.contains("interface") {
                module.public_functions.push(name);
            } else {
                module.private_functions.push(name);
            }
        }
    }

    // Extract imports
    let import_re = regex::Regex::new(r#"import\s+(?:\{[^}]+\}\s+from\s+)?["']([^"']+)["']"#)?;
    for cap in import_re.captures_iter(content) {
        module.imports.push(cap[1].to_string());
    }

    // Extract events
    let event_re = regex::Regex::new(r"event\s+(\w+)")?;
    for cap in event_re.captures_iter(content) {
        module.events.push(cap[1].to_string());
    }

    // Extract state variables
    let state_re = regex::Regex::new(
        r"^\s*(mapping|address|uint\d*|int\d*|bytes\d*|string|bool)\s+(?:public\s+|private\s+|internal\s+)?(\w+)\s*[;=]",
    )?;
    for cap in state_re.captures_iter(content) {
        module.state_variables.push(cap[2].to_string());
    }

    // Extract modifiers (Solidity specific)
    let modifier_re = regex::Regex::new(r"modifier\s+(\w+)")?;
    for cap in modifier_re.captures_iter(content) {
        module.modifiers.push(cap[1].to_string());
    }

    Ok(module)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_extract_solidity_contract() {
        let content = r#"
// SPDX-License-Identifier: MIT
pragma solidity ^0.8.0;

import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "./TokenRegistry.sol";

contract Bridge {
    mapping(address => uint256) public deposited;
    address private admin;

    event Deposited(address indexed token, uint256 amount);
    event Withdrawn(address indexed token, uint256 amount);

    modifier onlyAdmin() {
        require(msg.sender == admin, "Not admin");
        _;
    }

    function deposit(address token, uint256 amount) public {
        // deposit logic
    }

    function withdraw(address token, uint256 amount, bytes memory signature) external {
        // withdraw logic
    }

    function _verifySignature(bytes memory signature) internal pure returns (bool) {
        return true;
    }

    function _executeTransfer(address token, uint256 amount) private {
        // transfer logic
    }
}
"#;

        let mut file = NamedTempFile::new().unwrap();
        file.write_all(content.as_bytes()).unwrap();

        let extractor = SolidityExtractor::new();
        let module = extractor.extract(&file.path().to_path_buf()).unwrap();

        assert_eq!(module.name, "Bridge");
        assert_eq!(module.language, "solidity");

        assert!(module.public_functions.contains(&"deposit".to_string()));
        assert!(module.public_functions.contains(&"withdraw".to_string()));

        assert!(module
            .private_functions
            .contains(&"_verifySignature".to_string()));
        assert!(module
            .private_functions
            .contains(&"_executeTransfer".to_string()));

        assert!(module.imports.iter().any(|i| i.contains("IERC20")));
        assert!(module.imports.iter().any(|i| i.contains("TokenRegistry")));

        assert!(module.events.contains(&"Deposited".to_string()));
        assert!(module.events.contains(&"Withdrawn".to_string()));

        assert!(module.modifiers.contains(&"onlyAdmin".to_string()));
    }
}
