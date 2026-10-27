//! 极简 JSON 解析（零第三方 crate，供 MCP 与 semantic 权重文件复用）。
//! 值保留键序；递归下降解析，规模远小于深度限制风险。

#![forbid(unsafe_code)]
// 公共工具模块：mcp/semantic 各 feature 下消费点不同，允许部分方法在某构建未用。
#![allow(dead_code)]

#[derive(Clone, Debug)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn num(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }
}

/// JSON 字符串转义（唯一实现，供各序列化点复用，避免重复）。
pub fn string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
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
    out.push('"');
    out
}

/// 最大嵌套深度：递归下降解析器在更深处直接报错，防恶意深嵌套触发栈溢出。
const MAX_DEPTH: usize = 128;

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Parser {
            src: src.as_bytes(),
            pos: 0,
            depth: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn space(&mut self) {
        while let Some(b) = self.peek() {
            if b.is_ascii_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn eat(&mut self, b: u8) -> Result<(), String> {
        self.space();
        if self.peek() == Some(b) {
            self.pos += 1;
            Ok(())
        } else {
            Err(format!("expected byte {b:?} at pos {}", self.pos))
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.space();
        if matches!(self.peek(), Some(b'{') | Some(b'[')) {
            self.depth += 1;
            if self.depth > MAX_DEPTH {
                self.depth -= 1;
                return Err(format!("nesting exceeds {MAX_DEPTH} at pos {}", self.pos));
            }
            let v = match self.peek() {
                Some(b'{') => self.obj(),
                Some(b'[') => self.arr(),
                _ => unreachable!("guarded by matches!"),
            };
            self.depth -= 1;
            return v;
        }
        match self.peek() {
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.lit("true", Json::Bool(true)),
            Some(b'f') => self.lit("false", Json::Bool(false)),
            Some(b'n') => self.lit("null", Json::Null),
            Some(b'-') | Some(b'0'..=b'9') => self.num(),
            Some(b) => Err(format!("unexpected byte {b:?} at pos {}", self.pos)),
            None => Err("unexpected end of input".to_string()),
        }
    }

    fn lit(&mut self, word: &str, v: Json) -> Result<Json, String> {
        if self.src[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(v)
        } else {
            Err(format!("expected literal {word} at pos {}", self.pos))
        }
    }

    fn num(&mut self) -> Result<Json, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while matches!(
            self.peek(),
            Some(b'0'..=b'9') | Some(b'.') | Some(b'e') | Some(b'E') | Some(b'+') | Some(b'-')
        ) {
            self.pos += 1;
        }
        let raw = std::str::from_utf8(&self.src[start..self.pos]).map_err(|e| e.to_string())?;
        let n: f64 = raw
            .parse()
            .map_err(|e| format!("bad number {raw}: {e}"))?;
        // JSON 无 Infinity/NaN：`1e999` 会解析成 inf，放行会导致回显 `"id":inf`
        // 这类非法 JSON（MCP 客户端整流解析失败）。
        if !n.is_finite() {
            return Err(format!("number out of range: {raw}"));
        }
        Ok(Json::Num(n))
    }

    fn string(&mut self) -> Result<String, String> {
        self.eat(b'"')?;
        // 收集原始 UTF-8 字节，收尾统一解码：多字节序列（中文等）不再被按字节拆成 Latin-1 乱码。
        let mut out: Vec<u8> = Vec::new();
        loop {
            match self.peek() {
                Some(b'"') => {
                    self.pos += 1;
                    return String::from_utf8(out).map_err(|e| format!("bad string: {e}"));
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let esc = self
                        .peek()
                        .ok_or_else(|| "unterminated escape".to_string())?;
                    match esc {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'u' => {
                            let cp = self.u_escape()?;
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(cp.encode_utf8(&mut buf).as_bytes());
                        }
                        _ => return Err(format!("bad escape \\{esc:?}")),
                    }
                    self.pos += 1;
                }
                Some(b) => {
                    out.push(b);
                    self.pos += 1;
                }
                None => return Err("unterminated string".to_string()),
            }
        }
    }

    /// 解析 `\uXXXX`（进入时 pos 指向 'u'；返回后 pos 指向该转义序列最后一个 hex 字节，
    /// 由调用方统一 +1 前进），支持 UTF-16 代理对。
    fn u_escape(&mut self) -> Result<char, String> {
        fn hex4(src: &[u8], pos: usize) -> Result<u32, String> {
            // pos 指向 'u'，4 个 hex 紧跟其后
            let raw = src
                .get(pos + 1..pos + 5)
                .ok_or_else(|| "short \\u escape".to_string())?;
            u32::from_str_radix(std::str::from_utf8(raw).map_err(|e| e.to_string())?, 16)
                .map_err(|e| e.to_string())
        }
        let hi = hex4(self.src, self.pos)?;
        if (0xD800..=0xDBFF).contains(&hi) {
            // 高代理必须紧跟 \uXXXX 低代理
            if self.src.get(self.pos + 5) == Some(&b'\\')
                && self.src.get(self.pos + 6) == Some(&b'u')
            {
                let lo = hex4(self.src, self.pos + 6)?;
                if !(0xDC00..=0xDFFF).contains(&lo) {
                    return Err("bad \\u low surrogate".to_string());
                }
                self.pos += 10;
                let cp = 0x1_0000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
                return char::from_u32(cp).ok_or_else(|| "bad \\u code point".to_string());
            }
            return Err("\\u high surrogate without pair".to_string());
        }
        self.pos += 4;
        char::from_u32(hi).ok_or_else(|| "bad \\u code point".to_string())
    }

    fn arr(&mut self) -> Result<Json, String> {
        self.eat(b'[')?;
        let mut items = Vec::new();
        loop {
            self.space();
            if self.peek() == Some(b']') {
                self.pos += 1;
                return Ok(Json::Arr(items));
            }
            items.push(self.value()?);
            self.space();
            if self.peek() == Some(b',') {
                self.pos += 1;
            } else if self.peek() == Some(b']') {
                self.pos += 1;
                return Ok(Json::Arr(items));
            } else {
                return Err(format!("bad array at pos {}", self.pos));
            }
        }
    }

    fn obj(&mut self) -> Result<Json, String> {
        self.eat(b'{')?;
        let mut kv = Vec::new();
        loop {
            self.space();
            if self.peek() == Some(b'}') {
                self.pos += 1;
                return Ok(Json::Obj(kv));
            }
            let key = self.string()?;
            self.eat(b':')?;
            let val = self.value()?;
            kv.push((key, val));
            self.space();
            if self.peek() == Some(b',') {
                self.pos += 1;
            } else if self.peek() == Some(b'}') {
                self.pos += 1;
                return Ok(Json::Obj(kv));
            } else {
                return Err(format!("bad object at pos {}", self.pos));
            }
        }
    }
}

/// 解析 JSON 文本（递归下降；MCP 消息规模远小于深度限制风险）。
pub fn parse(s: &str) -> Result<Json, String> {
    let mut p = Parser::new(s);
    let v = p.value()?;
    p.space();
    if p.peek().is_some() {
        Err(format!("trailing data at pos {}", p.pos))
    } else {
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn str_utf8_multibyte() {
        // 多字节 UTF-8 不得被按字节拆成 Latin-1 乱码（回归：MCP 中文 query/text）。
        let v = parse(r#"{"query":"数据库连接失败"}"#).unwrap();
        assert_eq!(v.get("query").unwrap().str(), Some("数据库连接失败"));
        let v = parse(r#"{"text":"错误行: 端口5432 被占用"}"#).unwrap();
        assert_eq!(
            v.get("text").unwrap().str(),
            Some("错误行: 端口5432 被占用")
        );
    }

    #[test]
    fn str_ascii_escapes() {
        let v = parse(r#"{"a":"x\ny\t\"\\\/z"}"#).unwrap();
        assert_eq!(v.get("a").unwrap().str(), Some("x\ny\t\"\\/z"));
    }

    #[test]
    fn str_u_escapes_utf16_surrogate_pair() {
        // \uD83D\uDE80 = U+1F680 🚀（代理对），\u4E2D = 中
        let v = parse(r#"{"q":"\u4E2D\u6587\uD83D\uDE80"}"#).unwrap();
        assert_eq!(v.get("q").unwrap().str(), Some("中文🚀"));
    }

    #[test]
    fn str_u_escape_high_surrogate_without_pair_errors() {
        assert!(parse(r#"{"q":"\uD83D"}"#).is_err());
        assert!(parse(r#"{"q":"\uDE80"}"#).is_err());
    }

    /// 确定性模糊：MCP 请求是外部不可信输入，任意文本都不得 panic；
    /// 成功解析的值做访问器调用也不得 panic。
    #[test]
    fn parse_never_panics_on_arbitrary_input() {
        let pool: Vec<char> = "{}[]\",:0123456789.eE+-\\/ \n\t\u{0}中文🚀truefalsenull"
            .chars()
            .collect();
        let mut state = 0xfeed_faceu32;
        let mut next = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            state
        };
        for _ in 0..2000 {
            let n = next() as usize % 48;
            let mut s = String::new();
            for _ in 0..n {
                s.push(pool[next() as usize % pool.len()]);
            }
            if let Ok(v) = parse(&s) {
                // 访问器对任意已解析值都必须安全。
                let _ = v.get("id").and_then(Json::str);
                let _ = v.get("params").and_then(|p| p.get("name"));
                let _ = v.str();
                let _ = v.num();
            }
        }
    }

    #[test]
    fn non_finite_numbers_are_rejected() {
        // `1e999` 会解析成 inf；放行会让 MCP 回显 `"id":inf` 这种非法 JSON。
        assert!(parse(r#"{"id":1e999}"#).is_err());
        assert!(parse(r#"{"id":-1e999}"#).is_err());
        // 1.5e308 仍在 f64 范围内（有限），必须照常接受。
        assert!(parse(r#"{"id":1.5e308}"#).is_ok());
        assert!(parse(r#"{"id":1e10}"#).is_ok());
    }

    #[test]
    fn deep_nesting_is_rejected_not_stack_overflow() {
        let ok = format!("{}1{}", "[".repeat(MAX_DEPTH), "]".repeat(MAX_DEPTH));
        assert!(parse(&ok).is_ok(), "深度内应正常解析");
        let over = format!(
            "{}1{}",
            "[".repeat(MAX_DEPTH + 1),
            "]".repeat(MAX_DEPTH + 1)
        );
        let err = parse(&over).unwrap_err();
        assert!(err.contains("nesting"), "超深应报错而非栈溢出: {err}");
    }
}
