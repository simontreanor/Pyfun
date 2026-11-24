//! `pyfun fmt` (`DESIGN.md` §9): one way to space and indent a program.
//!
//! The formatter keeps every line break and every comment where the author put
//! it, and settles what is left to taste on a line: the indentation, the spaces
//! between tokens, blank lines and the end of the file. It never joins or
//! splits a line, so it has no line width to hold to, and a layout chosen for
//! the reader (a pipeline with a step per line, a list with an element per
//! line) stays as written.
//!
//! It works on the token stream. Each token is copied out exactly as the
//! source spells it, so a number, a string and an f-string are untouched; what
//! is written again is the text *between* tokens. A gap on one line becomes
//! one space or none by rule ([`wants_space`]); a gap that crosses a line
//! keeps its comments and is indented again ([`Columns`]).
//!
//! Indentation carries meaning in Pyfun, so the result is checked: the
//! formatted text must parse to the program the source parses to (the AST
//! compares without positions). When it does not, the formatter tries again
//! with the indentation left alone, and failing that reports the file as one
//! it will not touch. A file is never written unless it means what it meant.

use crate::CompileError;
use crate::lexer::{self, Tok, Token};

/// One step of indentation: two spaces, as the lessons and examples are written.
const INDENT: usize = 2;

/// Why a source was not formatted.
#[derive(Debug)]
pub enum FormatError {
    /// The source does not parse; a program is formatted once it does.
    Parse(CompileError),
    /// No safe formatting was found: every attempt changed what the program
    /// means, or lost something between its tokens. The file is left as is.
    Unsafe,
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormatError::Parse(error) => write!(f, "{}", error.message()),
            FormatError::Unsafe => write!(
                f,
                "this file has a layout the formatter cannot keep the meaning of, so it is \
                 left as it is; please report it"
            ),
        }
    }
}

/// `source`, formatted. Line endings are kept as the source has them.
pub fn format(source: &str) -> Result<String, FormatError> {
    let crlf = source.contains("\r\n");
    let unix = if crlf {
        source.replace("\r\n", "\n")
    } else {
        source.to_string()
    };
    let formatted = format_lf(&unix)?;
    Ok(if crlf {
        formatted.replace('\n', "\r\n")
    } else {
        formatted
    })
}

fn format_lf(source: &str) -> Result<String, FormatError> {
    let module = crate::parse(source).map_err(FormatError::Parse)?;
    let tokens = lexer::lex(source).map_err(|e| FormatError::Parse(CompileError::Lex(e)))?;
    for reindent in [true, false] {
        let Some(text) = Writer::new(source, &tokens, reindent).run() else {
            continue;
        };
        if crate::parse(&text).is_ok_and(|formatted| formatted == module) {
            return Ok(text);
        }
    }
    Err(FormatError::Unsafe)
}

/// How a pair of braces is spaced inside.
#[derive(Clone, Copy, PartialEq)]
enum Braces {
    /// `Rect { width = 2.0 }`, `{ p with x = 1 }`, `async { return 1 }`, a
    /// record type's `{ x: int }`.
    Spaced,
    /// `{"a": 1}`, `{1, 2}`, `{}`, and the effects of an arrow, `->{io}`.
    Tight,
}

/// What the tokens around a gap say about it, worked out before writing.
struct Marks {
    /// For each `{` and `}`, how the pair is spaced.
    braces: Vec<Option<Braces>>,
    /// Gaps left as written (a space stays a space, none stays none): inside
    /// a unit `<m/s^2>`, and through a `measure` declaration.
    as_written: Vec<bool>,
    /// Gaps closed whatever the tokens are: after the `*` of a `*rest`
    /// pattern.
    tight: Vec<bool>,
}

fn is_layout(tok: &Tok) -> bool {
    matches!(tok, Tok::Sep | Tok::Indent | Tok::Dedent | Tok::Eof)
}

fn is_ident(tok: &Tok) -> bool {
    matches!(tok, Tok::Ident(_))
}

