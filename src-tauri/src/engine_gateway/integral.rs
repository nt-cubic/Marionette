//! 工具调用参数的整数值去分数（Go `grok_integral.go` 的 1:1 移植）。
//!
//! 模型常把整数值写进工具参数时带一串零分数（`"yield_time_ms":2500.0`、
//! `"session_id":17277.0`）——不止 Grok（#1431：grok-4.6/4.7 经中转 11/11 轮），
//! muse-spark 也把 tool_search 的 limit 写成 8.0（#3152）。Codex 用 u64/usize/
//! i32/i64 读这些字段（exec_command、write_stdin、shell 的 timeout_ms、sleep、
//! tool_search 的 limit、wait），schema 却写着 "number"，serde 对浮点整单拒绝：
//! `failed to parse function arguments: invalid type: floating point \`2500.0\`,
//! expected u64`——一整轮就这样废掉。任何严格类型化参数的客户端都一样。
//!
//! 处理规则（与 Go 完全一致）：应答里工具调用参数中以纯零分数结尾、无指数的
//! 数字丢掉分数——`2500.0` 写成 `2500`。JSON 只有一种数字，2500 对所有客户端
//! 是同一个值；带非零分数（2.5）或指数（2.5e3）的保持原样；字符串内不动。
//! 规则作用在参数的**文本流**上，所以无论参数整块到达还是任意切的分片，
//! 结果一致——分片加起来等于 done 事件说的参数。
//!
//! 批 2 接线范围：翻译路径（responses/messages 方言的工具参数输出——Codex 与
//! Claude Code 是最严格的 serde 消费者）。chat→chat 直通的逐行改写（Go
//! `intTidy`）与流修补一起在批 3 做。

/// 零分数丢弃状态机（Go `wholeNums`）：逐段喂 JSON 文本，返回改写后的文本。
/// 持有的状态只有「一个零分数」（它的点加零）——下一段才知道数字是否在此
/// 结束：结束则分数丢弃，继续（2500.05、2500.0e3）则按原样写出。任何一段
/// 除这样的分数外不等待下一段；流结束时没有任何持有。
#[derive(Default)]
pub(crate) struct WholeNums {
    str_: bool,   // 在字符串里
    esc: bool,    // 字符串里反斜杠后
    num: bool,    // 在数字里
    as_is: bool,  // 本数字的余下部分按原样
    zeros: usize, // 持有的零分数：1 = 只有其点，N = 点 + N-1 个零
}

impl WholeNums {
    pub(crate) fn feed(&mut self, s: &str) -> String {
        let mut b = String::with_capacity(s.len());
        for &c in s.as_bytes() {
            let c = c as char;
            if self.num {
                if self.zeros > 0 && c == '0' {
                    self.zeros += 1;
                    continue;
                }
                if self.zeros > 0 && (c.is_ascii_digit() && c != '0' || c == 'e' || c == 'E') {
                    // 不是纯零分数：持有的按原样写出
                    b.push('.');
                    for _ in 1..self.zeros {
                        b.push('0');
                    }
                    self.zeros = 0;
                    self.as_is = true;
                    b.push(c);
                    continue;
                }
                if self.zeros == 0 && c == '.' && !self.as_is {
                    self.zeros = 1;
                    continue;
                }
                if self.zeros == 0
                    && (c.is_ascii_digit()
                        || c == '.'
                        || c == 'e'
                        || c == 'E'
                        || c == '+'
                        || c == '-')
                {
                    if c == 'e' || c == 'E' || c == '.' {
                        self.as_is = true;
                    }
                    b.push(c);
                    continue;
                }
                // 数字结束：持有的零分数丢弃
                self.num = false;
                self.as_is = false;
                self.zeros = 0;
            }
            if self.str_ {
                if self.esc {
                    self.esc = false;
                } else if c == '\\' {
                    self.esc = true;
                } else if c == '"' {
                    self.str_ = false;
                }
            } else if c == '"' {
                self.str_ = true;
            } else if c == '-' || c.is_ascii_digit() {
                self.num = true;
            }
            b.push(c);
        }
        b
    }
}

/// 一段完整的工具参数文本，丢掉其中每个数字的零分数（Go `wholeArgs`）。
/// 不含点直接原样返回（快路径）。
pub(crate) fn whole_args(s: &str) -> String {
    if !s.contains('.') {
        return s.to_string();
    }
    let mut n = WholeNums::default();
    n.feed(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 分片任意切：各段喂入的总输出等于整块喂入的输出（Go 的流不变量）。
    fn fed_in_pieces(pieces: &[&str]) -> String {
        let mut n = WholeNums::default();
        let mut out = String::new();
        for p in pieces {
            out.push_str(&n.feed(p));
        }
        out
    }

    #[test]
    fn zero_fraction_is_dropped() {
        assert_eq!(
            whole_args(r#"{"yield_time_ms":2500.0}"#),
            r#"{"yield_time_ms":2500}"#
        );
        assert_eq!(
            whole_args(r#"{"session_id":17277.0}"#),
            r#"{"session_id":17277}"#
        );
        assert_eq!(whole_args(r#"{"limit":8.0}"#), r#"{"limit":8}"#);
        assert_eq!(whole_args(r#"{"a":2500.00}"#), r#"{"a":2500}"#);
        assert_eq!(whole_args(r#"{"a":-3.0}"#), r#"{"a":-3}"#);
    }

    #[test]
    fn nonzero_fraction_and_exponent_stay() {
        assert_eq!(whole_args(r#"{"x":2.5}"#), r#"{"x":2.5}"#);
        assert_eq!(whole_args(r#"{"x":2500.0e3}"#), r#"{"x":2500.0e3}"#);
        assert_eq!(whole_args(r#"{"x":2500.05}"#), r#"{"x":2500.05}"#);
        assert_eq!(whole_args(r#"{"x":1e3}"#), r#"{"x":1e3}"#);
    }

    #[test]
    fn strings_are_untouched() {
        assert_eq!(whole_args(r#"{"s":"2500.0"}"#), r#"{"s":"2500.0"}"#);
        assert_eq!(whole_args(r#"{"s":"a\"2500.0"}"#), r#"{"s":"a\"2500.0"}"#);
        assert_eq!(whole_args(r#"{"name":"v1.0"}"#), r#"{"name":"v1.0"}"#);
    }

    #[test]
    fn pieces_cut_anywhere_give_the_whole_result() {
        let s = r#"{"a":2500.0,"b":"x.0","c":8.0}"#;
        let whole = whole_args(s);
        let mut cuts: Vec<Vec<&str>> = Vec::new();
        for cut in [3usize, 8, 11, 14, 19, 24, 27] {
            cuts.push(vec![&s[..cut], &s[cut..]]);
        }
        cuts.push(vec![&s[..1], &s[1..5], &s[5..12], &s[12..]]);
        for pieces in cuts {
            assert_eq!(fed_in_pieces(&pieces), whole, "pieces {:?} differ", pieces);
        }
    }

    #[test]
    fn held_zeros_release_when_number_continues() {
        // 25.0 之后来 'e'：分数按原样吐回，数字转 as-is
        let mut n = WholeNums::default();
        assert_eq!(n.feed("25.0"), "25");
        assert_eq!(n.feed("e3"), ".0e3");
        // 数字结束（}）：持有的分数丢弃
        let mut n = WholeNums::default();
        assert_eq!(n.feed("25.0"), "25");
        assert_eq!(n.feed("}"), "}");
    }

    #[test]
    fn dotless_fast_path_is_identity() {
        assert_eq!(whole_args(r#"{"a":2500}"#), r#"{"a":2500}"#);
    }
}
