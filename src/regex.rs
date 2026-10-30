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
pub fn parse(pat: &str) -> Result<Node, String> {
    let chars: Vec<char> = pat.chars().collect();
    if chars.is_empty() {
        return Err("re: pattern must not be empty".to_string());
    }
    let mut p = Parser {
        chars: &chars,
        pos: 0,
    };
    let node = p.alt()?;
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

    fn alt(&mut self) -> Result<Node, String> {
        let mut arms = vec![self.seq()?];
        while self.peek() == Some('|') {
            self.bump();
            arms.push(self.seq()?);
        }
        if arms.len() == 1 {
            Ok(arms.pop().expect("arms.len()==1 分支 pop 必有值"))
        } else {
            Ok(Node::Alt(arms))
        }
    }

    fn seq(&mut self) -> Result<Node, String> {
        let mut items = Vec::new();
        loop {
            match self.peek() {
                None | Some('|') | Some(']') => break,
                Some(q @ ('*' | '+' | '?')) => {
                    return Err(format!("re: quantifier {q:?} has no operand; {SUBSET}"));
                }
                Some(_) => items.push(self.atom()?),
            }
        }
        if items.is_empty() {
            return Err(format!("re: empty alternative; {SUBSET}"));
        }
        if items.len() == 1 {
            Ok(items.pop().expect("items 单元素分支 pop 必有值"))
        } else {
            Ok(Node::Seq(items))
        }
    }

    fn atom(&mut self) -> Result<Node, String> {
        let base = self.base()?;
        self.quant(base)
    }

    fn base(&mut self) -> Result<Node, String> {
        match self
            .bump()
            .ok_or_else(|| "re: unexpected end of pattern".to_string())?
        {
            '\\' => {
                let esc = self
                    .bump()
                    .ok_or_else(|| "re: dangling \\ escape".to_string())?;
                Ok(Node::Char(esc))
            }
            '[' => self.class(),
            c => Ok(Node::Char(c)),
        }
    }

    fn quant(&mut self, base: Node) -> Result<Node, String> {
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

    fn class(&mut self) -> Result<Node, String> {
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
                    self.item(c, &mut ranges)?;
                }
            }
        }
        Ok(Node::Class { ranges, neg })
    }

    /// 消费字符类的一个成员（字面 / 范围），失败带具体错误。
    fn item(&mut self, c: char, ranges: &mut Vec<(char, char)>) -> Result<(), String> {
        if c == ']' {
            // 首字符 `]` 为字面。
            ranges.push((']', ']'));
            return Ok(());
        }
        if self.peek() == Some('-') && self.chars.get(self.pos + 1).is_some() {
            let hi = self.chars[self.pos + 1];
            if hi == ']' {
                // `-` 后紧跟 `]`：`-` 按字面处理。
                ranges.push((c, c));
                ranges.push(('-', '-'));
                self.bump();
                return Ok(());
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
        Ok(())
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

fn emit(node: &Node, states: &mut Vec<St>) -> Frag {
    match node {
        Node::Char(c) => leaf(states, Kind::Char(*c)),
        Node::Class { ranges, neg } => leaf(
            states,
            Kind::Class {
                ranges: ranges.clone(),
                neg: *neg,
            },
        ),
        Node::Seq(items) => sequence(items, states),
        Node::Alt(arms) => choice(arms, states),
        Node::Star(inner) => repeat(inner, states, false),
        Node::Plus(inner) => repeat(inner, states, true),
        Node::Opt(inner) => option(inner, states),
    }
}

fn sequence(items: &[Node], states: &mut Vec<St>) -> Frag {
    let mut frag = emit(&items[0], states);
    for item in &items[1..] {
        let next = emit(item, states);
        patch(states, &frag.out, next.start);
        frag.out = next.out;
    }
    frag
}

fn choice(arms: &[Node], states: &mut Vec<St>) -> Frag {
    let mut frag = emit(&arms[0], states);
    for arm in &arms[1..] {
        let next = emit(arm, states);
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

/// Star/Plus 的共用编译：`keep_start` 区分入口（Star 从 split 入，Plus 从 inner 入）。
fn repeat(inner: &Node, states: &mut Vec<St>, keep_start: bool) -> Frag {
    let inner = emit(inner, states);
    let id = states.len();
    states.push(St {
        kind: Kind::Split,
        a: inner.start,
        b: OPEN,
    });
    patch(states, &inner.out, id);
    if keep_start {
        Frag {
            start: inner.start,
            out: vec![(id, false)],
        }
    } else {
        Frag {
            start: id,
            out: vec![(id, false)],
        }
    }
}

fn option(inner: &Node, states: &mut Vec<St>) -> Frag {
    let inner = emit(inner, states);
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
    .merge(inner.out)
}

impl Frag {
    fn merge(mut self, mut more: Vec<(usize, bool)>) -> Frag {
        self.out.append(&mut more);
        self
    }
}

/// 编译 AST 为 NFA（out 收尾 patch 到 Accept）。
pub fn compile(node: &Node) -> Compiled {
    let mut states: Vec<St> = Vec::new();
    let frag = emit(node, &mut states);
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
fn closure(
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
        closure(
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
                if let Some(next) = self.step(s, ch) {
                    cur.push(next);
                }
            }
            cur.push(self.start); // 非锚定：新位置重启
            closure(&self.states, &cur, &mut seen, &mut stack, &mut closed);
            // 正则搜索语义：accept 可达即存在匹配，立即命中。
            if closed.contains(&self.accept) {
                return true;
            }
        }
        false
    }

    /// 单状态单字符转移：Char 精确命中 / Class 命中取反语义；其余 ε 不转移。
    fn step(&self, s: usize, ch: char) -> Option<usize> {
        match &self.states[s].kind {
            Kind::Char(c) if *c == ch => Some(self.states[s].a),
            Kind::Class { ranges, neg } => {
                let hit = ranges.iter().any(|(lo, hi)| *lo <= ch && ch <= *hi);
                (hit != *neg).then_some(self.states[s].a)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pat: &str, line: &str) -> bool {
        let node = parse(pat).expect("pattern parses");
        compile(&node).matches(line)
    }

    /// 确定性模糊：`re:` 模式来自命令行/策略文件，任意字符序列都不得 panic；
    /// 解析成功的模式，编译与匹配任意文本也不得 panic。
    #[test]
    fn parse_and_match_never_panic_on_arbitrary_patterns() {
        let pool: Vec<char> = "abc|[]*+?\\-.^$(){}0123456789 \t中文".chars().collect();
        let mut state = 0x5eed_1234u32;
        let mut next = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            state
        };
        for _ in 0..1500 {
            let n = next() as usize % 12;
            let mut pat = String::new();
            for _ in 0..n {
                pat.push(pool[next() as usize % pool.len()]);
            }
            if let Ok(node) = parse(&pat) {
                let re = compile(&node);
                let _ = re.matches("");
                let _ = re.matches("abc 123 中文");
                let _ = re.matches(&pat);
            }
        }
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
        let e = parse("a||b").unwrap_err();
        assert!(e.contains("empty"), "{e}");
        let e2 = parse("a|").unwrap_err();
        assert!(e2.contains("empty"), "{e2}");
        let e3 = parse("[abc").unwrap_err();
        assert!(e3.contains("unterminated"), "{e3}");
        let e4 = parse("a**").unwrap_err();
        assert!(e4.contains("no operand"), "{e4}");
        let e5 = parse("").unwrap_err();
        assert!(e5.contains("must not be empty"), "{e5}");
        let e6 = parse("[z-a]").unwrap_err();
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
