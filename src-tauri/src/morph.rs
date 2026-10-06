//! 词形变化与派生词的**本地推断**（需求 5）。
//!
//! 用户反馈「相关词太少，缺少 happiness 这种派生变形」。关键观察是：
//! 这些词**大多已经在用户自己的词库里**（考研/四六级词表里 happy 与
//! happiness 是两条独立记录），只是查 happy 时没人把它们捞出来。
//!
//! 所以这里做两件事，且**都不联网、不调模型**：
//!   1. 由词形规则推出「可能存在的同族词」候选
//!      （happy → happiness / happier / happily / happiest…）；
//!   2. 交给调用方去词库里确认哪些**真的存在**。
//!
//! 只展示真实存在的词，是本设计的关键。凭规则直接编出 happiness 很容易，
//! 但用户点下去发现查不到，比不显示更糟 —— 那是在教用户「这个按钮是坏的」。
//!
//! 规则是英语中心的（其它语言拿到的候选多半不存在，自然被过滤掉），
//! 但它在中文/日文词条上也不会出错：候选只是字符串，确认环节会全部否掉。

/// 常见派生后缀。
///
/// 顺序按长度降序，只为让「去掉后缀」时先试 `ness` 再试 `s` —— 虽然
/// 实现上会遍历全部后缀（不存在抢匹配问题），但这个顺序让人读起来是
/// 「从最具体的开始」，也与截断上限的取舍一致。
const SUFFIXES: &[&str] = &[
    "ization", "isation", "ability", "ibility", "fulness", "lessness", "ness", "ment", "tion",
    "sion", "ance", "ence", "able", "ible", "ally", "ity", "ive", "ous", "ful", "less", "ly",
    "est", "er", "ing", "ed", "al", "es", "y", "s",
];

/// 候选上限。
///
/// 生成阶段会产出上百个字符串（每个后缀 × 三种词干变体）。这个上限是**安全网**
/// 而不是主要节流手段 —— 真正的上限在展示层（8 条，见 `cmd_word_family`）。
///
/// ★ 教训：这个值一开始设成 48，结果「去后缀」那一半候选被整体截断，
/// 于是 `happiness → happy` 直接推不出来（用户反馈的正是这个方向）。
/// 所以现在**先放「去后缀」的结果**（它更容易命中真词），再加后缀变体，
/// 并把上限放宽到足够容纳两个方向的全部产出。
const MAX_CANDIDATES: usize = 128;

fn is_consonant(c: char) -> bool {
    c.is_ascii_alphabetic() && !matches!(c.to_ascii_lowercase(), 'a' | 'e' | 'i' | 'o' | 'u')
}

/// `y → i` 变体：`happy → happi`。
///
/// 只有「辅音 + y 结尾」才变（`play → plai` 是错的，元音后的 y 不变），
/// 所以这里要先看一眼前一个字符。
fn y_to_i(w: &str) -> Option<String> {
    let mut it = w.chars().rev();
    if !it.next()?.eq_ignore_ascii_case(&'y') {
        return None;
    }
    if !is_consonant(it.next()?) {
        return None;
    }
    // 'y' 是 ASCII，末尾按字节切一定落在字符边界上
    let mut s = w[..w.len() - 1].to_string();
    s.push('i');
    Some(s)
}

/// 去尾 `e` 变体：`make → mak`（用于 `making` / `maker`）。
fn drop_e(w: &str) -> Option<String> {
    let s = w.strip_suffix(['e', 'E'])?;
    (s.len() >= 3).then(|| s.to_string())
}

