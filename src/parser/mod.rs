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

use crate::lexer::{Span, Tok, Token};
use ast::{
    BinOp, BlockStmt, CeBuilder, CeItem, Expr, ExprKind, ExternDecl, FieldDecl, FieldInit,
    FieldPattern, Item, LetBinding, MatchArm, Module, NodeSpan, Param, Pattern, TypeDecl,
    TypeDeclKind, TypeExpr, UnOp, UnitExpr, VariantDecl,
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
            items.push(self.parse_item()?);
            // An item is delimited by the offside-inserted separators (or EOF).
            self.skip_seps();
        }
        Ok(Module { items })
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
            let before = self.pos;
            match self.parse_item() {
                Ok(item) => items.push(item),
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

    fn parse_measure(&mut self) -> Result<Item, ParseError> {
        let start = self.cur_start();
        self.expect(&Tok::Measure, "`measure`")?;
        let name = self.parse_ident("measure name")?;
        let span = NodeSpan::new(Span::new(start, self.prev_end()));
        Ok(Item::Measure { name, span })
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
        let name = self.parse_upper_ident("type name")?;
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
            name,
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
        let name = self.parse_upper_ident("constructor name")?;
        let mut fields = Vec::new();
        while starts_type_atom(self.peek()) {
            fields.push(self.parse_type_atom()?);
        }
        Ok(VariantDecl { name, fields })
    }

    /// A type expression: an application optionally followed by `-> result`.
    fn parse_type(&mut self) -> Result<TypeExpr, ParseError> {
        let head = self.parse_type_app()?;
        if self.eat(&Tok::Arrow) {
            let result = self.parse_type()?;
            Ok(TypeExpr::Fun(Box::new(head), Box::new(result)))
        } else {
            Ok(head)
        }
    }

    fn parse_type_app(&mut self) -> Result<TypeExpr, ParseError> {
        // A capitalized head may be applied to argument atoms (`List a`).
        if let Tok::Ident(name) = self.peek().clone()
            && is_upper(&name)
        {
            self.bump();
            let mut args = Vec::new();
            while starts_type_atom(self.peek()) {
                args.push(self.parse_type_atom()?);
            }
            return Ok(TypeExpr::Con(name, args));
        }
        self.parse_type_atom()
    }

    fn parse_type_atom(&mut self) -> Result<TypeExpr, ParseError> {
        match self.peek().clone() {
            Tok::Ident(name) => {
                self.bump();
                Ok(TypeExpr::Con(name, Vec::new()))
            }
            Tok::LParen => {
                self.bump();
                let inner = self.parse_type()?;
                self.expect(&Tok::RParen, "`)`")?;
                Ok(inner)
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
        let name = self.parse_ident("binding name")?;
        let name_span = NodeSpan::new(Span::new(name_start, self.prev_end()));
        let mut params = Vec::new();
        while let Tok::Ident(_) = self.peek() {
            params.push(self.parse_param()?);
        }
        self.expect(&Tok::Eq, "`=`")?;
        let value = self.parse_block_or_expr()?;
        Ok(LetBinding {
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
        let cond = Box::new(self.parse_expr()?);
        self.expect(&Tok::Then, "`then`")?;
        let then = Box::new(self.parse_block_or_expr()?);
        self.expect(&Tok::Else, "`else`")?;
        let else_ = Box::new(self.parse_block_or_expr()?);
        Ok(self.mk(start, ExprKind::If { cond, then, else_ }))
    }

    fn parse_match(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        self.expect(&Tok::Match, "`match`")?;
        let scrutinee = Box::new(self.parse_expr()?);
        self.expect(&Tok::With, "`with`")?;
        let mut arms = Vec::new();
        while self.eat(&Tok::Bar) {
            let pattern = self.parse_pattern()?;
            self.expect(&Tok::Arrow, "`->`")?;
            let body = self.parse_block_or_expr()?;
            arms.push(MatchArm { pattern, body });
        }
        if arms.is_empty() {
            return Err(self.error("expected at least one `| pattern -> expr` arm"));
        }
        Ok(self.mk(start, ExprKind::Match { scrutinee, arms }))
    }

    fn parse_pipe(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        let mut lhs = self.parse_or()?;
        while self.eat(&Tok::PipeOp) {
            let rhs = self.parse_or()?;
            lhs = self.mk(
                start,
                ExprKind::Pipe {
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
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
        self.parse_comparison()
    }

    /// Comparison and equality: `== != < > <= >=`, looser than `+ -`, tighter than
    /// `not`. Left-associative (chained comparisons type-check arm by arm).
    fn parse_comparison(&mut self) -> Result<Expr, ParseError> {
        let start = self.cur_start();
        let mut lhs = self.parse_additive()?;
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
            let rhs = self.parse_additive()?;
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
        let mut lhs = self.parse_application()?;
        loop {
            let op = match self.peek() {
                Tok::Star => BinOp::Mul,
                Tok::Slash => BinOp::Div,
                Tok::SlashSlash => BinOp::FloorDiv,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_application()?;
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
                // A user builder: an uppercase (module) name immediately before a
                // `{` whose first token starts a CE item. The CE-keyword lookahead
                // distinguishes `Maybe { let! … }` (a CE) from `Some { x = 1 }`
                // (a constructor applied to a record literal).
                if is_upper(&name) && *self.peek2() == Tok::LBrace && starts_ce_item(self.peek3()) {
                    self.bump(); // builder name
                    return self.parse_ce(CeBuilder::User(name), start);
                }
                self.bump();
                ExprKind::Var(name)
            }
            Tok::LParen => {
                self.bump();
                // `()` is the unit value; `(expr)` is grouping.
                if self.eat(&Tok::RParen) {
                    return Ok(self.mk(start, ExprKind::Unit));
                }
                let inner = self.parse_expr()?;
                self.expect(&Tok::RParen, "`)`")?;
                // Keep the inner node's own (paren-free) span.
                return Ok(inner);
            }
            Tok::LBrace => return self.parse_record(start),
            Tok::LBracket => return self.parse_list(start),
            _ => return Err(self.error("expected an expression")),
        };
        Ok(self.mk(start, kind))
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

    /// Parse a record literal `{ x = 1, y = 2 }` or update `{ base with x = 3 }`.
    ///
    /// The two are distinguished by lookahead: a leading `ident =` is a literal's
    /// first field; anything else is the `base` expression of an update (followed
    /// by `with`).
    fn parse_record(&mut self, start: usize) -> Result<Expr, ParseError> {
        self.expect(&Tok::LBrace, "`{`")?;
        let is_literal = matches!(self.peek(), Tok::Ident(_)) && matches!(self.peek2(), Tok::Eq);
        let kind = if is_literal {
            ExprKind::Record {
                fields: self.parse_field_inits()?,
            }
        } else {
            let base = Box::new(self.parse_expr()?);
            self.expect(&Tok::With, "`with`")?;
            ExprKind::RecordUpdate {
                base,
                fields: self.parse_field_inits()?,
            }
        };
        self.expect(&Tok::RBrace, "`}`")?;
        Ok(self.mk(start, kind))
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
        if matches!(self.peek(), Tok::Int(1)) {
            self.bump();
            self.expect(&Tok::Gt, "`>`")?;
            return Ok(UnitExpr {
                factors: Vec::new(),
            });
        }
        let mut factors = Vec::new();
        while matches!(self.peek(), Tok::Ident(_)) {
            factors.push(self.parse_unit_factor(1)?);
        }
        if self.eat(&Tok::Slash) {
            while matches!(self.peek(), Tok::Ident(_)) {
                factors.push(self.parse_unit_factor(-1)?);
            }
        }
        self.expect(&Tok::Gt, "`>`")?;
        if factors.is_empty() {
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

    fn parse_pattern(&mut self) -> Result<Pattern, ParseError> {
        // A capitalized identifier at this level is a constructor that may take
        // argument patterns; everything else is a single atom pattern.
        if let Tok::Ident(name) = self.peek().clone()
            && is_upper(&name)
        {
            self.bump();
            let mut args = Vec::new();
            while starts_atom_pattern(self.peek()) {
                args.push(self.parse_atom_pattern()?);
            }
            return Ok(Pattern::Ctor { name, args });
        }
        self.parse_atom_pattern()
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
                    Ok(Pattern::Ctor {
                        name,
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
                let inner = self.parse_pattern()?;
                self.expect(&Tok::RParen, "`)`")?;
                Ok(inner)
            }
            Tok::LBrace => self.parse_record_pattern(),
            _ => Err(self.error("expected a pattern")),
        }
    }

    /// Parse `{ name [= pattern] (, name [= pattern])* }` — a record pattern.
    /// A bare `name` is shorthand for `name = name` (binds the field to a
    /// same-named variable).
    fn parse_record_pattern(&mut self) -> Result<Pattern, ParseError> {
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
        Ok(Pattern::Record { fields })
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

fn starts_atom(tok: &Tok) -> bool {
    matches!(
        tok,
        Tok::Int(_)
            | Tok::Float(_)
            | Tok::Str(_)
            | Tok::True
            | Tok::False
            | Tok::Ident(_)
            | Tok::LParen
            | Tok::LBrace
            | Tok::LBracket
    )
}

fn starts_atom_pattern(tok: &Tok) -> bool {
    matches!(
        tok,
        Tok::Underscore
            | Tok::Ident(_)
            | Tok::Int(_)
            | Tok::True
            | Tok::False
            | Tok::LParen
            | Tok::LBrace
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

/// A short human-readable name for a token, used in error messages.
fn describe(tok: &Tok) -> String {
    match tok {
        Tok::Int(n) => format!("integer `{n}`"),
        Tok::Float(f) => format!("float `{f}`"),
        Tok::Str(_) => "string literal".to_string(),
        Tok::Ident(name) => format!("identifier `{name}`"),
        Tok::Eof => "end of input".to_string(),
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
        Tok::LArrow => "<-",
        Tok::Plus => "+",
        Tok::Minus => "-",
        Tok::Star => "*",
        Tok::Slash => "/",
        Tok::SlashSlash => "//",
        Tok::PipeOp => "|>",
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
