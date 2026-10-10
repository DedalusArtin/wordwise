//! 语种判定与入库闸门（需求：背词表只保留目标语种）。
//!
//! # 为什么单独一个模块
//!
//! 语种污染的根因不是「某一行代码写错了」，而是**入库侧从来就没有语言闸门**：
//! `dict::detect_lang` 这么完整的字符级检测只用在查词和翻译两处**展示侧**，
//! 而 10 条入库路径（下载导入 / 文件导入 / 手动加词 / 批量导入 / AI 生成 /
//! 讲解并入 / 后台增强 / 备份导入 / 示例词库 / 音标回写）没有一条调用它。
//! 界面却宣称「程序按书写系统自动判断」，用户据此信任、随手粘贴日语，
//! 于是日语词被写成了 `('en','嗚呼')`。
//!
//! 而且同一套 Unicode 区间判断此前被抄了至少 5 遍（`dict/mod.rs`、`models.rs`、
//! `dict/importer.rs`、`llm/mod.rs`）—— 每抄一遍就多一处漂移温床。这里收成
//! 唯一一份实现，其余模块改成调它。
//!
//! # 为什么不放在 `dict` 或 `models`
//!
//! `db` 只依赖 `models`，让存储层反向依赖 `dict`（联网词典层）会制造模块环；
//! `models.rs` 已经 1600 行，再塞判定逻辑会继续膨胀。独立模块最干净。

use std::fmt;

/// 一段文本的主要书写系统。
///
/// 只区分「靠字形就能判死」的那些；拉丁字母内部无法再区分英/法/德，
/// 那是语言检测模型的活，不是字符区间能做的事。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Script {
    /// 拉丁字母（含 é / ñ 这类带附加符号的字母）
    Latin,
    /// 日文假名（平假名 + 片假名，含片假名 phonetic ext）
    Kana,
    /// 韩文谚文（含兼容字母区）
    Hangul,
    /// 汉字（CJK 统一表意文字及其扩展 A / 兼容区）
    Han,
    /// 西里尔字母
    Cyrillic,
    /// 希腊字母
    Greek,
    /// 泰文
    Thai,
    /// 阿拉伯字母
    Arabic,
    /// 希伯来字母
    Hebrew,
    /// 数字、标点、符号等判不出语言的文本
    Other,
}

impl fmt::Display for Script {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Script::Latin => "拉丁",
            Script::Kana => "假名",
            Script::Hangul => "谚文",
            Script::Han => "汉字",
            Script::Cyrillic => "西里尔",
            Script::Greek => "希腊",
            Script::Thai => "泰文",
            Script::Arabic => "阿拉伯",
            Script::Hebrew => "希伯来",
            Script::Other => "其它",
        };
        f.write_str(s)
    }
}

/// 非拉丁书写系统的语言代码。
///
/// 这些语言的词条**几乎不会**以纯拉丁字母书写，因此「拉丁词 + 非拉丁语言」
/// 是一个可疑组合（但不足以单独定罪，见 [`lang_compatible`]）。
pub const NON_LATIN_LANGS: [&str; 9] = ["zh", "ja", "ko", "ru", "ar", "hi", "th", "el", "he"];

/// 该语言是否使用非拉丁书写系统。
pub fn is_non_latin_lang(code: &str) -> bool {
    NON_LATIN_LANGS.iter().any(|l| l.eq_ignore_ascii_case(code))
}

