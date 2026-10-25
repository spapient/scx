//! 手写有限正则子集（`--force "re:..."`，零第三方 crate）。
//!
//! 支持的语法（有意最小，规避完整 regex 引擎的复杂度与回溯风险）：
//! - `|` 顶层交替（`a|b|c`）
//! - 字符类 `[...]`：`^` 首字符取反，`a-z` 范围，`]` 首字符为字面
//! - 量词 `*` `+` `?`（作用于单个原子）
//! - `\` 转义（`\|` `\*` `\+` `\?` `\[` `\]` `\\`）
//!
//! 不支持：`.` 通配、`()` 分组、锚点、`{}`、`\d` 等简写。
//!
//! 实现：Thompson NFA（fragment + out-list patch 编译），匹配为单趟
//! ε-closure 状态集合推进，耗时与（行字符数 × 状态数）线性相关，
//! 无指数回溯。解析错误返回带位置的描述并列出支持子集。

#![forbid(unsafe_code)]

/// 正则 AST（无分组，量词作用单原子）。
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// `a|b|c` 交替（≥1 支）。
    Alt(Vec<Node>),
    /// 序列（≥1 项）。
    Seq(Vec<Node>),
    /// `x*`（含空）。
    Star(Box<Node>),
    /// `x+`（至少一次）。
    Plus(Box<Node>),
    /// `x?`（零或一次）。
    Opt(Box<Node>),
    /// 字符类；`neg` 为 `[^...]` 取反。
    Class {
        ranges: Vec<(char, char)>,
        neg: bool,
    },
    /// 字面字符。
    Char(char),
}

/// 支持子集提示（报错时附上）。
const SUBSET: &str = "subset: '|' alt | [class] | * + ? quant | \\escape";

struct Parser<'a> {
    chars: &'a [char],
    pos: usize,
}

/// 解析 `pat` 为 AST；失败返回带位置的错误与支持子集提示。
pub fn parse_regex(pat: &str) -> Result<Node, String> {
    let chars: Vec<char> = pat.chars().collect();
    if chars.is_empty() {
        return Err("re: pattern must not be empty".to_string());
    }
    let mut p = Parser {
        chars: &chars,
        pos: 0,
    };
    let node = p.parse_alt()?;
    if p.pos != p.chars.len() {
        return Err(format!(
            "re: unexpected token {:?} at position {}; {SUBSET}",
            p.chars[p.pos], p.pos
        ));
    }
    Ok(node)
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
        Some(c)
    }

    fn parse_alt(&mut self) -> Result<Node, String> {
        let mut arms = vec![self.parse_seq()?];
        while self.peek() == Some('|') {
            self.bump();
            arms.push(self.parse_seq()?);
        }
        if arms.len() == 1 {
            Ok(arms.pop().unwrap())
        } else {
            Ok(Node::Alt(arms))
        }
    }

    fn parse_seq(&mut self) -> Result<Node, String> {
        let mut items = Vec::new();
        loop {
            match self.peek() {
                None | Some('|') | Some(']') => break,
                Some('*') | Some('+') | Some('?') => {
                    return Err(format!(
                        "re: quantifier {:?} has no operand; {SUBSET}",
                        self.peek().unwrap()
                    ));
                }
                Some(_) => items.push(self.parse_atom()?),
            }
        }
        if items.is_empty() {
            return Err(format!("re: empty alternative; {SUBSET}"));
        }
        if items.len() == 1 {
            Ok(items.pop().unwrap())
        } else {
            Ok(Node::Seq(items))
        }
    }

    fn parse_atom(&mut self) -> Result<Node, String> {
        let base = match self.bump().unwrap() {
            '\\' => {
                let esc = self
                    .bump()
                    .ok_or_else(|| "re: dangling \\ escape".to_string())?;
                Node::Char(esc)
            }
            '[' => self.parse_class()?,
            c => Node::Char(c),
        };
        match self.peek() {
            Some('*') => {
                self.bump();
                Ok(Node::Star(Box::new(base)))
            }
            Some('+') => {
                self.bump();
                Ok(Node::Plus(Box::new(base)))
            }
            Some('?') => {
                self.bump();
                Ok(Node::Opt(Box::new(base)))
            }
            _ => Ok(base),
        }
    }

    fn parse_class(&mut self) -> Result<Node, String> {
        let neg = self.peek() == Some('^');
        if neg {
            self.bump();
        }
        let mut ranges: Vec<(char, char)> = Vec::new();
        let mut first = true;
        loop {
            match self.bump() {
                None => {
                    return Err(format!("re: unterminated character class; {SUBSET}"));
                }
                Some(']') if !first => break,
                Some(c) => {
                    first = false;
                    if c == ']' {
                        // 首字符 `]` 为字面。
                        ranges.push((']', ']'));
                        continue;
                    }
                    if self.peek() == Some('-') && self.chars.get(self.pos + 1).is_some() {
                        let hi = self.chars[self.pos + 1];
                        if hi == ']' {
                            // `-` 后紧跟 `]`：`-` 按字面处理。
                            ranges.push((c, c));
                            ranges.push(('-', '-'));
                            self.bump();
                            continue;
                        }
                        self.bump();
                        self.bump();
                        if hi < c {
                            return Err(format!("re: inverted range [{c}-{hi}] in class"));
                        }
                        ranges.push((c, hi));
                    } else {
                        ranges.push((c, c));
                    }
                }
            }
        }
        Ok(Node::Class { ranges, neg })
    }
}

