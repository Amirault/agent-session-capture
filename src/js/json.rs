//! JSON with JavaScript semantics: `JSON.parse` (including V8's error
//! messages, which the tool reports verbatim) and `JSON.stringify`.
//!
//! Objects keep JavaScript property order: array-index keys first in
//! ascending numeric order, then every other key in insertion order; a
//! duplicate key keeps its first position and takes the last value. Numbers
//! are IEEE doubles printed as JavaScript prints them.

use super::number::number_to_string;
use super::string::{LONE_SURROGATE_TAG, lone_surrogate_unit, push_lone_surrogate, utf16_units};

/// A JSON value.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(Object),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&Object> {
        match self {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// `JSON.stringify(value)`.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        write_value(&mut out, self);
        out
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(s.to_string())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(s)
    }
}

impl From<f64> for Value {
    fn from(n: f64) -> Self {
        Value::Number(n)
    }
}

impl From<i64> for Value {
    fn from(n: i64) -> Self {
        Value::Number(n as f64)
    }
}

impl From<usize> for Value {
    fn from(n: usize) -> Self {
        Value::Number(n as f64)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl From<Object> for Value {
    fn from(o: Object) -> Self {
        Value::Object(o)
    }
}

impl From<Vec<Value>> for Value {
    fn from(a: Vec<Value>) -> Self {
        Value::Array(a)
    }
}

/// A JavaScript object's own enumerable properties, in JavaScript order.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Object {
    entries: Vec<(String, Value)>,
    /// Number of leading entries whose key is an array index (kept sorted).
    index_keys: usize,
}

/// The numeric value of an array-index key ("0", "1", … below 2^32 - 1).
fn array_index(key: &str) -> Option<u32> {
    let bytes = key.as_bytes();
    if bytes.is_empty() || bytes.len() > 10 || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if bytes.len() > 1 && bytes[0] == b'0' {
        return None;
    }
    let value: u64 = key.parse().ok()?;
    if value < u64::from(u32::MAX) {
        Some(value as u32)
    } else {
        None
    }
}

impl Object {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn position(&self, key: &str) -> Option<usize> {
        self.entries.iter().position(|(k, _)| k == key)
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.entries.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.position(key).is_some()
    }

    /// `obj[key] = value`: an existing key keeps its position.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        let key = key.into();
        let value = value.into();
        if let Some(i) = self.position(&key) {
            self.entries[i].1 = value;
            return;
        }
        if let Some(index) = array_index(&key) {
            let at =
                self.entries[..self.index_keys].partition_point(|(k, _)| array_index(k).expect("index key") < index);
            self.entries.insert(at, (key, value));
            self.index_keys += 1;
        } else {
            self.entries.push((key, value));
        }
    }

    /// `delete obj[key]`.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let i = self.position(key)?;
        if i < self.index_keys {
            self.index_keys -= 1;
        }
        Some(self.entries.remove(i).1)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.entries.iter().map(|(k, _)| k)
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Value::as_str)
    }

    /// `JSON.stringify(obj)`.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        write_object(&mut out, self);
        out
    }
}

impl FromIterator<(String, Value)> for Object {
    fn from_iter<I: IntoIterator<Item = (String, Value)>>(iter: I) -> Self {
        let mut object = Object::new();
        for (k, v) in iter {
            object.insert(k, v);
        }
        object
    }
}

// ------------------------------------------------------------------ stringify

fn write_value(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => {
            if n.is_finite() {
                out.push_str(&number_to_string(*n));
            } else {
                out.push_str("null");
            }
        }
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(out, item);
            }
            out.push(']');
        }
        Value::Object(object) => write_object(out, object),
    }
}

fn write_object(out: &mut String, object: &Object) {
    out.push('{');
    for (i, (key, value)) in object.entries.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_string(out, key);
        out.push(':');
        write_value(out, value);
    }
    out.push('}');
}

