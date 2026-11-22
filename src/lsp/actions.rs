//! Code actions (`textDocument/codeAction`, `DESIGN.md` §9): a fix the editor
//! can apply for a diagnostic the compiler has already explained.
//!
//! - A non-exhaustive `match` gets an arm for every case it leaves out, each
//!   with a typed hole for a body, from the checker's own witnesses.
//! - An unused local `let` is renamed with a leading `_`, which is how a name
//!   says it is not meant to be read.
//! - An unused `import` or `extern import` is removed.

use crate::diagnostics::{Warning, WarningKind};
use crate::lexer::Span;
use crate::syntax::{BlockStmt, CeItem, Expr, ExprKind, InterpPart, Item, MatchArm, Module};
use crate::types::MissingCases;

/// One fix: a title for the editor's menu and the text changes that make it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    pub title: String,
    /// The span of the diagnostic the fix answers.
    pub answers: Span,
    /// Each change as `(the text to replace, its replacement)`; an insertion
    /// replaces an empty span.
    pub edits: Vec<(Span, String)>,
}

/// Every fix on offer for a module, given what its analysis found.
pub fn fixes(
    module: &Module,
    text: &str,
    missing: &[MissingCases],
    warnings: &[Warning],
) -> Vec<Fix> {
    let mut out = Vec::new();
    for cases in missing {
        if let Some(fix) = add_cases(module, text, cases) {
            out.push(fix);
        }
    }
    for warning in warnings {
        match warning.kind {
            WarningKind::UnusedBinding => {
                let name = &text[warning.span.start..warning.span.end];
                out.push(Fix {
                    title: format!("Rename to `_{name}`"),
                    answers: warning.span,
                    edits: vec![(
                        Span::new(warning.span.start, warning.span.start),
                        "_".into(),
                    )],
                });
            }
            WarningKind::UnusedImport | WarningKind::UnusedExternImport => out.push(Fix {
                title: "Remove the unused import".to_string(),
                answers: warning.span,
                edits: vec![(whole_lines(text, warning.span), String::new())],
            }),
            WarningKind::UnreachableArm | WarningKind::Rebound => {}
        }
    }
    out
}

/// The lines `span` touches, from the start of its first to the start of the
/// line after its last.
fn whole_lines(text: &str, span: Span) -> Span {
    let start = text[..span.start].rfind('\n').map_or(0, |at| at + 1);
    let end = text[span.end..]
        .find('\n')
        .map_or(text.len(), |at| span.end + at + 1);
    Span::new(start, end)
}

/// The fix for a non-exhaustive `match`: an arm for each missing case after
/// its last arm, indented as its arms are, each with a hole for a body.
fn add_cases(module: &Module, text: &str, missing: &MissingCases) -> Option<Fix> {
    let arms = find_match(module, missing.span)?;
    let (first, last) = (arms.first()?, arms.last()?);
    // The arms' indentation: what stands ahead of the first arm's `case`.
    let pattern_at = first.pattern_span.span().start;
    let line_start = text[..pattern_at].rfind('\n').map_or(0, |at| at + 1);
    let line = &text[line_start..pattern_at];
    let indent = &line[..line.len() - line.trim_start().len()];
    if !line.trim_start().starts_with("case") {
        return None;
    }
    // After the last arm: at the end of its line when only a comment follows,
    // otherwise straight after its body (a closing parenthesis may follow).
    let body_end = last.body.span().end;
    let line_end = text[body_end..]
        .find('\n')
        .map_or(text.len(), |at| body_end + at);
    let rest = text[body_end..line_end].trim();
    let at = if rest.is_empty() || rest.starts_with('#') {
        // Keep a `\r` of a CRLF line ending after the inserted arms.
        body_end + text[body_end..line_end].trim_end_matches('\r').len()
    } else {
        body_end
    };
    let arms_text: String = missing
        .cases
        .iter()
        .map(|case| format!("\n{indent}case {case}: ?"))
        .collect();
    let title = match missing.cases.as_slice() {
        [one] => format!("Add the missing case `{one}`"),
        many => format!("Add the {} missing cases", many.len()),
    };
    Some(Fix {
        title,
        answers: missing.span,
        edits: vec![(Span::new(at, at), arms_text)],
    })
}

