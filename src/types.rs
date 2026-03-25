use std::fmt;

/// Structural representation of a type expression as written in source code.
/// This is syntactic, not semantic — `Vec<String>` ≠ `std::vec::Vec<std::string::String>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeRepr {
    /// A simple named type: `u32`, `String`, `Self`
    Named(String),
    /// A parameterized type: `Vec<String>`, `Result<T, E>`
    Applied(Box<TypeRepr>, Vec<TypeRepr>),
    /// A tuple type: `(A, B, C)`
    Tuple(Vec<TypeRepr>),
    /// A reference: `&T` or `&mut T`
    Reference {
        mutable: bool,
        inner: Box<TypeRepr>,
    },
    /// A slice: `[T]`
    Slice(Box<TypeRepr>),
    /// An array: `[T; N]`
    Array(Box<TypeRepr>, usize),
    /// A function pointer: `fn(A, B) -> C`
    FnPointer {
        params: Vec<TypeRepr>,
        ret: Box<TypeRepr>,
    },
    /// The unit type: `()`
    Unit,
    /// An inferred/wildcard type: `_`
    Infer,
}

impl fmt::Display for TypeRepr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeRepr::Named(name) => write!(f, "{}", name),
            TypeRepr::Applied(base, args) => {
                write!(f, "{}<", base)?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", arg)?;
                }
                write!(f, ">")
            }
            TypeRepr::Tuple(elems) => {
                write!(f, "(")?;
                for (i, elem) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", elem)?;
                }
                write!(f, ")")
            }
            TypeRepr::Reference { mutable, inner } => {
                if *mutable {
                    write!(f, "&mut {}", inner)
                } else {
                    write!(f, "&{}", inner)
                }
            }
            TypeRepr::Slice(inner) => write!(f, "[{}]", inner),
            TypeRepr::Array(inner, size) => write!(f, "[{}; {}]", inner, size),
            TypeRepr::FnPointer { params, ret } => {
                write!(f, "fn(")?;
                for (i, param) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", param)?;
                }
                write!(f, ") -> {}", ret)
            }
            TypeRepr::Unit => write!(f, "()"),
            TypeRepr::Infer => write!(f, "_"),
        }
    }
}

/// What kind of type definition this is
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeKind {
    Struct,
    Enum,
    Trait,
    TypeAlias,
}

impl fmt::Display for TypeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeKind::Struct => write!(f, "struct"),
            TypeKind::Enum => write!(f, "enum"),
            TypeKind::Trait => write!(f, "trait"),
            TypeKind::TypeAlias => write!(f, "type"),
        }
    }
}

/// A generic parameter with optional bounds
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenericParam {
    pub name: String,
    pub bounds: Vec<String>,
}

/// Visibility of a field or item
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Private,
}

/// A field in a struct or enum variant
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldInfo {
    pub name: String,
    pub type_repr: TypeRepr,
    pub visibility: Visibility,
}

/// A variant in an enum
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantInfo {
    pub name: String,
    /// Fields of the variant (empty for unit variants, positional names like "0", "1" for tuple variants)
    pub fields: Vec<FieldInfo>,
}

/// Rich type definition extracted from source code
#[derive(Debug, Clone)]
pub struct TypeInfo {
    pub name: String,
    pub kind: TypeKind,
    pub generics: Vec<GenericParam>,
    /// Fields (for structs)
    pub fields: Vec<FieldInfo>,
    /// Variants (for enums)
    pub variants: Vec<VariantInfo>,
    /// Traits implemented by this type (from `impl Trait for Type` blocks)
    pub trait_impls: Vec<String>,
    /// Derive macros (e.g., "Clone", "Serialize")
    pub derives: Vec<String>,
}

/// A function parameter with structured type info
#[derive(Debug, Clone)]
pub struct ParamInfo {
    pub name: Option<String>,
    pub type_repr: TypeRepr,
    pub is_receiver: bool,
}

/// Structured function/method information
#[derive(Debug, Clone)]
pub struct FunctionInfo {
    pub name: String,
    pub params: Vec<ParamInfo>,
    pub return_type: Option<TypeRepr>,
    pub generics: Vec<GenericParam>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_named() {
        assert_eq!(TypeRepr::Named("u32".into()).to_string(), "u32");
    }

    #[test]
    fn display_applied() {
        let t = TypeRepr::Applied(
            Box::new(TypeRepr::Named("Vec".into())),
            vec![TypeRepr::Named("String".into())],
        );
        assert_eq!(t.to_string(), "Vec<String>");
    }

    #[test]
    fn display_nested_applied() {
        let inner = TypeRepr::Applied(
            Box::new(TypeRepr::Named("Vec".into())),
            vec![TypeRepr::Named("String".into())],
        );
        let t = TypeRepr::Applied(
            Box::new(TypeRepr::Named("Result".into())),
            vec![inner, TypeRepr::Named("Error".into())],
        );
        assert_eq!(t.to_string(), "Result<Vec<String>, Error>");
    }

    #[test]
    fn display_reference() {
        let t = TypeRepr::Reference {
            mutable: false,
            inner: Box::new(TypeRepr::Named("str".into())),
        };
        assert_eq!(t.to_string(), "&str");

        let t = TypeRepr::Reference {
            mutable: true,
            inner: Box::new(TypeRepr::Named("Self".into())),
        };
        assert_eq!(t.to_string(), "&mut Self");
    }

    #[test]
    fn display_fn_pointer() {
        let t = TypeRepr::FnPointer {
            params: vec![TypeRepr::Named("u32".into()), TypeRepr::Named("u32".into())],
            ret: Box::new(TypeRepr::Named("bool".into())),
        };
        assert_eq!(t.to_string(), "fn(u32, u32) -> bool");
    }

    #[test]
    fn display_tuple() {
        let t = TypeRepr::Tuple(vec![
            TypeRepr::Named("u32".into()),
            TypeRepr::Named("String".into()),
        ]);
        assert_eq!(t.to_string(), "(u32, String)");
    }

    #[test]
    fn display_array_and_slice() {
        assert_eq!(
            TypeRepr::Slice(Box::new(TypeRepr::Named("u8".into()))).to_string(),
            "[u8]"
        );
        assert_eq!(
            TypeRepr::Array(Box::new(TypeRepr::Named("u8".into())), 32).to_string(),
            "[u8; 32]"
        );
    }

    #[test]
    fn display_unit_and_infer() {
        assert_eq!(TypeRepr::Unit.to_string(), "()");
        assert_eq!(TypeRepr::Infer.to_string(), "_");
    }

    #[test]
    fn type_repr_equality() {
        let a = TypeRepr::Applied(
            Box::new(TypeRepr::Named("Vec".into())),
            vec![TypeRepr::Named("String".into())],
        );
        let b = TypeRepr::Applied(
            Box::new(TypeRepr::Named("Vec".into())),
            vec![TypeRepr::Named("String".into())],
        );
        assert_eq!(a, b);

        let c = TypeRepr::Applied(
            Box::new(TypeRepr::Named("Vec".into())),
            vec![TypeRepr::Named("u8".into())],
        );
        assert_ne!(a, c);
    }
}
