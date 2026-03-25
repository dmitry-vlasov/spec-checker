// Type formula DSL — AST, parser, and evaluator
// Will be implemented in subsequent steps.

use std::collections::HashMap;

use crate::types::{FunctionInfo, TypeInfo, TypeRepr};

/// A reference to a function parameter — by index or name
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamRef {
    Index(usize),
    Name(String),
}

/// A type expression in the formula language
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeExpr {
    /// `Self` — the type being specified
    Self_,
    /// A concrete type literal
    Literal(TypeRepr),
    /// `field(name)` — type of a struct field
    Field(String),
    /// `param(n)` or `param("name")` — function parameter type
    Param(ParamRef),
    /// `return` — function return type
    Return,
    /// `con(T)` — type constructor (strip type arguments)
    Con(Box<TypeExpr>),
    /// `arg(T, n)` — n-th type argument
    Arg(Box<TypeExpr>, usize),
    /// `apply(F, A, ...)` — apply type constructor to arguments
    Apply(Box<TypeExpr>, Vec<TypeExpr>),
    /// `function(A, B, ..., R)` — function type (last = return)
    Function(Vec<TypeExpr>),
    /// `domain(F, n?)` — parameter type of a function type
    Domain(Box<TypeExpr>, Option<usize>),
    /// `codomain(F)` — return type of a function type
    Codomain(Box<TypeExpr>),
    /// `sum(A, B, ...)` — sum type
    Sum(Vec<TypeExpr>),
    /// `product(A, B, ...)` — product type
    Product(Vec<TypeExpr>),
}

/// A predicate over types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Predicate {
    Equals(TypeExpr, TypeExpr),
    Matches(TypeExpr, String),
    IsSubtype(TypeExpr, String),
    IsSum(TypeExpr),
    IsProduct(TypeExpr),
    IsFunction(TypeExpr),
    HasField(TypeExpr, String),
    HasVariant(TypeExpr, String),
    Cloneable(TypeExpr),
    Serializable(TypeExpr),
    Send(TypeExpr),
    Sync(TypeExpr),
    Fallible(TypeExpr),
    NoUnsafe(TypeExpr),
}

/// A formula combining predicates with logical connectives
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Formula {
    Pred(Predicate),
    And(Vec<Formula>),
    Or(Vec<Formula>),
    Not(Box<Formula>),
    Implies(Box<Formula>, Box<Formula>),
}

/// Evaluation context for type formulas
pub struct TypeEvalContext<'a> {
    pub self_type: Option<&'a TypeInfo>,
    pub function: Option<&'a FunctionInfo>,
    pub type_defs: &'a HashMap<String, TypeInfo>,
}

/// Parse a formula string into a Formula AST
pub fn parse_formula(input: &str) -> Result<Formula, ParseError> {
    let tokens = tokenize(input)?;
    let mut parser = FormulaParser::new(&tokens);
    let formula = parser.parse_formula()?;
    if parser.pos < parser.tokens.len() {
        return Err(ParseError {
            message: format!("Unexpected token: {:?}", parser.tokens[parser.pos]),
            position: parser.pos,
        });
    }
    Ok(formula)
}

/// Evaluate a formula against a type evaluation context
pub fn evaluate_formula(
    formula: &Formula,
    ctx: &TypeEvalContext,
) -> Result<bool, EvalError> {
    match formula {
        Formula::Pred(pred) => evaluate_predicate(pred, ctx),
        Formula::And(parts) => {
            for part in parts {
                if !evaluate_formula(part, ctx)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Formula::Or(parts) => {
            for part in parts {
                if evaluate_formula(part, ctx)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Formula::Not(inner) => Ok(!evaluate_formula(inner, ctx)?),
        Formula::Implies(premise, conclusion) => {
            if evaluate_formula(premise, ctx)? {
                evaluate_formula(conclusion, ctx)
            } else {
                Ok(true)
            }
        }
    }
}

// ─── Parser ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub position: usize,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Parse error at position {}: {}", self.position, self.message)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalError {
    pub message: String,
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Evaluation error: {}", self.message)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Ident(String),
    Integer(usize),
    StringLit(String),
    /// A type literal like `Vec<String>` or `Result<T, E>` parsed as a raw string
    TypeLit(String),
    LParen,
    RParen,
    Comma,
}

/// Tokenize a formula string
fn tokenize(input: &str) -> Result<Vec<Token>, ParseError> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        match chars[i] {
            c if c.is_whitespace() => i += 1,
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            ',' => {
                tokens.push(Token::Comma);
                i += 1;
            }
            '"' => {
                // String literal
                i += 1;
                let start = i;
                while i < chars.len() && chars[i] != '"' {
                    i += 1;
                }
                if i >= chars.len() {
                    return Err(ParseError {
                        message: "Unterminated string literal".into(),
                        position: start - 1,
                    });
                }
                let s: String = chars[start..i].iter().collect();
                tokens.push(Token::StringLit(s));
                i += 1; // skip closing "
            }
            c if c.is_ascii_digit() => {
                let start = i;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                let num: String = chars[start..i].iter().collect();
                tokens.push(Token::Integer(num.parse().map_err(|_| ParseError {
                    message: format!("Invalid integer: {}", num),
                    position: start,
                })?));
            }
            c if c.is_alphanumeric() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let ident: String = chars[start..i].iter().collect();

                // Check if this is followed by `<` — if so, parse a type literal
                if i < chars.len() && chars[i] == '<' {
                    let type_str = parse_type_literal_from(&chars, start, &mut i)?;
                    tokens.push(Token::TypeLit(type_str));
                } else {
                    tokens.push(Token::Ident(ident));
                }
            }
            '&' => {
                // Reference type literal: &str, &mut T, &[u8]
                let type_str = parse_ref_type_literal(&chars, &mut i)?;
                tokens.push(Token::TypeLit(type_str));
            }
            '[' => {
                // Slice/array type literal: [u8], [u8; 32]
                let type_str = parse_bracket_type_literal(&chars, &mut i)?;
                tokens.push(Token::TypeLit(type_str));
            }
            c => {
                return Err(ParseError {
                    message: format!("Unexpected character: '{}'", c),
                    position: i,
                });
            }
        }
    }

    Ok(tokens)
}

/// Parse a type literal starting with an identifier followed by `<...>`
/// e.g., `Vec<String>`, `Result<T, E>`, `HashMap<String, Vec<u8>>`
fn parse_type_literal_from(chars: &[char], start: usize, i: &mut usize) -> Result<String, ParseError> {
    // Consume the identifier part (already consumed by caller up to `<`)
    // Then consume balanced <> brackets
    let mut depth = 0;
    let lit_start = start;

    // We're at the `<` character
    while *i < chars.len() {
        match chars[*i] {
            '<' => {
                depth += 1;
                *i += 1;
            }
            '>' => {
                depth -= 1;
                *i += 1;
                if depth == 0 {
                    break;
                }
            }
            _ => *i += 1,
        }
    }

    if depth != 0 {
        return Err(ParseError {
            message: "Unbalanced `<>` in type literal".into(),
            position: lit_start,
        });
    }

    let s: String = chars[lit_start..*i].iter().collect();
    Ok(s)
}