/// The arms of the `match` expression at exactly `span`.
fn find_match<'a>(module: &'a Module, span: Span) -> Option<&'a [MatchArm]> {
    let mut found = None;
    let mut look = |expr: &'a Expr| {
        if found.is_none()
            && expr.span() == span
            && let ExprKind::Match { arms, .. } = &expr.kind
        {
            found = Some(arms.as_slice());
        }
    };
    for item in &module.items {
        match item {
            Item::Let(binding) => each_expr(&binding.value, &mut look),
            Item::Module { items, .. } => {
                for binding in items {
                    each_expr(&binding.value, &mut look);
                }
            }
            Item::Expr(expr) => each_expr(expr, &mut look),
            Item::ActivePattern(decl) => each_expr(&decl.value, &mut look),
            Item::Measure { .. }
            | Item::Type(_)
            | Item::Extern(_)
            | Item::Import { .. }
            | Item::ExternImport { .. } => {}
        }
    }
    found
}

/// Call `visit` on `expr` and on every expression inside it, parents first.
fn each_expr<'a>(expr: &'a Expr, visit: &mut dyn FnMut(&'a Expr)) {
    visit(expr);
    match &expr.kind {
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Str(_)
        | ExprKind::Bool(_)
        | ExprKind::Hole { .. }
        | ExprKind::Unit
        | ExprKind::Var(_)
        | ExprKind::OpFunc(_) => {}
        ExprKind::Fn { body, .. } | ExprKind::Try { body } => each_expr(body, visit),
        ExprKind::App { func, arg } => {
            each_expr(func, visit);
            each_expr(arg, visit);
        }
        ExprKind::If { cond, then, else_ } => {
            each_expr(cond, visit);
            each_expr(then, visit);
            each_expr(else_, visit);
        }
        ExprKind::Collection { expansion, .. } => each_expr(expansion, visit),
        ExprKind::Match { scrutinee, arms } => {
            each_expr(scrutinee, visit);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    each_expr(guard, visit);
                }
                each_expr(&arm.body, visit);
            }
        }
        ExprKind::Binary { lhs, rhs, .. }
        | ExprKind::Pipe { lhs, rhs, .. }
        | ExprKind::Compose { lhs, rhs, .. } => {
            each_expr(lhs, visit);
            each_expr(rhs, visit);
        }
        ExprKind::Unary { expr, .. } => each_expr(expr, visit),
        ExprKind::Compare { first, rest } => {
            each_expr(first, visit);
            for (_, operand) in rest {
                each_expr(operand, visit);
            }
        }
        ExprKind::Ce { items, .. } => each_item(items, visit),
        ExprKind::Annot { value, .. } | ExprKind::Assign { value, .. } => each_expr(value, visit),
        ExprKind::List { elems } | ExprKind::Tuple { elems } => {
            for e in elems {
                each_expr(e, visit);
            }
        }
        ExprKind::Interp { parts } => {
            for part in parts {
                if let InterpPart::Expr(e) = part {
                    each_expr(e, visit);
                }
            }
        }
        ExprKind::Record { fields, .. } => {
            for f in fields {
                each_expr(&f.value, visit);
            }
        }
        ExprKind::RecordUpdate { base, fields } => {
            each_expr(base, visit);
            for f in fields {
                each_expr(&f.value, visit);
            }
        }
        ExprKind::Field { base, .. } => each_expr(base, visit),
        ExprKind::Block { stmts } => {
            for stmt in stmts {
                match stmt {
                    BlockStmt::Let(binding) => each_expr(&binding.value, visit),
                    BlockStmt::Expr(e) => each_expr(e, visit),
                }
            }
        }
    }
}

