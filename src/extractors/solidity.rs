use anyhow::{Context, Result};
use serde::Deserialize;
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

// ============================================================================
// AST JSON Parsing (solc --ast-compact-json output)
// ============================================================================

/// Root structure of solc AST output
#[derive(Debug, Deserialize)]
struct SolcAstOutput {
    #[serde(default)]
    sources: std::collections::HashMap<String, SourceUnit>,
}

#[derive(Debug, Deserialize)]
struct SourceUnit {
    #[serde(rename = "AST")]
    ast: Option<AstNode>,
}

/// Generic AST node - solc uses a flat structure with nodeType discriminator
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct AstNode {
    #[serde(rename = "nodeType")]
    node_type: String,

    #[serde(default)]
    name: Option<String>,

    #[serde(default)]
    visibility: Option<String>,

    #[serde(default)]
    kind: Option<String>,

    #[serde(rename = "absolutePath")]
    #[serde(default)]
    absolute_path: Option<String>,

    #[serde(default)]
    parameters: Option<ParameterList>,

    #[serde(rename = "returnParameters")]
    #[serde(default)]
    return_parameters: Option<ParameterList>,

    #[serde(default)]
    nodes: Option<Vec<AstNode>>,

    #[serde(rename = "typeName")]
    #[serde(default)]
    type_name: Option<Box<AstNode>>,

    #[serde(rename = "typeDescriptions")]
    #[serde(default)]
    type_descriptions: Option<TypeDescriptions>,
}