/// 由词形推出「可能存在的同族派生词」候选（不含原词本身）。
///
/// 结果是**候选**，不代表词库里真有这些词 —— 调用方必须去确认。
///
/// 两个方向都会生成，而且会生成一些英语里不存在的组合（`happy` 会推出
/// `happyly`）。这是**刻意**的：词形规则无法知道某条派生是否真的成立
/// （`happy → happily` 要 y→i，但 `study → studying` 不变，两者字形条件一样），
/// 与其在规则层猜，不如全量生成、交给词库确认。多几个字符串的成本是
/// 一次本地 SQL，而猜错的成本是用户点到一个查不到的词。
pub fn derivative_candidates(word: &str) -> Vec<String> {
    let w = word.trim().to_lowercase();
    let mut out: Vec<String> = Vec::new();
    // 只处理纯拉丁字母：汉字/假名/西里尔文套英语后缀只会产生噪声
    if w.is_empty() || !w.chars().all(|c| c.is_ascii_alphabetic()) {
        return out;
    }

    let add = |out: &mut Vec<String>, s: String| {
        if s.len() >= 3 && s != w && !out.iter().any(|x| x == &s) {
            out.push(s);
        }
    };

    // 1) 去后缀：happiness → happy；happily → happy；making → make
    //
    //    放在前面是有原因的：这个方向更容易命中真词（派生词 → 词根），
    //    万一真的触发上限截断，先保住它。
    for suf in SUFFIXES {
        let Some(stem) = w.strip_suffix(suf) else {
            continue;
        };
        if stem.len() < 3 {
            continue;
        }
        add(&mut out, stem.to_string());
        // 还原被 y→i 吃掉的 y
        if let Some(rest) = stem.strip_suffix('i') {
            add(&mut out, format!("{}y", rest));
        }
        // 还原被去掉的 e
        add(&mut out, format!("{}e", stem));
    }

    // 2) 加后缀：happy → happiness / happier / happily
    let y2i = y_to_i(&w);
    let de = drop_e(&w);
    for suf in SUFFIXES {
        add(&mut out, format!("{}{}", w, suf));
        if let Some(b) = &y2i {
            add(&mut out, format!("{}{}", b, suf));
        }
        if let Some(b) = &de {
            add(&mut out, format!("{}{}", b, suf));
        }
    }

    out.truncate(MAX_CANDIDATES);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has(word: &str, want: &str) -> bool {
        derivative_candidates(word).iter().any(|c| c == want)
    }

    #[test]
    fn key_case_happiness() {
        // 用户举的例子：查 happy 必须能想到 happiness
        assert!(has("happy", "happiness"), "happy 应推出 happiness");
        assert!(has("happy", "happier"));
        assert!(has("happy", "happiest"));
        // 副词要注意 y→i：happily
        assert!(has("happy", "happily"), "happy 应推出 happily");
    }

    #[test]
    fn reverse_direction_finds_the_base() {
        // 反查：查 happiness 要能找回 happy（用户也可能从派生词点进来）
        //
        // ★ 这条测试当初真的救过一次：候选上限设小了之后，「去后缀」整段被
        //   截断，happiness 就再也推不回 happy —— 而它恰恰是最该工作的方向。
        assert!(has("happiness", "happy"), "happiness 应推回 happy");
        assert!(has("happily", "happy"), "happily 应推回 happy");
        assert!(has("making", "make"), "making 应推回 make");
        assert!(has("maker", "make"), "maker 应推回 make");
    }

    #[test]
    fn correct_forms_are_never_missing() {
        // 元音后的 y 不变这条规则，体现在「正确形式必须在候选里」：
        // 至于同时生成的 plaied 这类废候选，由词库确认环节过滤掉。
        assert!(has("play", "player"));
        assert!(has("play", "playing"));
        assert!(has("study", "studying"));
        assert!(has("study", "studies"));
        assert!(has("develop", "development"));
        assert!(has("develop", "developed"));
        assert!(has("happy", "happiness"));
    }

    #[test]
    fn never_returns_the_word_itself() {
        for w in ["happy", "make", "play", "study", "development"] {
            assert!(
                !derivative_candidates(w).iter().any(|c| c == w),
                "候选里混进了原词本身：{}",
                w
            );
        }
    }

    #[test]
    fn non_latin_input_yields_nothing() {
        // 中文 / 日文 / 假名套英语后缀只会产生噪声，直接不生成候选
        assert!(derivative_candidates("快乐").is_empty());
        assert!(derivative_candidates("たのしい").is_empty());
        assert!(derivative_candidates("").is_empty());
        assert!(derivative_candidates("   ").is_empty());
        // 带空格或连字符的整句同样不处理
        assert!(derivative_candidates("happy day").is_empty());
    }

    #[test]
    fn candidates_are_unique_and_bounded() {
        let c = derivative_candidates("happy");
        let mut sorted = c.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), c.len(), "候选里有重复项");
        assert!(c.len() <= MAX_CANDIDATES, "候选数量必须被截断");
        // 太短的垃圾词干（de+后缀）不该出现
        assert!(c.iter().all(|s| s.len() >= 3));
    }

    #[test]
    fn both_directions_fit_under_the_cap() {
        // 上限必须宽到能同时装下两个方向 —— 这是上面那条回归的护栏
        for w in ["happy", "happiness", "development", "internationalization"] {
            let c = derivative_candidates(w);
            assert!(c.len() < MAX_CANDIDATES, "{} 的候选数顶到了上限，方向可能被截断", w);
        }
    }

    #[test]
    fn uppercase_input_is_normalized() {
        // 用户常直接粘大写词；候选必须小写，否则和词库里的键对不上
        assert!(has("HAPPY", "happiness"));
    }
}
