//! 极简 JSON 子集解析/序列化（零依赖，仅覆盖本库自产自销的 case.json schema）。
//!
//! 只支持：object / array / string / number / bool / null；
//! 不支持：转义 \\uXXXX 之外的复杂规则、注释、尾逗号、科学计数法。
//! 解析失败返回 Err，序列化失败仅发生在字符串含未转义控制符时（用 escape_str 不会发生）。

#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Val>),
    Obj(Vec<(String, Val)>),
}

/// 字符串转义（JSON 字符串字面量所需的最小集）。
pub fn escape_str(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
}

/// 序列化 Val 到字符串。
pub fn write(v: &Val) -> String {
    let mut out = String::new();
    write_into(v, &mut out);
    out
}

fn write_into(v: &Val, out: &mut String) {
    match v {
        Val::Null => out.push_str("null"),
        Val::Bool(true) => out.push_str("true"),
        Val::Bool(false) => out.push_str("false"),
        Val::Num(n) => {
            if n.fract() == 0.0 && n.is_finite() && n.abs() < 1e15 {
                out.push_str(&format!("{}", *n as i64));
            } else {
                out.push_str(&format!("{}", n));
            }
        }
        Val::Str(s) => {
            out.push('"');
            escape_str(s, out);
            out.push('"');
        }
        Val::Arr(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_into(item, out);
            }
            out.push(']');
        }
        Val::Obj(kvs) => {
            out.push('{');
            for (i, (k, item)) in kvs.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push('"');
                escape_str(k, out);
                out.push_str("\":");
                write_into(item, out);
            }
            out.push('}');
        }
    }
}