/// 判定一段文本的主要书写系统。
///
/// 判定顺序有讲究：**假名优先于汉字**。像「食べる」这种「汉字 + 假名」的
/// 词必须判成日语而不是中文；只有纯汉字（如「勉強」「嗚呼」）才会落到
/// [`Script::Han`]，而那正是 zh / ja 说不清的灰色地带。
pub fn script_of(s: &str) -> Script {
    let mut kana = false;
    let mut hangul = false;
    let mut han = false;
    let mut cyrillic = false;
    let mut greek = false;
    let mut thai = false;
    let mut arabic = false;
    let mut hebrew = false;
    let mut latin = false;

    for c in s.chars() {
        match c as u32 {
            0x3040..=0x30FF | 0x31F0..=0x31FF => kana = true,
            0x1100..=0x11FF | 0xAC00..=0xD7AF | 0x3130..=0x318F => hangul = true,
            0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF => han = true,
            0x0400..=0x04FF => cyrillic = true,
            0x0370..=0x03FF => greek = true,
            0x0E00..=0x0E7F => thai = true,
            0x0600..=0x06FF => arabic = true,
            0x0590..=0x05FF => hebrew = true,
            // 走到这里说明不是上面任何一种；只要还是字母就归拉丁
            // —— 这样 `café` / `naïve` / `piñata` 不会被当成「其它」误杀。
            _ if c.is_alphabetic() => latin = true,
            _ => {}
        }
    }

    if kana {
        return Script::Kana;
    }
    if hangul {
        return Script::Hangul;
    }
    if cyrillic {
        return Script::Cyrillic;
    }
    if greek {
        return Script::Greek;
    }
    if thai {
        return Script::Thai;
    }
    if arabic {
        return Script::Arabic;
    }
    if hebrew {
        return Script::Hebrew;
    }
    if han {
        return Script::Han;
    }
    if latin {
        return Script::Latin;
    }
    Script::Other
}

/// 按字形猜测语言代码；判不出（`Latin` / `Other`）返回 `None`。
///
/// 这是 `dict::detect_lang` 的同一套结论，迁移到本模块后由 `dict` re-export
/// 以兼容既有调用方。纯汉字返回 `"zh"`（而不是 `"ja"`）—— 汉字词在 zh / ja
/// 之间无法靠字形区分，交给 [`resolve_lang`] 结合调用方意图去定。
pub fn detect_lang(word: &str) -> Option<&'static str> {
    match script_of(word) {
        Script::Kana => Some("ja"),
        Script::Hangul => Some("ko"),
        Script::Han => Some("zh"),
        Script::Cyrillic => Some("ru"),
        Script::Greek => Some("el"),
        Script::Thai => Some("th"),
        Script::Arabic => Some("ar"),
        Script::Hebrew => Some("he"),
        Script::Latin | Script::Other => None,
    }
}

/// ★ 入库闸门：这个词条的字形与它声明的语言是否自洽。
///
/// 判定规则（从严到宽）：
/// - **假名 ↔ 只有 ja**：日语词绝不可能属于英语词表，这是本闸门要拦的头号目标；
/// - **谚文 ↔ ko**、**西里尔 ↔ ru**、**希腊 ↔ el**、泰/阿拉伯/希伯来同理；
/// - **汉字 ↔ zh 或 ja**：汉字词在这两个词库里都成立，但在 `en` 词表里一定是噪声；
/// - **拉丁字母一律放行**：包括写进非拉丁语系词库的情况。日语词库里有
///   「April」「アイスクリーム 的罗马字」这类外来语条目，罗马字注音也常是
///   拉丁字母，判严了会误杀真实数据。英/法/德之间字形上分不出来，硬判只会
///   制造新的错判。
///
/// 换句话说：这道闸门保证的是「**书写系统**层面的干净」——日语不会再混进
/// 英语词表；至于英语表里混进一个法语词，那是 [`crate::commands`] 里 AI 自检
/// 那一层的职责。
pub fn lang_compatible(word: &str, lang: &str) -> bool {
    let lang = lang.trim().to_ascii_lowercase();
    match script_of(word) {
        Script::Kana => lang == "ja",
        Script::Hangul => lang == "ko",
        Script::Han => lang == "zh" || lang == "ja",
        Script::Cyrillic => lang == "ru",
        Script::Greek => lang == "el",
        Script::Thai => lang == "th",
        Script::Arabic => lang == "ar",
        Script::Hebrew => lang == "he",
        Script::Latin | Script::Other => true,
    }
}