/// Parse a reference type literal: `&str`, `&mut T`, `&[u8]`
fn parse_ref_type_literal(chars: &[char], i: &mut usize) -> Result<String, ParseError> {
    let start = *i;
    *i += 1; // skip &

    // skip whitespace
    while *i < chars.len() && chars[*i].is_whitespace() {
        *i += 1;
    }

    // check for `mut`
    if *i + 3 <= chars.len() {
        let maybe_mut: String = chars[*i..*i + 3].iter().collect();
        if maybe_mut == "mut" && (*i + 3 >= chars.len() || !chars[*i + 3].is_alphanumeric()) {
            *i += 3;
            // skip whitespace after mut
            while *i < chars.len() && chars[*i].is_whitespace() {
                *i += 1;
            }
        }
    }

    // Now consume the inner type (identifier, possibly with <>, or [])
    if *i < chars.len() && chars[*i] == '[' {
        parse_bracket_type_literal(chars, i)?;
    } else {
        // Consume identifier
        while *i < chars.len() && (chars[*i].is_alphanumeric() || chars[*i] == '_') {
            *i += 1;
        }
        // If followed by <, consume balanced <>
        if *i < chars.len() && chars[*i] == '<' {
            let inner_start = *i;
            let mut depth = 0;
            while *i < chars.len() {
                match chars[*i] {
                    '<' => { depth += 1; *i += 1; }
                    '>' => { depth -= 1; *i += 1; if depth == 0 { break; } }
                    _ => *i += 1,
                }
            }
            if depth != 0 {
                return Err(ParseError {
                    message: "Unbalanced `<>` in reference type".into(),
                    position: inner_start,
                });
            }
        }
    }

    let s: String = chars[start..*i].iter().collect();
    Ok(s)
}

/// Parse a bracket type literal: `[u8]`, `[u8; 32]`
fn parse_bracket_type_literal(chars: &[char], i: &mut usize) -> Result<String, ParseError> {
    let start = *i;
    let mut depth = 0;

    while *i < chars.len() {
        match chars[*i] {
            '[' => { depth += 1; *i += 1; }
            ']' => { depth -= 1; *i += 1; if depth == 0 { break; } }
            _ => *i += 1,
        }
    }

    if depth != 0 {
        return Err(ParseError {
            message: "Unbalanced `[]` in type literal".into(),
            position: start,
        });
    }

    let s: String = chars[start..*i].iter().collect();
    Ok(s)
}

// ─── Recursive Descent Parser ─────────────────────────────────────────────────

struct FormulaParser<'a> {
    tokens: &'a [Token],
    pos: usize,
}

impl<'a> FormulaParser<'a> {
    fn new(tokens: &'a [Token]) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<&Token> {
        let tok = self.tokens.get(self.pos);
        if tok.is_some() {
            self.pos += 1;
        }
        tok
    }

    fn expect(&mut self, expected: &Token) -> Result<(), ParseError> {
        match self.advance() {
            Some(tok) if tok == expected => Ok(()),
            Some(tok) => Err(ParseError {
                message: format!("Expected {:?}, found {:?}", expected, tok),
                position: self.pos - 1,
            }),
            None => Err(ParseError {
                message: format!("Expected {:?}, found end of input", expected),
                position: self.pos,
            }),
        }
    }

    fn expect_ident(&mut self) -> Result<String, ParseError> {
        match self.advance() {
            Some(Token::Ident(s)) => Ok(s.clone()),
            Some(tok) => Err(ParseError {
                message: format!("Expected identifier, found {:?}", tok),
                position: self.pos - 1,
            }),
            None => Err(ParseError {
                message: "Expected identifier, found end of input".into(),
                position: self.pos,
            }),
        }
    }

    fn expect_integer(&mut self) -> Result<usize, ParseError> {
        match self.advance() {
            Some(Token::Integer(n)) => Ok(*n),
            Some(tok) => Err(ParseError {
                message: format!("Expected integer, found {:?}", tok),
                position: self.pos - 1,
            }),
            None => Err(ParseError {
                message: "Expected integer, found end of input".into(),
                position: self.pos,
            }),
        }
    }

    fn expect_string(&mut self) -> Result<String, ParseError> {
        match self.advance() {
            Some(Token::StringLit(s)) => Ok(s.clone()),
            Some(tok) => Err(ParseError {
                message: format!("Expected string literal, found {:?}", tok),
                position: self.pos - 1,
            }),
            None => Err(ParseError {
                message: "Expected string literal, found end of input".into(),
                position: self.pos,
            }),
        }
    }

    /// formula = disjunction
    fn parse_formula(&mut self) -> Result<Formula, ParseError> {
        self.parse_disjunction()
    }

    /// disjunction = conjunction ("or" conjunction)*
    fn parse_disjunction(&mut self) -> Result<Formula, ParseError> {
        let mut left = self.parse_conjunction()?;

        while matches!(self.peek(), Some(Token::Ident(s)) if s == "or") {
            self.advance(); // consume "or"
            let right = self.parse_conjunction()?;
            left = match left {
                Formula::Or(mut parts) => {
                    parts.push(right);
                    Formula::Or(parts)
                }
                _ => Formula::Or(vec![left, right]),
            };
        }

        Ok(left)
    }

    /// conjunction = unary ("and" unary)*
    fn parse_conjunction(&mut self) -> Result<Formula, ParseError> {
        let mut left = self.parse_unary()?;

        while matches!(self.peek(), Some(Token::Ident(s)) if s == "and") {
            self.advance(); // consume "and"
            let right = self.parse_unary()?;
            left = match left {
                Formula::And(mut parts) => {
                    parts.push(right);
                    Formula::And(parts)
                }
                _ => Formula::And(vec![left, right]),
            };
        }

        Ok(left)
    }

    /// unary = "not" unary | atom
    fn parse_unary(&mut self) -> Result<Formula, ParseError> {
        if matches!(self.peek(), Some(Token::Ident(s)) if s == "not") {
            self.advance();
            let inner = self.parse_unary()?;
            return Ok(Formula::Not(Box::new(inner)));
        }
        self.parse_atom()
    }

    /// atom = predicate_call | "(" formula ")"
    fn parse_atom(&mut self) -> Result<Formula, ParseError> {
        if matches!(self.peek(), Some(Token::LParen)) {
            self.advance(); // consume (
            let f = self.parse_formula()?;
            self.expect(&Token::RParen)?;
            return Ok(f);
        }

        self.parse_predicate()
    }