/// The binary operators that read one way only, and so always sit between
/// spaces. `-`, `<`, `>`, `|`, `.` and `^` are left as written: each also has a
/// tight use (a negative number, a unit, an active pattern's name, a member).
fn always_spaced(tok: &Tok) -> bool {
    matches!(
        tok,
        Tok::Eq
            | Tok::EqEq
            | Tok::BangEq
            | Tok::Le
            | Tok::Ge
            | Tok::PipeOp
            | Tok::PipeLeft
            | Tok::GtGt
            | Tok::LtLt
            | Tok::LArrow
            | Tok::Arrow
            | Tok::Plus
            | Tok::Star
            | Tok::Slash
            | Tok::SlashSlash
            | Tok::Percent
            | Tok::StarStar
    )
}

impl Marks {
    /// `real` is the token stream without layout tokens; `heads[i]` says token
    /// `i` opens a statement.
    fn of(real: &[&Token], heads: &[bool]) -> Marks {
        let mut marks = Marks {
            braces: vec![None; real.len()],
            as_written: vec![false; real.len()],
            tight: vec![false; real.len() + 1],
        };
        let mut open: Vec<usize> = Vec::new();
        let mut head: Option<&Tok> = None;
        for (i, token) in real.iter().enumerate() {
            if heads[i] {
                head = Some(&token.tok);
            }
            match &token.tok {
                Tok::Star if i > 0 && matches!(real[i - 1].tok, Tok::LBracket | Tok::Comma) => {
                    marks.tight[i + 1] = true;
                }
                _ => {}
            }
            if matches!(head, Some(Tok::Measure)) {
                marks.as_written[i] = true;
            }
            match &token.tok {
                Tok::LBrace => {
                    marks.braces[i] = Some(brace_kind(real, i, head));
                    open.push(i);
                }
                Tok::RBrace => {
                    if let Some(opener) = open.pop() {
                        marks.braces[i] = marks.braces[opener];
                    }
                }
                // A unit: `<` hard against what it measures, then names,
                // numbers and unit operators up to the `>` on the same line.
                Tok::Lt if i > 0 && real[i - 1].span.end == token.span.start => {
                    let close = (i + 1..real.len())
                        .take_while(|&j| real[j].line_start.is_none())
                        .find(|&j| {
                            !matches!(
                                real[j].tok,
                                Tok::Ident(_)
                                    | Tok::Int(_)
                                    | Tok::Slash
                                    | Tok::Star
                                    | Tok::Caret
                                    | Tok::Minus
                            )
                        });
                    if let Some(close) = close
                        && real[close].tok == Tok::Gt
                    {
                        for mark in &mut marks.as_written[i..=close] {
                            *mark = true;
                        }
                    }
                }
                _ => {}
            }
        }
        marks
    }
}

/// How the braces opened by `real[i]` are spaced.
fn brace_kind(real: &[&Token], i: usize, head: Option<&Tok>) -> Braces {
    let next = real.get(i + 1).map(|t| &t.tok);
    if next == Some(&Tok::RBrace) {
        return Braces::Tight;
    }
    let prev = i.checked_sub(1).map(|p| real[p]);
    if let Some(prev) = prev {
        // The effects of an arrow: `->{io}`.
        if prev.tok == Tok::Arrow && prev.span.end == real[i].span.start {
            return Braces::Tight;
        }
        // A record's tag, a constructor's fields, a builder's block.
        if let Tok::Ident(name) = &prev.tok
            && (name.starts_with(|c: char| c.is_ascii_uppercase())
                || matches!(name.as_str(), "async" | "seq" | "result" | "option"))
        {
            return Braces::Spaced;
        }
    }
    if matches!(head, Some(Tok::Type)) {
        return Braces::Spaced;
    }
    // A record update or a block says so inside; a map or a set does not.
    let mut depth = 0usize;
    for (offset, token) in real[i + 1..].iter().enumerate() {
        match &token.tok {
            Tok::LBrace | Tok::LParen | Tok::LBracket => depth += 1,
            Tok::RBrace | Tok::RParen | Tok::RBracket => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            Tok::With | Tok::Eq if depth == 0 => return Braces::Spaced,
            Tok::Let | Tok::Return | Tok::Yield | Tok::Do if depth == 0 && offset == 0 => {
                return Braces::Spaced;
            }
            _ => {}
        }
    }
    Braces::Tight
}

