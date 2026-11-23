// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! Minimal order-preserving JSON reader for DynamoDB responses
//! (port of wire/json_reader.{hpp,cpp}).

use crate::error::{Errc, Error, Result};

const MAX_DEPTH: usize = 256;

#[derive(Debug, Clone, PartialEq, Default)]
pub enum JsonValue {
    #[default]
    Null,
    Bool(bool),
    /// Verbatim number token.
    Number(String),
    String(String),
    Array(Vec<JsonValue>),
    /// Members in document order.
    Object(Vec<(String, JsonValue)>),
}

impl JsonValue {
    pub fn find(&self, key: &str) -> Option<&JsonValue> {
        match self {
            JsonValue::Object(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn is_object(&self) -> bool {
        matches!(self, JsonValue::Object(_))
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsonValue::String(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            JsonValue::Bool(b) => Some(*b),
            _ => None,
        }
    }
    pub fn as_number(&self) -> Option<&str> {
        match self {
            JsonValue::Number(n) => Some(n),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&[JsonValue]> {
        match self {
            JsonValue::Array(a) => Some(a),
            _ => None,
        }
    }
    pub fn members(&self) -> Option<&[(String, JsonValue)]> {
        match self {
            JsonValue::Object(m) => Some(m),
            _ => None,
        }
    }
}

pub(crate) fn malformed(detail: &str) -> Error {
    Error::new(
        Errc::ServiceError,
        format!("malformed response: {}", detail),
    )
}

struct Parser<'a> {
    s: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn eof(&self) -> bool {
        self.pos >= self.s.len()
    }
    fn peek(&self) -> u8 {
        self.s[self.pos]
    }
    fn skip_ws(&mut self) {
        while !self.eof() && matches!(self.peek(), b' ' | b'\t' | b'\n' | b'\r') {
            self.pos += 1;
        }
    }

    fn parse_value(&mut self, depth: usize) -> Result<JsonValue> {
        if depth > MAX_DEPTH {
            return Err(malformed("nesting too deep"));
        }
        if self.eof() {
            return Err(malformed("unexpected end of input"));
        }
        match self.peek() {
            b'{' => self.parse_object(depth),
            b'[' => self.parse_array(depth),
            b'"' => Ok(JsonValue::String(self.parse_string()?)),
            b't' | b'f' => self.parse_bool(),
            b'n' => self.parse_null(),
            c if c == b'-' || c.is_ascii_digit() => self.parse_number(),
            _ => Err(malformed("unexpected token")),
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<JsonValue> {
        self.pos += 1;
        let mut members = Vec::new();
        self.skip_ws();
        if !self.eof() && self.peek() == b'}' {
            self.pos += 1;
            return Ok(JsonValue::Object(members));
        }
        loop {
            self.skip_ws();
            if self.eof() || self.peek() != b'"' {
                return Err(malformed("expected object key string"));
            }
            let key = self.parse_string()?;
            self.skip_ws();
            if self.eof() || self.peek() != b':' {
                return Err(malformed("expected ':' after object key"));
            }
            self.pos += 1;
            self.skip_ws();
            let val = self.parse_value(depth + 1)?;
            members.push((key, val));
            self.skip_ws();
            if self.eof() {
                return Err(malformed("unterminated object"));
            }
            match self.peek() {
                b',' => self.pos += 1,
                b'}' => {
                    self.pos += 1;
                    return Ok(JsonValue::Object(members));
                }
                _ => return Err(malformed("expected ',' or '}' in object")),
            }
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<JsonValue> {
        self.pos += 1;
        let mut elements = Vec::new();
        self.skip_ws();
        if !self.eof() && self.peek() == b']' {
            self.pos += 1;
            return Ok(JsonValue::Array(elements));
        }
        loop {
            self.skip_ws();
            elements.push(self.parse_value(depth + 1)?);
            self.skip_ws();
            if self.eof() {
                return Err(malformed("unterminated array"));
            }
            match self.peek() {
                b',' => self.pos += 1,
                b']' => {
                    self.pos += 1;
                    return Ok(JsonValue::Array(elements));
                }
                _ => return Err(malformed("expected ',' or ']' in array")),
            }
        }
    }

    fn parse_bool(&mut self) -> Result<JsonValue> {
        if self.s[self.pos..].starts_with(b"true") {
            self.pos += 4;
            Ok(JsonValue::Bool(true))
        } else if self.s[self.pos..].starts_with(b"false") {
            self.pos += 5;
            Ok(JsonValue::Bool(false))
        } else {
            Err(malformed("invalid literal"))
        }
    }

    fn parse_null(&mut self) -> Result<JsonValue> {
        if self.s[self.pos..].starts_with(b"null") {
            self.pos += 4;
            Ok(JsonValue::Null)
        } else {
            Err(malformed("invalid literal"))
        }
    }

    fn digit(&self) -> bool {
        !self.eof() && self.peek().is_ascii_digit()
    }

    fn parse_number(&mut self) -> Result<JsonValue> {
        let start = self.pos;
        if !self.eof() && self.peek() == b'-' {
            self.pos += 1;
        }
        if !self.digit() {
            return Err(malformed("invalid number"));
        }
        if self.peek() == b'0' {
            self.pos += 1;
        } else {
            while self.digit() {
                self.pos += 1;
            }
        }
        if !self.eof() && self.peek() == b'.' {
            self.pos += 1;
            if !self.digit() {
                return Err(malformed("invalid number fraction"));
            }
            while self.digit() {
                self.pos += 1;
            }
        }
        if !self.eof() && (self.peek() == b'e' || self.peek() == b'E') {
            self.pos += 1;
            if !self.eof() && (self.peek() == b'+' || self.peek() == b'-') {
                self.pos += 1;
            }
            if !self.digit() {
                return Err(malformed("invalid number exponent"));
            }
            while self.digit() {
                self.pos += 1;
            }
        }
        Ok(JsonValue::Number(
            String::from_utf8_lossy(&self.s[start..self.pos]).into_owned(),
        ))
    }

    fn read_hex4(&mut self) -> Result<u32> {
        if self.pos + 4 > self.s.len() {
            return Err(malformed("truncated \\u escape"));
        }
        let mut cp = 0u32;
        for i in 0..4 {
            let h = (self.s[self.pos + i] as char)
                .to_digit(16)
                .ok_or_else(|| malformed("invalid \\u escape"))?;
            cp = (cp << 4) | h;
        }
        self.pos += 4;
        Ok(cp)
    }

    fn parse_string(&mut self) -> Result<String> {
        self.pos += 1;
        let mut out: Vec<u8> = Vec::new();
        while !self.eof() {
            let c = self.peek();
            if c == b'"' {
                self.pos += 1;
                return Ok(String::from_utf8_lossy(&out).into_owned());
            }
            if c == b'\\' {
                self.pos += 1;
                if self.eof() {
                    return Err(malformed("unterminated escape"));
                }
                let e = self.peek();
                self.pos += 1;
                match e {
                    b'"' => out.push(b'"'),
                    b'\\' => out.push(b'\\'),
                    b'/' => out.push(b'/'),
                    b'b' => out.push(0x08),
                    b'f' => out.push(0x0c),
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'u' => {
                        let mut cp = self.read_hex4()?;
                        if (0xD800..=0xDBFF).contains(&cp) {
                            if self.pos + 2 > self.s.len()
                                || self.s[self.pos] != b'\\'
                                || self.s[self.pos + 1] != b'u'
                            {
                                return Err(malformed("unpaired high surrogate"));
                            }
                            self.pos += 2;
                            let lo = self.read_hex4()?;
                            if !(0xDC00..=0xDFFF).contains(&lo) {
                                return Err(malformed("invalid low surrogate"));
                            }
                            cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                        } else if (0xDC00..=0xDFFF).contains(&cp) {
                            return Err(malformed("unexpected low surrogate"));
                        }
                        let ch =
                            char::from_u32(cp).ok_or_else(|| malformed("invalid \\u escape"))?;
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    }
                    _ => return Err(malformed("invalid escape character")),
                }
                continue;
            }
            if c < 0x20 {
                return Err(malformed("unescaped control character in string"));
            }
            out.push(c);
            self.pos += 1;
        }
        Err(malformed("unterminated string"))
    }
}

/// Parse a complete JSON document.
pub fn parse(body: &str) -> Result<JsonValue> {
    let mut p = Parser {
        s: body.as_bytes(),
        pos: 0,
    };
    p.skip_ws();
    let v = p.parse_value(0)?;
    p.skip_ws();
    if p.pos != p.s.len() {
        return Err(malformed("trailing characters after value"));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_in_order() {
        let v = parse(r#"{"b":1,"a":[true,null,"x\u00e9\n"],"c":{"d":-1.5e3}}"#).unwrap();
        let m = v.members().unwrap();
        assert_eq!(m[0].0, "b");
        assert_eq!(m[1].1.as_array().unwrap()[2].as_str().unwrap(), "x\u{e9}\n");
        assert_eq!(
            v.find("c").unwrap().find("d").unwrap().as_number(),
            Some("-1.5e3")
        );
    }

    #[test]
    fn surrogate_pair() {
        assert_eq!(
            parse(r#""\ud83d\ude00""#).unwrap().as_str(),
            Some("\u{1F600}")
        );
    }

    #[test]
    fn malformed_inputs() {
        for bad in [
            "",
            "{",
            "[1,]",
            "{\"a\"}",
            "01",
            "tru",
            "\"\\x\"",
            "1 2",
            "\"\\udc00\"",
        ] {
            let e = parse(bad).unwrap_err();
            assert!(
                e.message.starts_with("malformed response: "),
                "{bad}: {}",
                e.message
            );
        }
    }
}