/// 从字符串解析 JSON，返回 Val。递归下降，O(n)。
pub fn parse(s: &str) -> Result<Val, String> {
    let mut p = Parser {
        bytes: s.as_bytes(),
        pos: 0,
    };
    p.skip_ws();
    let v = p.value()?;
    p.skip_ws();
    if p.pos != p.bytes.len() {
        return Err(format!("trailing bytes at {}", p.pos));
    }
    Ok(v)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }
    fn next(&mut self) -> Option<u8> {
        let b = self.peek();
        if b.is_some() {
            self.pos += 1;
        }
        b
    }
    fn skip_ws(&mut self) {
        while self
            .peek()
            .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
        {
            self.pos += 1;
        }
    }
    fn expect(&mut self, b: u8) -> Result<(), String> {
        if self.next() == Some(b) {
            Ok(())
        } else {
            Err(format!("expected {:?} at {}", b as char, self.pos))
        }
    }
    fn value(&mut self) -> Result<Val, String> {
        match self.peek() {
            Some(b'{') => self.obj(),
            Some(b'[') => self.arr(),
            Some(b'"') => self.str().map(Val::Str),
            Some(b't') => self.lit("true", Val::Bool(true)),
            Some(b'f') => self.lit("false", Val::Bool(false)),
            Some(b'n') => self.lit("null", Val::Null),
            Some(b'-') | Some(b'0'..=b'9') => self.num(),
            Some(other) => Err(format!(
                "unexpected byte {:?} at {}",
                other as char, self.pos
            )),
            None => Err("unexpected end of input".to_string()),
        }
    }
    fn lit(&mut self, word: &str, v: Val) -> Result<Val, String> {
        for (i, b) in word.bytes().enumerate() {
            if self.bytes.get(self.pos + i) != Some(&b) {
                return Err(format!("expected literal {} at {}", word, self.pos));
            }
        }
        self.pos += word.len();
        Ok(v)
    }
    fn str(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = String::new();
        let mut raw: Vec<u8> = Vec::new();
        let flush = |out: &mut String, raw: &mut Vec<u8>| -> Result<(), String> {
            if !raw.is_empty() {
                let s =
                    std::str::from_utf8(raw).map_err(|_| "invalid utf8 in string".to_string())?;
                out.push_str(s);
                raw.clear();
            }
            Ok(())
        };
        loop {
            match self.next() {
                Some(b'"') => {
                    flush(&mut out, &mut raw)?;
                    return Ok(out);
                }
                Some(b'\\') => {
                    flush(&mut out, &mut raw)?;
                    match self.next() {
                        Some(b'"') => out.push('"'),
                        Some(b'\\') => out.push('\\'),
                        Some(b'n') => out.push('\n'),
                        Some(b'r') => out.push('\r'),
                        Some(b't') => out.push('\t'),
                        Some(b'b') => out.push('\u{8}'),
                        Some(b'f') => out.push('\u{c}'),
                        Some(b'u') => {
                            let mut code = 0u32;
                            for _ in 0..4 {
                                let c = self.next().ok_or("truncated \\u escape")?;
                                let d = (c as char).to_digit(16).ok_or("bad \\u escape")?;
                                code = code * 16 + d;
                            }
                            out.push(char::from_u32(code).ok_or("bad \\u codepoint")?);
                        }
                        Some(other) => return Err(format!("bad escape {:?}", other as char)),
                        None => return Err("truncated escape".to_string()),
                    }
                }
                Some(b) if (b as char).is_ascii_control() => {
                    return Err(format!("unescaped control byte 0x{:02x}", b));
                }
                Some(b) => raw.push(b),
                None => return Err("unterminated string".to_string()),
            }
        }
    }
    fn num(&mut self) -> Result<Val, String> {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|b| matches!(b, b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'))
        {
            self.pos += 1;
        }
        let txt = std::str::from_utf8(&self.bytes[start..self.pos]).map_err(|_| "bad number")?;
        txt.parse::<f64>()
            .map(Val::Num)
            .map_err(|_| format!("bad number {:?}", txt))
    }
    fn arr(&mut self) -> Result<Val, String> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.next();
            return Ok(Val::Arr(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value()?);
            self.skip_ws();
            match self.next() {
                Some(b',') => continue,
                Some(b']') => return Ok(Val::Arr(items)),
                Some(other) => return Err(format!("expected , or ] got {:?}", other as char)),
                None => return Err("unterminated array".to_string()),
            }
        }
    }
    fn obj(&mut self) -> Result<Val, String> {
        self.expect(b'{')?;
        let mut kvs = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.next();
            return Ok(Val::Obj(kvs));
        }
        loop {
            self.skip_ws();
            let key = self.str()?;
            self.skip_ws();
            self.expect(b':')?;
            self.skip_ws();
            let v = self.value()?;
            kvs.push((key, v));
            self.skip_ws();
            match self.next() {
                Some(b',') => continue,
                Some(b'}') => return Ok(Val::Obj(kvs)),
                Some(other) => return Err(format!("expected , or }} got {:?}", other as char)),
                None => return Err("unterminated object".to_string()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(v: &Val) -> Val {
        parse(&write(v)).unwrap()
    }

    #[test]
    fn scalar_roundtrip() {
        assert_eq!(roundtrip(&Val::Null), Val::Null);
        assert_eq!(roundtrip(&Val::Bool(true)), Val::Bool(true));
        assert_eq!(roundtrip(&Val::Num(42.0)), Val::Num(42.0));
        assert_eq!(roundtrip(&Val::Num(-1.5)), Val::Num(-1.5));
        assert_eq!(
            roundtrip(&Val::Str("中文 \"q\" \\ \n".to_string())),
            Val::Str("中文 \"q\" \\ \n".to_string())
        );
    }

    #[test]
    fn nested_roundtrip() {
        let v = Val::Obj(vec![
            (
                "id".to_string(),
                Val::Str("2026-10-22_a1b2c3d4".to_string()),
            ),
            ("n".to_string(), Val::Num(3.0)),
            (
                "arr".to_string(),
                Val::Arr(vec![Val::Bool(true), Val::Null, Val::Str("x".to_string())]),
            ),
            (
                "fields".to_string(),
                Val::Obj(vec![(
                    "error_code".to_string(),
                    Val::Arr(vec![Val::Str("conn_id=9f2a".to_string())]),
                )]),
            ),
        ]);
        assert_eq!(roundtrip(&v), v);
    }

    #[test]
    fn parse_errors() {
        assert!(parse("{").is_err());
        assert!(parse("{\"a\":}").is_err());
        assert!(parse("[1,2,]").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn cjk_and_utf8_roundtrip() {
        let v = Val::Str("数据库连接超时 conn_id=9f2a 🚀".to_string());
        assert_eq!(roundtrip(&v), v);
    }

    #[test]
    fn write_format_sanity() {
        assert_eq!(
            write(&Val::Obj(vec![("a".to_string(), Val::Num(1.0))])),
            "{\"a\":1}"
        );
        assert_eq!(write(&Val::Str("a\"b".to_string())), "\"a\\\"b\"");
        assert_eq!(write(&Val::Arr(vec![])), "[]");
    }
}
