/// AST types for parsed Flow9 modules.

#[derive(Debug, Clone)]
pub struct Flow9Module {
    pub imports: Vec<String>,
    pub forbids: Vec<String>,
    pub exports: Vec<String>,
    pub declarations: Vec<Decl>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum Decl {
    Import(String),
    Forbid(String),
    Native {
        name: String,
        io: bool,
        type_sig: Flow9Type,
        binding: String,
    },
    Union {
        name: String,
        type_params: Vec<String>,
        variants: Vec<TypeName>,
    },
    Function {
        name: String,
        args: Vec<FunArg>,
        return_type: Option<Flow9Type>,
        body: Option<String>,
        exported: bool,
    },
    FunctionDecl {
        name: String,
        arg_types: Vec<ArgType>,
        return_type: Flow9Type,
        exported: bool,
    },
    StructDecl {
        name: String,
        fields: Vec<FunArg>,
        exported: bool,
    },
    VarDecl {
        name: String,
        type_annotation: Flow9Type,
        has_init: bool,
        exported: bool,
    },
    Assign {
        name: String,
        body: String,
        exported: bool,
    },
}

#[derive(Debug, Clone)]
pub struct FunArg {
    #[allow(dead_code)]
    pub mutable: bool,
    pub name: String,
    pub type_annotation: Option<Flow9Type>,
}

#[derive(Debug, Clone)]
pub struct ArgType {
    pub name: Option<String>,
    pub type_: Flow9Type,
}

#[derive(Debug, Clone)]
pub struct TypeName {
    pub name: String,
    #[allow(dead_code)]
    pub type_params: Vec<Flow9Type>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Flow9Type {
    Named(String),
    TypeVar(String),
    Array(Box<Flow9Type>),
    Ref(Box<Flow9Type>),
    FnType(Vec<Flow9Type>, Box<Flow9Type>),
    Parameterized(String, Vec<Flow9Type>),
    Void,
    Bool,
    Int,
    Double,
    Str,
    Flow,
    NativeType,
}
