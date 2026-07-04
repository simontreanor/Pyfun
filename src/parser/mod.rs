//! Recursive-descent parser for the Phase 1 Pyfun subset.
//!
//! Grammar (informal):
//! ```text
//! module      := item*
//! item        := let | expr
//! let         := "let" ["mut"] ident ident* "=" (block | expr)
//! block       := INDENT stmt (SEP stmt)* DEDENT      // last stmt is the value
//! stmt        := let | expr
//! expr        := assign
//! assign      := head ("<-" expr)?                   // target must be a variable
//! head        := fun | if | match | pipe
//! fun         := "fun" ident+ "->" expr
//! if          := "if" expr "then" expr "else" expr
//! match       := "match" expr "with" ("|" pattern "->" expr)+
//! pipe        := or ("|>" or)*
//! or          := and ("or" and)*
//! and         := not ("and" not)*
//! not         := "not" not | comparison
//! comparison  := additive (("=="|"!="|"<"|">"|"<="|">=") additive)*
//! additive    := multiplicative (("+"|"-") multiplicative)*
//! multiplicative := application (("*"|"/"|"//") application)*
//! application := postfix postfix*    // juxtaposition; curried, left-assoc
//! postfix     := atom ("." ident)*   // record field access, binds tightest
//! atom        := int | float | string | "true" | "false" | ident | "(" expr ")"
//!              | record
//! record      := "{" ident "=" expr ("," ident "=" expr)* "}"   // literal
//!              | "{" expr "with" ident "=" expr ("," ...)* "}"   // update
//! pattern     := ctor | atom_pattern
//! ctor        := UpperIdent atom_pattern*
//! atom_pattern:= "_" | ident | int | "true" | "false" | "(" pattern ")"
//! ```
//! Operator precedence, lowest to highest:
//! `|>` < `or` < `and` < `not` < comparison/equality < `+ -` < `* / //` < application.

pub mod ast;

use crate::lexer::{FStrPart, Span, Tok, Token};
use ast::{
    BinOp, BlockStmt, CeBuilder, CeItem, Expr, ExprKind, ExternDecl, FieldDecl, FieldInit,
    FieldPattern, InterpPart, Item, LetBinding, MatchArm, Module, NodeSpan, Param, Pattern,
    TypeDecl, TypeDeclKind, TypeExpr, UnOp, UnitExpr, VariantDecl,
};

/// An error produced during parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} (at {}..{})",
            self.message, self.span.start, self.span.end
        )
    }
}

/// Parse a token stream (terminated by [`Tok::Eof`]) into a [`Module`].
pub fn parse(tokens: Vec<Token>) -> Result<Module, ParseError> {
    Parser { tokens, pos: 0 }.parse_module()
}

