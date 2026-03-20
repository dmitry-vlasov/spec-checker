mod rust;
mod solidity;

use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;

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
        lang => Err(anyhow::anyhow!(
            "Unsupported language: {}. Supported: solidity, rust",
            lang
        )),
    }
}