/// `JSON.stringify(s)` for a string.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    write_string(&mut out, s);
    out
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    let bytes = s.as_bytes();
    let mut plain_from = 0;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let escape: Option<&str> = match b {
            b'"' => Some("\\\""),
            b'\\' => Some("\\\\"),
            b'\n' => Some("\\n"),
            b'\r' => Some("\\r"),
            b'\t' => Some("\\t"),
            0x08 => Some("\\b"),
            0x0C => Some("\\f"),
            _ => None,
        };
        if let Some(escape) = escape {
            out.push_str(&s[plain_from..i]);
            out.push_str(escape);
            i += 1;
            plain_from = i;
        } else if b < 0x20 {
            out.push_str(&s[plain_from..i]);
            out.push_str(&format!("\\u{b:04x}"));
            i += 1;
            plain_from = i;
        } else if b == 0xF4 && s[i..].starts_with(LONE_SURROGATE_TAG) {
            // An encoded lone surrogate: `JSON.stringify` escapes it, unless a
            // high and a low one ended up adjacent, which JS sees as one pair.
            let rest = &s[i + LONE_SURROGATE_TAG.len_utf8()..];
            let Some(unit) = rest.chars().next().and_then(lone_surrogate_unit) else {
                i += 1;
                continue;
            };
            out.push_str(&s[plain_from..i]);
            let consumed = LONE_SURROGATE_TAG.len_utf8() + 4;
            let next = &s[i + consumed..];
            let low = next
                .strip_prefix(LONE_SURROGATE_TAG)
                .and_then(|r| r.chars().next())
                .and_then(lone_surrogate_unit)
                .filter(|low| (0xD800..0xDC00).contains(&unit) && (0xDC00..0xE000).contains(low));
            if let Some(low) = low {
                let c = char::decode_utf16([unit, low])
                    .next()
                    .and_then(Result::ok)
                    .expect("valid pair");
                out.push(c);
                i += consumed * 2;
            } else {
                out.push_str(&format!("\\u{unit:04x}"));
                i += consumed;
            }
            plain_from = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&s[plain_from..]);
    out.push('"');
}

// ------------------------------------------------------------------ parse

/// `JSON.parse(text)`; the error is V8's `SyntaxError` message.
pub fn parse(text: &str) -> Result<Value, String> {
    let mut parser = Parser {
        src: text,
        bytes: text.as_bytes(),
        pos: 0,
    };
    let value = parser.parse_value(0)?;
    parser.skip_whitespace();
    if parser.pos < parser.bytes.len() {
        return Err(parser.error_at(Some(Message::UnexpectedNonWhiteSpace), Token::Other));
    }
    Ok(value)
}

#[derive(Clone, Copy, PartialEq)]
enum Token {
    Eos,
    String,
    Number,
    Other,
}

#[derive(Clone, Copy)]
enum Message {
    UnexpectedNonWhiteSpace,
    ExpectedPropNameOrRBrace,
    ExpectedColonAfterPropertyName,
    ExpectedCommaOrRBrace,
    ExpectedCommaOrRBrack,
    ExpectedDoubleQuotedPropertyName,
    UnterminatedString,
    BadControlCharacter,
    BadUnicodeEscape,
    BadEscapedCharacter,
    NoNumberAfterMinusSign,
    UnterminatedFractionalNumber,
    ExponentPartMissingNumber,
}

impl Message {
    fn text(self) -> &'static str {
        match self {
            Message::UnexpectedNonWhiteSpace => "Unexpected non-whitespace character after JSON",
            Message::ExpectedPropNameOrRBrace => "Expected property name or '}'",
            Message::ExpectedColonAfterPropertyName => "Expected ':' after property name",
            Message::ExpectedCommaOrRBrace => "Expected ',' or '}' after property value",
            Message::ExpectedCommaOrRBrack => "Expected ',' or ']' after array element",
            Message::ExpectedDoubleQuotedPropertyName => "Expected double-quoted property name",
            Message::UnterminatedString => "Unterminated string",
            Message::BadControlCharacter => "Bad control character in string literal",
            Message::BadUnicodeEscape => "Bad Unicode escape",
            Message::BadEscapedCharacter => "Bad escaped character",
            Message::NoNumberAfterMinusSign => "No number after minus sign",
            Message::UnterminatedFractionalNumber => "Unterminated fractional number",
            Message::ExponentPartMissingNumber => "Exponent part is missing a number",
        }
    }
}

/// V8 keeps this much context around an unexpected token.
const MAX_CONTEXT_CHARACTERS: usize = 10;
const MIN_ORIGINAL_SOURCE_LENGTH_FOR_CONTEXT: usize = MAX_CONTEXT_CHARACTERS * 2 + 1;
/// Nesting deeper than this overflows V8's stack; reported the same way.
const MAX_DEPTH: usize = 4096;

struct Parser<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