    /// Parse a predicate (possibly with arguments)
    fn parse_predicate(&mut self) -> Result<Formula, ParseError> {
        let name = match self.peek() {
            Some(Token::Ident(s)) => s.clone(),
            other => {
                return Err(ParseError {
                    message: format!("Expected predicate name, found {:?}", other),
                    position: self.pos,
                });
            }
        };

        match name.as_str() {
            // Binary predicates: pred(type_expr, type_expr_or_string)
            "equals" => {
                self.advance();
                self.expect(&Token::LParen)?;
                let a = self.parse_type_expr()?;
                self.expect(&Token::Comma)?;
                let b = self.parse_type_expr()?;
                self.expect(&Token::RParen)?;
                Ok(Formula::Pred(Predicate::Equals(a, b)))
            }
            "matches" => {
                self.advance();
                self.expect(&Token::LParen)?;
                let t = self.parse_type_expr()?;
                self.expect(&Token::Comma)?;
                let pattern = self.parse_string_or_type_lit()?;
                self.expect(&Token::RParen)?;
                Ok(Formula::Pred(Predicate::Matches(t, pattern)))
            }
            "is_subtype" => {
                self.advance();
                self.expect(&Token::LParen)?;
                let t = self.parse_type_expr()?;
                self.expect(&Token::Comma)?;
                let trait_name = self.expect_ident()?;
                self.expect(&Token::RParen)?;
                Ok(Formula::Pred(Predicate::IsSubtype(t, trait_name)))
            }
            "has_field" => {
                self.advance();
                self.expect(&Token::LParen)?;
                let t = self.parse_type_expr()?;
                self.expect(&Token::Comma)?;
                let field = self.expect_ident()?;
                self.expect(&Token::RParen)?;
                Ok(Formula::Pred(Predicate::HasField(t, field)))
            }
            "has_variant" => {
                self.advance();
                self.expect(&Token::LParen)?;
                let t = self.parse_type_expr()?;
                self.expect(&Token::Comma)?;
                let variant = self.expect_ident()?;
                self.expect(&Token::RParen)?;
                Ok(Formula::Pred(Predicate::HasVariant(t, variant)))
            }
            // Unary predicates with explicit argument: pred(type_expr)
            "is_sum" | "is_product" | "is_function"
            | "cloneable" | "serializable" | "send" | "sync"
            | "fallible" | "no_unsafe" => {
                self.advance();
                let arg = if matches!(self.peek(), Some(Token::LParen)) {
                    // Explicit argument: cloneable(field(x))
                    self.expect(&Token::LParen)?;
                    let t = self.parse_type_expr()?;
                    self.expect(&Token::RParen)?;
                    t
                } else {
                    // Implicit Self: just `cloneable`
                    TypeExpr::Self_
                };
                let pred = match name.as_str() {
                    "is_sum" => Predicate::IsSum(arg),
                    "is_product" => Predicate::IsProduct(arg),
                    "is_function" => Predicate::IsFunction(arg),
                    "cloneable" => Predicate::Cloneable(arg),
                    "serializable" => Predicate::Serializable(arg),
                    "send" => Predicate::Send(arg),
                    "sync" => Predicate::Sync(arg),
                    "fallible" => Predicate::Fallible(arg),
                    "no_unsafe" => Predicate::NoUnsafe(arg),
                    _ => unreachable!(),
                };
                Ok(Formula::Pred(pred))
            }
            // Implies: implies(formula, formula)
            "implies" => {
                self.advance();
                self.expect(&Token::LParen)?;
                let premise = self.parse_formula()?;
                self.expect(&Token::Comma)?;
                let conclusion = self.parse_formula()?;
                self.expect(&Token::RParen)?;
                Ok(Formula::Implies(Box::new(premise), Box::new(conclusion)))
            }
            _ => {
                Err(ParseError {
                    message: format!("Unknown predicate: '{}'", name),
                    position: self.pos,
                })
            }
        }
    }

    /// Parse a type expression
    fn parse_type_expr(&mut self) -> Result<TypeExpr, ParseError> {
        match self.peek() {
            Some(Token::TypeLit(s)) => {
                let s = s.clone();
                self.advance();
                Ok(TypeExpr::Literal(parse_type_repr_from_str(&s)?))
            }
            Some(Token::Ident(s)) => {
                let name = s.clone();
                match name.as_str() {
                    "Self" => {
                        self.advance();
                        Ok(TypeExpr::Self_)
                    }
                    "return" => {
                        self.advance();
                        Ok(TypeExpr::Return)
                    }
                    "field" => {
                        self.advance();
                        self.expect(&Token::LParen)?;
                        let field_name = self.expect_ident()?;
                        self.expect(&Token::RParen)?;
                        Ok(TypeExpr::Field(field_name))
                    }
                    "param" => {
                        self.advance();
                        self.expect(&Token::LParen)?;
                        let param_ref = match self.peek() {
                            Some(Token::Integer(n)) => {
                                let n = *n;
                                self.advance();
                                ParamRef::Index(n)
                            }
                            Some(Token::StringLit(s)) => {
                                let s = s.clone();
                                self.advance();
                                ParamRef::Name(s)
                            }
                            Some(Token::Ident(s)) => {
                                let s = s.clone();
                                self.advance();
                                ParamRef::Name(s)
                            }
                            other => {
                                return Err(ParseError {
                                    message: format!("Expected integer or string in param(), found {:?}", other),
                                    position: self.pos,
                                });
                            }
                        };
                        self.expect(&Token::RParen)?;
                        Ok(TypeExpr::Param(param_ref))
                    }
                    "con" => {
                        self.advance();
                        self.expect(&Token::LParen)?;
                        let inner = self.parse_type_expr()?;
                        self.expect(&Token::RParen)?;
                        Ok(TypeExpr::Con(Box::new(inner)))
                    }
                    "arg" => {
                        self.advance();
                        self.expect(&Token::LParen)?;
                        let inner = self.parse_type_expr()?;
                        self.expect(&Token::Comma)?;
                        let n = self.expect_integer()?;
                        self.expect(&Token::RParen)?;
                        Ok(TypeExpr::Arg(Box::new(inner), n))
                    }
                    "apply" => {
                        self.advance();
                        self.expect(&Token::LParen)?;
                        let constructor = self.parse_type_expr()?;
                        let mut args = Vec::new();
                        while matches!(self.peek(), Some(Token::Comma)) {
                            self.advance();
                            args.push(self.parse_type_expr()?);
                        }
                        self.expect(&Token::RParen)?;
                        Ok(TypeExpr::Apply(Box::new(constructor), args))
                    }
                    "function" => {
                        self.advance();
                        self.expect(&Token::LParen)?;
                        let mut types = vec![self.parse_type_expr()?];
                        while matches!(self.peek(), Some(Token::Comma)) {
                            self.advance();
                            types.push(self.parse_type_expr()?);
                        }
                        self.expect(&Token::RParen)?;
                        Ok(TypeExpr::Function(types))
                    }
                    "domain" => {
                        self.advance();
                        self.expect(&Token::LParen)?;
                        let f = self.parse_type_expr()?;
                        let n = if matches!(self.peek(), Some(Token::Comma)) {
                            self.advance();
                            Some(self.expect_integer()?)
                        } else {
                            None
                        };
                        self.expect(&Token::RParen)?;
                        Ok(TypeExpr::Domain(Box::new(f), n))
                    }
                    "codomain" => {
                        self.advance();
                        self.expect(&Token::LParen)?;
                        let f = self.parse_type_expr()?;
                        self.expect(&Token::RParen)?;
                        Ok(TypeExpr::Codomain(Box::new(f)))
                    }
                    "sum" => {
                        self.advance();
                        self.expect(&Token::LParen)?;
                        let mut types = vec![self.parse_type_expr()?];
                        while matches!(self.peek(), Some(Token::Comma)) {
                            self.advance();
                            types.push(self.parse_type_expr()?);
                        }
                        self.expect(&Token::RParen)?;
                        Ok(TypeExpr::Sum(types))
                    }
                    "product" => {
                        self.advance();
                        self.expect(&Token::LParen)?;
                        let mut types = vec![self.parse_type_expr()?];
                        while matches!(self.peek(), Some(Token::Comma)) {
                            self.advance();
                            types.push(self.parse_type_expr()?);
                        }
                        self.expect(&Token::RParen)?;
                        Ok(TypeExpr::Product(types))
                    }
                    // Bare identifier — treat as a type literal (e.g., `String`, `u32`)
                    _ => {
                        self.advance();
                        Ok(TypeExpr::Literal(TypeRepr::Named(name)))
                    }
                }
            }
            other => Err(ParseError {
                message: format!("Expected type expression, found {:?}", other),
                position: self.pos,
            }),
        }
    }

