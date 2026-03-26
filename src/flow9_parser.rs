/// Recursive descent parser for Flow9 source files.
///
/// Expects input with comments already stripped.

use crate::flow9_ast::*;

pub fn parse_flow9_source(input: &str) -> Result<Flow9Module, String> {
    let mut parser = Parser::new(input);
    parser.parse()
}

struct Parser<'a> {
    input: &'a str,
    pos: usize,
    original: &'a str,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Parser {
            input,
            pos: 0,
            original: input,
        }
    }

    // ── Utilities ────────────────────────────────────────────────────────

    fn remaining(&self) -> &'a str {
        &self.input[self.pos..]
    }

    fn skip_ws(&mut self) {
        while self.pos < self.input.len() {
            let b = self.input.as_bytes()[self.pos];
            if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<char> {
        self.remaining().chars().next()
    }

    fn peek_str(&self, n: usize) -> &'a str {
        let rem = self.remaining();
        &rem[..rem.len().min(n)]
    }

    fn advance(&mut self, n: usize) {
        self.pos += n;
    }

    fn at_end(&self) -> bool {
        self.pos >= self.input.len()
    }

    fn expect_char(&mut self, c: char) -> Result<(), String> {
        self.skip_ws();
        match self.peek() {
            Some(ch) if ch == c => {
                self.advance(ch.len_utf8());
                Ok(())
            }
            other => Err(format!(
                "Expected '{}' at position {}, found {:?}",
                c, self.pos, other
            )),
        }
    }

    fn expect_keyword(&mut self, kw: &str) -> Result<(), String> {
        let rem = self.remaining();
        if rem.starts_with(kw) {
            // Check that the keyword is not followed by a letter/digit/_
            let after = rem.as_bytes().get(kw.len()).copied();
            if let Some(b) = after {
                if b.is_ascii_alphanumeric() || b == b'_' {
                    return Err(format!(
                        "Expected keyword '{}' at position {}, but it continues",
                        kw, self.pos
                    ));
                }
            }
            self.advance(kw.len());
            Ok(())
        } else {
            Err(format!(
                "Expected keyword '{}' at position {}, found '{}'",
                kw,
                self.pos,
                &rem[..rem.len().min(20)]
            ))
        }
    }

    /// Check if the remaining input starts with a keyword (followed by non-alnum)
    fn at_keyword(&self, kw: &str) -> bool {
        let rem = self.remaining();
        if rem.starts_with(kw) {
            let after = rem.as_bytes().get(kw.len()).copied();
            match after {
                Some(b) => !b.is_ascii_alphanumeric() && b != b'_',
                None => true,
            }
        } else {
            false
        }
    }

    fn is_id_start(c: char) -> bool {
        c.is_ascii_alphabetic() || c == '_'
    }

    fn is_id_continue(c: char) -> bool {
        c.is_ascii_alphanumeric() || c == '_'
    }

    fn parse_id(&mut self) -> Result<String, String> {
        self.skip_ws();
        let start = self.pos;
        let rem = self.remaining();
        let mut chars = rem.chars();
        match chars.next() {
            Some(c) if Self::is_id_start(c) => {
                self.advance(c.len_utf8());
            }
            _ => {
                return Err(format!(
                    "Expected identifier at position {}, found '{}'",
                    self.pos,
                    &rem[..rem.len().min(20)]
                ));
            }
        }
        while let Some(c) = self.peek() {
            if Self::is_id_continue(c) {
                self.advance(c.len_utf8());
            } else {
                break;
            }
        }
        Ok(self.input[start..self.pos].to_string())
    }

    fn parse_path(&mut self) -> Result<String, String> {
        let mut path = self.parse_id()?;
        while self.peek() == Some('/') {
            self.advance(1);
            let seg = self.parse_id()?;
            path.push('/');
            path.push_str(&seg);
        }
        Ok(path)
    }

    /// Parse a dotted name like `Native.println`
    fn parse_dotted_name(&mut self) -> Result<String, String> {
        let mut name = self.parse_id()?;
        self.skip_ws();
        while self.peek() == Some('.') {
            self.advance(1);
            let seg = self.parse_id()?;
            name.push('.');
            name.push_str(&seg);
            self.skip_ws();
        }
        Ok(name)
    }

    // ── Brace / expression body extraction ──────────────────────────────

    /// Starting just after '{', scan to the matching '}'. Returns the body
    /// content (between the braces) and advances past the '}'.
    fn extract_brace_body(&mut self) -> Result<String, String> {
        let start = self.pos;
        let mut depth = 1i32;
        let bytes = self.input.as_bytes();
        let mut in_string = false;
        let mut prev = 0u8;

        while self.pos < bytes.len() {
            let b = bytes[self.pos];
            if in_string {
                if b == b'"' && prev != b'\\' {
                    in_string = false;
                }
                prev = b;
                self.pos += 1;
                continue;
            }
            match b {
                b'"' => {
                    in_string = true;
                    prev = b;
                    self.pos += 1;
                }
                b'{' => {
                    depth += 1;
                    self.pos += 1;
                }
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        let body = self.input[start..self.pos].to_string();
                        self.pos += 1; // skip '}'
                        return Ok(body);
                    }
                    self.pos += 1;
                }
                _ => {
                    self.pos += 1;
                }
            }
            prev = b;
        }
        Err(format!(
            "Unmatched '{{' starting at position {}",
            start - 1
        ))
    }

    /// Extract expression body up to a ';' at depth 0.
    /// Handles nested parens, braces, brackets, and strings.
    fn extract_exp_body(&mut self) -> Result<String, String> {
        let start = self.pos;
        let bytes = self.input.as_bytes();
        let mut depth_paren = 0i32;
        let mut depth_brace = 0i32;
        let mut depth_bracket = 0i32;
        let mut in_string = false;
        let mut prev = 0u8;

        while self.pos < bytes.len() {
            let b = bytes[self.pos];
            if in_string {
                if b == b'"' && prev != b'\\' {
                    in_string = false;
                }
                prev = b;
                self.pos += 1;
                continue;
            }
            match b {
                b'"' => {
                    in_string = true;
                }
                b'(' => depth_paren += 1,
                b')' => depth_paren -= 1,
                b'{' => depth_brace += 1,
                b'}' => {
                    // If we hit a '}' at depth 0, that's the end of an enclosing block
                    // (e.g., export {}), so treat as end of expression too.
                    if depth_brace == 0 {
                        let body = self.input[start..self.pos].trim().to_string();
                        return Ok(body);
                    }
                    depth_brace -= 1;
                }
                b'[' => depth_bracket += 1,
                b']' => depth_bracket -= 1,
                b';' if depth_paren == 0 && depth_brace == 0 && depth_bracket == 0 => {
                    let body = self.input[start..self.pos].trim().to_string();
                    self.pos += 1; // skip ';'
                    return Ok(body);
                }
                _ => {}
            }
            prev = b;
            self.pos += 1;
        }
        // Reached end of input without semicolon — take what we have
        let body = self.input[start..self.pos].trim().to_string();
        Ok(body)
    }

    // ── Type parsing ────────────────────────────────────────────────────

    fn parse_type(&mut self) -> Result<Flow9Type, String> {
        self.skip_ws();

        // Array: [type]
        if self.peek() == Some('[') {
            self.advance(1);
            self.skip_ws();
            let inner = self.parse_type()?;
            self.skip_ws();
            self.expect_char(']')?;
            self.skip_ws();
            return Ok(Flow9Type::Array(Box::new(inner)));
        }

        // Function type or parenthesized type: (...)
        if self.peek() == Some('(') {
            self.advance(1);
            self.skip_ws();
            return self.parse_type_fn_par();
        }

        // Caret: ^type
        if self.peek() == Some('^') {
            self.advance(1);
            self.skip_ws();
            let inner = self.parse_type()?;
            // In flow9, ^ is used for type references; treat as Named("^") wrapping
            // Actually, in practice it's rare. We'll just wrap it.
            return Ok(Flow9Type::Ref(Box::new(inner)));
        }

        // Type variable: ?+
        if self.peek() == Some('?') {
            let start = self.pos;
            while self.peek() == Some('?') {
                self.advance(1);
            }
            self.skip_ws();
            return Ok(Flow9Type::TypeVar(self.input[start..self.pos].trim().to_string()));
        }

        // Keyword types
        if self.at_keyword("bool") {
            self.expect_keyword("bool")?;
            self.skip_ws();
            return Ok(Flow9Type::Bool);
        }
        if self.at_keyword("int") {
            self.expect_keyword("int")?;
            self.skip_ws();
            return Ok(Flow9Type::Int);
        }
        if self.at_keyword("double") {
            self.expect_keyword("double")?;
            self.skip_ws();
            return Ok(Flow9Type::Double);
        }
        if self.at_keyword("string") {
            self.expect_keyword("string")?;
            self.skip_ws();
            return Ok(Flow9Type::Str);
        }
        if self.at_keyword("flow") {
            self.expect_keyword("flow")?;
            self.skip_ws();
            return Ok(Flow9Type::Flow);
        }
        if self.at_keyword("void") {
            self.expect_keyword("void")?;
            self.skip_ws();
            return Ok(Flow9Type::Void);
        }
        if self.at_keyword("native") {
            self.expect_keyword("native")?;
            self.skip_ws();
            return Ok(Flow9Type::NativeType);
        }

        // ref type
        if self.at_keyword("ref") {
            self.expect_keyword("ref")?;
            self.skip_ws();
            let inner = self.parse_type()?;
            return Ok(Flow9Type::Ref(Box::new(inner)));
        }

        // typename with optional type params
        let name = self.parse_id()?;
        self.skip_ws();

        if self.peek() == Some('<') {
            let type_params = self.parse_typelist()?;
            self.skip_ws();
            return Ok(Flow9Type::Parameterized(name, type_params));
        }

        Ok(Flow9Type::Named(name))
    }

    /// Parse after '(' in a type context.
    /// Could be a function type: (argtypes) -> rettype
    /// Or a parenthesized type: (type)
    fn parse_type_fn_par(&mut self) -> Result<Flow9Type, String> {
        self.skip_ws();

        // Empty parens: () -> type
        if self.peek() == Some(')') {
            self.advance(1);
            self.skip_ws();
            if self.peek_str(2) == "->" {
                self.advance(2);
                self.skip_ws();
                let ret = self.parse_type()?;
                return Ok(Flow9Type::FnType(vec![], Box::new(ret)));
            }
            // () as void
            return Ok(Flow9Type::Void);
        }

        // We need to try argtypes first, then fall back to single type.
        // Save position for backtracking.
        let saved = self.pos;

        // Try parsing as argtypes ) -> type
        match self.try_parse_fn_type() {
            Ok(t) => Ok(t),
            Err(_) => {
                // Backtrack and try as parenthesized type
                self.pos = saved;
                let t = self.parse_type()?;
                self.skip_ws();
                // Optional trailing comma
                if self.peek() == Some(',') {
                    self.advance(1);
                    self.skip_ws();
                }
                self.expect_char(')')?;
                self.skip_ws();

                // Check if this is actually a fn type with single arg
                if self.peek_str(2) == "->" {
                    self.advance(2);
                    self.skip_ws();
                    let ret = self.parse_type()?;
                    return Ok(Flow9Type::FnType(vec![t], Box::new(ret)));
                }
                Ok(t)
            }
        }
    }

    /// Try parsing as: argtypes? trailingComma? ) -> type
    fn try_parse_fn_type(&mut self) -> Result<Flow9Type, String> {
        let mut arg_types = Vec::new();

        // Parse first arg type
        if self.peek() != Some(')') {
            let at = self.parse_argtype_type_only()?;
            arg_types.push(at);

            while self.peek() == Some(',') {
                self.advance(1);
                self.skip_ws();
                // trailing comma check
                if self.peek() == Some(')') {
                    break;
                }
                let at = self.parse_argtype_type_only()?;
                arg_types.push(at);
            }
        }

        self.expect_char(')')?;
        self.skip_ws();

        if self.peek_str(2) == "->" {
            self.advance(2);
            self.skip_ws();
            let ret = self.parse_type()?;
            Ok(Flow9Type::FnType(arg_types, Box::new(ret)))
        } else {
            Err(format!("Expected '->' at position {}", self.pos))
        }
    }

    /// Parse an argtype for the purpose of type_fn_par — just get the type part.
    fn parse_argtype_type_only(&mut self) -> Result<Flow9Type, String> {
        self.skip_ws();
        // Try: id ':' type  (named arg type)
        let saved = self.pos;
        if let Ok(_id) = self.parse_id() {
            self.skip_ws();
            if self.peek() == Some(':') {
                self.advance(1);
                self.skip_ws();
                return self.parse_type();
            }
            // Not a named arg. The id might be a type name.
            self.pos = saved;
        }
        self.parse_type()
    }

    fn parse_typelist(&mut self) -> Result<Vec<Flow9Type>, String> {
        self.expect_char('<')?;
        self.skip_ws();
        let mut types = vec![self.parse_type()?];
        while self.peek() == Some(',') {
            self.advance(1);
            self.skip_ws();
            types.push(self.parse_type()?);
        }
        self.expect_char('>')?;
        self.skip_ws();
        Ok(types)
    }

    fn parse_typename(&mut self) -> Result<TypeName, String> {
        let name = self.parse_id()?;
        self.skip_ws();
        let type_params = if self.peek() == Some('<') {
            self.parse_typelist()?
        } else {
            vec![]
        };
        Ok(TypeName { name, type_params })
    }

    fn parse_typenames(&mut self) -> Result<Vec<TypeName>, String> {
        let mut names = vec![self.parse_typename()?];
        self.skip_ws();
        while self.peek() == Some(',') {
            self.advance(1);
            self.skip_ws();
            names.push(self.parse_typename()?);
            self.skip_ws();
        }
        Ok(names)
    }

    fn parse_return_type(&mut self) -> Result<Flow9Type, String> {
        self.skip_ws();
        if self.peek_str(2) != "->" {
            return Err(format!("Expected '->' at position {}", self.pos));
        }
        self.advance(2);
        self.skip_ws();
        self.parse_type()
    }

    // ── Argument parsing ────────────────────────────────────────────────

    fn parse_funarg(&mut self) -> Result<FunArg, String> {
        self.skip_ws();
        let mutable = if self.at_keyword("mutable") {
            self.expect_keyword("mutable")?;
            self.skip_ws();
            true
        } else {
            false
        };
        let name = self.parse_id()?;
        self.skip_ws();
        let type_annotation = if self.peek() == Some(':') {
            self.advance(1);
            self.skip_ws();
            Some(self.parse_type()?)
        } else {
            None
        };
        Ok(FunArg {
            mutable,
            name,
            type_annotation,
        })
    }

    fn parse_funargs(&mut self) -> Result<Vec<FunArg>, String> {
        let mut args = vec![self.parse_funarg()?];
        self.skip_ws();
        while self.peek() == Some(',') {
            self.advance(1);
            self.skip_ws();
            // trailing comma
            if self.peek() == Some(')') {
                break;
            }
            args.push(self.parse_funarg()?);
            self.skip_ws();
        }
        Ok(args)
    }

    fn parse_argtype(&mut self) -> Result<ArgType, String> {
        self.skip_ws();
        // Try: id ':' type
        let saved = self.pos;
        if let Ok(id) = self.parse_id() {
            self.skip_ws();
            if self.peek() == Some(':') {
                self.advance(1);
                self.skip_ws();
                let t = self.parse_type()?;
                return Ok(ArgType {
                    name: Some(id),
                    type_: t,
                });
            }
            // Backtrack — the id was actually a type name
            self.pos = saved;
        }
        let t = self.parse_type()?;
        Ok(ArgType {
            name: None,
            type_: t,
        })
    }

    fn parse_argtypes(&mut self) -> Result<Vec<ArgType>, String> {
        let mut args = vec![self.parse_argtype()?];
        self.skip_ws();
        while self.peek() == Some(',') {
            self.advance(1);
            self.skip_ws();
            // trailing comma
            if self.peek() == Some(')') {
                break;
            }
            args.push(self.parse_argtype()?);
            self.skip_ws();
        }
        Ok(args)
    }

    // ── Top-level parsing ───────────────────────────────────────────────

    fn parse(&mut self) -> Result<Flow9Module, String> {
        // Skip BOM
        if self.remaining().starts_with('\u{FEFF}') {
            self.advance(3); // UTF-8 BOM is 3 bytes
        }
        self.skip_ws();

        let mut module = Flow9Module {
            imports: vec![],
            forbids: vec![],
            exports: vec![],
            declarations: vec![],
        };

        // Parse import/export/forbid blocks
        loop {
            self.skip_ws();
            if self.at_end() {
                break;
            }
            if self.at_keyword("import") {
                let decl = self.parse_import()?;
                if let Decl::Import(ref path) = decl {
                    module.imports.push(path.clone());
                }
                module.declarations.push(decl);
            } else if self.at_keyword("require") {
                self.parse_dynamic_import()?;
                // dynamic imports are not tracked in the module struct
            } else if self.at_keyword("export") {
                let decls = self.parse_export()?;
                for d in decls {
                    match &d {
                        Decl::Function { name, .. }
                        | Decl::FunctionDecl { name, .. }
                        | Decl::StructDecl { name, .. }
                        | Decl::VarDecl { name, .. }
                        | Decl::Assign { name, .. }
                        | Decl::Native { name, .. }
                        | Decl::Union { name, .. } => {
                            if !module.exports.contains(name) {
                                module.exports.push(name.clone());
                            }
                        }
                        Decl::Import(_) | Decl::Forbid(_) => {}
                    }
                    module.declarations.push(d);
                }
            } else if self.at_keyword("forbid") {
                let decl = self.parse_forbid()?;
                if let Decl::Forbid(ref path) = decl {
                    module.forbids.push(path.clone());
                }
                module.declarations.push(decl);
            } else {
                break;
            }
        }

        // Parse top-level declarations
        loop {
            self.skip_ws();
            if self.at_end() {
                break;
            }
            match self.parse_toplevel_decl(false) {
                Ok(decl) => {
                    module.declarations.push(decl);
                }
                Err(e) => {
                    // If we can't parse any more, stop gracefully.
                    // Skip forward to try to recover.
                    if self.at_end() {
                        break;
                    }
                    // Try to skip to next statement
                    if !self.skip_to_next_toplevel() {
                        return Err(e);
                    }
                }
            }
        }

        Ok(module)
    }

    /// Skip forward past the current problematic declaration to try to continue parsing.
    /// Returns true if we managed to advance.
    fn skip_to_next_toplevel(&mut self) -> bool {
        let start = self.pos;
        let bytes = self.input.as_bytes();
        let mut depth_brace = 0i32;
        let mut in_string = false;
        let mut prev = 0u8;

        while self.pos < bytes.len() {
            let b = bytes[self.pos];
            if in_string {
                if b == b'"' && prev != b'\\' {
                    in_string = false;
                }
                prev = b;
                self.pos += 1;
                continue;
            }
            match b {
                b'"' => in_string = true,
                b'{' => depth_brace += 1,
                b'}' => {
                    if depth_brace > 0 {
                        depth_brace -= 1;
                    }
                    if depth_brace == 0 {
                        self.pos += 1;
                        self.skip_ws();
                        // Skip optional trailing semicolon
                        if self.peek() == Some(';') {
                            self.advance(1);
                        }
                        self.skip_ws();
                        return self.pos > start;
                    }
                }
                b';' if depth_brace == 0 => {
                    self.pos += 1;
                    self.skip_ws();
                    return self.pos > start;
                }
                _ => {}
            }
            prev = b;
            self.pos += 1;
        }
        self.pos > start
    }

    fn parse_import(&mut self) -> Result<Decl, String> {
        self.expect_keyword("import")?;
        self.skip_ws();
        let path = self.parse_path()?;
        self.skip_ws();
        self.expect_char(';')?;
        self.skip_ws();
        Ok(Decl::Import(path))
    }

    fn parse_dynamic_import(&mut self) -> Result<(), String> {
        self.expect_keyword("require")?;
        self.skip_ws();
        let _path = self.parse_path()?;
        self.skip_ws();
        self.expect_char(';')?;
        self.skip_ws();
        Ok(())
    }

    fn parse_forbid(&mut self) -> Result<Decl, String> {
        self.expect_keyword("forbid")?;
        self.skip_ws();
        let path = self.parse_path()?;
        self.skip_ws();
        self.expect_char(';')?;
        self.skip_ws();
        Ok(Decl::Forbid(path))
    }

    fn parse_export(&mut self) -> Result<Vec<Decl>, String> {
        self.expect_keyword("export")?;
        self.skip_ws();
        self.expect_char('{')?;
        self.skip_ws();

        let mut decls = Vec::new();
        while self.peek() != Some('}') && !self.at_end() {
            match self.parse_toplevel_decl(true) {
                Ok(decl) => decls.push(decl),
                Err(_) => {
                    // Try to skip past the problematic declaration within export
                    if !self.skip_to_next_toplevel_or_close_brace() {
                        break;
                    }
                }
            }
            self.skip_ws();
        }

        self.expect_char('}')?;
        self.skip_ws();
        Ok(decls)
    }

    /// Like skip_to_next_toplevel but also stops at '}'
    fn skip_to_next_toplevel_or_close_brace(&mut self) -> bool {
        let start = self.pos;
        let bytes = self.input.as_bytes();
        let mut depth_brace = 0i32;
        let mut in_string = false;
        let mut prev = 0u8;

        while self.pos < bytes.len() {
            let b = bytes[self.pos];
            if in_string {
                if b == b'"' && prev != b'\\' {
                    in_string = false;
                }
                prev = b;
                self.pos += 1;
                continue;
            }
            match b {
                b'"' => in_string = true,
                b'{' => depth_brace += 1,
                b'}' => {
                    if depth_brace == 0 {
                        // Don't consume — the caller (parse_export) needs to see it
                        return self.pos > start;
                    }
                    depth_brace -= 1;
                }
                b';' if depth_brace == 0 => {
                    self.pos += 1;
                    self.skip_ws();
                    return self.pos > start;
                }
                _ => {}
            }
            prev = b;
            self.pos += 1;
        }
        self.pos > start
    }

    fn parse_toplevel_decl(&mut self, exported: bool) -> Result<Decl, String> {
        self.skip_ws();

        // native
        if self.at_keyword("native") {
            return self.parse_native(exported);
        }

        // We need an identifier to proceed
        let saved = self.pos;
        let id = self.parse_id()?;
        self.skip_ws();

        // union: id typelist? ::= typenames ;
        if self.peek_str(3) == "::=" || {
            // Check if there's a typelist then ::=
            let probe = self.pos;
            let has_typelist = self.peek() == Some('<');
            if has_typelist {
                // peek ahead past typelist
                self.skip_typelist_probe();
                self.skip_ws();
            }
            let is_union = self.peek_str(3) == "::=";
            self.pos = probe;
            is_union
        } {
            self.pos = saved;
            return self.parse_union(exported);
        }

        // Peek at what follows the id
        match self.peek() {
            Some('(') => {
                // function or struct constructor
                self.pos = saved;
                return self.parse_function(exported);
            }
            Some(':') => {
                // type decl: functiondecl, structdecl, or vardecl
                self.pos = saved;
                return self.parse_typedecl(exported);
            }
            Some('=') => {
                // Check it's not '=='
                if self.peek_str(2) != "==" {
                    // assign: id = exp ;
                    self.advance(1); // skip '='
                    self.skip_ws();
                    let body = self.extract_exp_body()?;
                    self.skip_ws();
                    return Ok(Decl::Assign {
                        name: id,
                        body,
                        exported,
                    });
                }
            }
            Some('<') => {
                // Could be a union with type params: id<T> ::= ...
                // Already handled above, but double-check
                self.pos = saved;
                return self.parse_union(exported);
            }
            _ => {}
        }

        Err(format!(
            "Unexpected token at position {} after identifier '{}', found '{}'",
            self.pos,
            id,
            &self.remaining()[..self.remaining().len().min(30)]
        ))
    }

    /// Skip past a < ... > block for probing
    fn skip_typelist_probe(&mut self) {
        if self.peek() != Some('<') {
            return;
        }
        self.advance(1);
        let mut depth = 1i32;
        while !self.at_end() && depth > 0 {
            match self.peek() {
                Some('<') => {
                    depth += 1;
                    self.advance(1);
                }
                Some('>') => {
                    depth -= 1;
                    self.advance(1);
                }
                Some(c) => self.advance(c.len_utf8()),
                None => break,
            }
        }
    }

    fn parse_native(&mut self, _exported: bool) -> Result<Decl, String> {
        self.expect_keyword("native")?;
        self.skip_ws();
        let name = self.parse_id()?;
        self.skip_ws();
        self.expect_char(':')?;
        self.skip_ws();

        let io = if self.at_keyword("io") {
            self.expect_keyword("io")?;
            self.skip_ws();
            true
        } else {
            false
        };

        let type_sig = self.parse_type()?;
        self.skip_ws();
        self.expect_char('=')?;
        self.skip_ws();
        let binding = self.parse_dotted_name()?;
        self.skip_ws();
        self.expect_char(';')?;
        self.skip_ws();

        Ok(Decl::Native {
            name,
            io,
            type_sig,
            binding,
        })
    }

    fn parse_union(&mut self, _exported: bool) -> Result<Decl, String> {
        let name = self.parse_id()?;
        self.skip_ws();

        let type_params = if self.peek() == Some('<') {
            self.advance(1);
            self.skip_ws();
            let mut params = Vec::new();
            // Parse type param names (usually ?, ??, or identifiers)
            loop {
                self.skip_ws();
                if self.peek() == Some('>') {
                    break;
                }
                let start = self.pos;
                // Type params in flow9 unions are type variables like ?, ??, or named
                if self.peek() == Some('?') {
                    while self.peek() == Some('?') {
                        self.advance(1);
                    }
                    params.push(self.input[start..self.pos].to_string());
                } else {
                    let id = self.parse_id()?;
                    params.push(id);
                }
                self.skip_ws();
                if self.peek() == Some(',') {
                    self.advance(1);
                    self.skip_ws();
                }
            }
            self.expect_char('>')?;
            self.skip_ws();
            params
        } else {
            vec![]
        };

        if self.peek_str(3) != "::=" {
            return Err(format!("Expected '::=' at position {}", self.pos));
        }
        self.advance(3);
        self.skip_ws();

        let variants = self.parse_typenames()?;
        self.skip_ws();
        if self.peek() == Some(';') {
            self.advance(1);
        }
        self.skip_ws();

        Ok(Decl::Union {
            name,
            type_params,
            variants,
        })
    }

    fn parse_function(&mut self, exported: bool) -> Result<Decl, String> {
        let name = self.parse_id()?;
        self.skip_ws();
        self.expect_char('(')?;
        self.skip_ws();

        // Parse args
        let args = if self.peek() == Some(')') {
            vec![]
        } else {
            self.parse_funargs()?
        };
        self.skip_ws();
        self.expect_char(')')?;
        self.skip_ws();

        // Now: returnType? and then either brace, semicolon, or expression body
        let return_type = if self.peek_str(2) == "->" {
            Some(self.parse_return_type()?)
        } else {
            None
        };
        self.skip_ws();

        // Brace body
        if self.peek() == Some('{') {
            self.advance(1);
            let body = self.extract_brace_body()?;
            self.skip_ws();
            // Optional trailing semicolon
            if self.peek() == Some(';') {
                self.advance(1);
            }
            self.skip_ws();
            return Ok(Decl::Function {
                name,
                args,
                return_type,
                body: Some(body),
                exported,
            });
        }

        // Semicolon — declaration only (no body)
        if self.peek() == Some(';') {
            self.advance(1);
            self.skip_ws();
            return Ok(Decl::Function {
                name,
                args,
                return_type,
                body: None,
                exported,
            });
        }

        // Expression body (function_2 → exp ;? ws)
        let body = self.extract_exp_body()?;
        self.skip_ws();

        Ok(Decl::Function {
            name,
            args,
            return_type,
            body: Some(body),
            exported,
        })
    }

    fn parse_typedecl(&mut self, exported: bool) -> Result<Decl, String> {
        let name = self.parse_id()?;
        self.skip_ws();
        self.expect_char(':')?;
        self.skip_ws();

        // Look ahead to determine which kind of typedecl:
        // functiondecl: name : ( argtypes ) -> type ;
        // structdecl:   name : ( funargs ) ;
        // vardecl:      name : type (= exp)? ;

        if self.peek() == Some('(') {
            // Could be functiondecl or structdecl
            self.advance(1);
            self.skip_ws();

            // Save position for potential backtracking
            let saved = self.pos;

            // Try to parse as functiondecl: (argtypes?) -> type
            // We'll first try to find if there's a -> after the closing paren
            // by parsing content and seeing what follows ')'
            if self.peek() == Some(')') {
                // Empty parens
                self.advance(1);
                self.skip_ws();
                if self.peek_str(2) == "->" {
                    let ret = self.parse_return_type()?;
                    self.skip_ws();
                    if self.peek() == Some(';') {
                        self.advance(1);
                    }
                    self.skip_ws();
                    return Ok(Decl::FunctionDecl {
                        name,
                        arg_types: vec![],
                        return_type: ret,
                        exported,
                    });
                }
                // Empty struct
                if self.peek() == Some(';') {
                    self.advance(1);
                }
                self.skip_ws();
                return Ok(Decl::StructDecl {
                    name,
                    fields: vec![],
                    exported,
                });
            }

            // Non-empty parens. We need to distinguish:
            // functiondecl has argtypes (may have named args: id : type, or plain types)
            // followed by ) -> type
            // structdecl has funargs (id with optional : type) followed by ) ;
            //
            // Key difference: functiondecl has -> after ), structdecl doesn't.
            // Strategy: parse as funargs, look at what follows ')'.

            // Try parsing as argtypes for functiondecl
            let argtypes_result = self.parse_argtypes();
            match argtypes_result {
                Ok(argtypes) => {
                    self.skip_ws();
                    // Optional trailing comma
                    if self.peek() == Some(',') {
                        self.advance(1);
                        self.skip_ws();
                    }
                    if self.peek() == Some(')') {
                        self.advance(1);
                        self.skip_ws();
                        if self.peek_str(2) == "->" {
                            // functiondecl
                            let ret = self.parse_return_type()?;
                            self.skip_ws();
                            if self.peek() == Some(';') {
                                self.advance(1);
                            }
                            self.skip_ws();
                            return Ok(Decl::FunctionDecl {
                                name,
                                arg_types: argtypes,
                                return_type: ret,
                                exported,
                            });
                        }
                        // structdecl — convert argtypes to funargs
                        let fields: Vec<FunArg> = argtypes
                            .into_iter()
                            .map(|at| FunArg {
                                mutable: false,
                                name: at.name.unwrap_or_default(),
                                type_annotation: Some(at.type_),
                            })
                            .collect();
                        self.skip_ws();
                        if self.peek() == Some(';') {
                            self.advance(1);
                        }
                        self.skip_ws();
                        return Ok(Decl::StructDecl {
                            name,
                            fields,
                            exported,
                        });
                    }
                    // Unexpected — try backtrack
                    self.pos = saved;
                }
                Err(_) => {
                    self.pos = saved;
                }
            }

            // Try parsing as funargs for structdecl
            let funargs = self.parse_funargs()?;
            self.skip_ws();
            self.expect_char(')')?;
            self.skip_ws();

            // Check for ->
            if self.peek_str(2) == "->" {
                // It was actually a functiondecl with funarg-style args
                let ret = self.parse_return_type()?;
                let arg_types: Vec<ArgType> = funargs
                    .into_iter()
                    .map(|fa| ArgType {
                        name: Some(fa.name),
                        type_: fa.type_annotation.unwrap_or(Flow9Type::Flow),
                    })
                    .collect();
                self.skip_ws();
                if self.peek() == Some(';') {
                    self.advance(1);
                }
                self.skip_ws();
                return Ok(Decl::FunctionDecl {
                    name,
                    arg_types,
                    return_type: ret,
                    exported,
                });
            }

            if self.peek() == Some(';') {
                self.advance(1);
            }
            self.skip_ws();
            return Ok(Decl::StructDecl {
                name,
                fields: funargs,
                exported,
            });
        }

        // vardecl: name : type (= exp)? ;?
        let type_ann = self.parse_type()?;
        self.skip_ws();

        let has_init = if self.peek() == Some('=') && self.peek_str(2) != "==" {
            self.advance(1);
            self.skip_ws();
            // Skip the expression body
            let _body = self.extract_exp_body()?;
            true
        } else {
            if self.peek() == Some(';') {
                self.advance(1);
            }
            self.skip_ws();
            false
        };

        Ok(Decl::VarDecl {
            name,
            type_annotation: type_ann,
            has_init,
            exported,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_import() {
        let src = "import runtime;";
        let module = parse_flow9_source(src).unwrap();
        assert_eq!(module.imports, vec!["runtime"]);
    }

    #[test]
    fn test_parse_nested_import() {
        let src = "import ds/array;";
        let module = parse_flow9_source(src).unwrap();
        assert_eq!(module.imports, vec!["ds/array"]);
    }

    #[test]
    fn test_parse_forbid() {
        let src = "forbid ds/array;";
        let module = parse_flow9_source(src).unwrap();
        assert_eq!(module.forbids, vec!["ds/array"]);
    }

    #[test]
    fn test_parse_union() {
        let src = "Maybe<?> ::= None, Some<?>;";
        let module = parse_flow9_source(src).unwrap();
        assert_eq!(module.declarations.len(), 1);
        match &module.declarations[0] {
            Decl::Union {
                name,
                type_params,
                variants,
            } => {
                assert_eq!(name, "Maybe");
                assert_eq!(type_params, &vec!["?".to_string()]);
                assert_eq!(variants.len(), 2);
                assert_eq!(variants[0].name, "None");
                assert_eq!(variants[1].name, "Some");
            }
            other => panic!("Expected Union, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_native() {
        let src = "native println : io (?) -> void = Native.println;";
        let module = parse_flow9_source(src).unwrap();
        match &module.declarations[0] {
            Decl::Native {
                name,
                io,
                binding,
                ..
            } => {
                assert_eq!(name, "println");
                assert!(*io);
                assert_eq!(binding, "Native.println");
            }
            other => panic!("Expected Native, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_function_with_brace_body() {
        let src = "isNone(m : Maybe<?>) -> bool { switch (m : Maybe) { None(): true; Some(__): false; } }";
        let module = parse_flow9_source(src).unwrap();
        match &module.declarations[0] {
            Decl::Function {
                name,
                args,
                return_type,
                body,
                ..
            } => {
                assert_eq!(name, "isNone");
                assert_eq!(args.len(), 1);
                assert_eq!(args[0].name, "m");
                assert!(return_type.is_some());
                assert!(body.is_some());
            }
            other => panic!("Expected Function, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_function_decl() {
        let src = "isNone : (m : Maybe<?>) -> bool;";
        let module = parse_flow9_source(src).unwrap();
        match &module.declarations[0] {
            Decl::FunctionDecl {
                name,
                arg_types,
                return_type,
                ..
            } => {
                assert_eq!(name, "isNone");
                assert_eq!(arg_types.len(), 1);
                assert_eq!(arg_types[0].name.as_deref(), Some("m"));
                assert_eq!(*return_type, Flow9Type::Bool);
            }
            other => panic!("Expected FunctionDecl, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_struct_decl() {
        let src = "Some : (value : ?);";
        let module = parse_flow9_source(src).unwrap();
        match &module.declarations[0] {
            Decl::StructDecl { name, fields, .. } => {
                assert_eq!(name, "Some");
                assert_eq!(fields.len(), 1);
                assert_eq!(fields[0].name, "value");
            }
            other => panic!("Expected StructDecl, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_export_block() {
        let src = r#"
export {
    Maybe<?> ::= None, Some<?>;
    isNone : (m : Maybe<?>) -> bool;
}
"#;
        let module = parse_flow9_source(src).unwrap();
        assert!(module.exports.contains(&"Maybe".to_string()));
        assert!(module.exports.contains(&"isNone".to_string()));
    }

    #[test]
    fn test_parse_assign() {
        let src = "emptyList = [];";
        let module = parse_flow9_source(src).unwrap();
        match &module.declarations[0] {
            Decl::Assign { name, body, .. } => {
                assert_eq!(name, "emptyList");
                assert_eq!(body, "[]");
            }
            other => panic!("Expected Assign, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_types() {
        let src = "f : (int, string) -> bool;";
        let module = parse_flow9_source(src).unwrap();
        match &module.declarations[0] {
            Decl::FunctionDecl {
                arg_types,
                return_type,
                ..
            } => {
                assert_eq!(arg_types.len(), 2);
                assert_eq!(arg_types[0].type_, Flow9Type::Int);
                assert_eq!(arg_types[1].type_, Flow9Type::Str);
                assert_eq!(*return_type, Flow9Type::Bool);
            }
            other => panic!("Expected FunctionDecl, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_array_type() {
        let src = "f : ([int]) -> [string];";
        let module = parse_flow9_source(src).unwrap();
        match &module.declarations[0] {
            Decl::FunctionDecl {
                arg_types,
                return_type,
                ..
            } => {
                assert_eq!(
                    arg_types[0].type_,
                    Flow9Type::Array(Box::new(Flow9Type::Int))
                );
                assert_eq!(
                    *return_type,
                    Flow9Type::Array(Box::new(Flow9Type::Str))
                );
            }
            other => panic!("Expected FunctionDecl, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_fn_type() {
        let src = "apply : ((int) -> bool, int) -> bool;";
        let module = parse_flow9_source(src).unwrap();
        match &module.declarations[0] {
            Decl::FunctionDecl { arg_types, .. } => {
                assert_eq!(arg_types.len(), 2);
                match &arg_types[0].type_ {
                    Flow9Type::FnType(params, ret) => {
                        assert_eq!(params.len(), 1);
                        assert_eq!(params[0], Flow9Type::Int);
                        assert_eq!(**ret, Flow9Type::Bool);
                    }
                    other => panic!("Expected FnType, got {:?}", other),
                }
            }
            other => panic!("Expected FunctionDecl, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_full_module() {
        let src = r#"
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
        let module = parse_flow9_source(src).unwrap();
        assert_eq!(module.imports, vec!["runtime"]);
        assert!(module.exports.contains(&"Maybe".to_string()));
        assert!(module.exports.contains(&"None".to_string()));
        assert!(module.exports.contains(&"Some".to_string()));
        assert!(module.exports.contains(&"isNone".to_string()));
        assert!(module.exports.contains(&"isSome".to_string()));
        assert!(module.exports.contains(&"either".to_string()));

        // Should have declarations for:
        // import, union, struct None, struct Some,
        // functiondecl isNone, functiondecl isSome, functiondecl either,
        // function isNone, function isSome, function either, function helperPrivate
        let func_count = module
            .declarations
            .iter()
            .filter(|d| matches!(d, Decl::Function { .. }))
            .count();
        assert!(func_count >= 4, "Expected at least 4 functions, got {}", func_count);
    }

    #[test]
    fn test_parse_vardecl() {
        let src = "x : int;";
        let module = parse_flow9_source(src).unwrap();
        match &module.declarations[0] {
            Decl::VarDecl {
                name,
                type_annotation,
                has_init,
                ..
            } => {
                assert_eq!(name, "x");
                assert_eq!(*type_annotation, Flow9Type::Int);
                assert!(!has_init);
            }
            other => panic!("Expected VarDecl, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_vardecl_with_init() {
        let src = "x : int = 42;";
        let module = parse_flow9_source(src).unwrap();
        match &module.declarations[0] {
            Decl::VarDecl {
                name, has_init, ..
            } => {
                assert_eq!(name, "x");
                assert!(has_init);
            }
            other => panic!("Expected VarDecl, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_parameterized_type() {
        let src = "f : (Tree<int>) -> [Tree<string>];";
        let module = parse_flow9_source(src).unwrap();
        match &module.declarations[0] {
            Decl::FunctionDecl {
                arg_types,
                return_type,
                ..
            } => {
                match &arg_types[0].type_ {
                    Flow9Type::Parameterized(name, params) => {
                        assert_eq!(name, "Tree");
                        assert_eq!(params[0], Flow9Type::Int);
                    }
                    other => panic!("Expected Parameterized, got {:?}", other),
                }
                match return_type {
                    Flow9Type::Array(inner) => match inner.as_ref() {
                        Flow9Type::Parameterized(name, _) => assert_eq!(name, "Tree"),
                        other => panic!("Expected Parameterized, got {:?}", other),
                    },
                    other => panic!("Expected Array, got {:?}", other),
                }
            }
            other => panic!("Expected FunctionDecl, got {:?}", other),
        }
    }
}
