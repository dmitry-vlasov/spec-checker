mod flow9;
pub(crate) mod flow9_ast;
pub(crate) mod flow9_parser;
mod rust;
mod solidity;

use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::types::{FunctionInfo, TypeInfo};

pub use flow9::Flow9Extractor;
pub use rust::RustExtractor;
pub use solidity::SolidityExtractor;

/// Information extracted from a source file
#[derive(Debug, Clone, Default)]
pub struct ExtractedModule {
    /// Module/contract name
    pub name: String,

    /// Programming language
    pub language: String,

    /// Source file path
    pub source_path: Option<String>,

    /// Public/external functions
    pub public_functions: Vec<String>,

    /// Private/internal functions
    pub private_functions: Vec<String>,

    /// Function signatures (name -> signature string)
    pub function_signatures: HashMap<String, String>,

    /// Import statements / dependencies
    pub imports: Vec<String>,

    /// Events defined
    pub events: Vec<String>,

    /// State variables
    pub state_variables: Vec<String>,

    /// Modifiers (Solidity) or attributes (Rust)
    pub modifiers: Vec<String>,

    /// Rich type definitions (structs, enums, traits)
    pub type_definitions: HashMap<String, TypeInfo>,

    /// Structured function info (name -> params, return type, generics)
    pub function_info: HashMap<String, FunctionInfo>,
}

/// Trait for language-specific extractors
pub trait Extractor {
    fn extract(&self, path: &PathBuf) -> Result<ExtractedModule>;
}

/// Get the appropriate extractor for a language
pub fn get_extractor(language: &str) -> Result<Box<dyn Extractor>> {
    match language.to_lowercase().as_str() {
        "solidity" | "sol" => Ok(Box::new(SolidityExtractor::new())),
        "rust" | "rs" => Ok(Box::new(RustExtractor::new())),
        "flow9" | "flow" => Ok(Box::new(Flow9Extractor::new())),
        lang => Err(anyhow::anyhow!(
            "Unsupported language: {}. Supported: solidity, rust, flow9",
            lang
        )),
    }
}