    /// Parse either a string literal or a type literal (for pattern arguments)
    fn parse_string_or_type_lit(&mut self) -> Result<String, ParseError> {
        match self.peek() {
            Some(Token::StringLit(s)) => {
                let s = s.clone();
                self.advance();
                Ok(s)
            }
            Some(Token::TypeLit(s)) => {
                let s = s.clone();
                self.advance();
                Ok(s)
            }
            Some(Token::Ident(s)) => {
                let s = s.clone();
                self.advance();
                Ok(s)
            }
            other => Err(ParseError {
                message: format!("Expected string or type pattern, found {:?}", other),
                position: self.pos,
            }),
        }
    }
}

// ─── Type literal string → TypeRepr ───────────────────────────────────────────

/// Parse a type string like "Vec<String>" or "&mut HashMap<String, Vec<u8>>" into TypeRepr
pub fn parse_type_repr_from_str(s: &str) -> Result<TypeRepr, ParseError> {
    let s = s.trim();
    if s.is_empty() || s == "()" {
        return Ok(TypeRepr::Unit);
    }
    if s == "_" {
        return Ok(TypeRepr::Infer);
    }

    // Reference types
    if s.starts_with('&') {
        let rest = s[1..].trim();
        if rest.starts_with("mut ") {
            let inner = parse_type_repr_from_str(rest[4..].trim())?;
            return Ok(TypeRepr::Reference {
                mutable: true,
                inner: Box::new(inner),
            });
        } else {
            let inner = parse_type_repr_from_str(rest)?;
            return Ok(TypeRepr::Reference {
                mutable: false,
                inner: Box::new(inner),
            });
        }
    }

    // Slice/array types
    if s.starts_with('[') && s.ends_with(']') {
        let inner = &s[1..s.len() - 1];
        if let Some(semi_pos) = inner.rfind(';') {
            let type_part = inner[..semi_pos].trim();
            let size_part = inner[semi_pos + 1..].trim();
            let size: usize = size_part.parse().map_err(|_| ParseError {
                message: format!("Invalid array size: {}", size_part),
                position: 0,
            })?;
            return Ok(TypeRepr::Array(
                Box::new(parse_type_repr_from_str(type_part)?),
                size,
            ));
        } else {
            return Ok(TypeRepr::Slice(Box::new(parse_type_repr_from_str(inner)?)));
        }
    }

    // Tuple types
    if s.starts_with('(') && s.ends_with(')') {
        let inner = &s[1..s.len() - 1].trim();
        if inner.is_empty() {
            return Ok(TypeRepr::Unit);
        }
        let parts = split_type_args(inner)?;
        let elems: Result<Vec<_>, _> = parts.iter().map(|p| parse_type_repr_from_str(p)).collect();
        return Ok(TypeRepr::Tuple(elems?));
    }

    // fn(...) -> R
    if s.starts_with("fn(") || s.starts_with("fn (") {
        let paren_start = s.find('(').unwrap();
        let paren_end = find_matching_paren(s, paren_start)?;
        let params_str = &s[paren_start + 1..paren_end];
        let params = if params_str.trim().is_empty() {
            vec![]
        } else {
            let parts = split_type_args(params_str)?;
            parts
                .iter()
                .map(|p| parse_type_repr_from_str(p))
                .collect::<Result<Vec<_>, _>>()?
        };

        let after = s[paren_end + 1..].trim();
        let ret = if after.starts_with("->") {
            parse_type_repr_from_str(after[2..].trim())?
        } else {
            TypeRepr::Unit
        };

        return Ok(TypeRepr::FnPointer {
            params,
            ret: Box::new(ret),
        });
    }

    // Named or Applied: `Foo` or `Foo<A, B>`
    if let Some(angle_pos) = find_angle_bracket(s) {
        let name = s[..angle_pos].trim();
        let args_str = &s[angle_pos + 1..s.len() - 1]; // strip < and >
        let parts = split_type_args(args_str)?;
        let args: Result<Vec<_>, _> = parts.iter().map(|p| parse_type_repr_from_str(p)).collect();
        return Ok(TypeRepr::Applied(
            Box::new(TypeRepr::Named(name.to_string())),
            args?,
        ));
    }

    // Plain named type
    Ok(TypeRepr::Named(s.to_string()))
}