#[derive(Debug, Deserialize)]
struct ParameterList {
    #[serde(default)]
    parameters: Vec<Parameter>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Parameter {
    #[serde(default)]
    name: String,

    #[serde(rename = "typeName")]
    #[serde(default)]
    type_name: Option<Box<AstNode>>,

    #[serde(rename = "typeDescriptions")]
    #[serde(default)]
    type_descriptions: Option<TypeDescriptions>,
}

#[derive(Debug, Deserialize)]
struct TypeDescriptions {
    #[serde(rename = "typeString")]
    #[serde(default)]
    type_string: Option<String>,
}

fn parse_solidity_ast(ast_output: &str, path: &PathBuf) -> Result<ExtractedModule> {
    // solc --ast-compact-json outputs JSON after some header text
    // Find the JSON part (starts with '{' or the sources object)
    let json_start = ast_output.find('{');

    if json_start.is_none() {
        // No JSON found, fall back to regex
        let content = std::fs::read_to_string(path)?;
        return parse_solidity_regex(&content, path);
    }

    let json_str = &ast_output[json_start.unwrap()..];

    // Try to parse as SolcAstOutput first (multiple files format)
    if let Ok(output) = serde_json::from_str::<SolcAstOutput>(json_str) {
        return extract_from_solc_output(&output, path);
    }

    // Try to parse as single AstNode (single file format)
    if let Ok(ast) = serde_json::from_str::<AstNode>(json_str) {
        return extract_from_ast_node(&ast, path);
    }

    // Fall back to regex
    let content = std::fs::read_to_string(path)?;
    parse_solidity_regex(&content, path)
}

fn extract_from_solc_output(output: &SolcAstOutput, path: &PathBuf) -> Result<ExtractedModule> {
    let mut module = ExtractedModule {
        language: "solidity".to_string(),
        source_path: Some(path.to_string_lossy().to_string()),
        ..Default::default()
    };

    for (_source_path, source_unit) in &output.sources {
        if let Some(ast) = &source_unit.ast {
            extract_from_node(ast, &mut module);
        }
    }

    if module.name.is_empty() {
        module.name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_string();
    }

    Ok(module)
}

fn extract_from_ast_node(ast: &AstNode, path: &PathBuf) -> Result<ExtractedModule> {
    let mut module = ExtractedModule {
        language: "solidity".to_string(),
        source_path: Some(path.to_string_lossy().to_string()),
        ..Default::default()
    };

    extract_from_node(ast, &mut module);

    if module.name.is_empty() {
        module.name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_string();
    }

    Ok(module)
}

fn extract_from_node(node: &AstNode, module: &mut ExtractedModule) {
    match node.node_type.as_str() {
        "SourceUnit" => {
            if let Some(nodes) = &node.nodes {
                for child in nodes {
                    extract_from_node(child, module);
                }
            }
        }
        "ContractDefinition" => {
            if let Some(name) = &node.name {
                if module.name.is_empty() {
                    module.name = name.clone();
                }
            }
            if let Some(nodes) = &node.nodes {
                for child in nodes {
                    extract_from_node(child, module);
                }
            }
        }
        "FunctionDefinition" => {
            if let Some(name) = &node.name {
                // Skip constructor and fallback
                let kind = node.kind.as_deref().unwrap_or("");
                if kind == "constructor" || kind == "fallback" || kind == "receive" {
                    return;
                }

                let visibility = node.visibility.as_deref().unwrap_or("internal");
                let signature = format_function_signature(node);

                module.function_signatures.insert(name.clone(), signature);

                if visibility == "public" || visibility == "external" {
                    module.public_functions.push(name.clone());
                } else {
                    module.private_functions.push(name.clone());
                }
            }
        }
        "EventDefinition" => {
            if let Some(name) = &node.name {
                module.events.push(name.clone());
            }
        }
        "ModifierDefinition" => {
            if let Some(name) = &node.name {
                module.modifiers.push(name.clone());
            }
        }
        "VariableDeclaration" => {
            // State variables at contract level
            if let Some(name) = &node.name {
                module.state_variables.push(name.clone());
            }
        }
        "ImportDirective" => {
            if let Some(abs_path) = &node.absolute_path {
                module.imports.push(abs_path.clone());
            }
        }
        _ => {
            // Recurse into children for other node types
            if let Some(nodes) = &node.nodes {
                for child in nodes {
                    extract_from_node(child, module);
                }
            }
        }
    }
}

fn format_function_signature(node: &AstNode) -> String {
    let params = if let Some(param_list) = &node.parameters {
        param_list
            .parameters
            .iter()
            .map(|p| {
                let type_str = p
                    .type_descriptions
                    .as_ref()
                    .and_then(|td| td.type_string.as_ref())
                    .map(|s| s.as_str())
                    .unwrap_or("unknown");

                if p.name.is_empty() {
                    type_str.to_string()
                } else {
                    format!("{} {}", type_str, p.name)
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        String::new()
    };

    // Extract return types
    let returns = if let Some(return_params) = &node.return_parameters {
        if return_params.parameters.is_empty() {
            None
        } else {
            let ret_types: Vec<String> = return_params
                .parameters
                .iter()
                .map(|p| {
                    p.type_descriptions
                        .as_ref()
                        .and_then(|td| td.type_string.as_ref())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "unknown".to_string())
                })
                .collect();

            if ret_types.len() == 1 {
                Some(ret_types[0].clone())
            } else {
                Some(format!("({})", ret_types.join(", ")))
            }
        }
    } else {
        None
    };

    let params_str = format!("({})", params);

    match returns {
        Some(ret) => format!("{} -> {}", params_str, ret),
        None => params_str,
    }
}

// ============================================================================
// Regex Fallback (when solc is not available)
// ============================================================================

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

    // Extract functions with optional return types
    // Pattern: function name(params) modifiers returns (type)
    let func_re = regex::Regex::new(
        r"function\s+(\w+)\s*\(([^)]*)\)\s*((?:public|external|internal|private|view|pure|payable|virtual|override|\s)*)(?:returns\s*\(([^)]*)\))?",
    )?;

    for cap in func_re.captures_iter(content) {
        let name = cap[1].to_string();
        let params = cap[2].to_string();
        let modifiers = cap[3].to_string();
        let returns = cap.get(4).map(|m| m.as_str().trim().to_string());

        let params_str = format!("({})", params.trim());
        let signature = match returns {
            Some(ret) if !ret.is_empty() => format!("{} -> {}", params_str, ret),
            _ => params_str,
        };
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

    // Extract state variables (simplified)
    let state_re = regex::Regex::new(
        r"^\s*(mapping|address|uint\d*|int\d*|bytes\d*|string|bool)\s+(?:public\s+|private\s+|internal\s+)?(\w+)\s*[;=]",
    )?;
    for cap in state_re.captures_iter(content) {
        module.state_variables.push(cap[2].to_string());
    }

    // Extract modifiers
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
    fn test_extract_solidity_contract_regex() {
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

    #[test]
    fn test_parse_ast_json() {
        // Sample AST JSON from solc with return types
        let ast_json = r#"{
            "nodeType": "SourceUnit",
            "nodes": [
                {
                    "nodeType": "ContractDefinition",
                    "name": "TestContract",
                    "nodes": [
                        {
                            "nodeType": "FunctionDefinition",
                            "name": "publicFunc",
                            "visibility": "public",
                            "kind": "function",
                            "parameters": {
                                "parameters": [
                                    {
                                        "name": "amount",
                                        "typeDescriptions": {
                                            "typeString": "uint256"
                                        }
                                    }
                                ]
                            },
                            "returnParameters": {
                                "parameters": [
                                    {
                                        "name": "",
                                        "typeDescriptions": {
                                            "typeString": "bool"
                                        }
                                    }
                                ]
                            }
                        },
                        {
                            "nodeType": "FunctionDefinition",
                            "name": "privateFunc",
                            "visibility": "private",
                            "kind": "function",
                            "parameters": {
                                "parameters": []
                            }
                        },
                        {
                            "nodeType": "EventDefinition",
                            "name": "Transfer"
                        }
                    ]
                }
            ]
        }"#;

        let ast: AstNode = serde_json::from_str(ast_json).unwrap();
        let mut module = ExtractedModule {
            language: "solidity".to_string(),
            ..Default::default()
        };

        extract_from_node(&ast, &mut module);

        assert_eq!(module.name, "TestContract");
        assert!(module.public_functions.contains(&"publicFunc".to_string()));
        assert!(module
            .private_functions
            .contains(&"privateFunc".to_string()));
        assert!(module.events.contains(&"Transfer".to_string()));

        // Check signature extraction with return type
        let sig = module.function_signatures.get("publicFunc").unwrap();
        assert!(
            sig.contains("(uint256 amount)"),
            "Expected params in signature but got: {}",
            sig
        );
        assert!(
            sig.contains("-> bool"),
            "Expected '-> bool' return type in signature but got: {}",
            sig
        );
    }
}
