//! 极简 JSON 解析（零第三方 crate，供 MCP 与 semantic 权重文件复用）。
//! 值保留键序；递归下降解析，规模远小于深度限制风险。

#![forbid(unsafe_code)]
// 公共工具模块：mcp/semantic 各 feature 下消费点不同，允许部分方法在某构建未用。
#![allow(dead_code)]

#[derive(Clone)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub(crate) fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(kv) => kv.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub(crate) fn str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub(crate) fn num(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Parser {
            src: src.as_bytes(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek() {
            if b.is_ascii_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn eat(&mut self, b: u8) -> Result<(), String> {
        self.skip_ws();
        if self.peek() == Some(b) {
            self.pos += 1;
            Ok(())
        } else {
            Err(format!("expected byte {b:?} at pos {}", self.pos))
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.skip_ws();
        match self.peek() {
            Some(b'{') => self.obj(),
            Some(b'[') => self.arr(),
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
        raw.parse::<f64>()
            .map(Json::Num)
            .map_err(|e| format!("bad number {raw}: {e}"))
    }

    fn string(&mut self) -> Result<String, String> {
        self.eat(b'"')?;
        let mut out = String::new();
        loop {
            match self.peek() {
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let esc = self
                        .peek()
                        .ok_or_else(|| "unterminated escape".to_string())?;
                    out.push(match esc {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'u' => {
                            let hex = self
                                .src
                                .get(self.pos + 1..self.pos + 5)
                                .ok_or_else(|| "short \\u escape".to_string())?;
                            let cp = u32::from_str_radix(
                                std::str::from_utf8(hex).map_err(|e| e.to_string())?,
                                16,
                            )
                            .map_err(|e| e.to_string())?;
                            self.pos += 4;
                            char::from_u32(cp).ok_or_else(|| "bad \\u code point".to_string())?
                        }
                        _ => return Err(format!("bad escape \\{esc:?}")),
                    });
                    self.pos += 1;
                }
                Some(b) => {
                    // 单字节即构成 UTF-8 前缀；非 ASCII 按字节透传（源为合法 UTF-8 文本）。
                    out.push(b as char);
                    self.pos += 1;
                }
                None => return Err("unterminated string".to_string()),
            }
        }
    }

    fn arr(&mut self) -> Result<Json, String> {
        self.eat(b'[')?;
        let mut items = Vec::new();
        loop {
            self.skip_ws();
            if self.peek() == Some(b']') {
                self.pos += 1;
                return Ok(Json::Arr(items));
            }
            items.push(self.value()?);
            self.skip_ws();
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
            self.skip_ws();
            if self.peek() == Some(b'}') {
                self.pos += 1;
                return Ok(Json::Obj(kv));
            }
            let key = self.string()?;
            self.eat(b':')?;
            let val = self.value()?;
            kv.push((key, val));
            self.skip_ws();
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
pub fn parse_json(s: &str) -> Result<Json, String> {
    let mut p = Parser::new(s);
    let v = p.value()?;
    p.skip_ws();
    if p.peek().is_some() {
        Err(format!("trailing data at pos {}", p.pos))
    } else {
        Ok(v)
    }
}
