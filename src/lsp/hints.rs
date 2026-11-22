//! Inlay hints (`textDocument/inlayHint`, `DESIGN.md` §9): the inferred type
//! of each binding, shown beside it.
//!
//! Types are never written in a `let`, so the editor writes them in for the
//! reader, in Python's notation: `total: int` after a value's name,
//! `shape: Shape` after a parameter and `-> float` ahead of the `=` of a
//! function. A hint is the editor's text; none of it is Pyfun syntax.

use crate::lexer::Span;
use crate::syntax::{
    BlockStmt, CeItem, Expr, ExprKind, InterpPart, Item, LetBinding, Module, Pattern,
};
use crate::types::{TypeSpan, shown_arrows};

/// One hint: `label` displayed at byte `offset` of the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    pub offset: usize,
    pub label: String,
    /// Whether the editor should leave a space before the label (` -> float`).
    pub padding_left: bool,
}

/// Every hint of `module`, in source order, given the types its analysis
/// recorded.
pub fn hints(module: &Module, types: &[TypeSpan]) -> Vec<Hint> {
    let mut found = Collector {
        types,
        hints: Vec::new(),
    };
    for item in &module.items {
        match item {
            Item::Let(binding) => found.binding(binding),
            Item::Module { items, .. } => {
                for binding in items {
                    found.binding(binding);
                }
            }
            Item::Expr(expr) => found.expr(expr),
            Item::Measure { .. }
            | Item::Type(_)
            | Item::Extern(_)
            | Item::Import { .. }
            | Item::ExternImport { .. }
            | Item::ActivePattern(_) => {}
        }
    }
    found.hints.sort_by_key(|hint| hint.offset);
    found.hints
}

struct Collector<'a> {
    types: &'a [TypeSpan],
    hints: Vec<Hint>,
}