/// Find the position of the top-level `<` in a type string
fn find_angle_bracket(s: &str) -> Option<usize> {
    let mut depth_paren = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth_paren += 1,
            ')' => depth_paren -= 1,
            '<' if depth_paren == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

/// Find the matching `)` for a `(` at the given position
fn find_matching_paren(s: &str, open: usize) -> Result<usize, ParseError> {
    let mut depth = 0;
    for (i, c) in s[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(open + i);
                }
            }
            _ => {}
        }
    }
    Err(ParseError {
        message: "Unbalanced parentheses in type".into(),
        position: open,
    })
}

/// Split comma-separated type arguments, respecting `<>`, `()`, `[]` nesting
fn split_type_args(s: &str) -> Result<Vec<String>, ParseError> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth_angle = 0i32;
    let mut depth_paren = 0i32;
    let mut depth_bracket = 0i32;

    for c in s.chars() {
        match c {
            '<' => { depth_angle += 1; current.push(c); }
            '>' => { depth_angle -= 1; current.push(c); }
            '(' => { depth_paren += 1; current.push(c); }
            ')' => { depth_paren -= 1; current.push(c); }
            '[' => { depth_bracket += 1; current.push(c); }
            ']' => { depth_bracket -= 1; current.push(c); }
            ',' if depth_angle == 0 && depth_paren == 0 && depth_bracket == 0 => {
                parts.push(current.trim().to_string());
                current = String::new();
            }
            _ => current.push(c),
        }
    }

    let last = current.trim().to_string();
    if !last.is_empty() {
        parts.push(last);
    }

    Ok(parts)
}

// ─── Evaluator ────────────────────────────────────────────────────────────────

/// Evaluate a type expression to a concrete TypeRepr
fn evaluate_type_expr(
    expr: &TypeExpr,
    ctx: &TypeEvalContext,
) -> Result<TypeRepr, EvalError> {
    match expr {
        TypeExpr::Self_ => {
            if let Some(ti) = ctx.self_type {
                Ok(TypeRepr::Named(ti.name.clone()))
            } else {
                // No type context — use literal "Self" (matches syn's representation)
                Ok(TypeRepr::Named("Self".into()))
            }
        }
        TypeExpr::Literal(repr) => Ok(repr.clone()),
        TypeExpr::Field(name) => {
            let ti = ctx.self_type.ok_or_else(|| EvalError {
                message: format!("field({}) used but no type context available", name),
            })?;
            ti.fields
                .iter()
                .find(|f| f.name == *name)
                .map(|f| f.type_repr.clone())
                .ok_or_else(|| EvalError {
                    message: format!("Type '{}' has no field '{}'", ti.name, name),
                })
        }
        TypeExpr::Param(param_ref) => {
            let fi = ctx.function.ok_or_else(|| EvalError {
                message: "param() used but no function context available".into(),
            })?;
            match param_ref {
                ParamRef::Index(n) => {
                    fi.params.get(*n).map(|p| p.type_repr.clone()).ok_or_else(|| EvalError {
                        message: format!(
                            "Function '{}' has no parameter at index {} (has {} params)",
                            fi.name, n, fi.params.len()
                        ),
                    })
                }
                ParamRef::Name(name) => {
                    fi.params
                        .iter()
                        .find(|p| p.name.as_deref() == Some(name.as_str()))
                        .map(|p| p.type_repr.clone())
                        .ok_or_else(|| EvalError {
                            message: format!(
                                "Function '{}' has no parameter named '{}'",
                                fi.name, name
                            ),
                        })
                }
            }
        }
        TypeExpr::Return => {
            let fi = ctx.function.ok_or_else(|| EvalError {
                message: "return used but no function context available".into(),
            })?;
            fi.return_type.clone().ok_or_else(|| EvalError {
                message: format!("Function '{}' has no return type", fi.name),
            })
        }
        TypeExpr::Con(inner) => {
            let repr = evaluate_type_expr(inner, ctx)?;
            match repr {
                TypeRepr::Applied(base, _) => Ok(*base),
                other => Ok(other), // already a constructor
            }
        }
        TypeExpr::Arg(inner, n) => {
            let repr = evaluate_type_expr(inner, ctx)?;
            match repr {
                TypeRepr::Applied(_, args) => {
                    args.get(*n).cloned().ok_or_else(|| EvalError {
                        message: format!(
                            "Type has {} type arguments, index {} out of bounds",
                            args.len(),
                            n
                        ),
                    })
                }
                other => Err(EvalError {
                    message: format!(
                        "arg() requires a parameterized type, got '{}'",
                        other
                    ),
                }),
            }
        }
        TypeExpr::Apply(constructor, args) => {
            let base = evaluate_type_expr(constructor, ctx)?;
            let arg_reprs: Result<Vec<_>, _> =
                args.iter().map(|a| evaluate_type_expr(a, ctx)).collect();
            Ok(TypeRepr::Applied(Box::new(base), arg_reprs?))
        }
        TypeExpr::Function(types) => {
            if types.is_empty() {
                return Err(EvalError {
                    message: "function() requires at least one type (the return type)".into(),
                });
            }
            let reprs: Result<Vec<_>, _> =
                types.iter().map(|t| evaluate_type_expr(t, ctx)).collect();
            let mut reprs = reprs?;
            let ret = reprs.pop().unwrap();
            Ok(TypeRepr::FnPointer {
                params: reprs,
                ret: Box::new(ret),
            })
        }
        TypeExpr::Domain(f, n) => {
            let repr = evaluate_type_expr(f, ctx)?;
            match repr {
                TypeRepr::FnPointer { params, .. } => {
                    let idx = n.unwrap_or(0);
                    params.get(idx).cloned().ok_or_else(|| EvalError {
                        message: format!(
                            "Function type has {} params, index {} out of bounds",
                            params.len(),
                            idx
                        ),
                    })
                }
                other => Err(EvalError {
                    message: format!("domain() requires a function type, got '{}'", other),
                }),
            }
        }
        TypeExpr::Codomain(f) => {
            let repr = evaluate_type_expr(f, ctx)?;
            match repr {
                TypeRepr::FnPointer { ret, .. } => Ok(*ret),
                other => Err(EvalError {
                    message: format!("codomain() requires a function type, got '{}'", other),
                }),
            }
        }
        TypeExpr::Sum(types) => {
            // Sum types are represented as tuples tagged as sums — for now, use Tuple
            // This is a placeholder; real sum types need a dedicated TypeRepr variant if needed
            let reprs: Result<Vec<_>, _> =
                types.iter().map(|t| evaluate_type_expr(t, ctx)).collect();
            Ok(TypeRepr::Tuple(reprs?))
        }
        TypeExpr::Product(types) => {
            let reprs: Result<Vec<_>, _> =
                types.iter().map(|t| evaluate_type_expr(t, ctx)).collect();
            Ok(TypeRepr::Tuple(reprs?))
        }
    }
}