fn token_of(b: Option<u8>) -> Token {
    match b {
        None => Token::Eos,
        Some(b'"') => Token::String,
        Some(b'-' | b'0'..=b'9') => Token::Number,
        Some(_) => Token::Other,
    }
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_whitespace(&mut self) {
        while let Some(b' ' | b'\t' | b'\r' | b'\n') = self.peek() {
            self.pos += 1;
        }
    }

    /// Build V8's SyntaxError message for an unexpected token at `self.pos`.
    fn error_at(&self, message: Option<Message>, token: Token) -> String {
        let prefix = &self.src[..self.pos];
        let units_before = utf16_units(prefix);
        let position = units_before.len();
        let location = || {
            let mut line = 1;
            let mut last_break = 0;
            let mut i = 0;
            while i < units_before.len() {
                let u = units_before[i];
                if u == u16::from(b'\r') && i + 1 < units_before.len() && units_before[i + 1] == u16::from(b'\n') {
                    i += 1;
                }
                let u = units_before[i];
                if u == u16::from(b'\r') || u == u16::from(b'\n') {
                    line += 1;
                    last_break = i + 1;
                }
                i += 1;
            }
            format!(
                "at position {position} (line {line} column {})",
                1 + position - last_break
            )
        };
        if let Some(message) = message {
            let place = if matches!(message, Message::UnexpectedNonWhiteSpace) {
                ""
            } else {
                "in JSON "
            };
            return format!("{} {place}{}", message.text(), location());
        }
        match token {
            Token::Eos => "Unexpected end of JSON input".to_string(),
            Token::Number => format!("Unexpected number in JSON {}", location()),
            Token::String => format!("Unexpected string in JSON {}", location()),
            Token::Other => {
                let all = utf16_units(self.src);
                let length = all.len();
                let unexpected = super::string::from_utf16_units(&all[position..position + 1]);
                let special = ["undefined", "NaN", "Infinity", "[object Object]"];
                if special.contains(&self.src) {
                    return format!("\"{}\" is not valid JSON", self.src);
                }
                if length <= MIN_ORIGINAL_SOURCE_LENGTH_FOR_CONTEXT {
                    return format!("Unexpected token '{unexpected}', \"{}\" is not valid JSON", self.src);
                }
                let (start, end, pre, post) = if position < MAX_CONTEXT_CHARACTERS {
                    (0, position + MAX_CONTEXT_CHARACTERS, "", "...")
                } else if position < length - MAX_CONTEXT_CHARACTERS {
                    (
                        position - MAX_CONTEXT_CHARACTERS,
                        position + MAX_CONTEXT_CHARACTERS,
                        "...",
                        "...",
                    )
                } else {
                    (position - MAX_CONTEXT_CHARACTERS, length, "...", "")
                };
                let context = super::string::from_utf16_units(&all[start..end]);
                format!("Unexpected token '{unexpected}', {pre}\"{context}\"{post} is not valid JSON")
            }
        }
    }

    fn unexpected(&self) -> String {
        self.error_at(None, token_of(self.peek()))
    }

    fn expect_next(&mut self, wanted: u8, message: Message) -> Result<(), String> {
        self.skip_whitespace();
        if self.peek() == Some(wanted) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.error_at(Some(message), token_of(self.peek())))
        }
    }

    fn parse_value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err("Maximum call stack size exceeded".to_string());
        }
        self.skip_whitespace();
        match self.peek() {
            Some(b'"') => Ok(Value::String(self.parse_string()?)),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            Some(b'{') => self.parse_object(depth),
            Some(b'[') => self.parse_array(depth),
            Some(b't') => self.scan_literal("true", Value::Bool(true)),
            Some(b'f') => self.scan_literal("false", Value::Bool(false)),
            Some(b'n') => self.scan_literal("null", Value::Null),
            _ => Err(self.unexpected()),
        }
    }

    fn scan_literal(&mut self, literal: &str, value: Value) -> Result<Value, String> {
        let lit = literal.as_bytes();
        if self.bytes[self.pos..].starts_with(lit) {
            self.pos += lit.len();
            return Ok(value);
        }
        self.pos += 1;
        for &expected in &lit[1..] {
            match self.peek() {
                None => return Err(self.error_at(None, Token::Eos)),
                Some(b) if b != expected => return Err(self.unexpected()),
                Some(_) => self.pos += 1,
            }
        }
        unreachable!("literal matched after a mismatch")
    }

    fn parse_object(&mut self, depth: usize) -> Result<Value, String> {
        self.pos += 1;
        let mut object = Object::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Value::Object(object));
        }
        self.expect_string_start(Message::ExpectedPropNameOrRBrace)?;
        loop {
            let key = self.parse_string()?;
            self.expect_next(b':', Message::ExpectedColonAfterPropertyName)?;
            let value = self.parse_value(depth + 1)?;
            object.insert(key, value);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.expect_string_start(Message::ExpectedDoubleQuotedPropertyName)?;
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Object(object));
                }
                _ => return Err(self.error_at(Some(Message::ExpectedCommaOrRBrace), token_of(self.peek()))),
            }
        }
    }

    /// Skip whitespace and require a `"` (left in place for `parse_string`).
    fn expect_string_start(&mut self, message: Message) -> Result<(), String> {
        self.skip_whitespace();
        if self.peek() == Some(b'"') {
            Ok(())
        } else {
            Err(self.error_at(Some(message), token_of(self.peek())))
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<Value, String> {
        self.pos += 1;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Value::Array(items));
        }
        loop {
            items.push(self.parse_value(depth + 1)?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(self.error_at(Some(Message::ExpectedCommaOrRBrack), token_of(self.peek()))),
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.pos += 1; // opening quote
        let mut out = String::new();
        let mut plain_from = self.pos;
        loop {
            let Some(b) = self.peek() else {
                return Err(self.error_at(Some(Message::UnterminatedString), Token::Other));
            };
            match b {
                b'"' => {
                    out.push_str(&self.src[plain_from..self.pos]);
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    out.push_str(&self.src[plain_from..self.pos]);
                    self.pos += 1;
                    let Some(escaped) = self.peek() else {
                        return Err(self.error_at(None, Token::Eos));
                    };
                    match escaped {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let unit = self.scan_unicode_escape()?;
                            self.push_unit(&mut out, unit)?;
                            plain_from = self.pos;
                            continue;
                        }
                        _ => {
                            let c = self.src[self.pos..].chars().next().expect("char");
                            if (c as u32) > 0xFF {
                                return Err(self.error_at(None, Token::Other));
                            }
                            return Err(self.error_at(Some(Message::BadEscapedCharacter), Token::Other));
                        }
                    }
                    self.pos += 1;
                    plain_from = self.pos;
                }
                0x00..=0x1F => {
                    return Err(self.error_at(Some(Message::BadControlCharacter), Token::Other));
                }
                _ => self.pos += 1,
            }
        }
    }

    /// Read the 4 hex digits after `\u` (cursor on the `u`).
    fn scan_unicode_escape(&mut self) -> Result<u16, String> {
        let mut value: u16 = 0;
        for _ in 0..4 {
            self.pos += 1;
            let digit = self.peek().and_then(|b| (b as char).to_digit(16));
            match digit {
                Some(d) => value = value * 16 + d as u16,
                None => return Err(self.error_at(Some(Message::BadUnicodeEscape), Token::Other)),
            }
        }
        self.pos += 1;
        Ok(value)
    }

    /// Append one UTF-16 unit from an escape, pairing surrogate escapes.
    fn push_unit(&mut self, out: &mut String, unit: u16) -> Result<(), String> {
        if (0xD800..0xDC00).contains(&unit) && self.bytes[self.pos..].starts_with(b"\\u") {
            let save = self.pos;
            self.pos += 1;
            let low = self.scan_unicode_escape()?;
            if (0xDC00..0xE000).contains(&low) {
                let c = char::decode_utf16([unit, low])
                    .next()
                    .and_then(Result::ok)
                    .expect("valid pair");
                out.push(c);
                return Ok(());
            }
            self.pos = save;
        }
        match char::from_u32(u32::from(unit)) {
            Some(c) => out.push(c),
            None => push_lone_surrogate(out, unit),
        }
        Ok(())
    }

    fn parse_number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        let mut negative = false;
        if self.peek() == Some(b'-') {
            negative = true;
            self.pos += 1;
        }
        let is_number_part = |b: Option<u8>| matches!(b, Some(b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-'));
        if self.peek() == Some(b'0') {
            self.pos += 1;
            if is_number_part(self.peek()) {
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(self.error_at(None, Token::Number));
                }
            } else if !negative {
                return Ok(Value::Number(0.0));
            }
        } else {
            let digits_start = self.pos;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
            if self.pos == digits_start {
                return Err(self.error_at(Some(Message::NoNumberAfterMinusSign), Token::Other));
            }
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.error_at(Some(Message::UnterminatedFractionalNumber), Token::Other));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(self.error_at(Some(Message::ExponentPartMissingNumber), Token::Other));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.pos += 1;
            }
        }
        let number: f64 = self.src[start..self.pos].parse().expect("validated JSON number");
        Ok(Value::Number(number))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_with_javascript_key_order() {
        let v = parse(r#"{"b":1,"2":true,"a":[null,"x\n"],"1":1.50,"b":2}"#).unwrap();
        assert_eq!(v.to_json(), r#"{"1":1.5,"2":true,"b":2,"a":[null,"x\n"]}"#);
    }

    #[test]
    fn reports_v8_messages() {
        assert_eq!(parse("").unwrap_err(), "Unexpected end of JSON input");
        assert_eq!(
            parse("{not json").unwrap_err(),
            "Expected property name or '}' in JSON at position 1 (line 1 column 2)"
        );
        assert_eq!(
            parse("not json args").unwrap_err(),
            "Unexpected token 'o', \"not json args\" is not valid JSON"
        );
        assert_eq!(
            parse("[1, 2").unwrap_err(),
            "Expected ',' or ']' after array element in JSON at position 5 (line 1 column 6)"
        );
        assert_eq!(
            parse(r#"{"a": 1,}"#).unwrap_err(),
            "Expected double-quoted property name in JSON at position 8 (line 1 column 9)"
        );
    }

    #[test]
    fn keeps_lone_surrogates() {
        let v = parse(r#""\ud83d x 🚀""#).unwrap();
        assert_eq!(v.to_json(), "\"\\ud83d x 🚀\"");
    }
}