// ---- Thompson NFA（状态图 + out-list patch） ----

/// 转移槽（后继）；OPEN 表示待 patch。
const OPEN: usize = usize::MAX;

#[derive(Clone, Debug)]
enum Kind {
    /// 消费一个字符后走 `a`。
    Char(char),
    /// 消费类内字符后走 `a`；`neg` 取反。
    Class {
        ranges: Vec<(char, char)>,
        neg: bool,
    },
    /// ε 分叉到 `a` / `b`。
    Split,
    /// 接受态。
    Accept,
}

#[derive(Clone, Debug)]
struct St {
    kind: Kind,
    a: usize,
    b: usize,
}

/// 编译片段：`start` 入口；`out` 为待 patch 的 (状态, 槽)。
struct Frag {
    start: usize,
    out: Vec<(usize, bool)>, // bool: true=a 槽, false=b 槽
}

/// 把 `outs` 全部 patch 到 `target`。
fn patch(states: &mut [St], outs: &[(usize, bool)], target: usize) {
    for &(id, is_a) in outs {
        if is_a {
            states[id].a = target;
        } else {
            states[id].b = target;
        }
    }
}

fn leaf(states: &mut Vec<St>, kind: Kind) -> Frag {
    let id = states.len();
    states.push(St {
        kind,
        a: OPEN,
        b: OPEN,
    });
    Frag {
        start: id,
        out: vec![(id, true)],
    }
}

fn compile(node: &Node, states: &mut Vec<St>) -> Frag {
    match node {
        Node::Char(c) => leaf(states, Kind::Char(*c)),
        Node::Class { ranges, neg } => leaf(
            states,
            Kind::Class {
                ranges: ranges.clone(),
                neg: *neg,
            },
        ),
        Node::Seq(items) => {
            let mut frag = compile(&items[0], states);
            for item in &items[1..] {
                let next = compile(item, states);
                patch(states, &frag.out, next.start);
                frag.out = next.out;
            }
            frag
        }
        Node::Alt(arms) => {
            let mut frag = compile(&arms[0], states);
            for arm in &arms[1..] {
                let next = compile(arm, states);
                let id = states.len();
                states.push(St {
                    kind: Kind::Split,
                    a: frag.start,
                    b: next.start,
                });
                frag.start = id;
                frag.out.extend(next.out);
            }
            frag
        }
        Node::Star(inner) => {
            let inner = compile(inner, states);
            let id = states.len();
            states.push(St {
                kind: Kind::Split,
                a: inner.start,
                b: OPEN,
            });
            patch(states, &inner.out, id); // 循环回 id
            Frag {
                start: id,
                out: vec![(id, false)],
            }
        }
        Node::Plus(inner) => {
            let inner = compile(inner, states);
            let id = states.len();
            states.push(St {
                kind: Kind::Split,
                a: inner.start,
                b: OPEN,
            });
            patch(states, &inner.out, id); // 循环
            Frag {
                start: inner.start,
                out: vec![(id, false)],
            }
        }
        Node::Opt(inner) => {
            let inner = compile(inner, states);
            let id = states.len();
            states.push(St {
                kind: Kind::Split,
                a: inner.start,
                b: OPEN,
            });
            Frag {
                start: id,
                out: vec![(id, false)],
            }
            .extend_outs(inner.out)
        }
    }
}

impl Frag {
    fn extend_outs(mut self, mut more: Vec<(usize, bool)>) -> Frag {
        self.out.append(&mut more);
        self
    }
}

/// 编译 AST 为 NFA（out 收尾 patch 到 Accept）。
pub fn compile_nfa(node: &Node) -> Compiled {
    let mut states: Vec<St> = Vec::new();
    let frag = compile(node, &mut states);
    let accept = states.len();
    states.push(St {
        kind: Kind::Accept,
        a: OPEN,
        b: OPEN,
    });
    patch(&mut states, &frag.out, accept);
    Compiled {
        states,
        start: frag.start,
        accept,
    }
}

/// 已编译正则（状态表 + 入口 + 接受态）。
#[derive(Debug)]
pub struct Compiled {
    states: Vec<St>,
    start: usize,
    accept: usize,
}

/// ε-closure：从一组起始状态出发，经 Split 全部可达的状态集。
/// `seen` / `stack` / `out` 为调用方持有的复用缓冲；返回后 `seen` 恢复全
/// false，`out` 为闭包集合（元素顺序不影响判定正确性）。
fn closure_into(
    states: &[St],
    starts: &[usize],
    seen: &mut [bool],
    stack: &mut Vec<usize>,
    out: &mut Vec<usize>,
) {
    out.clear();
    stack.clear();
    stack.extend_from_slice(starts);
    while let Some(s) = stack.pop() {
        if seen[s] {
            continue;
        }
        seen[s] = true;
        out.push(s);
        if let Kind::Split = states[s].kind {
            stack.push(states[s].a);
            stack.push(states[s].b);
        }
    }
    for &s in out.iter() {
        seen[s] = false;
    }
}