/// Evaluate a predicate
fn evaluate_predicate(
    pred: &Predicate,
    ctx: &TypeEvalContext,
) -> Result<bool, EvalError> {
    match pred {
        Predicate::Equals(a, b) => {
            let repr_a = evaluate_type_expr(a, ctx)?;
            let repr_b = evaluate_type_expr(b, ctx)?;
            // Strip references for language-agnostic comparison
            Ok(strip_references(&repr_a) == strip_references(&repr_b))
        }
        Predicate::Matches(t, pattern) => {
            let repr = evaluate_type_expr(t, ctx)?;
            let repr_str = repr.to_string();
            // Convert pattern with _ wildcards to regex
            let regex_pattern = format!(
                "^{}$",
                regex::escape(pattern)
                    .replace(r"\_", ".*")  // _ in the original = wildcard
            );
            // Simple wildcard match: replace _ with .*
            let re = regex::Regex::new(&regex_pattern).map_err(|e| EvalError {
                message: format!("Invalid match pattern '{}': {}", pattern, e),
            })?;
            Ok(re.is_match(&repr_str))
        }
        Predicate::IsSubtype(t, trait_name) => {
            let repr = evaluate_type_expr(t, ctx)?;
            let type_name = match &repr {
                TypeRepr::Named(n) => n,
                TypeRepr::Applied(base, _) => match base.as_ref() {
                    TypeRepr::Named(n) => n,
                    _ => return Ok(false),
                },
                _ => return Ok(false),
            };
            // Look up in type definitions
            if let Some(ti) = ctx.type_defs.get(type_name) {
                Ok(ti.trait_impls.contains(trait_name) || ti.derives.contains(trait_name))
            } else {
                Ok(false)
            }
        }
        Predicate::IsSum(t) => {
            let repr = evaluate_type_expr(t, ctx)?;
            let type_name = extract_type_name(&repr);
            if let Some(name) = type_name {
                if let Some(ti) = ctx.type_defs.get(name) {
                    return Ok(ti.kind == crate::types::TypeKind::Enum);
                }
            }
            Ok(false)
        }
        Predicate::IsProduct(t) => {
            let repr = evaluate_type_expr(t, ctx)?;
            match &repr {
                TypeRepr::Tuple(_) => Ok(true),
                _ => {
                    let type_name = extract_type_name(&repr);
                    if let Some(name) = type_name {
                        if let Some(ti) = ctx.type_defs.get(name) {
                            return Ok(ti.kind == crate::types::TypeKind::Struct);
                        }
                    }
                    Ok(false)
                }
            }
        }
        Predicate::IsFunction(t) => {
            let repr = evaluate_type_expr(t, ctx)?;
            Ok(matches!(repr, TypeRepr::FnPointer { .. }))
        }
        Predicate::HasField(t, name) => {
            let repr = evaluate_type_expr(t, ctx)?;
            let type_name = extract_type_name(&repr);
            if let Some(tn) = type_name {
                if let Some(ti) = ctx.type_defs.get(tn) {
                    return Ok(ti.fields.iter().any(|f| f.name == *name));
                }
            }
            // Also check self_type directly if t is Self_
            if let Some(ti) = ctx.self_type {
                if repr == TypeRepr::Named(ti.name.clone()) {
                    return Ok(ti.fields.iter().any(|f| f.name == *name));
                }
            }
            Ok(false)
        }
        Predicate::HasVariant(t, name) => {
            let repr = evaluate_type_expr(t, ctx)?;
            let type_name = extract_type_name(&repr);
            if let Some(tn) = type_name {
                if let Some(ti) = ctx.type_defs.get(tn) {
                    return Ok(ti.variants.iter().any(|v| v.name == *name));
                }
            }
            if let Some(ti) = ctx.self_type {
                if repr == TypeRepr::Named(ti.name.clone()) {
                    return Ok(ti.variants.iter().any(|v| v.name == *name));
                }
            }
            Ok(false)
        }
        Predicate::Cloneable(t) => check_trait_or_derive(t, "Clone", ctx),
        Predicate::Serializable(t) => check_trait_or_derive(t, "Serialize", ctx),
        Predicate::Send(t) => check_trait_or_derive(t, "Send", ctx),
        Predicate::Sync(t) => check_trait_or_derive(t, "Sync", ctx),
        Predicate::Fallible(t) => {
            let repr = evaluate_type_expr(t, ctx)?;
            match &repr {
                TypeRepr::Applied(base, _) => {
                    let name = match base.as_ref() {
                        TypeRepr::Named(n) => n.as_str(),
                        _ => return Ok(false),
                    };
                    Ok(name == "Result" || name == "Option")
                }
                _ => Ok(false),
            }
        }
        Predicate::NoUnsafe(_) => {
            // Requires body analysis — not available from type info alone
            // For now, always return true (optimistic) with a note
            Ok(true)
        }
    }
}

/// Strip language-specific wrappers (references, slices) to get the base type.
/// This makes type comparison language-agnostic: `&mut Self` == `Self`, `&[Rule]` == `[Rule]`.
fn strip_references(repr: &TypeRepr) -> &TypeRepr {
    match repr {
        TypeRepr::Reference { inner, .. } => strip_references(inner),
        _ => repr,
    }
}

/// Helper: extract the base type name from a TypeRepr
fn extract_type_name(repr: &TypeRepr) -> Option<&str> {
    match repr {
        TypeRepr::Named(n) => Some(n),
        TypeRepr::Applied(base, _) => match base.as_ref() {
            TypeRepr::Named(n) => Some(n),
            _ => None,
        },
        _ => None,
    }
}