/// Whether one space goes between `a` and `b` on a line, where the source had
/// a space there or not (`had`).
fn wants_space(a: &Token, b: &Token, had: bool, braces: (Option<Braces>, Option<Braces>)) -> bool {
    match (&a.tok, &b.tok) {
        (Tok::LParen | Tok::LBracket, _) | (_, Tok::RParen | Tok::RBracket) => false,
        (_, Tok::Comma) => false,
        (Tok::Comma, _) => true,
        (Tok::LBrace, Tok::RBrace) => false,
        (Tok::LBrace, _) => braces.0 == Some(Braces::Spaced),
        (_, Tok::RBrace) => braces.1 == Some(Braces::Spaced),
        // `->{io}` is an arrow's effects, `-> {1, 2}` a set: the space says which.
        (Tok::Arrow, Tok::LBrace) => had,
        (_, Tok::LBrace) if is_ident(&a.tok) && braces.1 == Some(Braces::Spaced) => true,
        (_, Tok::Colon) => false,
        (Tok::Colon, _) => true,
        _ if always_spaced(&a.tok) || always_spaced(&b.tok) => true,
        _ => had,
    }
}

/// The mapping from the columns a source is indented to, to the columns it is
/// written at: each level one step deeper than the one it sits in. The offside
/// rule reads columns, so a mapping that keeps equal columns equal and deeper
/// ones deeper keeps the blocks.
struct Columns {
    /// `(column in the source, column written)`, shallowest first.
    levels: Vec<(usize, usize)>,
}

impl Columns {
    fn new() -> Columns {
        Columns {
            levels: vec![(0, 0)],
        }
    }

    /// The column a line that starts at `col` in the source is written at.
    fn place(&mut self, col: usize) -> usize {
        while self.levels.len() > 1 && self.levels.last().is_some_and(|&(from, _)| from > col) {
            self.levels.pop();
        }
        let &(from, to) = self.levels.last().expect("the margin is always a level");
        if from >= col {
            return to;
        }
        self.levels.push((col, to + INDENT));
        to + INDENT
    }

    /// The column of the level a comment at `col` belongs to, without opening
    /// one: the deepest level that starts at or before it.
    fn nearest(&self, col: usize) -> usize {
        self.levels
            .iter()
            .rev()
            .find(|&&(from, _)| from <= col)
            .map_or(0, |&(_, to)| to)
    }
}

struct Writer<'a> {
    source: &'a str,
    real: Vec<&'a Token>,
    marks: Marks,
    reindent: bool,
    columns: Columns,
    out: String,
}

