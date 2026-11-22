//! Signature help (`textDocument/signatureHelp`, `DESIGN.md` §9): while the
//! arguments of a call are being written, the function's type with the
//! parameter the cursor is on picked out.
//!
//! A call is juxtaposition (`f a b`), so there is no bracket to anchor on. The
//! call the cursor belongs to is the innermost application whose function has
//! been written and whose arguments the cursor is among, or just past.

use std::collections::HashSet;

use crate::lexer::Span;
use crate::syntax::{BlockStmt, CeItem, Expr, ExprKind, InterpPart, Item, Module};

/// The call a cursor is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// The span of the function being applied (a name, or `Module.member`).
    pub head: Span,
    /// How many arguments lie wholly before the cursor: the index of the
    /// parameter the next thing typed will fill.
    pub active: usize,
}

/// The call `offset` is in, if any. `text` is the source `module` was parsed
/// from.
pub fn call_at(module: &Module, text: &str, offset: usize) -> Option<Call> {
    let mut finder = Finder {
        text,
        offset,
        inner: HashSet::new(),
        best: None,
    };
    for item in &module.items {
        match item {
            Item::Let(binding) => finder.expr(&binding.value, false),
            Item::Module { items, .. } => {
                for binding in items {
                    finder.expr(&binding.value, false);
                }
            }
            Item::Expr(expr) => finder.expr(expr, false),
            Item::ActivePattern(decl) => finder.expr(&decl.value, false),
            Item::Measure { .. }
            | Item::Type(_)
            | Item::Extern(_)
            | Item::Import { .. }
            | Item::ExternImport { .. } => {}
        }
    }
    finder.best.map(|(_, call)| call)
}

struct Finder<'a> {
    text: &'a str,
    offset: usize,
    /// Applications already counted as part of a longer one (`f a` within
    /// `f a b`), by span.
    inner: HashSet<Span>,
    /// The narrowest call found so far, with the width of its extent.
    best: Option<(usize, Call)>,
}