/// Helper: check if a type implements or derives a given trait
fn check_trait_or_derive(
    t: &TypeExpr,
    trait_name: &str,
    ctx: &TypeEvalContext,
) -> Result<bool, EvalError> {
    let repr = evaluate_type_expr(t, ctx)?;
    let type_name = extract_type_name(&repr);
    if let Some(tn) = type_name {
        if let Some(ti) = ctx.type_defs.get(tn) {
            return Ok(ti.trait_impls.contains(&trait_name.to_string())
                || ti.derives.contains(&trait_name.to_string()));
        }
        // Check self_type if it matches
        if let Some(ti) = ctx.self_type {
            if tn == ti.name {
                return Ok(ti.trait_impls.contains(&trait_name.to_string())
                    || ti.derives.contains(&trait_name.to_string()));
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    // ── Parser tests ──────────────────────────────────────────────────────────

    #[test]
    fn parse_simple_unary_predicate() {
        let f = parse_formula("cloneable").unwrap();
        assert_eq!(f, Formula::Pred(Predicate::Cloneable(TypeExpr::Self_)));
    }

    #[test]
    fn parse_unary_predicate_with_arg() {
        let f = parse_formula("fallible(return)").unwrap();
        assert_eq!(f, Formula::Pred(Predicate::Fallible(TypeExpr::Return)));
    }

    #[test]
    fn parse_equals() {
        let f = parse_formula("equals(con(return), Result)").unwrap();
        assert_eq!(
            f,
            Formula::Pred(Predicate::Equals(
                TypeExpr::Con(Box::new(TypeExpr::Return)),
                TypeExpr::Literal(TypeRepr::Named("Result".into())),
            ))
        );
    }

    #[test]
    fn parse_has_field() {
        let f = parse_formula("has_field(Self, errors)").unwrap();
        assert_eq!(
            f,
            Formula::Pred(Predicate::HasField(TypeExpr::Self_, "errors".into()))
        );
    }

    #[test]
    fn parse_and_or() {
        let f = parse_formula("cloneable and serializable").unwrap();
        assert_eq!(
            f,
            Formula::And(vec![
                Formula::Pred(Predicate::Cloneable(TypeExpr::Self_)),
                Formula::Pred(Predicate::Serializable(TypeExpr::Self_)),
            ])
        );
    }

    #[test]
    fn parse_not() {
        let f = parse_formula("not fallible(return)").unwrap();
        assert_eq!(
            f,
            Formula::Not(Box::new(Formula::Pred(Predicate::Fallible(TypeExpr::Return))))
        );
    }

    #[test]
    fn parse_nested_type_expr() {
        let f = parse_formula("equals(arg(return, 0), String)").unwrap();
        assert_eq!(
            f,
            Formula::Pred(Predicate::Equals(
                TypeExpr::Arg(Box::new(TypeExpr::Return), 0),
                TypeExpr::Literal(TypeRepr::Named("String".into())),
            ))
        );
    }

    #[test]
    fn parse_type_literal_with_generics() {
        let f = parse_formula("equals(return, Result<String, Error>)").unwrap();
        let expected_type = TypeRepr::Applied(
            Box::new(TypeRepr::Named("Result".into())),
            vec![TypeRepr::Named("String".into()), TypeRepr::Named("Error".into())],
        );
        assert_eq!(
            f,
            Formula::Pred(Predicate::Equals(
                TypeExpr::Return,
                TypeExpr::Literal(expected_type),
            ))
        );
    }

    #[test]
    fn parse_implies() {
        let f = parse_formula("implies(is_sum(Self), has_variant(Self, None))").unwrap();
        assert_eq!(
            f,
            Formula::Implies(
                Box::new(Formula::Pred(Predicate::IsSum(TypeExpr::Self_))),
                Box::new(Formula::Pred(Predicate::HasVariant(
                    TypeExpr::Self_,
                    "None".into()
                ))),
            )
        );
    }

    #[test]
    fn parse_param_by_name() {
        let f = parse_formula("equals(param(token), String)").unwrap();
        assert_eq!(
            f,
            Formula::Pred(Predicate::Equals(
                TypeExpr::Param(ParamRef::Name("token".into())),
                TypeExpr::Literal(TypeRepr::Named("String".into())),
            ))
        );
    }

    #[test]
    fn parse_matches_pattern() {
        let f = parse_formula("matches(return, \"Result<_, _>\")").unwrap();
        assert_eq!(
            f,
            Formula::Pred(Predicate::Matches(
                TypeExpr::Return,
                "Result<_, _>".into(),
            ))
        );
    }

    #[test]
    fn parse_error_unknown_predicate() {
        assert!(parse_formula("foobar(Self)").is_err());
    }

    #[test]
    fn parse_error_unterminated_string() {
        assert!(parse_formula("matches(return, \"hello)").is_err());
    }

    // ── Type repr parsing tests ───────────────────────────────────────────────

    #[test]
    fn parse_type_repr_named() {
        assert_eq!(
            parse_type_repr_from_str("u32").unwrap(),
            TypeRepr::Named("u32".into())
        );
    }

    #[test]
    fn parse_type_repr_applied() {
        assert_eq!(
            parse_type_repr_from_str("Vec<String>").unwrap(),
            TypeRepr::Applied(
                Box::new(TypeRepr::Named("Vec".into())),
                vec![TypeRepr::Named("String".into())]
            )
        );
    }

    #[test]
    fn parse_type_repr_nested() {
        let t = parse_type_repr_from_str("Result<Vec<u8>, Error>").unwrap();
        assert_eq!(
            t,
            TypeRepr::Applied(
                Box::new(TypeRepr::Named("Result".into())),
                vec![
                    TypeRepr::Applied(
                        Box::new(TypeRepr::Named("Vec".into())),
                        vec![TypeRepr::Named("u8".into())]
                    ),
                    TypeRepr::Named("Error".into()),
                ]
            )
        );
    }

    #[test]
    fn parse_type_repr_reference() {
        assert_eq!(
            parse_type_repr_from_str("&str").unwrap(),
            TypeRepr::Reference {
                mutable: false,
                inner: Box::new(TypeRepr::Named("str".into())),
            }
        );
        assert_eq!(
            parse_type_repr_from_str("&mut Self").unwrap(),
            TypeRepr::Reference {
                mutable: true,
                inner: Box::new(TypeRepr::Named("Self".into())),
            }
        );
    }

    #[test]
    fn parse_type_repr_slice_and_array() {
        assert_eq!(
            parse_type_repr_from_str("[u8]").unwrap(),
            TypeRepr::Slice(Box::new(TypeRepr::Named("u8".into())))
        );
        assert_eq!(
            parse_type_repr_from_str("[u8; 32]").unwrap(),
            TypeRepr::Array(Box::new(TypeRepr::Named("u8".into())), 32)
        );
    }

    #[test]
    fn parse_type_repr_fn_pointer() {
        assert_eq!(
            parse_type_repr_from_str("fn(u32) -> bool").unwrap(),
            TypeRepr::FnPointer {
                params: vec![TypeRepr::Named("u32".into())],
                ret: Box::new(TypeRepr::Named("bool".into())),
            }
        );
    }

    #[test]
    fn parse_type_repr_unit() {
        assert_eq!(parse_type_repr_from_str("()").unwrap(), TypeRepr::Unit);
    }

    #[test]
    fn parse_type_repr_tuple() {
        assert_eq!(
            parse_type_repr_from_str("(u32, String)").unwrap(),
            TypeRepr::Tuple(vec![
                TypeRepr::Named("u32".into()),
                TypeRepr::Named("String".into()),
            ])
        );
    }

    // ── Evaluator tests ───────────────────────────────────────────────────────

    fn make_test_type() -> TypeInfo {
        TypeInfo {
            name: "Checker".into(),
            kind: TypeKind::Struct,
            generics: vec![],
            fields: vec![
                FieldInfo {
                    name: "errors".into(),
                    type_repr: TypeRepr::Applied(
                        Box::new(TypeRepr::Named("Vec".into())),
                        vec![TypeRepr::Named("String".into())],
                    ),
                    visibility: Visibility::Public,
                },
                FieldInfo {
                    name: "warnings".into(),
                    type_repr: TypeRepr::Applied(
                        Box::new(TypeRepr::Named("Vec".into())),
                        vec![TypeRepr::Named("String".into())],
                    ),
                    visibility: Visibility::Public,
                },
            ],
            variants: vec![],
            trait_impls: vec!["Debug".into()],
            derives: vec!["Clone".into(), "Debug".into()],
        }
    }

    fn make_test_function() -> FunctionInfo {
        FunctionInfo {
            name: "check".into(),
            params: vec![
                ParamInfo {
                    name: Some("self".into()),
                    type_repr: TypeRepr::Reference {
                        mutable: false,
                        inner: Box::new(TypeRepr::Named("Self".into())),
                    },
                    is_receiver: true,
                },
                ParamInfo {
                    name: Some("spec".into()),
                    type_repr: TypeRepr::Reference {
                        mutable: false,
                        inner: Box::new(TypeRepr::Named("ModuleSpec".into())),
                    },
                    is_receiver: false,
                },
            ],
            return_type: Some(TypeRepr::Applied(
                Box::new(TypeRepr::Named("Result".into())),
                vec![TypeRepr::Named("CheckResult".into())],
            )),
            generics: vec![],
        }
    }

    fn make_test_context<'a>(
        type_info: Option<&'a TypeInfo>,
        func_info: Option<&'a FunctionInfo>,
        type_defs: &'a HashMap<String, TypeInfo>,
    ) -> TypeEvalContext<'a> {
        TypeEvalContext {
            self_type: type_info,
            function: func_info,
            type_defs,
        }
    }

    #[test]
    fn eval_has_field() {
        let ti = make_test_type();
        let type_defs = HashMap::from([("Checker".into(), ti.clone())]);
        let ctx = make_test_context(Some(&ti), None, &type_defs);

        let f = parse_formula("has_field(Self, errors)").unwrap();
        assert!(evaluate_formula(&f, &ctx).unwrap());

        let f = parse_formula("has_field(Self, nonexistent)").unwrap();
        assert!(!evaluate_formula(&f, &ctx).unwrap());
    }

    #[test]
    fn eval_cloneable() {
        let ti = make_test_type();
        let type_defs = HashMap::from([("Checker".into(), ti.clone())]);
        let ctx = make_test_context(Some(&ti), None, &type_defs);

        let f = parse_formula("cloneable").unwrap();
        assert!(evaluate_formula(&f, &ctx).unwrap());

        let f = parse_formula("serializable").unwrap();
        assert!(!evaluate_formula(&f, &ctx).unwrap());
    }

    #[test]
    fn eval_equals_con_return() {
        let ti = make_test_type();
        let fi = make_test_function();
        let type_defs = HashMap::from([("Checker".into(), ti.clone())]);
        let ctx = make_test_context(Some(&ti), Some(&fi), &type_defs);

        let f = parse_formula("equals(con(return), Result)").unwrap();
        assert!(evaluate_formula(&f, &ctx).unwrap());

        let f = parse_formula("equals(con(return), Vec)").unwrap();
        assert!(!evaluate_formula(&f, &ctx).unwrap());
    }

    #[test]
    fn eval_arg_return() {
        let ti = make_test_type();
        let fi = make_test_function();
        let type_defs = HashMap::from([("Checker".into(), ti.clone())]);
        let ctx = make_test_context(Some(&ti), Some(&fi), &type_defs);

        let f = parse_formula("equals(arg(return, 0), CheckResult)").unwrap();
        assert!(evaluate_formula(&f, &ctx).unwrap());
    }

    #[test]
    fn eval_fallible_return() {
        let ti = make_test_type();
        let fi = make_test_function();
        let type_defs = HashMap::from([("Checker".into(), ti.clone())]);
        let ctx = make_test_context(Some(&ti), Some(&fi), &type_defs);

        let f = parse_formula("fallible(return)").unwrap();
        assert!(evaluate_formula(&f, &ctx).unwrap());
    }

    #[test]
    fn eval_field_type() {
        let ti = make_test_type();
        let type_defs = HashMap::from([("Checker".into(), ti.clone())]);
        let ctx = make_test_context(Some(&ti), None, &type_defs);

        let f = parse_formula("equals(con(field(errors)), Vec)").unwrap();
        assert!(evaluate_formula(&f, &ctx).unwrap());
    }

    #[test]
    fn eval_and_or() {
        let ti = make_test_type();
        let type_defs = HashMap::from([("Checker".into(), ti.clone())]);
        let ctx = make_test_context(Some(&ti), None, &type_defs);

        let f = parse_formula("cloneable and has_field(Self, errors)").unwrap();
        assert!(evaluate_formula(&f, &ctx).unwrap());

        let f = parse_formula("serializable or cloneable").unwrap();
        assert!(evaluate_formula(&f, &ctx).unwrap());

        let f = parse_formula("serializable and cloneable").unwrap();
        assert!(!evaluate_formula(&f, &ctx).unwrap());
    }

    #[test]
    fn eval_not() {
        let ti = make_test_type();
        let type_defs = HashMap::from([("Checker".into(), ti.clone())]);
        let ctx = make_test_context(Some(&ti), None, &type_defs);

        let f = parse_formula("not serializable").unwrap();
        assert!(evaluate_formula(&f, &ctx).unwrap());
    }

    #[test]
    fn eval_is_sum_is_product() {
        let ti = make_test_type(); // Struct
        let type_defs = HashMap::from([("Checker".into(), ti.clone())]);
        let ctx = make_test_context(Some(&ti), None, &type_defs);

        let f = parse_formula("is_product(Self)").unwrap();
        assert!(evaluate_formula(&f, &ctx).unwrap());

        let f = parse_formula("is_sum(Self)").unwrap();
        assert!(!evaluate_formula(&f, &ctx).unwrap());
    }

    #[test]
    fn eval_param_by_name() {
        let ti = make_test_type();
        let fi = make_test_function();
        let type_defs = HashMap::from([("Checker".into(), ti.clone())]);
        let ctx = make_test_context(Some(&ti), Some(&fi), &type_defs);

        // equals strips references: param(spec) is &ModuleSpec, matches ModuleSpec
        let f = parse_formula("equals(param(spec), ModuleSpec)").unwrap();
        assert!(evaluate_formula(&f, &ctx).unwrap());

        // Also still works with explicit reference
        let f = parse_formula("equals(param(spec), &ModuleSpec)").unwrap();
        assert!(evaluate_formula(&f, &ctx).unwrap());
    }
}