impl<'a> Collector<'a> {
    /// The displayed type recorded for exactly `span`.
    fn shown(&self, span: Span) -> Option<&'a str> {
        self.types
            .iter()
            .find(|t| t.span == span)
            .map(|t| t.ty.as_str())
    }

    fn binding(&mut self, binding: &LetBinding) {
        if binding.params.is_empty() {
            self.value(&binding.target, &binding.value);
        } else {
            self.function(binding);
        }
        self.expr(&binding.value);
    }

    /// A value binding: `: T` after each name it binds. A name bound to a
    /// literal says its type already, and a name that opens with `_` asks not
    /// to be looked at.
    fn value(&mut self, target: &Pattern, value: &Expr) {
        let obvious = matches!(target, Pattern::Var { .. }) && is_literal(value);
        if obvious {
            return;
        }
        for (name, span) in target.bound_vars() {
            if name.starts_with('_') {
                continue;
            }
            if let Some(shown) = self.shown(span) {
                self.hints.push(Hint {
                    offset: span.end,
                    label: format!(": {shown}"),
                    padding_left: false,
                });
            }
        }
    }

    /// A function binding: `: T` after each parameter and `-> R` after the
    /// last, taken from the function's own displayed type so that its type
    /// variables read the same in every hint.
    fn function(&mut self, binding: &LetBinding) {
        let Some((_, name_span)) = binding.bound_vars().into_iter().next() else {
            return;
        };
        let Some(shown) = self.shown(name_span) else {
            return;
        };
        let arrows = shown_arrows(shown);
        let arity = binding.params.len();
        // A function of N parameters has at least N arrows; anything else is a
        // binding the checker could not type, and it gets no hints.
        if arrows.arrows.len() < arity {
            return;
        }
        for (param, part) in binding.params.iter().zip(&arrows.parts) {
            let text = &shown[part.clone()];
            let named = match &param.pattern {
                Pattern::Wildcard => false,
                Pattern::Var { name, .. } => !name.starts_with('_'),
                _ => true,
            };
            if named {
                self.hints.push(Hint {
                    offset: param.span.span().end,
                    label: format!(": {text}"),
                    padding_left: false,
                });
            }
        }
        // The result is everything past the last parameter's arrow, which
        // carries the effect the call performs (`->{io} unit`).
        let result = &shown[arrows.parts[arity].start..];
        if let Some(last) = binding.params.last() {
            self.hints.push(Hint {
                offset: last.span.span().end,
                label: format!("{} {result}", arrows.arrows[arity - 1]),
                padding_left: true,
            });
        }
    }

    fn items(&mut self, items: &[CeItem]) {
        for item in items {
            match item {
                CeItem::LetBang { target, value, .. } | CeItem::Let { target, value, .. } => {
                    self.value(target, value);
                    self.expr(value);
                }
                CeItem::DoBang(e)
                | CeItem::Return(e)
                | CeItem::ReturnBang(e)
                | CeItem::Yield(e)
                | CeItem::YieldBang(e) => self.expr(e),
                CeItem::For { source, body, .. } => {
                    self.expr(source);
                    self.items(body);
                }
            }
        }
    }

    /// Reach the bindings nested inside an expression.
    fn expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Int(_)
            | ExprKind::Float(_)
            | ExprKind::Str(_)
            | ExprKind::Bool(_)
            | ExprKind::Hole { .. }
            | ExprKind::Unit
            | ExprKind::Var(_)
            | ExprKind::OpFunc(_) => {}
            ExprKind::Fn { body, .. } => self.expr(body),
            ExprKind::App { func, arg } => {
                self.expr(func);
                self.expr(arg);
            }
            ExprKind::If { cond, then, else_ } => {
                self.expr(cond);
                self.expr(then);
                self.expr(else_);
            }
            ExprKind::Try { body } => self.expr(body),
            // The elements are walked through the expansion they desugar to.
            ExprKind::Collection { expansion, .. } => self.expr(expansion),
            ExprKind::Match { scrutinee, arms } => {
                self.expr(scrutinee);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        self.expr(guard);
                    }
                    self.expr(&arm.body);
                }
            }
            ExprKind::Binary { lhs, rhs, .. }
            | ExprKind::Pipe { lhs, rhs, .. }
            | ExprKind::Compose { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            ExprKind::Unary { expr, .. } => self.expr(expr),
            ExprKind::Compare { first, rest } => {
                self.expr(first);
                for (_, operand) in rest {
                    self.expr(operand);
                }
            }
            ExprKind::Ce { items, .. } => self.items(items),
            ExprKind::Annot { value, .. } => self.expr(value),
            ExprKind::List { elems } | ExprKind::Tuple { elems } => {
                for e in elems {
                    self.expr(e);
                }
            }
            ExprKind::Interp { parts } => {
                for part in parts {
                    if let InterpPart::Expr(e) = part {
                        self.expr(e);
                    }
                }
            }
            ExprKind::Record { fields, .. } => {
                for f in fields {
                    self.expr(&f.value);
                }
            }
            ExprKind::RecordUpdate { base, fields } => {
                self.expr(base);
                for f in fields {
                    self.expr(&f.value);
                }
            }
            ExprKind::Field { base, .. } => self.expr(base),
            ExprKind::Block { stmts } => {
                for stmt in stmts {
                    match stmt {
                        BlockStmt::Let(binding) => self.binding(binding),
                        BlockStmt::Expr(e) => self.expr(e),
                    }
                }
            }
            ExprKind::Assign { value, .. } => self.expr(value),
        }
    }
}