fn each_item<'a>(items: &'a [CeItem], visit: &mut dyn FnMut(&'a Expr)) {
    for item in items {
        match item {
            CeItem::LetBang { value, .. } | CeItem::Let { value, .. } => each_expr(value, visit),
            CeItem::DoBang(e)
            | CeItem::Return(e)
            | CeItem::ReturnBang(e)
            | CeItem::Yield(e)
            | CeItem::YieldBang(e) => each_expr(e, visit),
            CeItem::For { source, body, .. } => {
                each_expr(source, visit);
                each_item(body, visit);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `source` after its one fix titled `title` is applied.
    fn fixed(source: &str, title: &str) -> String {
        let analysis = crate::analyze(source);
        let module = analysis.module.as_ref().expect("the example parses");
        let all = fixes(module, source, &analysis.missing_cases, &analysis.warnings);
        let titles: Vec<&str> = all.iter().map(|f| f.title.as_str()).collect();
        let fix = all
            .iter()
            .find(|f| f.title == title)
            .unwrap_or_else(|| panic!("no fix titled {title:?} among {titles:?}"));
        let mut out = source.to_string();
        let mut edits = fix.edits.clone();
        edits.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
        for (span, text) in edits {
            out.replace_range(span.start..span.end, &text);
        }
        out
    }

    /// The errors left in `source` (a hole is not one).
    fn errors(source: &str) -> Vec<String> {
        crate::analyze(source)
            .diagnostics
            .into_iter()
            .map(|e| e.message)
            .collect()
    }

    #[test]
    fn a_match_gets_an_arm_for_every_case_it_leaves_out() {
        let source = "type Light = Red | Amber | Green\nlet go light =\n    match light:\n        case Red: False\n";
        let after = fixed(source, "Add the 2 missing cases");
        assert_eq!(
            after,
            "type Light = Red | Amber | Green\nlet go light =\n    match light:\n        case Red: False\n        case Amber: ?\n        case Green: ?\n"
        );
        // The match is complete: what remains are the holes to fill in.
        assert_eq!(errors(&after), Vec::<String>::new());
        assert_eq!(crate::analyze(&after).holes.len(), 2);
    }

    #[test]
    fn one_missing_case_is_named_in_the_title() {
        let source = "let size xs =\n    match xs:\n        case []: 0\n";
        let after = fixed(source, "Add the missing case `[_, *_]`");
        assert!(
            after.ends_with("        case []: 0\n        case [_, *_]: ?\n"),
            "{after}"
        );
        assert_eq!(errors(&after), Vec::<String>::new());
    }

    #[test]
    fn constructors_with_fields_are_written_as_patterns() {
        let source = "type Shape =\n    | Circle(float)\n    | Rect { width: float, height: float }\n    | Dot\nlet area shape =\n    match shape:\n        case Dot: 0.0\n";
        let after = fixed(source, "Add the 2 missing cases");
        assert!(
            after.ends_with(
                "        case Dot: 0.0\n        case Circle _: ?\n        case Rect { width = _, height = _ }: ?\n"
            ),
            "{after}"
        );
        assert_eq!(errors(&after), Vec::<String>::new());
    }

    #[test]
    fn nested_cases_are_found_one_at_a_time() {
        let source = "let both pair =\n    match pair:\n        case (True, True): 1\n";
        let after = fixed(source, "Add the 3 missing cases");
        assert!(
            after.ends_with(
                "        case (True, True): 1\n        case (False, True): ?\n        case (True, False): ?\n        case (False, False): ?\n"
            ),
            "{after}"
        );
        assert_eq!(errors(&after), Vec::<String>::new());
    }

    #[test]
    fn a_type_with_no_end_of_values_gets_a_wildcard() {
        let source = "let name n =\n    match n:\n        case 0: \"zero\"\n";
        let after = fixed(source, "Add the missing case `_`");
        assert!(
            after.ends_with("        case 0: \"zero\"\n        case _: ?\n"),
            "{after}"
        );
        assert_eq!(errors(&after), Vec::<String>::new());
    }

    #[test]
    fn the_arms_follow_a_trailing_comment_and_keep_their_indentation() {
        let source = "let pick flag =\n  match flag:\n    case True: 1  # the easy one\n";
        let after = fixed(source, "Add the missing case `False`");
        assert_eq!(
            after,
            "let pick flag =\n  match flag:\n    case True: 1  # the easy one\n    case False: ?\n"
        );
    }

    #[test]
    fn an_unused_name_is_renamed_to_say_so() {
        let source = "let total xs =\n    let spare = List.len xs\n    List.sum xs\n";
        let after = fixed(source, "Rename to `_spare`");
        assert_eq!(
            after,
            "let total xs =\n    let _spare = List.len xs\n    List.sum xs\n"
        );
        assert!(crate::analyze(&after).warnings.is_empty());
    }

    #[test]
    fn an_unused_extern_import_is_removed_with_its_line() {
        let source = "extern import json\nlet n = 1\n";
        let after = fixed(source, "Remove the unused import");
        assert_eq!(after, "let n = 1\n");
        assert!(crate::analyze(&after).warnings.is_empty());
    }
}