impl Compiled {
    /// 行内任意位置命中（非锚定搜索：每字符后并入入口 closure，
    /// 等效于从每个位置重新尝试；含空匹配，与标准搜索语义一致）。
    pub fn matches(&self, line: &str) -> bool {
        if self.states.is_empty() {
            return false;
        }
        let mut seen = vec![false; self.states.len()];
        let mut stack: Vec<usize> = Vec::new();
        let mut closed: Vec<usize> = Vec::new();
        let mut cur: Vec<usize> = Vec::new();
        closure_into(
            &self.states,
            &[self.start],
            &mut seen,
            &mut stack,
            &mut closed,
        );
        // 起始 ε-closure 即含 accept：模式可空，空行即命中。
        if closed.contains(&self.accept) {
            return true;
        }
        for ch in line.chars() {
            cur.clear();
            for &s in &closed {
                match &self.states[s].kind {
                    Kind::Char(c) if *c == ch => cur.push(self.states[s].a),
                    Kind::Class { ranges, neg } => {
                        let hit = ranges.iter().any(|(lo, hi)| *lo <= ch && ch <= *hi);
                        if hit != *neg {
                            cur.push(self.states[s].a);
                        }
                    }
                    _ => {}
                }
            }
            cur.push(self.start); // 非锚定：新位置重启
            closure_into(&self.states, &cur, &mut seen, &mut stack, &mut closed);
            // 正则搜索语义：accept 可达即存在匹配，立即命中。
            if closed.contains(&self.accept) {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pat: &str, line: &str) -> bool {
        let node = parse_regex(pat).expect("pattern parses");
        compile_nfa(&node).matches(line)
    }

    #[test]
    fn literal_alt_class_and_quant() {
        assert!(m("err|fatal", "line has err here"));
        assert!(m("err|fatal", "fatal only"));
        assert!(!m("err|fatal", "nothing relevant"));
        assert!(m("error\\[[0-9]+\\]", "err: error[42] raised"));
        assert!(!m("error\\[[0-9]+\\]", "err: error[aa] raised"));
        assert!(m("conn[0-9]+", "conn9f2a broken"));
        assert!(!m("conn[0-9]+", "connX broken"));
        assert!(m("a+", "baaaad"));
        assert!(m("colou?r", "color"));
        assert!(m("colou?r", "colour"));
    }

    #[test]
    fn negated_class_and_escape() {
        assert!(m("[^0-9]+", "abc"));
        // 非锚定搜索：abc123 含非数字前缀 abc，属命中。
        assert!(m("[^0-9]+", "abc123"));
        assert!(m("a\\*b", "a*b literal"));
        assert!(!m("a\\*b", "axb"));
    }

    #[test]
    fn char_class_ranges_and_literal_brackets() {
        assert!(m("[a-cx-z]", "y"));
        assert!(!m("[a-cx-z]", "d"));
        assert!(m("[]a]+", "]]]"));
        assert!(m("[-x]+", "---"));
    }

    #[test]
    fn invalid_patterns_rejected_with_subset_hint() {
        let e = parse_regex("a||b").unwrap_err();
        assert!(e.contains("empty"), "{e}");
        let e2 = parse_regex("a|").unwrap_err();
        assert!(e2.contains("empty"), "{e2}");
        let e3 = parse_regex("[abc").unwrap_err();
        assert!(e3.contains("unterminated"), "{e3}");
        let e4 = parse_regex("a**").unwrap_err();
        assert!(e4.contains("no operand"), "{e4}");
        let e5 = parse_regex("").unwrap_err();
        assert!(e5.contains("must not be empty"), "{e5}");
        let e6 = parse_regex("[z-a]").unwrap_err();
        assert!(e6.contains("inverted range"), "{e6}");
    }

    #[test]
    fn empty_match_semantics_and_literal_parenthesis() {
        // 标准搜索语义：可空模式匹配空行；不可空模式不匹配空行。
        assert!(m("x?", ""));
        assert!(!m("x", ""));
        assert!(m("x?y", "y"));
        // `(` 不在元字符集，按字面处理（无分组支持）。
        assert!(m("a(b", "a(b"));
    }

    #[test]
    fn star_plus_and_adjacent_classes() {
        assert!(m("a*b", "aaaaaaab"));
        assert!(m("a*b", "b"));
        assert!(m("x+", "xxx"));
        assert!(m("[a-z]+[0-9]+", "abc123"));
    }

    #[test]
    fn alternation_and_empty_start() {
        assert!(m("abc|def", "xxdefxx"));
        assert!(!m("abc|def", "xxabexx"));
        assert!(m("ab?c", "ac"));
        assert!(m("ab?c", "abc"));
    }
}