/// 决议入库语言：**字形强判定优先，其次调用方指定，最后兜底**。
///
/// 与旧的「一律信任调用方」相比，差别只发生在字形能判死的时候
/// （假名 / 谚文 / 西里尔 / 希腊 / 泰 / 阿拉伯 / 希伯来 / 纯汉字），
/// 而这恰好就是污染发生的全部场景。
pub fn resolve_lang(word: &str, requested: Option<&str>, fallback: &str) -> String {
    let req = requested.map(|s| s.trim()).filter(|s| !s.is_empty());
    let detected = script_of(word);
    match detected {
        Script::Kana => "ja",
        Script::Hangul => "ko",
        Script::Cyrillic => "ru",
        Script::Greek => "el",
        Script::Thai => "th",
        Script::Arabic => "ar",
        Script::Hebrew => "he",
        // 汉字：zh / ja 都说得通。调用方明确说是日语词库就判 ja
        // （日语词表里汉字词是常态），其余一律判 zh —— 关键是**绝不判成 en**。
        Script::Han => {
            if let Some(r) = req {
                if r.eq_ignore_ascii_case("ja") {
                    return "ja".to_string();
                }
            }
            "zh"
        }
        // 拉丁词：尊重调用方，拿不到才用兜底（一般是 target_lang）
        Script::Latin | Script::Other => req.unwrap_or(fallback),
    }
    .to_string()
}

/// 一句话说明某个词为什么不该进某个语言的词表（用于日志与界面提示）。
pub fn mismatch_reason(word: &str, lang: &str) -> String {
    format!(
        "「{}」是{}书写，不属于 {} 词表",
        word,
        script_of(word),
        lang
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 假名判日语() {
        assert_eq!(script_of("嗚呼"), Script::Han);
        assert_eq!(script_of("ああ"), Script::Kana);
        assert_eq!(script_of("たべる"), Script::Kana);
        assert_eq!(script_of("アイスクリーム"), Script::Kana);
        // 汉字 + 假名 → 假名优先（日语）
        assert_eq!(script_of("食べる"), Script::Kana);
        assert_eq!(detect_lang("食べる"), Some("ja"));
    }

    #[test]
    fn 汉字与拉丁() {
        assert_eq!(script_of("勉強"), Script::Han);
        assert_eq!(detect_lang("勉強"), Some("zh"));
        assert_eq!(script_of("consider"), Script::Latin);
        assert_eq!(detect_lang("consider"), None);
    }

    #[test]
    fn 带附加符号的拉丁词仍是拉丁() {
        for w in ["café", "naïve", "piñata", "Zoë"] {
            assert_eq!(script_of(w), Script::Latin, "{w} 应判为拉丁");
            assert!(lang_compatible(w, "en"), "{w} 不该被闸门拦下");
        }
    }

    #[test]
    fn 闸门拦住日语混入英语词表() {
        assert!(!lang_compatible("嗚呼", "en"));
        assert!(!lang_compatible("たべる", "en"));
        assert!(!lang_compatible("勉強", "en"));
        assert!(!lang_compatible("こんにちは", "en"));
        // 反过来：日语词表里这些词都合法
        assert!(lang_compatible("たべる", "ja"));
        assert!(lang_compatible("勉強", "ja"));
        // 日语词库里的拉丁外来语不该被误杀
        assert!(lang_compatible("April", "ja"));
        assert!(lang_compatible("コーヒー", "ja"));
    }

    #[test]
    fn 其它书写系统也拦得住() {
        assert!(!lang_compatible("привет", "en"));
        assert!(lang_compatible("привет", "ru"));
        assert!(!lang_compatible("안녕", "en"));
        assert!(lang_compatible("안녕", "ko"));
        assert!(!lang_compatible("αβγ", "en"));
        assert!(lang_compatible("αβγ", "el"));
    }

    #[test]
    fn 决议语言时字形强判定优先() {
        // 调用方说是英语、词是假名 → 必须判 ja
        assert_eq!(resolve_lang("たべる", Some("en"), "en"), "ja");
        // 汉字 + 调用方说 ja → ja
        assert_eq!(resolve_lang("勉強", Some("ja"), "en"), "ja");
        // 汉字 + 调用方说 en → zh（绝不判成 en）
        assert_eq!(resolve_lang("勉強", Some("en"), "en"), "zh");
        // 拉丁词尊重调用方
        assert_eq!(resolve_lang("consider", Some("en"), "ja"), "en");
        // 拉丁词 + 没指定 → 用兜底
        assert_eq!(resolve_lang("consider", None, "en"), "en");
        // 空字符串当没指定
        assert_eq!(resolve_lang("consider", Some(""), "en"), "en");
    }

    #[test]
    fn 非拉丁语言表() {
        assert!(is_non_latin_lang("ja"));
        assert!(is_non_latin_lang("ZH"));
        assert!(!is_non_latin_lang("en"));
        assert!(!is_non_latin_lang("fr"));
    }
}
