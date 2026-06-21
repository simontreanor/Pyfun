//! A tiny, dependency-free JSON value type with a recursive-descent parser and a
//! compact serializer — just enough to speak LSP's JSON-RPC (`DESIGN.md` §9).
//!
//! The crate is deliberately dependency-free (no `serde`), so the LSP brings its
//! own JSON. This is the smallest thing that works: objects keep insertion order
//! in a `Vec` (LSP payloads are small, so linear key lookup is fine), and numbers
//! collapse to `f64` (LSP integers all fit). It is not a general-purpose JSON
//! library — it covers exactly what the protocol needs.

/// A JSON value.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Array(Vec<Json>),
    /// An object as ordered `(key, value)` pairs.
    Object(Vec<(String, Json)>),
}

impl Json {
    /// Look up a key in an object (the first match), or `None` for non-objects.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// A JSON number as an `i64` (LSP ids and positions are integers).
    pub fn as_i64(&self) -> Option<i64> {
        self.as_f64().map(|n| n as i64)
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(true) => out.push_str("true"),
            Json::Bool(false) => out.push_str("false"),
            Json::Num(n) => {
                // Emit integral values without a trailing `.0` (LSP wants ints
                // for ids, positions, severities).
                if n.fract() == 0.0 && n.is_finite() {
                    out.push_str(&(*n as i64).to_string());
                } else {
                    out.push_str(&n.to_string());
                }
            }
            Json::Str(s) => write_string(s, out),
            Json::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            Json::Object(entries) => {
                out.push('{');
                for (i, (k, v)) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_string(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

/// Serialize to compact JSON (no insignificant whitespace) via `to_string()`.
impl std::fmt::Display for Json {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut out = String::new();
        self.write(&mut out);
        f.write_str(&out)
    }
}

/// Write a JSON string literal with the mandatory escapes.
fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Parse a JSON document, returning the value or an error message.
pub fn parse(input: &str) -> Result<Json, String> {
    let mut p = Parser {
        chars: input.chars().collect(),
        pos: 0,
    };
    p.skip_ws();
    let value = p.value()?;
    p.skip_ws();
    if p.pos != p.chars.len() {
        return Err("trailing characters after JSON value".to_string());
    }
    Ok(value)
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.pos += 1;
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        match self.peek() {
            Some('{') => self.object(),
            Some('[') => self.array(),
            Some('"') => Ok(Json::Str(self.string()?)),
            Some('t') | Some('f') => self.boolean(),
            Some('n') => self.null(),
            Some(c) if c == '-' || c.is_ascii_digit() => self.number(),
            Some(c) => Err(format!("unexpected character `{c}` in JSON")),
            None => Err("unexpected end of JSON".to_string()),
        }
    }

    fn expect(&mut self, c: char) -> Result<(), String> {
        if self.bump() == Some(c) {
            Ok(())
        } else {
            Err(format!("expected `{c}` in JSON"))
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.expect('{')?;
        let mut entries = Vec::new();
        self.skip_ws();
        if self.peek() == Some('}') {
            self.pos += 1;
            return Ok(Json::Object(entries));
        }
        loop {
            self.skip_ws();
            let key = self.string()?;
            self.skip_ws();
            self.expect(':')?;
            self.skip_ws();
            let val = self.value()?;
            entries.push((key, val));
            self.skip_ws();
            match self.bump() {
                Some(',') => continue,
                Some('}') => break,
                _ => return Err("expected `,` or `}` in object".to_string()),
            }
        }
        Ok(Json::Object(entries))
    }

    fn array(&mut self) -> Result<Json, String> {
        self.expect('[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(']') {
            self.pos += 1;
            return Ok(Json::Array(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value()?);
            self.skip_ws();
            match self.bump() {
                Some(',') => continue,
                Some(']') => break,
                _ => return Err("expected `,` or `]` in array".to_string()),
            }
        }
        Ok(Json::Array(items))
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect('"')?;
        let mut s = String::new();
        loop {
            match self.bump() {
                Some('"') => break,
                Some('\\') => match self.bump() {
                    Some('"') => s.push('"'),
                    Some('\\') => s.push('\\'),
                    Some('/') => s.push('/'),
                    Some('b') => s.push('\u{0008}'),
                    Some('f') => s.push('\u{000c}'),
                    Some('n') => s.push('\n'),
                    Some('r') => s.push('\r'),
                    Some('t') => s.push('\t'),
                    Some('u') => s.push(self.unicode_escape()?),
                    _ => return Err("invalid escape in JSON string".to_string()),
                },
                Some(c) => s.push(c),
                None => return Err("unterminated JSON string".to_string()),
            }
        }
        Ok(s)
    }

    /// Read the four hex digits after `\u`, decoding a surrogate pair if present.
    fn unicode_escape(&mut self) -> Result<char, String> {
        let hi = self.hex4()?;
        let code = if (0xD800..=0xDBFF).contains(&hi) {
            // High surrogate — a low surrogate must follow.
            if self.bump() != Some('\\') || self.bump() != Some('u') {
                return Err("expected low surrogate in JSON string".to_string());
            }
            let lo = self.hex4()?;
            0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
        } else {
            hi
        };
        char::from_u32(code).ok_or_else(|| "invalid unicode escape in JSON".to_string())
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let mut code = 0u32;
        for _ in 0..4 {
            let c = self.bump().ok_or("truncated unicode escape")?;
            let digit = c.to_digit(16).ok_or("invalid hex in unicode escape")?;
            code = code * 16 + digit;
        }
        Ok(code)
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.pos;
        if self.peek() == Some('-') {
            self.pos += 1;
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
        {
            self.pos += 1;
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        text.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| format!("invalid number `{text}` in JSON"))
    }

    fn boolean(&mut self) -> Result<Json, String> {
        if self.literal("true") {
            Ok(Json::Bool(true))
        } else if self.literal("false") {
            Ok(Json::Bool(false))
        } else {
            Err("invalid literal in JSON".to_string())
        }
    }

    fn null(&mut self) -> Result<Json, String> {
        if self.literal("null") {
            Ok(Json::Null)
        } else {
            Err("invalid literal in JSON".to_string())
        }
    }

    fn literal(&mut self, word: &str) -> bool {
        let end = self.pos + word.len();
        if end <= self.chars.len() && self.chars[self.pos..end].iter().copied().eq(word.chars()) {
            self.pos = end;
            true
        } else {
            false
        }
    }
}

/// Build a JSON object from `(key, value)` pairs — sugar for handler code.
pub fn obj(entries: Vec<(&str, Json)>) -> Json {
    Json::Object(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}

/// A JSON string value from anything string-like.
pub fn str(s: impl Into<String>) -> Json {
    Json::Str(s.into())
}

/// A JSON number from an integer.
pub fn int(n: i64) -> Json {
    Json::Num(n as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_nested_values() {
        let src = r#"{"a":1,"b":[true,null,"x"],"c":{"d":-2.5}}"#;
        let parsed = parse(src).unwrap();
        assert_eq!(parsed.to_string(), src);
    }

    #[test]
    fn parses_escapes_and_unicode() {
        let parsed = parse(r#""line\nbreak A 😀""#).unwrap();
        assert_eq!(parsed.as_str().unwrap(), "line\nbreak A 😀");
    }

    #[test]
    fn accessors_navigate_objects() {
        let parsed = parse(r#"{"params":{"id":7}}"#).unwrap();
        assert_eq!(
            parsed.get("params").unwrap().get("id").unwrap().as_i64(),
            Some(7)
        );
    }

    #[test]
    fn serializes_control_chars() {
        assert_eq!(Json::Str("\u{1}".to_string()).to_string(), "\"\\u0001\"");
    }
}