/// Whether `expr` is a literal, whose type its own spelling gives away.
fn is_literal(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Bool(_)
        | ExprKind::Interp { .. }
        | ExprKind::Unit => true,
        ExprKind::Unary { expr, .. } => is_literal(expr),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `source` with each hint written in at its place, between `‹` and `›`.
    fn annotated(source: &str) -> String {
        let analysis = crate::analyze(source);
        assert!(
            analysis.diagnostics.is_empty(),
            "the example type-checks: {:?}",
            analysis.diagnostics
        );
        let module = analysis.module.as_ref().expect("the example parses");
        let mut out = source.to_string();
        for hint in hints(module, &analysis.types).into_iter().rev() {
            let pad = if hint.padding_left { " " } else { "" };
            out.insert_str(hint.offset, &format!("‹{pad}{}›", hint.label));
        }
        out
    }

    #[test]
    fn a_value_shows_its_type_after_its_name() {
        assert_eq!(
            annotated("let xs = [1, 2, 3]\nlet total = List.sum xs"),
            "let xs‹: List int› = [1, 2, 3]\nlet total‹: int› = List.sum xs"
        );
    }

    #[test]
    fn a_literal_says_its_own_type() {
        assert_eq!(
            annotated("let n = 5\nlet name = \"Ada\"\nlet low = -1.5\nlet ok = True"),
            "let n = 5\nlet name = \"Ada\"\nlet low = -1.5\nlet ok = True"
        );
    }

    #[test]
    fn a_function_shows_each_parameter_and_its_result() {
        assert_eq!(
            annotated("let shout times word = String.repeat times (String.upper word)"),
            "let shout times‹: int› word‹: str›‹ -> str› = String.repeat times (String.upper word)"
        );
    }

    #[test]
    fn a_constraint_is_left_to_hover() {
        // Hover shows `num a => a<'u> -> a<'u>`; the hints show its pieces.
        assert_eq!(
            annotated("let double x = x * 2"),
            "let double x‹: a<'u>›‹ -> a<'u>› = x * 2"
        );
    }

    #[test]
    fn type_variables_read_the_same_across_a_functions_hints() {
        // Each hint is a piece of the one displayed type, so the second
        // parameter is `b`, not a second `a`.
        assert_eq!(
            annotated("let pair x y = (x, y)"),
            "let pair x‹: a› y‹: b›‹ -> (a, b)› = (x, y)"
        );
    }

    #[test]
    fn a_function_parameter_keeps_its_parentheses() {
        assert_eq!(
            annotated("let twice f x = f (f x)"),
            "let twice f‹: (a -> a)› x‹: a›‹ -> a› = f (f x)"
        );
    }

    #[test]
    fn the_result_carries_the_effect() {
        assert_eq!(
            annotated("let greet name = print (String.upper name)"),
            "let greet name‹: str›‹ ->{io} unit› = print (String.upper name)"
        );
    }

    #[test]
    fn an_ignored_parameter_gets_no_hint() {
        assert_eq!(
            annotated("let always _ = 1\nlet keep _x = 1"),
            "let always _‹ -> int› = 1\nlet keep _x‹ -> int› = 1"
        );
    }

    #[test]
    fn a_pattern_parameter_is_hinted_as_a_whole() {
        assert_eq!(
            annotated("let swap (a, b) = (b, a)"),
            "let swap (a, b)‹: (a, b)›‹ -> (b, a)› = (b, a)"
        );
    }

    #[test]
    fn a_destructuring_let_hints_each_name() {
        assert_eq!(
            annotated("let pair = (1, \"one\")\nlet (n, word) = pair"),
            "let pair‹: (int, str)› = (1, \"one\")\nlet (n‹: int›, word‹: str›) = pair"
        );
    }

    #[test]
    fn bindings_inside_a_body_are_reached() {
        assert_eq!(
            annotated(
                "let mean xs =\n    let total = List.sum xs\n    let count = List.len xs\n    total // count"
            ),
            "let mean xs‹: List int›‹ -> int› =\n    let total‹: int› = List.sum xs\n    let count‹: int› = List.len xs\n    total // count"
        );
    }

    #[test]
    fn a_computation_expressions_bindings_are_reached() {
        assert_eq!(
            annotated(
                "let parse text = String.to_int text |> Option.to_result \"not a number\"\nlet next text =\n    result {\n        let! n = parse text\n        return n + 1\n    }"
            ),
            "let parse text‹: str›‹ -> Result int str› = String.to_int text |> Option.to_result \"not a number\"\nlet next text‹: str›‹ -> Result int str› =\n    result {\n        let! n‹: int› = parse text\n        return n + 1\n    }"
        );
    }

    #[test]
    fn a_displayed_type_splits_at_its_own_arrows() {
        let shown = "(comparison a, a in b) => (a -> bool) -> b ->{io} List (a, a<'u>)";
        let arrows = shown_arrows(shown);
        assert_eq!(arrows.constraints, Some("(comparison a, a in b)"));
        let parts: Vec<&str> = arrows.parts.iter().map(|r| &shown[r.clone()]).collect();
        assert_eq!(parts, vec!["(a -> bool)", "b", "List (a, a<'u>)"]);
        assert_eq!(arrows.arrows, vec!["->", "->{io}"]);

        let plain = shown_arrows("List int");
        assert_eq!(plain.constraints, None);
        assert_eq!(plain.parts, vec![0..8]);
        assert!(plain.arrows.is_empty());
    }
}
