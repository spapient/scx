//! 文本归一化层：单趟数字/十六进制折叠、ANSI 转义剥离、裸 CR 折叠。
//!
//! 全部写入调用方缓冲（零逐行分配）。输出归一化文本仅供匹配/排序/去重使用；
//! 引擎输出仍写原始行，不破坏"保留行逐字原文"承诺。

// 单趟归一化：8+ 位十六进制串折叠为 '#'；其余数字段折叠为 '#'；空白段折叠为单空格；首尾去空。
// run 为复用的十六进制串中转缓冲。
pub(crate) fn norm_into(s: &str, out: &mut String, run: &mut String) {
    let mut pending_space = false;
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c.is_ascii_hexdigit() {
            run.clear();
            run.push(c);
            while it.peek().is_some_and(|&n| n.is_ascii_hexdigit()) {
                run.push(it.next().unwrap());
            }
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            if run.chars().count() >= 8 {
                out.push('#');
            } else {
                let mut ri = run.chars().peekable();
                while let Some(rc) = ri.next() {
                    if rc.is_ascii_digit() {
                        while ri.peek().is_some_and(|&n| n.is_ascii_digit()) {
                            ri.next();
                        }
                        out.push('#');
                    } else {
                        out.push(rc);
                    }
                }
            }
        } else if c.is_ascii_digit() {
            while it.peek().is_some_and(|&n| n.is_ascii_digit()) {
                it.next();
            }
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push('#');
        } else if c.is_whitespace() {
            while it.peek().is_some_and(|&n| n.is_whitespace()) {
                it.next();
            }
            pending_space = true;
        } else {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(c);
        }
    }
}

// 分析用小写缓冲：剥离 ANSI 转义并折叠裸 CR。输出仍保留原始行，因此只影响匹配/排序，
// 不影响"保留行逐字原文"的承诺。进度条常用裸 \r 回写，按空白处理避免污染词面。
pub(crate) fn push_analysis_lower(s: &str, out: &mut String) {
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            match it.peek().copied() {
                Some('[') => {
                    it.next();
                    for esc in it.by_ref() {
                        if ('@'..='~').contains(&esc) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    it.next();
                    while let Some(esc) = it.next() {
                        if esc == '\u{7}' {
                            break;
                        }
                        if esc == '\u{1b}' && it.peek() == Some(&'\\') {
                            it.next();
                            break;
                        }
                    }
                }
                Some(_) => {
                    it.next();
                }
                None => {}
            }
            continue;
        }
        if c == '\r' {
            out.push(' ');
            continue;
        }
        for lc in c.to_lowercase() {
            out.push(lc);
        }
    }
}