impl<'a> Writer<'a> {
    fn new(source: &'a str, tokens: &'a [Token], reindent: bool) -> Writer<'a> {
        let mut real = Vec::new();
        let mut heads = Vec::new();
        let mut at_head = true;
        for token in tokens {
            if is_layout(&token.tok) {
                at_head = true;
            } else {
                real.push(token);
                heads.push(at_head);
                at_head = false;
            }
        }
        let marks = Marks::of(&real, &heads);
        Writer {
            source,
            real,
            marks,
            reindent,
            columns: Columns::new(),
            out: String::with_capacity(source.len()),
        }
    }

    /// The formatted text, or `None` when something other than space and
    /// comments sits between two tokens (which no source that lexes has).
    fn run(mut self) -> Option<String> {
        let mut end = 0;
        for i in 0..self.real.len() {
            let token = self.real[i];
            let gap = &self.source[end..token.span.start];
            if i == 0 || gap.contains('\n') {
                self.between_lines(gap, i == 0, Some(token.span.start))?;
            } else {
                if !gap.bytes().all(|b| b == b' ' || b == b'\t') {
                    return None;
                }
                let had = !gap.is_empty();
                let space = if self.marks.tight[i] {
                    false
                } else if self.marks.as_written[i] {
                    had
                } else {
                    wants_space(
                        self.real[i - 1],
                        token,
                        had,
                        (self.marks.braces[i - 1], self.marks.braces[i]),
                    )
                };
                if space {
                    self.out.push(' ');
                }
            }
            self.out
                .push_str(&self.source[token.span.start..token.span.end]);
            end = token.span.end;
        }
        let tail = &self.source[end..];
        let empty = self.real.is_empty();
        self.between_lines(tail, empty, None)?;
        // One newline closes the file, and no blank line before it.
        let kept = self.out.trim_end_matches([' ', '\n']).len();
        self.out.truncate(kept);
        if !self.out.is_empty() {
            self.out.push('\n');
        }
        Some(self.out)
    }

    /// The column of the byte at `at` in its line.
    fn column(&self, at: usize) -> usize {
        at - self.source[..at].rfind('\n').map_or(0, |nl| nl + 1)
    }

    /// Write what lies between two tokens on different lines: the comment that
    /// closes the first one's line, the comment lines and blank lines between,
    /// and the indentation of the second one's line (`next`, the offset of its
    /// first token; `None` at the end of the file). `opening` is the stretch
    /// before the first token, which starts on a line of its own.
    fn between_lines(&mut self, gap: &str, opening: bool, next: Option<usize>) -> Option<()> {
        let mut lines: Vec<&str> = gap.split('\n').collect();
        // What follows the last newline is the next line's indentation; at the
        // end of the file it is a last line with no newline after it.
        let last = lines.pop().unwrap_or("");
        if next.is_some() {
            if !last.trim().is_empty() {
                return None;
            }
        } else {
            lines.push(last);
        }
        let mut lines = lines.into_iter();
        if !opening {
            // The rest of the line the previous token is on.
            let rest = lines.next().unwrap_or("");
            let comment = rest.trim();
            if comment.starts_with('#') {
                // Two spaces at least, and more when the author lined
                // comments up in a column.
                let before = rest.len() - rest.trim_start().len();
                self.out.push_str(&" ".repeat(before.max(2)));
                self.out.push_str(comment);
            } else if !comment.is_empty() {
                return None;
            }
            self.out.push('\n');
        }
        // The level each comment line sits at is decided against the line
        // that follows, so place that one first.
        let before = Columns {
            levels: self.columns.levels.clone(),
        };
        let above = before.levels.last().map_or(0, |&(from, _)| from);
        let next_col = next.map(|at| self.column(at));
        let next_indent = next_col.map(|col| {
            if self.reindent {
                self.columns.place(col)
            } else {
                col
            }
        });
        let mut blank = false;
        for line in lines {
            let text = line.trim();
            if text.is_empty() {
                blank = true;
                continue;
            }
            if !text.starts_with('#') {
                return None;
            }
            // One blank line at most, and none at the head of the file.
            if blank && !self.out.is_empty() {
                self.out.push('\n');
            }
            blank = false;
            let col = line.len() - line.trim_start().len();
            let indent = match (next_col, next_indent) {
                _ if !self.reindent => col,
                // A comment goes with the code after it when it is no deeper
                // than that code, or when that code opens a block.
                (Some(next_col), Some(next_indent)) if col <= next_col || next_col > above => {
                    next_indent
                }
                // A deeper one closes the block above.
                _ => before.nearest(col),
            };
            self.out.push_str(&" ".repeat(indent));
            self.out.push_str(text);
            self.out.push('\n');
        }
        if let Some(indent) = next_indent {
            if blank && !self.out.is_empty() {
                self.out.push('\n');
            }
            self.out.push_str(&" ".repeat(indent));
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(source: &str) -> String {
        format(source).unwrap_or_else(|e| panic!("{e}\n{source}"))
    }

    #[test]
    fn spaces_on_a_line_are_settled() {
        assert_eq!(fmt("let  x=1+2*3\n"), "let x = 1 + 2 * 3\n");
        assert_eq!(fmt("let xs = [ 1,2 ,3 ]\n"), "let xs = [1, 2, 3]\n");
        assert_eq!(
            fmt("let t = ( 1 , \"a\" )\nprint( t )\n"),
            "let t = (1, \"a\")\nprint(t)\n"
        );
        assert_eq!(
            fmt("let f = fun x->x|>double\nlet double x = x*2\n"),
            "let f = fun x -> x |> double\nlet double x = x * 2\n"
        );
    }

    #[test]
    fn what_reads_two_ways_is_left_as_written() {
        // A negative number, a subtraction, and an operator taken as a value.
        let source = "let a = -1\nlet b = a - 1\nlet c = List.map ((+) 1) [a, -b]\nlet d = (-)\n";
        assert_eq!(fmt(source), source);
        // A unit hugs its number, and what is inside is the author's.
        let source =
            "measure m\nmeasure s\nlet v = 5.0<m/s>\nlet w = 3.0<m / s>\nlet slow = v < w\n";
        assert_eq!(fmt(source), source);
        // An active pattern's name.
        let source = "let (|Even|Odd|) n = if n % 2 == 0 then Even else Odd\n";
        assert_eq!(fmt(source), source);
    }

    #[test]
    fn braces_are_spaced_by_what_they_hold() {
        assert_eq!(
            fmt("type Pt = {x: int, y: int}\nlet p = Pt {x = 1, y = 2}\nlet q = {p with x = 3}\n"),
            "type Pt = { x: int, y: int }\nlet p = Pt { x = 1, y = 2 }\nlet q = { p with x = 3 }\n"
        );
        assert_eq!(
            fmt("let m = { \"a\" : 1, \"b\" : 2 }\nlet s = { 1, 2 }\nlet e = { }\n"),
            "let m = {\"a\": 1, \"b\": 2}\nlet s = {1, 2}\nlet e = {}\n"
        );
        assert_eq!(
            fmt("let job = async {return 1}\nextern now : unit ->{io} float = time.time\n"),
            "let job = async { return 1 }\nextern now: unit ->{io} float = time.time\n"
        );
    }

    #[test]
    fn a_block_is_indented_one_step() {
        let source = "let area shape =\n    match shape:\n            case Circle(r): 3.0 * r * r\n            case Rect { width, height }:\n                 width * height\ntype Shape = Circle(float) | Rect { width: float, height: float }\n";
        assert_eq!(
            fmt(source),
            "let area shape =\n  match shape:\n    case Circle(r): 3.0 * r * r\n    case Rect { width, height }:\n      width * height\ntype Shape = Circle(float) | Rect { width: float, height: float }\n"
        );
    }

    #[test]
    fn line_breaks_are_the_authors() {
        // A step per line stays a step per line, and so does an element per line.
        let source = "let total =\n  [1, 2, 3]\n  |> List.map (fun x -> x * 2)\n  |> List.sum\nlet names = [\n  \"ada\",\n  \"grace\",\n]\n";
        assert_eq!(fmt(source), source);
        // A continuation hung under a bracket moves to one step.
        assert_eq!(
            fmt("let p = Pt { x = 1,\n              y = 2 }\ntype Pt = { x: int, y: int }\n"),
            "let p = Pt { x = 1,\n  y = 2 }\ntype Pt = { x: int, y: int }\n"
        );
    }

    #[test]
    fn comments_stay_and_take_the_level_of_their_code() {
        let source = "# top\nlet f x =\n        # about the body\n    x + 1   # trailing\n  # before g\nlet g = 2 # close\n";
        assert_eq!(
            fmt(source),
            "# top\nlet f x =\n  # about the body\n  x + 1   # trailing\n# before g\nlet g = 2  # close\n"
        );
        // A comment that closes a block stays in it.
        let source = "let f x =\n  x + 1\n  # end of f\nlet g = 2\n";
        assert_eq!(fmt(source), source);
        // A doc comment is the declaration's.
        let source = "## Doubles a number.\nlet double x = x * 2\n";
        assert_eq!(fmt(source), source);
    }

    #[test]
    fn blank_lines_and_the_end_of_the_file() {
        assert_eq!(
            fmt("\n\nlet a = 1\n\n\n\nlet b = 2   \n\n\n"),
            "let a = 1\n\nlet b = 2\n"
        );
        assert_eq!(fmt("let a = 1"), "let a = 1\n");
        assert_eq!(fmt(""), "");
        assert_eq!(fmt("# only a comment"), "# only a comment\n");
    }

    #[test]
    fn a_string_is_never_touched() {
        let source = "let s = \"a  ,b\"\nlet t = f\"{1+2}  x\"\nlet u = \"\"\"\n  keep   this\n      and this\n\"\"\"\n";
        assert_eq!(fmt(source), source);
    }

    #[test]
    fn line_endings_are_kept() {
        assert_eq!(
            fmt("let  a = 1\r\nlet b=2\r\n"),
            "let a = 1\r\nlet b = 2\r\n"
        );
    }

    #[test]
    fn a_program_that_does_not_parse_is_not_formatted() {
        assert!(matches!(format("let = ="), Err(FormatError::Parse(_))));
    }

    #[test]
    fn formatting_twice_changes_nothing() {
        for source in [
            "let  x=1+2*3\nlet f x =\n  x\n",
            "let p = Pt { x = 1,\n              y = 2 }\ntype Pt = { x: int, y: int }\n",
            "# top\nlet f x =\n        # about the body\n    x + 1   # trailing\n",
        ] {
            let once = fmt(source);
            assert_eq!(fmt(&once), once);
        }
    }
}