/// Parse a token stream into a [`Module`], **recovering** from errors at item
/// boundaries. Always returns a module — of the items that parsed — together with
/// every error encountered, so a single broken `let` no longer hides the rest of
/// the file. This is the entry point the editor tooling uses ([`crate::analyze`]);
/// the compiler keeps the strict [`parse`] (it must reject any broken program).
pub fn parse_recover(tokens: Vec<Token>) -> (Module, Vec<ParseError>) {
    Parser { tokens, pos: 0 }.parse_module_recover()
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.tokens[self.pos].tok
    }

    fn peek2(&self) -> &Tok {
        &self.tokens[(self.pos + 1).min(self.tokens.len() - 1)].tok
    }

    fn peek3(&self) -> &Tok {
        &self.tokens[(self.pos + 2).min(self.tokens.len() - 1)].tok
    }

    fn peek4(&self) -> &Tok {
        &self.tokens[(self.pos + 3).min(self.tokens.len() - 1)].tok
    }

    fn span(&self) -> Span {
        self.tokens[self.pos].span
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek(), Tok::Eof)
    }

    /// Consume and return the current token kind.
    fn bump(&mut self) -> Tok {
        let tok = self.tokens[self.pos].tok.clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn eat(&mut self, expected: &Tok) -> bool {
        if self.peek() == expected {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: &Tok, what: &str) -> Result<(), ParseError> {
        if self.eat(expected) {
            Ok(())
        } else {
            Err(self.error(&format!("expected {what}")))
        }
    }

    fn error(&self, message: &str) -> ParseError {
        ParseError {
            message: format!("{message}, found {}", describe(self.peek())),
            span: self.span(),
        }
    }

    /// Start offset of the current (next-to-consume) token.
    fn cur_start(&self) -> usize {
        self.tokens[self.pos].span.start
    }

    /// End offset of the most recently consumed token.
    fn prev_end(&self) -> usize {
        if self.pos == 0 {
            0
        } else {
            self.tokens[self.pos - 1].span.end
        }
    }

    /// Build an expression spanning from `start` to the end of the last token.
    fn mk(&self, start: usize, kind: ExprKind) -> Expr {
        Expr::new(kind, Span::new(start, self.prev_end()))
    }

    // ----- grammar -----

    fn parse_module(&mut self) -> Result<Module, ParseError> {
        let mut items = Vec::new();
        self.skip_seps();
        while !self.at_eof() {
            let doc = self.collect_docs();
            if self.at_eof() {
                break; // trailing doc lines with no declaration to attach to
            }
            let mut item = self.parse_item()?;
            attach_doc(&mut item, doc);
            items.push(item);
            // An item is delimited by the offside-inserted separators (or EOF).
            self.skip_seps();
        }
        Ok(Module { items })
    }

    /// Consume a run of doc-comment lines (`Tok::Doc`, separated by the offside
    /// `Sep`s) preceding a top-level item, joining them with `\n`. `None` when the
    /// next token is not a doc comment.
    fn collect_docs(&mut self) -> Option<String> {
        let mut lines: Vec<String> = Vec::new();
        while let Tok::Doc(text) = self.peek() {
            lines.push(text.clone());
            self.bump();
            self.skip_seps();
        }
        if lines.is_empty() {
            None
        } else {
            Some(lines.join("\n"))
        }
    }

    /// Error-recovering variant of [`Self::parse_module`]: parse items one by one,
    /// and on a failure record the error, ensure forward progress, then
    /// [`synchronize`](Self::synchronize) to the next item boundary and carry on.
    /// The result is the module of the items that did parse plus all the errors.
    fn parse_module_recover(&mut self) -> (Module, Vec<ParseError>) {
        let mut items = Vec::new();
        let mut errors = Vec::new();
        self.skip_seps();
        while !self.at_eof() {
            let doc = self.collect_docs();
            if self.at_eof() {
                break; // trailing doc lines with no declaration to attach to
            }
            let before = self.pos;
            match self.parse_item() {
                Ok(mut item) => {
                    attach_doc(&mut item, doc);
                    items.push(item);
                }
                Err(e) => {
                    errors.push(e);
                    // Guarantee progress even if `parse_item` consumed nothing,
                    // then skip ahead to the next plausible item start.
                    if self.pos == before {
                        self.bump();
                    }
                    self.synchronize();
                }
            }
            self.skip_seps();
        }
        (Module { items }, errors)
    }

    /// Skip tokens after a failed item until the next top-level item boundary: a
    /// statement separator at block depth 0 (left for [`skip_seps`](Self::skip_seps)
    /// to consume) or end of input. `Indent`/`Dedent` are tracked so a separator
    /// *inside* a broken block is not mistaken for the boundary.
    fn synchronize(&mut self) {
        let mut depth = 0i32;
        loop {
            match self.peek() {
                Tok::Eof => return,
                Tok::Sep if depth <= 0 => return,
                Tok::Indent => depth += 1,
                Tok::Dedent => depth -= 1,
                _ => {}
            }
            self.bump();
        }
    }

    /// Consume any statement separators between top-level items.
    fn skip_seps(&mut self) {
        while matches!(self.peek(), Tok::Sep) {
            self.bump();
        }
    }

    fn parse_item(&mut self) -> Result<Item, ParseError> {
        match self.peek() {
            Tok::Measure => self.parse_measure(),
            Tok::Type => Ok(Item::Type(self.parse_type_decl()?)),
            Tok::Extern => Ok(Item::Extern(self.parse_extern()?)),
            Tok::Module => self.parse_module_item(),
            Tok::Import => self.parse_import(),
            Tok::Let => Ok(Item::Let(self.parse_let_binding()?)),
            _ => Ok(Item::Expr(self.parse_expr()?)),
        }
    }

    /// `module Name = <indented let bindings>` — an in-file namespace. The body is
    /// an indented block (opened by the offside rule after `=`) of `let` bindings
    /// only (MVP; `type`/`measure`/`extern` inside a module are deferred).
    fn parse_module_item(&mut self) -> Result<Item, ParseError> {
        self.expect(&Tok::Module, "`module`")?;
        let name_start = self.cur_start();
        let name = self.parse_upper_ident("module name")?;
        let name_span = NodeSpan::new(Span::new(name_start, self.prev_end()));
        self.expect(&Tok::Eq, "`=`")?;
        self.expect(&Tok::Indent, "an indented module body")?;
        let mut items = Vec::new();
        loop {
            if !matches!(self.peek(), Tok::Let) {
                return Err(self.error("a module body may only contain `let` bindings"));
            }
            items.push(self.parse_let_binding()?);
            if self.eat(&Tok::Sep) {
                if matches!(self.peek(), Tok::Dedent) {
                    break;
                }
                continue;
            }
            break;
        }
        self.expect(&Tok::Dedent, "end of the module body")?;
        Ok(Item::Module {
            name,
            name_span,
            items,
        })
    }

    /// `import Name` — bring another source file's module into scope under its
    /// capitalized name (`DESIGN.md` §6.1). The name is a single capitalized
    /// identifier (flat namespace; dotted/nested packages are deferred). The
    /// multi-file driver that resolves the import lands in a later slice.
    fn parse_import(&mut self) -> Result<Item, ParseError> {
        let start = self.cur_start();
        self.expect(&Tok::Import, "`import`")?;
        let name = self.parse_upper_ident("an imported module name")?;
        let span = NodeSpan::new(Span::new(start, self.prev_end()));
        Ok(Item::Import { name, span })
    }

    fn parse_measure(&mut self) -> Result<Item, ParseError> {
        let start = self.cur_start();
        self.expect(&Tok::Measure, "`measure`")?;
        let name = self.parse_ident("measure name")?;
        // `measure N = kg m / s^2` declares a derived alias; bare `measure m` a base
        // measure. The alias body is a unit expression without the `<>` brackets.
        let definition = if self.eat(&Tok::Eq) {
            Some(self.parse_unit_body()?)
        } else {
            None
        };
        let span = NodeSpan::new(Span::new(start, self.prev_end()));
        Ok(Item::Measure {
            name,
            definition,
            span,
        })
    }

    /// `extern [pure] name : type [= a.b.c]` — a typed import of a Python callable
    /// or value (`DESIGN.md` §6). The optional `= …` clause gives the dotted Python
    /// path; omitted, the target is the Pyfun name itself.
    fn parse_extern(&mut self) -> Result<ExternDecl, ParseError> {
        let start = self.cur_start();
        self.expect(&Tok::Extern, "`extern`")?;
        let pure = self.eat(&Tok::Pure);
        let name = self.parse_ident("extern name")?;
        self.expect(&Tok::Colon, "`:`")?;
        let ty = self.parse_type()?;
        // Optional `= a.b.c` Python target; defaults to the Pyfun name.
        let target = if self.eat(&Tok::Eq) {
            let mut segs = vec![self.parse_ident("Python target")?];
            while self.eat(&Tok::Dot) {
                segs.push(self.parse_ident("Python attribute")?);
            }
            segs
        } else {
            vec![name.clone()]
        };
        let span = NodeSpan::new(Span::new(start, self.prev_end()));
        Ok(ExternDecl {
            doc: None,
            pure,
            name,
            ty,
            target,
            span,
        })
    }

    fn parse_type_decl(&mut self) -> Result<TypeDecl, ParseError> {
        let start = self.cur_start();
        self.expect(&Tok::Type, "`type`")?;
        let name_start = self.cur_start();
        let name = self.parse_upper_ident("type name")?;
        let name_span = crate::parser::ast::NodeSpan::new(Span::new(name_start, self.prev_end()));
        let mut params = Vec::new();
        while let Tok::Ident(_) = self.peek() {
            params.push(self.parse_ident("type parameter")?);
        }
        self.expect(&Tok::Eq, "`=`")?;
        // A `{` after `=` introduces a record body; otherwise it is a sum of
        // constructors.
        let kind = if matches!(self.peek(), Tok::LBrace) {
            TypeDeclKind::Record(self.parse_record_decl_fields()?)
        } else {
            self.eat(&Tok::Bar); // optional leading bar
            let mut variants = vec![self.parse_variant()?];
            while self.eat(&Tok::Bar) {
                variants.push(self.parse_variant()?);
            }
            TypeDeclKind::Sum(variants)
        };
        let span = crate::parser::ast::NodeSpan::new(Span::new(start, self.prev_end()));
        Ok(TypeDecl {
            doc: None,
            name,
            name_span,
            params,
            kind,
            span,
        })
    }

    /// Parse a record body `{ x: type, y: type }` in a `type` declaration.
    fn parse_record_decl_fields(&mut self) -> Result<Vec<FieldDecl>, ParseError> {
        self.expect(&Tok::LBrace, "`{`")?;
        let mut fields = Vec::new();
        while !matches!(self.peek(), Tok::RBrace) {
            let name = self.parse_ident("field name")?;
            self.expect(&Tok::Colon, "`:`")?;
            let ty = self.parse_type()?;
            fields.push(FieldDecl { name, ty });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RBrace, "`}`")?;
        if fields.is_empty() {
            return Err(self.error("a record must have at least one field"));
        }
        Ok(fields)
    }

    fn parse_variant(&mut self) -> Result<VariantDecl, ParseError> {
        let start = self.cur_start();
        let name = self.parse_upper_ident("constructor name")?;
        let name_span = NodeSpan::new(Span::new(start, self.prev_end()));
        let mut fields = Vec::new();
        while starts_type_atom(self.peek()) {
            fields.push(self.parse_type_atom()?);
        }
        Ok(VariantDecl {
            name,
            name_span,
            fields,
        })
    }

    /// A type expression: an application optionally followed by `-> result`,
    /// where the arrow may carry an effect annotation (`->{io} result`,
    /// `->{io, async} result` — `DESIGN.md` §4). A bare `->` stays pure.
    fn parse_type(&mut self) -> Result<TypeExpr, ParseError> {
        let head = self.parse_type_app()?;
        if self.eat(&Tok::Arrow) {
            let effects = self.parse_effect_annotation()?;
            let result = self.parse_type()?;
            Ok(TypeExpr::Fun(Box::new(head), Box::new(result), effects))
        } else {
            Ok(head)
        }
    }

    /// An optional effect annotation right after `->` in a type: `{label, …}`.
    /// Labels are lowercase identifiers, collected as written — the type checker
    /// validates them against the known label set (`io`, `async`).
    fn parse_effect_annotation(&mut self) -> Result<Vec<String>, ParseError> {
        if !self.eat(&Tok::LBrace) {
            return Ok(Vec::new());
        }
        let mut labels = vec![self.parse_ident("effect label")?];
        while self.eat(&Tok::Comma) {
            labels.push(self.parse_ident("effect label")?);
        }
        self.expect(&Tok::RBrace, "`}`")?;
        Ok(labels)
    }

    fn parse_type_app(&mut self) -> Result<TypeExpr, ParseError> {
        // A capitalized head may be applied to argument atoms (`List a`).
        if let Tok::Ident(name) = self.peek().clone()
            && is_upper(&name)
        {
            let start = self.cur_start();
            self.bump();
            let name_span = NodeSpan::new(Span::new(start, self.prev_end()));
            let mut args = Vec::new();
            while starts_type_atom(self.peek()) {
                args.push(self.parse_type_atom()?);
            }
            return Ok(TypeExpr::Con(name, name_span, args));
        }
        self.parse_type_atom()
    }

    fn parse_type_atom(&mut self) -> Result<TypeExpr, ParseError> {
        match self.peek().clone() {
            Tok::Ident(name) => {
                let start = self.cur_start();
                self.bump();
                let name_span = NodeSpan::new(Span::new(start, self.prev_end()));
                Ok(TypeExpr::Con(name, name_span, Vec::new()))
            }
            Tok::LParen => {
                self.bump();
                let first = self.parse_type()?;
                if self.eat(&Tok::Comma) {
                    let mut elems = vec![first];
                    loop {
                        elems.push(self.parse_type()?);
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                    self.expect(&Tok::RParen, "`)`")?;
                    return Ok(TypeExpr::Tuple(elems));
                }
                self.expect(&Tok::RParen, "`)`")?;
                Ok(first)
            }
            _ => Err(self.error("expected a type")),
        }
    }

    fn parse_let_binding(&mut self) -> Result<LetBinding, ParseError> {
        self.expect(&Tok::Let, "`let`")?;
        // Optional `mut` / `pure` modifiers, in any order.
        let mut mutable = false;
        let mut pure = false;
        loop {
            if self.eat(&Tok::Mut) {
                mutable = true;
            } else if self.eat(&Tok::Pure) {
                pure = true;
            } else {
                break;
            }
        }
        let name_start = self.cur_start();
        // `let _ = e` discards the result (useful for a non-`unit` expression whose
        // value isn't needed but whose effect is). A discard takes no parameters and
        // can't be `mut` — it binds nothing.
        let is_discard = matches!(self.peek(), Tok::Underscore);
        let name = if is_discard {
            self.bump();
            "_".to_string()
        } else {
            self.parse_ident("binding name")?
        };
        let name_span = NodeSpan::new(Span::new(name_start, self.prev_end()));
        let mut params = Vec::new();
        while let Tok::Ident(_) = self.peek() {
            params.push(self.parse_param()?);
        }
        if is_discard && !params.is_empty() {
            return Err(self.error("a discard binding `_` cannot take parameters"));
        }
        if is_discard && mutable {
            return Err(self.error("a discard binding `_` cannot be `mut`"));
        }
        self.expect(&Tok::Eq, "`=`")?;
        let value = self.parse_block_or_expr()?;
        Ok(LetBinding {
            doc: None,
            mutable,
            pure,
            name,
            name_span,
            params,
            value,
        })
    }

    /// Parse a body that is either an indented block (the lexer opened one as a
    /// leading `Indent`) or an inline expression. Used wherever a block may open:
    /// a `let` body, a `match` arm, an `if` branch, a lambda body.
    fn parse_block_or_expr(&mut self) -> Result<Expr, ParseError> {
        if matches!(self.peek(), Tok::Indent) {
            self.parse_block()
        } else {
            self.parse_expr()
        }
    }

    /// Parse an indented block `Indent stmt (Sep stmt)* Dedent`. A block with a
    /// single expression statement is unwrapped to that expression so existing
    /// single-expression bodies (a multi-line `match`/`if`) keep their plain AST.
    fn parse_block(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        self.expect(&Tok::Indent, "an indented block")?;
        let mut stmts = Vec::new();
        loop {
            let stmt = if matches!(self.peek(), Tok::Let) {
                BlockStmt::Let(self.parse_let_binding()?)
            } else {
                BlockStmt::Expr(self.parse_expr()?)
            };
            stmts.push(stmt);
            if self.eat(&Tok::Sep) {
                if matches!(self.peek(), Tok::Dedent) {
                    break;
                }
                continue;
            }
            break;
        }
        self.expect(&Tok::Dedent, "end of the block")?;
        if matches!(stmts.last(), Some(BlockStmt::Let(_))) {
            return Err(self.error("a block must end with an expression"));
        }
        if stmts.len() == 1
            && let Some(BlockStmt::Expr(_)) = stmts.last()
        {
            let BlockStmt::Expr(e) = stmts.pop().unwrap() else {
                unreachable!()
            };
            return Ok(e);
        }
        Ok(self.mk(start, ExprKind::Block { stmts }))
    }

    fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        let lhs = self.parse_expr_head()?;
        // `target <- value` — reassignment, the lowest-precedence form.
        if matches!(self.peek(), Tok::LArrow) {
            self.bump();
            let value = self.parse_expr()?;
            let ExprKind::Var(target) = lhs.kind else {
                return Err(ParseError {
                    message: "the target of `<-` must be a variable".to_string(),
                    span: lhs.span(),
                });
            };
            return Ok(self.mk(
                start,
                ExprKind::Assign {
                    target,
                    value: Box::new(value),
                },
            ));
        }
        Ok(lhs)
    }

    fn parse_expr_head(&mut self) -> Result<Expr, ParseError> {
        match self.peek() {
            Tok::Fun => self.parse_fun(),
            Tok::If => self.parse_if(),
            Tok::Match => self.parse_match(),
            _ => self.parse_pipe(),
        }
    }

    fn parse_fun(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        self.expect(&Tok::Fun, "`fun`")?;
        let mut params = vec![self.parse_param()?];
        while let Tok::Ident(_) = self.peek() {
            params.push(self.parse_param()?);
        }
        self.expect(&Tok::Arrow, "`->`")?;
        let body = Box::new(self.parse_block_or_expr()?);
        Ok(self.mk(start, ExprKind::Fn { params, body }))
    }

    /// Parse a single parameter name, capturing its span (for editor jump/hover).
    fn parse_param(&mut self) -> Result<Param, ParseError> {
        let start = self.cur_start();
        let name = self.parse_ident("parameter name")?;
        Ok(Param {
            name,
            span: NodeSpan::new(Span::new(start, self.prev_end())),
        })
    }

    fn parse_if(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        self.expect(&Tok::If, "`if`")?;
        self.parse_if_rest(start)
    }

    /// Parse the `cond then branch (elif … | else …)` tail of an `if`/`elif`. `elif`
    /// is pure sugar for `else if` (`DESIGN.md` §7.2): it produces a nested `If` in
    /// the else branch, so there is no distinct AST node.
    fn parse_if_rest(&mut self, start: usize) -> Result<Expr, ParseError> {
        let cond = Box::new(self.parse_expr()?);
        self.expect(&Tok::Then, "`then`")?;
        let then = Box::new(self.parse_block_or_expr()?);
        let else_ = if matches!(self.peek(), Tok::Elif) {
            let elif_start = self.cur_start();
            self.bump();
            Box::new(self.parse_if_rest(elif_start)?)
        } else {
            self.expect(&Tok::Else, "`else` or `elif`")?;
            Box::new(self.parse_block_or_expr()?)
        };
        Ok(self.mk(start, ExprKind::If { cond, then, else_ }))
    }

    /// Parse `match e: case pat [if guard]: body …` (`DESIGN.md` §7.2). At bracket
    /// depth 0 the `:` after the scrutinee opens an offside block of `case` arms;
    /// inside brackets there is no `Indent` and the arms simply follow.
    fn parse_match(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        self.expect(&Tok::Match, "`match`")?;
        let scrutinee = Box::new(self.parse_expr()?);
        if matches!(self.peek(), Tok::With) {
            return Err(self.error(
                "match now uses `match e:` with `case pattern: body` arms, \
                 not `with | pattern -> body` (DESIGN.md §7.2)",
            ));
        }
        self.expect(&Tok::Colon, "`:` after the match scrutinee")?;
        let indented = self.eat(&Tok::Indent);
        let mut arms = Vec::new();
        while matches!(self.peek(), Tok::Case) {
            arms.push(self.parse_case_arm()?);
            if indented {
                if !self.eat(&Tok::Sep) {
                    break;
                }
                if matches!(self.peek(), Tok::Dedent) {
                    break;
                }
            }
        }
        if indented {
            self.expect(&Tok::Dedent, "end of the match arms")?;
        }
        if arms.is_empty() {
            return Err(self.error("a match needs at least one `case pattern: body` arm"));
        }
        Ok(self.mk(start, ExprKind::Match { scrutinee, arms }))
    }

    /// Parse one `case pattern [if guard]: body` arm. The pattern may be an
    /// or-pattern (`case a | b:`); the optional `if guard` makes the arm refutable.
    fn parse_case_arm(&mut self) -> Result<MatchArm, ParseError> {
        self.expect(&Tok::Case, "`case`")?;
        let pattern = self.parse_pattern()?;
        let guard = if self.eat(&Tok::If) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.expect(&Tok::Colon, "`:` after the case pattern")?;
        let body = self.parse_block_or_expr()?;
        Ok(MatchArm {
            pattern,
            guard,
            body,
        })
    }

    /// Pipes: forward `|>` (left-associative) and backward `<|` (right-associative,
    /// `f <| g <| x` = `f (g x)`), both at the lowest precedence.
    fn parse_pipe(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        let mut lhs = self.parse_compose()?;
        loop {
            if self.eat(&Tok::PipeOp) {
                let rhs = self.parse_compose()?;
                lhs = self.mk(
                    start,
                    ExprKind::Pipe {
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                        backward: false,
                    },
                );
            } else if self.eat(&Tok::PipeLeft) {
                // Right-associative: the whole remaining pipe is the argument.
                let rhs = self.parse_pipe()?;
                return Ok(self.mk(
                    start,
                    ExprKind::Pipe {
                        lhs: Box::new(lhs),
                        rhs: Box::new(rhs),
                        backward: true,
                    },
                ));
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    /// Function composition `>>` / `<<` — tighter than `|>`, looser than everything
    /// else, and **left-associative** (`f >> g >> h` = `(f >> g) >> h`).
    fn parse_compose(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        let mut lhs = self.parse_or()?;
        loop {
            let right_to_left = match self.peek() {
                Tok::GtGt => false,
                Tok::LtLt => true,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_or()?;
            lhs = self.mk(
                start,
                ExprKind::Compose {
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                    right_to_left,
                },
            );
        }
        Ok(lhs)
    }

    /// Logical `or` — looser than `and`.
    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        let mut lhs = self.parse_and()?;
        while self.eat(&Tok::Or) {
            let rhs = self.parse_and()?;
            lhs = self.mk(
                start,
                ExprKind::Binary {
                    op: BinOp::Or,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
            );
        }
        Ok(lhs)
    }

    /// Logical `and` — looser than `not`/comparison, tighter than `or`.
    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        let mut lhs = self.parse_not()?;
        while self.eat(&Tok::And) {
            let rhs = self.parse_not()?;
            lhs = self.mk(
                start,
                ExprKind::Binary {
                    op: BinOp::And,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
            );
        }
        Ok(lhs)
    }

    /// Prefix `not` — binds looser than comparison (`not a == b` is `not (a == b)`,
    /// matching Python), tighter than `&&`.
    fn parse_not(&mut self) -> Result<Expr, ParseError> {
        if matches!(self.peek(), Tok::Not) {
            let start = self.cur_start();
            self.bump();
            let expr = Box::new(self.parse_not()?);
            return Ok(self.mk(
                start,
                ExprKind::Unary {
                    op: UnOp::Not,
                    expr,
                },
            ));
        }
        // `try body` binds looser than `+`/comparison but tighter than `|>`/`and`/
        // `or`, so `try parse s` is `try (parse s)` and `try parse s |> f` pipes the
        // resulting `Result` out (`(try parse s) |> f`). Parens capture a wider body.
        if matches!(self.peek(), Tok::Try) {
            let start = self.cur_start();
            self.bump();
            let body = Box::new(self.parse_not()?);
            return Ok(self.mk(start, ExprKind::Try { body }));
        }
        self.parse_comparison()
    }

    /// Comparison and equality: `== != < > <= >=`, looser than `+ -`, tighter than
    /// `not`. Left-associative (chained comparisons type-check arm by arm).
    fn parse_comparison(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        let first = self.parse_additive()?;
        // Collect the chain of `op operand` links. Python-style: `a < b < c` is a
        // *single* chained comparison (`a < b and b < c`, `b` evaluated once), not
        // the left-associative `(a < b) < c`. A lone link stays a plain `Binary`.
        let mut rest = Vec::new();
        loop {
            let op = match self.peek() {
                Tok::EqEq => BinOp::Eq,
                Tok::BangEq => BinOp::Ne,
                Tok::Lt => BinOp::Lt,
                Tok::Gt => BinOp::Gt,
                Tok::Le => BinOp::Le,
                Tok::Ge => BinOp::Ge,
                _ => break,
            };
            self.bump();
            rest.push((op, self.parse_additive()?));
        }
        match rest.len() {
            0 => Ok(first),
            1 => {
                let (op, rhs) = rest.pop().expect("one link");
                Ok(self.mk(
                    start,
                    ExprKind::Binary {
                        op,
                        lhs: Box::new(first),
                        rhs: Box::new(rhs),
                    },
                ))
            }
            _ => Ok(self.mk(
                start,
                ExprKind::Compare {
                    first: Box::new(first),
                    rest,
                },
            )),
        }
    }

    fn parse_additive(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        let mut lhs = self.parse_multiplicative()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_multiplicative()?;
            lhs = self.mk(
                start,
                ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
            );
        }
        Ok(lhs)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        let mut lhs = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinOp::Mul,
                Tok::Slash => BinOp::Div,
                Tok::SlashSlash => BinOp::FloorDiv,
                Tok::Percent => BinOp::Mod,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_unary()?;
            lhs = self.mk(
                start,
                ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
            );
        }
        Ok(lhs)
    }

    /// Prefix arithmetic negation `-e`, binding tighter than `*`/`/` and looser
    /// than application (`-f x` is `-(f x)`, `2 * -3` is `2 * (-3)`). A prefix
    /// operator, not a lexer negative-literal, so `x-1` stays subtraction and only
    /// `f (-1)` (parenthesized) applies `f` to a negative. `-` is subtraction when
    /// it has a left operand (handled in `parse_additive`); here it has none.
    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        if matches!(self.peek(), Tok::Minus) {
            self.bump();
            let operand = self.parse_unary()?;
            return Ok(self.mk(
                start,
                ExprKind::Unary {
                    op: UnOp::Neg,
                    expr: Box::new(operand),
                },
            ));
        }
        self.parse_power()
    }

    /// Exponentiation `a ** b` — binds tighter than unary minus (`-2 ** 2` is
    /// `-(2 ** 2)`) and is **right-associative** (`2 ** 3 ** 2` is `2 ** (3 ** 2)`).
    /// The exponent is parsed at the unary level (a Python `factor`), so both the
    /// right-associativity and a signed exponent (`2 ** -3`) fall out.
    fn parse_power(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        let lhs = self.parse_application()?;
        if matches!(self.peek(), Tok::StarStar) {
            self.bump();
            let rhs = self.parse_unary()?;
            return Ok(self.mk(
                start,
                ExprKind::Binary {
                    op: BinOp::Pow,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
            ));
        }
        Ok(lhs)
    }

    fn parse_application(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        let mut func = self.parse_postfix()?;
        while starts_atom(self.peek()) {
            let arg = self.parse_postfix()?;
            func = self.mk(
                start,
                ExprKind::App {
                    func: Box::new(func),
                    arg: Box::new(arg),
                },
            );
        }
        Ok(func)
    }

    /// An atom followed by zero or more `.field` accesses. Field access binds
    /// tighter than application (`f p.x` is `f (p.x)`) and chains left-to-right
    /// (`p.x.y` is `(p.x).y`).
    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        let mut base = self.parse_atom()?;
        while self.eat(&Tok::Dot) {
            let name = self.parse_ident("field name")?;
            base = self.mk(
                start,
                ExprKind::Field {
                    base: Box::new(base),
                    name,
                },
            );
        }
        Ok(base)
    }

    fn parse_atom(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        let kind = match self.peek().clone() {
            Tok::Int(n) => {
                self.bump();
                return self.maybe_unit(start, ExprKind::Int(n));
            }
            Tok::Float(f) => {
                self.bump();
                return self.maybe_unit(start, ExprKind::Float(f));
            }
            Tok::Str(s) => {
                self.bump();
                ExprKind::Str(s)
            }
            Tok::FStr(parts) => {
                self.bump();
                ExprKind::Interp {
                    parts: Self::parse_interp(parts)?,
                }
            }
            Tok::True => {
                self.bump();
                ExprKind::Bool(true)
            }
            Tok::False => {
                self.bump();
                ExprKind::Bool(false)
            }
            Tok::Ident(name) => {
                // `async`/`seq`/`result` are computation-expression builders only
                // when immediately followed by `{`; otherwise they are ordinary
                // identifiers.
                if let Some(builder) = CeBuilder::from_name(&name)
                    && *self.peek2() == Tok::LBrace
                {
                    self.bump(); // builder name
                    return self.parse_ce(builder, start);
                }
                // `Geometry.Point { x = 1, … }` — a qualified (cross-module) record
                // literal (`DESIGN.md` §8.3). An uppercase module name, `.`, an
                // uppercase record name, then a `{` — distinguished from a qualified
                // constructor application (`Geometry.Circle 2.0`) and a qualified
                // member (`Geometry.area x`) by the immediately-following brace.
                if is_upper(&name)
                    && *self.peek2() == Tok::Dot
                    && matches!(self.peek3(), Tok::Ident(n) if is_upper(n))
                    && *self.peek4() == Tok::LBrace
                {
                    self.bump(); // module name
                    self.bump(); // `.`
                    let Tok::Ident(rec) = self.bump() else {
                        unreachable!("peek3 guaranteed an identifier")
                    };
                    let ty = format!("{name}.{rec}");
                    let ty_span = NodeSpan::new(Span::new(start, self.prev_end()));
                    return self.parse_record_literal(ty, ty_span, start);
                }
                // A user builder: an uppercase (module) name immediately before a
                // `{` whose first token starts a CE item. The CE-keyword lookahead
                // distinguishes `Maybe { let! … }` (a CE) from `Some { x = 1 }`
                // (a constructor applied to a record literal).
                if is_upper(&name) && *self.peek2() == Tok::LBrace && starts_ce_item(self.peek3()) {
                    self.bump(); // builder name
                    return self.parse_ce(CeBuilder::User(name), start);
                }
                // `Point { x = 1, y = 2 }` — a constructor-tagged record literal
                // (`DESIGN.md` §8.3): an uppercase name before a `{` whose body is
                // not a CE item (that case was handled just above).
                if is_upper(&name) && *self.peek2() == Tok::LBrace {
                    self.bump(); // type tag
                    let ty_span = NodeSpan::new(Span::new(start, self.prev_end()));
                    return self.parse_record_literal(name, ty_span, start);
                }
                self.bump();
                ExprKind::Var(name)
            }
            Tok::LParen => {
                self.bump();
                // `()` is the unit value; `(expr)` is grouping; `(a, b)` is a tuple;
                // `(op)` is an operator section (a binary operator as a function).
                if self.eat(&Tok::RParen) {
                    return Ok(self.mk(start, ExprKind::Unit));
                }
                if let Some(op) = binop_from_tok(self.peek())
                    && matches!(self.peek2(), Tok::RParen)
                {
                    self.bump(); // the operator
                    self.bump(); // `)`
                    return Ok(self.mk(start, ExprKind::OpFunc(op)));
                }
                let first = self.parse_expr()?;
                if self.eat(&Tok::Comma) {
                    let mut elems = vec![first];
                    loop {
                        elems.push(self.parse_expr()?);
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                    self.expect(&Tok::RParen, "`)`")?;
                    return Ok(self.mk(start, ExprKind::Tuple { elems }));
                }
                self.expect(&Tok::RParen, "`)`")?;
                // Keep the inner node's own (paren-free) span.
                return Ok(first);
            }
            Tok::LBrace => return self.parse_record(start),
            Tok::LBracket => return self.parse_list(start),
            _ => return Err(self.error("expected an expression")),
        };
        Ok(self.mk(start, kind))
    }

    /// Parse the segments of an interpolated string into `InterpPart`s. Literal
    /// chunks pass through; each hole's pre-lexed tokens (absolute spans, `Eof`-
    /// terminated) are parsed as a full expression by a fresh sub-parser, and any
    /// leftover tokens after that expression are an error.
    fn parse_interp(parts: Vec<FStrPart>) -> Result<Vec<InterpPart>, ParseError> {
        let mut out = Vec::with_capacity(parts.len());
        for part in parts {
            match part {
                FStrPart::Lit(s) => out.push(InterpPart::Lit(s)),
                FStrPart::Hole(tokens) => {
                    let mut sub = Parser { tokens, pos: 0 };
                    let expr = sub.parse_expr()?;
                    if !sub.at_eof() {
                        return Err(sub.error("unexpected token after f-string hole expression"));
                    }
                    out.push(InterpPart::Expr(Box::new(expr)));
                }
            }
        }
        Ok(out)
    }

    /// Parse a list literal `[a, b, c]` — comma-separated, possibly empty, with an
    /// optional trailing comma.
    fn parse_list(&mut self, start: usize) -> Result<Expr, ParseError> {
        self.expect(&Tok::LBracket, "`[`")?;
        let mut elems = Vec::new();
        while !matches!(self.peek(), Tok::RBracket) {
            elems.push(self.parse_expr()?);
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RBracket, "`]`")?;
        Ok(self.mk(start, ExprKind::List { elems }))
    }

    /// Parse the `{ … }` body of a constructor-tagged record literal `ty { … }`
    /// (`DESIGN.md` §8.3); the tag `ty` has already been consumed.
    fn parse_record_literal(
        &mut self,
        ty: String,
        ty_span: NodeSpan,
        start: usize,
    ) -> Result<Expr, ParseError> {
        self.expect(&Tok::LBrace, "`{`")?;
        let fields = self.parse_field_inits()?;
        self.expect(&Tok::RBrace, "`}`")?;
        Ok(self.mk(
            start,
            ExprKind::Record {
                ty,
                ty_span,
                fields,
            },
        ))
    }

    /// Parse a functional record update `{ base with x = 3 }`. Record *literals*
    /// are constructor-tagged (`Point { … }`, parsed in `parse_atom`), so a bare
    /// `{` in expression position is always an update (`DESIGN.md` §8.3).
    fn parse_record(&mut self, start: usize) -> Result<Expr, ParseError> {
        self.expect(&Tok::LBrace, "`{`")?;
        // A targeted migration error for the old bare record literal `{ x = 1 }`.
        if matches!(self.peek(), Tok::Ident(_)) && matches!(self.peek2(), Tok::Eq) {
            return Err(self.error(
                "record literals are now constructor-tagged: write `T { x = … }` \
                 naming the record type (DESIGN.md §8.3)",
            ));
        }
        let base = Box::new(self.parse_expr()?);
        self.expect(&Tok::With, "`with`")?;
        let fields = self.parse_field_inits()?;
        self.expect(&Tok::RBrace, "`}`")?;
        Ok(self.mk(start, ExprKind::RecordUpdate { base, fields }))
    }

    /// Parse `ident = expr (, ident = expr)*` field initializers up to `}`.
    fn parse_field_inits(&mut self) -> Result<Vec<FieldInit>, ParseError> {
        let mut fields = Vec::new();
        while !matches!(self.peek(), Tok::RBrace) {
            let name = self.parse_ident("field name")?;
            self.expect(&Tok::Eq, "`=`")?;
            let value = self.parse_expr()?;
            fields.push(FieldInit { name, value });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        if fields.is_empty() {
            return Err(self.error("a record needs at least one field"));
        }
        Ok(fields)
    }

    /// Wrap a freshly-parsed numeric literal in a unit annotation if one follows.
    ///
    /// A `<` is a unit annotation only when it sits *immediately* after the
    /// literal (no whitespace), e.g. `5<m>`. With a space, `5 < m`, it is the
    /// less-than operator — the F# rule, which keeps units and comparison apart.
    fn maybe_unit(&mut self, start: usize, kind: ExprKind) -> Result<Expr, ParseError> {
        let literal = self.mk(start, kind);
        let adjacent = self.cur_start() == self.prev_end();
        if adjacent && matches!(self.peek(), Tok::Lt) {
            let unit = self.parse_unit_annotation()?;
            Ok(self.mk(
                start,
                ExprKind::Annot {
                    value: Box::new(literal),
                    unit,
                },
            ))
        } else {
            Ok(literal)
        }
    }

    /// Parse `<unit>`, e.g. `<m>`, `<m s>`, `<m/s^2>`, or `<1>` (dimensionless).
    fn parse_unit_annotation(&mut self) -> Result<UnitExpr, ParseError> {
        self.expect(&Tok::Lt, "`<`")?;
        let unit = self.parse_unit_body()?;
        self.expect(&Tok::Gt, "`>`")?;
        Ok(unit)
    }

    /// Parse the body of a unit expression (without the surrounding `<>`): numerator
    /// factors, an optional `/` then denominator factors. A leading `1` is the
    /// empty numerator — bare `1` is dimensionless, `1/s` is "per second". Shared by
    /// `<…>` annotations and `measure N = …` aliases.
    fn parse_unit_body(&mut self) -> Result<UnitExpr, ParseError> {
        let mut factors = Vec::new();
        // `1` stands for an empty numerator (so it also leads `1/s`); otherwise
        // collect numerator factors.
        let unit_numerator = matches!(self.peek(), Tok::Int(1));
        if unit_numerator {
            self.bump();
        } else {
            while matches!(self.peek(), Tok::Ident(_)) {
                factors.push(self.parse_unit_factor(1)?);
            }
        }
        if self.eat(&Tok::Slash) {
            while matches!(self.peek(), Tok::Ident(_)) {
                factors.push(self.parse_unit_factor(-1)?);
            }
        }
        // Valid: `1` (dimensionless), `m…`, `m…/s…`, `1/s…`. A bare empty body is not.
        if !unit_numerator && factors.is_empty() {
            return Err(self.error("expected a unit (e.g. `m`, `m/s`, or `1`)"));
        }
        Ok(UnitExpr { factors })
    }

    fn parse_unit_factor(&mut self, sign: i32) -> Result<(String, i32), ParseError> {
        let name = self.parse_ident("measure name")?;
        let exp = if self.eat(&Tok::Caret) {
            match self.peek().clone() {
                Tok::Int(n) => {
                    self.bump();
                    n as i32
                }
                _ => return Err(self.error("expected an integer exponent after `^`")),
            }
        } else {
            1
        };
        Ok((name, sign * exp))
    }

    /// Parse `builder { items }`. Items are delimited by their leading keyword
    /// (`let!`, `let`, `do!`, `return`, `yield`), so no separators are needed.
    fn parse_ce(&mut self, builder: CeBuilder, start: usize) -> Result<Expr, ParseError> {
        self.expect(&Tok::LBrace, "`{`")?;
        let mut items = Vec::new();
        while !matches!(self.peek(), Tok::RBrace) {
            if self.at_eof() {
                return Err(self.error("unterminated computation expression"));
            }
            items.push(self.parse_ce_item()?);
        }
        self.expect(&Tok::RBrace, "`}`")?;
        Ok(self.mk(start, ExprKind::Ce { builder, items }))
    }

    fn parse_ce_item(&mut self) -> Result<CeItem, ParseError> {
        match self.peek() {
            Tok::Let => {
                self.bump();
                let bang = self.eat(&Tok::Bang);
                let name_start = self.cur_start();
                let name = self.parse_ident("binding name")?;
                let name_span = NodeSpan::new(Span::new(name_start, self.prev_end()));
                self.expect(&Tok::Eq, "`=`")?;
                let value = self.parse_expr()?;
                Ok(if bang {
                    CeItem::LetBang {
                        name,
                        name_span,
                        value,
                    }
                } else {
                    CeItem::Let {
                        name,
                        name_span,
                        value,
                    }
                })
            }
            Tok::Return => {
                self.bump();
                let bang = self.eat(&Tok::Bang);
                let value = self.parse_expr()?;
                Ok(if bang {
                    CeItem::ReturnBang(value)
                } else {
                    CeItem::Return(value)
                })
            }
            Tok::Yield => {
                self.bump();
                let bang = self.eat(&Tok::Bang);
                let value = self.parse_expr()?;
                Ok(if bang {
                    CeItem::YieldBang(value)
                } else {
                    CeItem::Yield(value)
                })
            }
            Tok::Do => {
                self.bump();
                self.expect(&Tok::Bang, "`!`")?;
                Ok(CeItem::DoBang(self.parse_expr()?))
            }
            _ => Err(self.error("expected `let!`, `let`, `do!`, `return`, or `yield`")),
        }
    }

    /// Parse a full pattern, including a top-level or-pattern `a | b | c`
    /// (`DESIGN.md` §7.2). Alternatives are joined at the constructor-application
    /// level, so `Some a | None` is `(Some a) | None`, not `Some (a | None)`.
    fn parse_pattern(&mut self) -> Result<Pattern, ParseError> {
        let first = self.parse_pattern_app()?;
        let pat = if matches!(self.peek(), Tok::Bar) {
            let mut alts = vec![first];
            while self.eat(&Tok::Bar) {
                alts.push(self.parse_pattern_app()?);
            }
            Pattern::Or(alts)
        } else {
            first
        };
        // `p as x` binds the whole matched value to `x`. `as` binds looser than `|`,
        // so `a | b as x` is `(a | b) as x`.
        if self.eat(&Tok::As) {
            let name_start = self.cur_start();
            let name = self.parse_ident("a name after `as`")?;
            let name_span = NodeSpan::new(Span::new(name_start, self.prev_end()));
            return Ok(Pattern::As {
                pattern: Box::new(pat),
                name,
                name_span,
            });
        }
        Ok(pat)
    }

    /// Parse a constructor-application pattern (no top-level `|`). A capitalized
    /// identifier is a constructor that may take argument patterns, a
    /// constructor-tagged record pattern (`Point { … }`), or — after a `.` — a
    /// qualified constructor from an imported module (`Geometry.Circle r`,
    /// `DESIGN.md` §6.1). Everything else is a single atom pattern.
    fn parse_pattern_app(&mut self) -> Result<Pattern, ParseError> {
        if let Tok::Ident(name) = self.peek().clone()
            && is_upper(&name)
        {
            let start = self.cur_start();
            self.bump();
            // Consume a `.Ctor`/`.Record` suffix first, so a qualified record pattern
            // (`Geometry.Point { … }`) and a qualified constructor (`Geometry.Circle r`)
            // are both recognized (`DESIGN.md` §8.3, §6.1).
            let name = self.maybe_qualify_ctor(name)?;
            let name_span = NodeSpan::new(Span::new(start, self.prev_end()));
            // `Point { … }` / `Geometry.Point { … }` — a constructor-tagged record
            // pattern (the tag may be qualified for an imported record).
            if matches!(self.peek(), Tok::LBrace) {
                return self.parse_record_pattern_body(name, name_span);
            }
            let mut args = Vec::new();
            while starts_atom_pattern(self.peek()) {
                args.push(self.parse_atom_pattern()?);
            }
            return Ok(Pattern::Ctor {
                name,
                name_span,
                args,
            });
        }
        self.parse_atom_pattern()
    }

    /// After a capitalized identifier in pattern position, consume a `.Ctor`
    /// suffix if present, yielding the qualified constructor name
    /// (`Geometry.Circle`); otherwise return the bare name.
    fn maybe_qualify_ctor(&mut self, base: String) -> Result<String, ParseError> {
        if self.eat(&Tok::Dot) {
            let ctor = self.parse_upper_ident("a constructor name after `.`")?;
            Ok(format!("{base}.{ctor}"))
        } else {
            Ok(base)
        }
    }

    fn parse_atom_pattern(&mut self) -> Result<Pattern, ParseError> {
        match self.peek().clone() {
            Tok::Underscore => {
                self.bump();
                Ok(Pattern::Wildcard)
            }
            Tok::Ident(name) => {
                let start = self.cur_start();
                self.bump();
                if is_upper(&name) {
                    // A `.Ctor`/`.Record` suffix is consumed first (qualified imports).
                    let name = self.maybe_qualify_ctor(name)?;
                    let name_span = NodeSpan::new(Span::new(start, self.prev_end()));
                    // `Point { … }` / `Geometry.Point { … }` — a constructor-tagged
                    // record pattern as an atom (e.g. a constructor argument), else a
                    // nullary constructor, possibly qualified (`Geometry.Nothing`).
                    if matches!(self.peek(), Tok::LBrace) {
                        return self.parse_record_pattern_body(name, name_span);
                    }
                    Ok(Pattern::Ctor {
                        name,
                        name_span,
                        args: Vec::new(),
                    })
                } else {
                    Ok(Pattern::Var {
                        name,
                        span: NodeSpan::new(Span::new(start, self.prev_end())),
                    })
                }
            }
            Tok::Int(n) => {
                self.bump();
                Ok(Pattern::Int(n))
            }
            // A negative integer literal pattern `case -1:` — the sign folds into
            // the literal (Python's `match` likewise allows `-` before a number in a
            // literal pattern). Only integers have literal patterns, so `- Int` only.
            Tok::Minus if matches!(self.peek2(), Tok::Int(_)) => {
                self.bump();
                let Tok::Int(n) = self.bump() else {
                    unreachable!("peek2 guaranteed an integer")
                };
                Ok(Pattern::Int(-n))
            }
            Tok::Str(s) => {
                self.bump();
                Ok(Pattern::Str(s))
            }
            Tok::True => {
                self.bump();
                Ok(Pattern::Bool(true))
            }
            Tok::False => {
                self.bump();
                Ok(Pattern::Bool(false))
            }
            Tok::LParen => {
                self.bump();
                let first = self.parse_pattern()?;
                if self.eat(&Tok::Comma) {
                    let mut elems = vec![first];
                    loop {
                        elems.push(self.parse_pattern()?);
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                    self.expect(&Tok::RParen, "`)`")?;
                    return Ok(Pattern::Tuple { elems });
                }
                self.expect(&Tok::RParen, "`)`")?;
                Ok(first)
            }
            Tok::LBracket => self.parse_list_pattern(),
            // Float literals are deliberately not matchable: a literal pattern
            // compiles to `==`, and float equality is unreliable (`0.1 + 0.2 ≠
            // 0.3`, `NaN ≠ NaN`). Point at the guard alternative rather than the
            // generic "expected a pattern". Covers `case 1.5:` and `case -1.5:`.
            Tok::Float(_) => Err(self.float_pattern_error()),
            Tok::Minus if matches!(self.peek2(), Tok::Float(_)) => Err(self.float_pattern_error()),
            _ => Err(self.error("expected a pattern")),
        }
    }

    /// The guiding error for a float used as a pattern (see `parse_atom_pattern`).
    fn float_pattern_error(&self) -> ParseError {
        ParseError {
            message: "float literals can't be matched — float equality is \
                      unreliable; bind a variable and use a guard instead, \
                      e.g. `case x if x == 1.5:`"
                .to_string(),
            span: self.span(),
        }
    }

    /// Parse a list/sequence pattern `[a, b, *mid, z]` (`DESIGN.md` §7.2). Elements
    /// are comma-separated; an element is a normal pattern, or `*pat` — the rest
    /// binder, which may sit **anywhere** (Python's rule): elements before it are
    /// the `prefix`, elements after it the `suffix`. `[]` is allowed. At most one
    /// star.
    fn parse_list_pattern(&mut self) -> Result<Pattern, ParseError> {
        self.expect(&Tok::LBracket, "`[`")?;
        let mut prefix = Vec::new();
        let mut rest: Option<Box<Pattern>> = None;
        let mut suffix = Vec::new();
        while !matches!(self.peek(), Tok::RBracket) {
            if self.eat(&Tok::Star) {
                if rest.is_some() {
                    return Err(self.error("a list pattern can have at most one `*` rest element"));
                }
                // The rest binder `*rest` / `*_` — a variable or wildcard (it binds
                // the unmatched middle slice, itself a list).
                let start = self.cur_start();
                let binder = match self.peek().clone() {
                    Tok::Underscore => {
                        self.bump();
                        Pattern::Wildcard
                    }
                    Tok::Ident(name) if !is_upper(&name) => {
                        self.bump();
                        Pattern::Var {
                            name,
                            span: NodeSpan::new(Span::new(start, self.prev_end())),
                        }
                    }
                    _ => {
                        return Err(self.error("the `*` rest binder must be a variable or `_`"));
                    }
                };
                rest = Some(Box::new(binder));
            } else {
                let elem = self.parse_pattern()?;
                if rest.is_some() {
                    suffix.push(elem);
                } else {
                    prefix.push(elem);
                }
            }
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RBracket, "`]`")?;
        Ok(Pattern::List {
            prefix,
            rest,
            suffix,
        })
    }

    /// Parse the `{ name [= pattern] (, name [= pattern])* }` body of a
    /// constructor-tagged record pattern `ty { … }` (`DESIGN.md` §8.3); the tag
    /// `ty` has already been consumed. A bare `name` is shorthand for `name = name`
    /// (binds the field to a same-named variable).
    fn parse_record_pattern_body(
        &mut self,
        ty: String,
        ty_span: NodeSpan,
    ) -> Result<Pattern, ParseError> {
        self.expect(&Tok::LBrace, "`{`")?;
        let mut fields = Vec::new();
        while !matches!(self.peek(), Tok::RBrace) {
            let start = self.cur_start();
            let name = self.parse_ident("field name")?;
            let name_span = NodeSpan::new(Span::new(start, self.prev_end()));
            let pattern = if self.eat(&Tok::Eq) {
                self.parse_pattern()?
            } else {
                Pattern::Var {
                    name: name.clone(),
                    span: name_span,
                }
            };
            fields.push(FieldPattern {
                name,
                name_span,
                pattern,
            });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        self.expect(&Tok::RBrace, "`}`")?;
        if fields.is_empty() {
            return Err(self.error("a record pattern needs at least one field"));
        }
        Ok(Pattern::Record {
            ty,
            ty_span,
            fields,
        })
    }

    fn parse_ident(&mut self, what: &str) -> Result<String, ParseError> {
        match self.peek().clone() {
            Tok::Ident(name) => {
                self.bump();
                Ok(name)
            }
            _ => Err(self.error(&format!("expected {what}"))),
        }
    }

    fn parse_upper_ident(&mut self, what: &str) -> Result<String, ParseError> {
        match self.peek().clone() {
            Tok::Ident(name) if is_upper(&name) => {
                self.bump();
                Ok(name)
            }
            _ => Err(self.error(&format!("expected {what}"))),
        }
    }
}

/// The `BinOp` an operator section `(op)` denotes, or `None` if the token isn't a
/// sectionable binary operator. `and`/`or` are deliberately excluded: they are
/// keywords, and a strict function value would silently drop their short-circuit
/// evaluation (F# excludes `&&`/`||` for the same reason).
fn binop_from_tok(tok: &Tok) -> Option<BinOp> {
    Some(match tok {
        Tok::Plus => BinOp::Add,
        Tok::Minus => BinOp::Sub,
        Tok::Star => BinOp::Mul,
        Tok::Slash => BinOp::Div,
        Tok::SlashSlash => BinOp::FloorDiv,
        Tok::Percent => BinOp::Mod,
        Tok::StarStar => BinOp::Pow,
        Tok::EqEq => BinOp::Eq,
        Tok::BangEq => BinOp::Ne,
        Tok::Lt => BinOp::Lt,
        Tok::Gt => BinOp::Gt,
        Tok::Le => BinOp::Le,
        Tok::Ge => BinOp::Ge,
        _ => return None,
    })
}

fn starts_atom(tok: &Tok) -> bool {
    matches!(
        tok,
        Tok::Int(_)
            | Tok::Float(_)
            | Tok::Str(_)
            | Tok::FStr(_)
            | Tok::True
            | Tok::False
            | Tok::Ident(_)
            | Tok::LParen
            | Tok::LBrace
            | Tok::LBracket
    )
}

fn starts_atom_pattern(tok: &Tok) -> bool {
    // A `{` no longer starts a pattern on its own: record patterns are
    // constructor-tagged (`Point { … }`), so they begin with an `Ident`
    // (`DESIGN.md` §8.3).
    matches!(
        tok,
        Tok::Underscore
            | Tok::Ident(_)
            | Tok::Int(_)
            | Tok::True
            | Tok::False
            | Tok::LParen
            | Tok::LBracket
    )
}

/// Does this token begin a computation-expression item (`let`/`let!`, `do!`,
/// `return`/`return!`, `yield`/`yield!`)? Used to tell a user CE (`Maybe { let!
/// … }`) from a constructor applied to a record (`Some { x = 1 }`).
fn starts_ce_item(tok: &Tok) -> bool {
    matches!(tok, Tok::Let | Tok::Return | Tok::Yield | Tok::Do)
}

fn starts_type_atom(tok: &Tok) -> bool {
    matches!(tok, Tok::Ident(_) | Tok::LParen)
}

fn is_upper(name: &str) -> bool {
    name.chars().next().is_some_and(|c| c.is_uppercase())
}

/// Attach collected doc-comment text to the item it precedes. Docs attach to the
/// documentable declarations — `let` / `type` / `extern` (MVP: top level only);
/// preceding anything else (`measure`, `module`, `import`, an expression) they are
/// accepted and dropped, like an ordinary comment.
fn attach_doc(item: &mut Item, doc: Option<String>) {
    let Some(doc) = doc else { return };
    match item {
        Item::Let(binding) => binding.doc = Some(doc),
        Item::Type(decl) => decl.doc = Some(doc),
        Item::Extern(decl) => decl.doc = Some(doc),
        _ => {}
    }
}

/// A short human-readable name for a token, used in error messages.
fn describe(tok: &Tok) -> String {
    match tok {
        Tok::Int(n) => format!("integer `{n}`"),
        Tok::Float(f) => format!("float `{f}`"),
        Tok::Str(_) => "string literal".to_string(),
        Tok::Ident(name) => format!("identifier `{name}`"),
        Tok::Eof => "end of input".to_string(),
        Tok::Doc(_) => "doc comment".to_string(),
        Tok::Sep => "end of statement".to_string(),
        Tok::Indent => "start of an indented block".to_string(),
        Tok::Dedent => "end of block".to_string(),
        other => format!("`{}`", token_symbol(other)),
    }
}

fn token_symbol(tok: &Tok) -> &'static str {
    match tok {
        Tok::Let => "let",
        Tok::Mut => "mut",
        Tok::Pure => "pure",
        Tok::If => "if",
        Tok::Then => "then",
        Tok::Else => "else",
        Tok::Match => "match",
        Tok::With => "with",
        Tok::Fun => "fun",
        Tok::Type => "type",
        Tok::Return => "return",
        Tok::Yield => "yield",
        Tok::Do => "do",
        Tok::Measure => "measure",
        Tok::Extern => "extern",
        Tok::Module => "module",
        Tok::Import => "import",
        Tok::Not => "not",
        Tok::And => "and",
        Tok::Or => "or",
        Tok::Bang => "!",
        Tok::Caret => "^",
        Tok::Lt => "<",
        Tok::Gt => ">",
        Tok::LBrace => "{",
        Tok::RBrace => "}",
        Tok::LBracket => "[",
        Tok::RBracket => "]",
        Tok::True => "true",
        Tok::False => "false",
        Tok::Eq => "=",
        Tok::EqEq => "==",
        Tok::BangEq => "!=",
        Tok::Le => "<=",
        Tok::Ge => ">=",
        Tok::GtGt => ">>",
        Tok::LtLt => "<<",
        Tok::LArrow => "<-",
        Tok::Plus => "+",
        Tok::Minus => "-",
        Tok::Star => "*",
        Tok::Slash => "/",
        Tok::SlashSlash => "//",
        Tok::PipeOp => "|>",
        Tok::PipeLeft => "<|",
        Tok::Bar => "|",
        Tok::Arrow => "->",
        Tok::LParen => "(",
        Tok::RParen => ")",
        Tok::Comma => ",",
        Tok::Colon => ":",
        Tok::Dot => ".",
        Tok::Underscore => "_",
        _ => "token",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;

    fn names(module: &Module) -> Vec<&str> {
        module
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Let(b) => Some(b.name.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn recovers_items_around_a_broken_let() {
        // The middle `let bad =` has no body; recovery should still yield the
        // surrounding well-formed items.
        let src = "let good = 1\nlet bad =\nlet also = 2";
        let (module, errors) = parse_recover(lex(src).unwrap());
        assert_eq!(names(&module), ["good", "also"]);
        assert_eq!(errors.len(), 1, "errors: {errors:?}");
    }

    #[test]
    fn recovers_after_a_broken_block() {
        // A garbage expression statement inside an indented block is one broken
        // item; the following top-level `let` must still parse.
        let src = "let f x =\n  x +\nlet g = 2";
        let (module, errors) = parse_recover(lex(src).unwrap());
        assert!(names(&module).contains(&"g"), "names: {:?}", names(&module));
        assert!(!errors.is_empty());
    }

    #[test]
    fn clean_source_recovers_with_no_errors() {
        let src = "let a = 1\nlet b = 2";
        let (module, errors) = parse_recover(lex(src).unwrap());
        assert_eq!(names(&module), ["a", "b"]);
        assert!(errors.is_empty());
    }
}