impl Finder<'_> {
    /// Where an expression's reach ends for the cursor: past the spaces that
    /// follow it on its line, where its next argument would be typed.
    fn reach(&self, end: usize) -> usize {
        end + self.text[end.min(self.text.len())..]
            .bytes()
            .take_while(|b| *b == b' ' || *b == b'\t')
            .count()
    }

    /// Offer the call `head args…` whose extent is `whole`.
    fn offer(&mut self, head: &Expr, args: &[&Expr], whole: Span) {
        if !matches!(head.kind, ExprKind::Var(_) | ExprKind::Field { .. }) {
            return;
        }
        let head_span = head.span();
        // The function has been written, and the cursor is past it.
        if self.offset <= head_span.end || self.offset > self.reach(whole.end) {
            return;
        }
        let active = args
            .iter()
            .filter(|arg| arg.span().end < self.offset)
            .count();
        let width = whole.end - whole.start;
        if self.best.as_ref().is_none_or(|(best, _)| width <= *best) {
            self.best = Some((
                width,
                Call {
                    head: head_span,
                    active,
                },
            ));
        }
    }

    /// Visit `expr`. `argument` says it is an argument of a call, where a bare
    /// name is something being passed, not something about to be applied.
    fn expr(&mut self, expr: &Expr, argument: bool) {
        match &expr.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Str(_)
            | ExprKind::Bool(_)
            | ExprKind::Hole { .. }
            | ExprKind::Unit
            | ExprKind::OpFunc(_) => {}
            // A name with nothing applied to it yet: `add ▮`.
            ExprKind::Var(_) => {
                if !argument {
                    self.offer(expr, &[], expr.span());
                }
            }
            ExprKind::Field { base, .. } => {
                if crate::types::qualified_name(expr).is_some() {
                    if !argument {
                        self.offer(expr, &[], expr.span());
                    }
                } else {
                    self.expr(base, false);
                }
            }
            ExprKind::App { .. } => {
                if self.inner.contains(&expr.span()) {
                    return;
                }
                // The whole spine: `f a b` is `App(App(f, a), b)`.
                let mut args = Vec::new();
                let mut head = expr;
                while let ExprKind::App { func, arg } = &head.kind {
                    args.push(&**arg);
                    self.inner.insert(func.span());
                    head = func;
                }
                args.reverse();
                self.offer(head, &args, expr.span());
                if !matches!(head.kind, ExprKind::Var(_) | ExprKind::Field { .. }) {
                    self.expr(head, true);
                }
                for arg in args {
                    self.expr(arg, true);
                }
            }
            ExprKind::Fn { body, .. } => self.expr(body, false),
            ExprKind::If { cond, then, else_ } => {
                self.expr(cond, false);
                self.expr(then, false);
                self.expr(else_, false);
            }
            ExprKind::Try { body } => self.expr(body, false),
            ExprKind::Collection { expansion, .. } => self.expr(expansion, true),
            ExprKind::Match { scrutinee, arms } => {
                self.expr(scrutinee, false);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        self.expr(guard, false);
                    }
                    self.expr(&arm.body, false);
                }
            }
            ExprKind::Binary { lhs, rhs, .. }
            | ExprKind::Pipe { lhs, rhs, .. }
            | ExprKind::Compose { lhs, rhs, .. } => {
                self.expr(lhs, false);
                self.expr(rhs, false);
            }
            ExprKind::Unary { expr, .. } => self.expr(expr, false),
            ExprKind::Compare { first, rest } => {
                self.expr(first, false);
                for (_, operand) in rest {
                    self.expr(operand, false);
                }
            }
            ExprKind::Ce { items, .. } => self.items(items),
            ExprKind::Annot { value, .. } => self.expr(value, false),
            // An element of a list or a tuple is a value being listed.
            ExprKind::List { elems } | ExprKind::Tuple { elems } => {
                for e in elems {
                    self.expr(e, true);
                }
            }
            ExprKind::Interp { parts } => {
                for part in parts {
                    if let InterpPart::Expr(e) = part {
                        self.expr(e, true);
                    }
                }
            }
            ExprKind::Record { fields, .. } => {
                for f in fields {
                    self.expr(&f.value, false);
                }
            }
            ExprKind::RecordUpdate { base, fields } => {
                self.expr(base, true);
                for f in fields {
                    self.expr(&f.value, false);
                }
            }
            ExprKind::Block { stmts } => {
                for stmt in stmts {
                    match stmt {
                        BlockStmt::Let(binding) => self.expr(&binding.value, false),
                        BlockStmt::Expr(e) => self.expr(e, false),
                    }
                }
            }
            ExprKind::Assign { value, .. } => self.expr(value, false),
        }
    }

    fn items(&mut self, items: &[CeItem]) {
        for item in items {
            match item {
                CeItem::LetBang { value, .. } | CeItem::Let { value, .. } => {
                    self.expr(value, false)
                }
                CeItem::DoBang(e)
                | CeItem::Return(e)
                | CeItem::ReturnBang(e)
                | CeItem::Yield(e)
                | CeItem::YieldBang(e) => self.expr(e, false),
                CeItem::For { source, body, .. } => {
                    self.expr(source, false);
                    self.items(body);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The call at the `▮` in `marked`, as `(function text, active)`.
    fn at(marked: &str) -> Option<(String, usize)> {
        let offset = marked.find('▮').expect("a cursor mark");
        let source = marked.replace('▮', "");
        let analysis = crate::analyze(&source);
        let module = analysis.module.as_ref().expect("the example parses");
        call_at(module, &source, offset).map(|call| {
            (
                source[call.head.start..call.head.end].to_string(),
                call.active,
            )
        })
    }

    fn call(name: &str, active: usize) -> Option<(String, usize)> {
        Some((name.to_string(), active))
    }

    #[test]
    fn a_name_with_a_space_after_it_is_a_call_about_to_start() {
        assert_eq!(at("let add a b = a + b\nlet y = add ▮"), call("add", 0));
    }

    #[test]
    fn each_argument_written_moves_to_the_next_parameter() {
        let defs = "let add a b = a + b\n";
        assert_eq!(at(&format!("{defs}let y = add 1▮")), call("add", 0));
        assert_eq!(at(&format!("{defs}let y = add 1 ▮")), call("add", 1));
        assert_eq!(at(&format!("{defs}let y = add 1 2▮")), call("add", 1));
        assert_eq!(at(&format!("{defs}let y = add 1 2 ▮")), call("add", 2));
    }

    #[test]
    fn the_cursor_inside_an_argument_stays_on_its_parameter() {
        assert_eq!(
            at("let y = List.map (fun x -> x +▮ 1) [1, 2]"),
            call("List.map", 0)
        );
        assert_eq!(
            at("let y = List.map (fun x -> x + 1) [1,▮ 2]"),
            call("List.map", 1)
        );
    }

    #[test]
    fn the_innermost_call_wins() {
        let defs = "let add a b = a + b\n";
        assert_eq!(
            at(&format!("{defs}let y = add (add 1 ▮) 2")),
            call("add", 1)
        );
        assert_eq!(
            at(&format!("{defs}let y = List.map (add 1) ▮")),
            call("List.map", 1)
        );
    }

    #[test]
    fn a_function_passed_as_an_argument_is_not_being_called() {
        // The cursor after `double ` continues `List.map`'s arguments.
        assert_eq!(
            at("let double x = x * 2\nlet y = List.map double ▮"),
            call("List.map", 1)
        );
    }

    #[test]
    fn a_piped_stage_is_a_call() {
        assert_eq!(at("let y = [1, 2] |> List.map ▮"), call("List.map", 0));
    }

    #[test]
    fn typing_the_function_itself_is_not_yet_a_call() {
        assert_eq!(at("let add a b = a + b\nlet y = ad▮d"), None);
        assert_eq!(at("let add a b = a + b\nlet y = add▮"), None);
    }

    #[test]
    fn the_next_line_is_out_of_reach() {
        assert_eq!(at("let add a b = a + b\nlet y = add 1\n▮"), None);
    }
}
