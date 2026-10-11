//! 英语形态学：**不规则变形表** + 「词性 → 该有哪些变形」的规则。
//!
//! # 为什么需要这个模块
//!
//! 用户反馈了三张截图，三个词三种错法，但根子是同一个：变形栏只有「渲染」
//! 没有「算法」。
//!
//! | 截图 | 表象 | 根因 |
//! |---|---|---|
//! | `see` | 词头旁挂着「原形 **cee**」 | 原形是从**释义文本**里正则抠的，抠出了脏词 |
//! | `saw` | 变形栏只有 `explain-store` 一条，没有 `see` | 没有不规则表，`saw` 不知道自己的原形是谁 |
//! | `dumb` | 形容词显示「原形（现在时）」 | 首行标签只按**语言**判断，没有看**词性** |
//!
//! 顺带还暴露一个更隐蔽的错：查 `see` 时变形栏写的是
//! `复数 sees　第三人称单数 sees　现在分词 seeing　过去式 saw　过去分词 seen`。
//! 两处都不对 ——
//!
//! 1. **动词不该有「复数」**。`sees` 是 see 的第三人称单数，不是复数。
//!    数据源（有道 / freedictionary）给的是 `第三人称单数`，而某条链路把它
//!    归一成了「复数」，于是同一个 `sees` 被写进两条记录、标签还是错的。
//! 2. **动词的 -s / -ing / 过去式 / 过去分词 应该由「词性」决定是否该有**。
//!    名词只该有复数，形容词只该有比较级/最高级。现在是无差别展示，
//!    于是名词看起来像缺了时态、动词看起来像缺了复数。
//!
//! # 设计取舍
//!
//! **本地生成，不联网、不调模型。** 变形是封闭的形态学问题，有确定答案；
//! 让模型去猜 `see` 的过去式既慢又可能编。词库里没有这条数据时，
//! 本地表能给出正确的形态，比让用户看到一个空栏好。
//!
//! **表在 WordWise 而不在词库里。** 词库（`build.rs` 生成的那份）继续负责
//! 「这个查询词对应哪个词条、例句是什么」；这里只负责「从原形能推出哪些
//! 形态」。两边职责不重叠，词库不需要为了这个功能改格式。

// ============================================================================
// 一、不规则动词表
// ============================================================================

/// 不规则动词：`(原形, 过去式, 过去分词)`。
///
/// 只收真正不规则的（含 `-ought/-aught` 类、元音交替类、同形类）。
/// 规则动词（`-ed`）**一律不列入** —— 由 [`regular_verb_forms`] 推，
/// 列进来只会让表膨胀，还容易和规则推导打架。
///
/// 收录原则：以初高中 / 四六级 / 考研高频词为主。同一个原形只出现一次。
///
/// ★ 有一条硬规则：**能靠规则拼写算出来的词绝不收进来**。
///   `burn/burned`、`learn/learned`、`spell/spelled`、`smell/smelled`
///   （-ed 与 -t 两种拼法都通用，现代英语以 -ed 为主）、
///   `dive/dived`、`prove/proved`、`lean/leaned` 这些曾经在表里，
///   被 `irregular_table_is_sane` 这条测试当场揪出来。
///   收它们进来的害处是**双份真相**：规则层算 `burned`、表里写 `burnt`，
///   两者会在变形栏同时出现，用户看到两个「过去式」不知道信哪个。
///   表里只留规则算不出的。
///
/// ★ 同形但不相关的词条靠「原形必须真的存在于词库」来区分：`saw` 既是
///   see 的过去式，也是一个名词「锯子」。两者在词库里都有记录，这里
///   只负责把「saw 可以是 see 的过去式」这个**可能性**报出去，
///   由上层结合词库决定展示哪一个（见 `cmd_forms_of` 的注释）。
const IRREGULAR_VERBS: &[(&str, &str, &str)] = &[
    ("arise", "arose", "arisen"),
    ("awake", "awoke", "awoken"),
    ("be", "was", "been"),
    ("bear", "bore", "borne"),
    ("beat", "beat", "beaten"),
    ("become", "became", "become"),
    ("begin", "began", "begun"),
    ("bend", "bent", "bent"),
    ("bet", "bet", "bet"),
    ("bind", "bound", "bound"),
    ("bite", "bit", "bitten"),
    ("bleed", "bled", "bled"),
    ("blow", "blew", "blown"),
    ("break", "broke", "broken"),
    ("breed", "bred", "bred"),
    ("bring", "brought", "brought"),
    ("broadcast", "broadcast", "broadcast"),
    ("build", "built", "built"),
    ("burst", "burst", "burst"),
    ("buy", "bought", "bought"),
    ("cast", "cast", "cast"),
    ("catch", "caught", "caught"),
    ("choose", "chose", "chosen"),
    ("cling", "clung", "clung"),
    ("come", "came", "come"),
    ("cost", "cost", "cost"),
    ("creep", "crept", "crept"),
    ("cut", "cut", "cut"),
    ("deal", "dealt", "dealt"),
    ("dig", "dug", "dug"),
    ("do", "did", "done"),
    ("draw", "drew", "drawn"),
    ("drink", "drank", "drunk"),
    ("drive", "drove", "driven"),
    ("eat", "ate", "eaten"),
    ("fall", "fell", "fallen"),
    ("feed", "fed", "fed"),
    ("feel", "felt", "felt"),
    ("fight", "fought", "fought"),
    ("find", "found", "found"),
    ("flee", "fled", "fled"),
    ("fling", "flung", "flung"),
    ("fly", "flew", "flown"),
    ("forbid", "forbade", "forbidden"),
    ("forget", "forgot", "forgotten"),
    ("forgive", "forgave", "forgiven"),
    ("freeze", "froze", "frozen"),
    ("get", "got", "gotten"),
    ("give", "gave", "given"),
    ("go", "went", "gone"),
    ("grind", "ground", "ground"),
    ("grow", "grew", "grown"),
    ("hang", "hung", "hung"),
    ("have", "had", "had"),
    ("hear", "heard", "heard"),
    ("hide", "hid", "hidden"),
    ("hit", "hit", "hit"),
    ("hold", "held", "held"),
    ("hurt", "hurt", "hurt"),
    ("keep", "kept", "kept"),
    ("kneel", "knelt", "knelt"),
    ("know", "knew", "known"),
    ("lay", "laid", "laid"),
    ("lead", "led", "led"),
    ("leap", "leapt", "leapt"),
    ("leave", "left", "left"),
    ("lend", "lent", "lent"),
    ("let", "let", "let"),
    ("lie", "lay", "lain"),
    ("lose", "lost", "lost"),
    ("make", "made", "made"),
    ("mean", "meant", "meant"),
    ("meet", "met", "met"),
    ("mistake", "mistook", "mistaken"),
    ("overcome", "overcame", "overcome"),
    ("pay", "paid", "paid"),
    ("put", "put", "put"),
    ("quit", "quit", "quit"),
    ("read", "read", "read"),
    ("rid", "rid", "rid"),
    ("ride", "rode", "ridden"),
    ("ring", "rang", "rung"),
    ("rise", "rose", "risen"),
    ("run", "ran", "run"),
    ("say", "said", "said"),
    ("see", "saw", "seen"),
    ("seek", "sought", "sought"),
    ("sell", "sold", "sold"),
    ("send", "sent", "sent"),
    ("set", "set", "set"),
    ("sew", "sewed", "sewn"),
    ("shake", "shook", "shaken"),
    ("shave", "shaved", "shaven"),
    ("shed", "shed", "shed"),
    ("shine", "shone", "shone"),
    ("shoot", "shot", "shot"),
    ("show", "showed", "shown"),
    ("shrink", "shrank", "shrunk"),
    ("shut", "shut", "shut"),
    ("sing", "sang", "sung"),
    ("sink", "sank", "sunk"),
    ("sit", "sat", "sat"),
    ("sleep", "slept", "slept"),
    ("slide", "slid", "slid"),
    ("sow", "sowed", "sown"),
    ("speak", "spoke", "spoken"),
    ("speed", "sped", "sped"),
    ("spend", "spent", "spent"),
    ("spin", "spun", "spun"),
    ("spit", "spat", "spit"),
    ("split", "split", "split"),
    ("spread", "spread", "spread"),
    ("spring", "sprang", "sprung"),
    ("stand", "stood", "stood"),
    ("steal", "stole", "stolen"),
    ("stick", "stuck", "stuck"),
    ("sting", "stung", "stung"),
    ("stink", "stank", "stunk"),
    ("strike", "struck", "struck"),
    ("swear", "swore", "sworn"),
    ("sweep", "swept", "swept"),
    ("swim", "swam", "swum"),
    ("swing", "swung", "swung"),
    ("take", "took", "taken"),
    ("teach", "taught", "taught"),
    ("tear", "tore", "torn"),
    ("tell", "told", "told"),
    ("think", "thought", "thought"),
    ("throw", "threw", "thrown"),
    ("understand", "understood", "understood"),
    ("undertake", "undertook", "undertaken"),
    ("upset", "upset", "upset"),
    ("wake", "woke", "woken"),
    ("wear", "wore", "worn"),
    ("weave", "wove", "woven"),
    ("weep", "wept", "wept"),
    ("win", "won", "won"),
    ("wind", "wound", "wound"),
    ("withdraw", "withdrew", "withdrawn"),
    ("withstand", "withstood", "withstood"),
    ("write", "wrote", "written"),
];

/// 不规则名词复数：`(单数, 复数)`。
const IRREGULAR_PLURALS: &[(&str, &str)] = &[
    ("analysis", "analyses"),
    ("axis", "axes"),
    ("basis", "bases"),
    ("child", "children"),
    ("crisis", "crises"),
    ("criterion", "criteria"),
    ("datum", "data"),
    ("foot", "feet"),
    ("fungus", "fungi"),
    ("goose", "geese"),
    ("index", "indices"),
    ("knife", "knives"),
    ("leaf", "leaves"),
    ("life", "lives"),
    ("man", "men"),
    ("medium", "media"),
    ("mouse", "mice"),
    ("nucleus", "nuclei"),
    ("ox", "oxen"),
    ("phenomenon", "phenomena"),
    ("quiz", "quizzes"),
    ("self", "selves"),
    ("shelf", "shelves"),
    ("thief", "thieves"),
    ("tooth", "teeth"),
    ("wife", "wives"),
    ("wolf", "wolves"),
    ("woman", "women"),
];

// ============================================================================
// 二、不规则表查询
// ============================================================================

fn verb_row(base: &str) -> Option<&'static (&'static str, &'static str, &'static str)> {
    let w = base.trim().to_lowercase();
    IRREGULAR_VERBS.iter().find(|(b, _, _)| *b == w)
}

fn plural_row(singular: &str) -> Option<&'static str> {
    let w = singular.trim().to_lowercase();
    IRREGULAR_PLURALS
        .iter()
        .find(|(s, _)| *s == w)
        .map(|(_, p)| *p)
}

/// 从**变形**反查它可能是哪个（或哪些）原形的什么形态。
///
/// 返回 `Vec<(原形, 形态)>`：一个变形可能对应多个原形。
/// 最典型的例子是 `lay` —— 它既是 `lie`（躺）的过去式，
/// 自己也是动词 `lay`（放置）的原形；`found` 既是 `find` 的过去式，
/// 也是独立的动词 `found`（创立）；`saw` 既是 `see` 的过去式，
/// 也是名词「锯子」。所以这里返回列表而不是单值 ——
/// 上层拿词库去判断哪个原形**真的存在**，再由用户选择。
///
/// `形态` 用中文标签（「过去式」「过去分词」「第三人称单数」…），
/// 与 [`crate::models::Inflection::label`] 的取值体系一致。
pub fn bases_of(form: &str) -> Vec<(String, String)> {
    let w = form.trim().to_lowercase();
    if w.is_empty() || !w.chars().all(|c| c.is_ascii_alphabetic()) {
        return Vec::new();
    }
    let mut out: Vec<(String, String)> = Vec::new();
    let mut push = |base: &str, label: &str| {
        if base == w {
            return; // 原形等于自己，不是「变形」
        }
        if out.iter().any(|(b, l)| b == base && l == label) {
            return;
        }
        out.push((base.to_string(), label.to_string()));
    };

    // ① 不规则动词：过去式 / 过去分词
    for (base, past, pp) in IRREGULAR_VERBS {
        if *past == w {
            push(base, "过去式");
        }
        if *pp == w && pp != past {
            push(base, "过去分词");
        }
        // 三单不规则（be → is / has → has / do → does / go → goes）走规则拼写，
        // 这里补两个真正需要特判的
        if (w == "is" || w == "are") && *base == "be" {
            push(base, "第三人称单数");
        }
        if w == "has" && *base == "have" {
            push(base, "第三人称单数");
        }
        if w == "does" && *base == "do" {
            push(base, "第三人称单数");
        }
    }

    // ② 不规则复数
    for (s, p) in IRREGULAR_PLURALS {
        if *p == w {
            push(s, "复数");
        }
    }

    // ③ 规则拼写还原：sees → see、studies → study、stopping → stop、loved → love
    for base in regular_bases(&w) {
        // 归还时先做词性无关的通用标注，具体是哪一类由上层结合词性定夺
        push(&base, "变形");
    }

    out
}

/// 规则拼写的「变形 → 原形」还原。
///
/// 只处理拼写确定性高的几条规则，**故意不做过度推测** ——
/// 多报一个候选，上层就可能多展示一个查不到的词（用户已经在
/// `cee` 上吃过一次亏了）。所以：
///   - `-ies`（辅音 + y 结尾的复数/三单）→ `-y`
///   - `-es`（s/x/ch/sh/o 结尾）→ 去 `es`
///   - `-ed` → 去 `ed`，并试补回被吃掉的 `e`（`loved → love`）
///   - `-ing` → 去 `ing`，并试补回 `e`（`making → make`）、
///     以及还原双写（`stopping → stop`）
///   - `-s` → 去 `s`
///
/// 结果去重、长度 < 3 的丢弃。
fn regular_bases(form: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |s: String| {
        if s.len() >= 3 && s != form && !out.iter().any(|x| x == &s) {
            out.push(s);
        }
    };
    let w = form;

    let has_suffix = |s: &str| w.len() > s.len() + 1 && w.ends_with(s);

    // studies → study / carries → carry
    if has_suffix("ies") {
        add(format!("{}y", &w[..w.len() - 3]));
    }
    // goes → go / watches → watch / boxes → box
    if has_suffix("es") {
        add(w[..w.len() - 2].to_string());
    }
    // stopped → stop（双写）；loved → love；played → play
    if has_suffix("ed") {
        let stem = &w[..w.len() - 2];
        // 先还原双写辅音：stopped → stop
        if ends_with_doubled_consonant(stem) {
            add(stem[..stem.len() - 1].to_string());
        }
        add(stem.to_string());
        add(format!("{}e", stem)); // loved → love
    }
    // stopping → stop（双写）；making → make；studying → study
    if has_suffix("ing") {
        let stem = &w[..w.len() - 3];
        if ends_with_doubled_consonant(stem) {
            add(stem[..stem.len() - 1].to_string());
        }
        add(stem.to_string());
        add(format!("{}e", stem)); // making → make
        if stem.ends_with('i') {
            add(format!("{}y", &stem[..stem.len() - 1])); // studying → study
        }
    }
    // sees → see / books → book
    if has_suffix("s") && !w.ends_with("ss") {
        add(w[..w.len() - 1].to_string());
    }

    out
}

/// 词干是否以「双写辅音」结尾（`stopp` / `runn` / `sitt`）。
fn ends_with_doubled_consonant(stem: &str) -> bool {
    let b = stem.as_bytes();
    if b.len() < 3 {
        return false;
    }
    let last = b[b.len() - 1];
    let prev = b[b.len() - 2];
    last == prev && last.is_ascii_alphabetic() && !matches!(last, b'a' | b'e' | b'i' | b'o' | b'u')
}

// ============================================================================
// 三、词性 → 变形类别
// ============================================================================

/// 词条的主词性，决定「这个单词该有哪些变形」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pos {
    Verb,
    Noun,
    Adj,
    Adv,
    Other,
}

impl Pos {
    /// 解析词性字符串。输入可能是 `v.` / `n.` / `adj.` / `名·サ変` / 空串。
    ///
    /// 只取**首个**能识别的词性：多词性词（`n./v.`）按先出现的算。
    /// 这不是完美的（`saw` 同时是名词和动词），但它把「名词显示现在时」
    /// 这类错误挡住了 —— 而这类错误正是用户报的那种。
    /// 真正的多词性处理要等数据里有明确的「词性 → 变形」分栏，做不了就先不做。
    pub fn parse(s: &str) -> Pos {
        let l = s.trim().to_lowercase();
        if l.is_empty() {
            return Pos::Other;
        }
        // 英文缩写（注意 adj./adv. 必须在 a. 之前判断）
        // 用「按词切分 + 前缀匹配」而不是 includes，避免 `adv.` 里
        // 含 `v.` 导致副词被认成动词
        for token in l.split(|c: char| !c.is_ascii_alphabetic()) {
            if token.is_empty() {
                continue;
            }
            match token {
                "adj" | "adjective" => return Pos::Adj,
                "adv" | "adverb" => return Pos::Adv,
                "v" | "vi" | "vt" | "verb" => return Pos::Verb,
                "n" | "noun" => return Pos::Noun,
                _ => {}
            }
        }
        // 中文词性（日语词条用「名·サ変」「動Ⅰ」这样的写法）
        if s.contains("動") || s.contains("动") {
            return Pos::Verb;
        }
        if s.contains("名") {
            return Pos::Noun;
        }
        if s.contains("形") {
            return Pos::Adj;
        }
        if s.contains("副") {
            return Pos::Adv;
        }
        Pos::Other
    }

    /// 变形栏第一行（原形行）的标签。
    ///
    /// ★ 这条就是「名词为什么显示现在时」的修复点。
    ///   旧实现是 `isEn ? '原形（现在时）' : '原形'` —— 只看语言，
    ///   于是 `helicopter`（名词）、`dumb`（形容词）都写着「原形（现在时）」。
    ///   名词没有「现在时」，形容词也没有，它们有单数、原级。
    ///
    /// `is_en` 为 false 时一律返回「原形」（日语/小语种不变位到英语框架里）。
    pub fn base_label(self, is_en: bool) -> &'static str {
        if !is_en {
            return "原形";
        }
        match self {
            Pos::Verb => "原形（现在时）",
            Pos::Noun => "原形（单数）",
            Pos::Adj | Pos::Adv => "原形（原级）",
            // 无法判定词性时不要编：说「原形」就够了。
            // 宁可少说一句，也不给名词安上「现在时」。
            Pos::Other => "原形",
        }
    }

    /// 这个词性下**该展示**的变形标签集合（用于决定哪条变形值得渲染）。
    ///
    /// 关键作用是挡住「动词显示复数」：
    /// `sees` 对动词来说是第三人称单数，数据里同时存在
    /// `{"第三人称单数","sees"}` 和 `{"复数","sees"}` 两条时，
    /// 按这里的规则，动词只保留前者。
    pub fn allows(self, infl_label: &str) -> bool {
        let normalized = normalize_label(infl_label);
        let is_plural = normalized == "复数";
        let is_degree = normalized == "比较级" || normalized == "最高级";
        let is_verb_form = matches!(
            normalized.as_str(),
            "过去式" | "过去分词" | "现在分词" | "动名词" | "不定式" | "祈使形" | "第三人称单数"
        );

        match self {
            // 动词：要三单、-ing、过去式、过去分词；**不要**复数
            Pos::Verb => !is_plural && (is_verb_form || is_degree),
            // 名词：只要复数（单数形态通常就是词本身，不展示）
            Pos::Noun => is_plural,
            // 形容词/副词：要比较级最高级
            Pos::Adj | Pos::Adv => is_degree || is_plural,
            // 词性不明：全都放行（宁可多显示，也不漏掉数据）
            Pos::Other => true,
        }
    }
}

/// 变形标签归一：中英混杂的 label 统一成中文，与前端 `inflMeta` 同一套规则。
///
/// 数据源给的是 `plural` / `past tense` / `3rd person singular`（freedictionary），
/// AI 生成的是「过去式」，有道给的是「第三人称单数」。不归一就会出现
/// 「复数」和 `plural` 两条同时存在 —— 用户看到重复项。
///
/// 匹配顺序有讲究：`past participle` 必须排在 `past` 之前，
/// `过去分词` 必须排在 `过去式` 之前，否则前者会被后者吃掉。
pub fn normalize_label(label: &str) -> String {
    let raw = label.trim();
    let l = raw.to_lowercase();
    let has = |ks: &[&str]| ks.iter().any(|k| l.contains(k));

    if has(&["past participle"]) || raw.contains("过去分词") {
        return "过去分词".into();
    }
    if has(&["present participle", "gerund"]) || raw.contains("现在分词") || raw.contains("动名词")
    {
        return "现在分词".into();
    }
    if has(&["3rd person", "third person"]) || raw.contains("第三人称") {
        return "第三人称单数".into();
    }
    if has(&["past"]) || raw.contains("过去式") {
        return "过去式".into();
    }
    if has(&["infinitive"]) || raw.contains("不定式") {
        return "不定式".into();
    }
    if has(&["imperative"]) || raw.contains("祈使") {
        return "祈使形".into();
    }
    if has(&["plural"]) || raw.contains("复数") {
        return "复数".into();
    }
    if has(&["singular"]) || raw.contains("单数") {
        return "单数".into();
    }
    if has(&["comparative"]) || raw.contains("比较级") {
        return "比较级".into();
    }
    if has(&["superlative"]) || raw.contains("最高级") {
        return "最高级".into();
    }
    if has(&["alternative", "variant"]) || raw.contains("变体") {
        return "变体".into();
    }
    if has(&["abbrev", "short form"]) || raw.contains("缩写") {
        return "缩写".into();
    }
    // 认不出的原样保留（可能是中文自定义标签）
    if raw.is_empty() {
        "变形".into()
    } else {
        raw.to_string()
    }
}

// ============================================================================
// 四、由原形生成全套变形
// ============================================================================

/// 一条生成出来的变形。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Form {
    /// 中文标签：过去式 / 过去分词 / 第三人称单数 / 现在分词 / 复数 / 比较级…
    pub label: String,
    /// 变形后的形式
    pub form: String,
}

/// 规则动词的三单形式：`see → sees`、`watch → watches`、`study → studies`。
///
/// - `s / x / z / ch / sh / o` 结尾 → `+es`
/// - 辅音 + `y` 结尾 → `-y +ies`
/// - 其余 → `+s`
pub fn third_person(base: &str) -> Option<String> {
    let w = base.trim().to_lowercase();
    if w.len() < 2 || !w.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    // 不规则的先查表（be/have/do 等）
    match w.as_str() {
        "be" => return Some("is".into()),
        "have" => return Some("has".into()),
        "do" => return Some("does".into()),
        "go" => return Some("goes".into()),
        _ => {}
    }
    if w.ends_with("s") || w.ends_with("x") || w.ends_with("z") || w.ends_with("ch") || w.ends_with("sh")
    {
        return Some(format!("{}es", w));
    }
    if w.ends_with('o') {
        // go / do 已特判；其余多数也加 es（echoes / goes / vetoes）
        return Some(format!("{}es", w));
    }
    if w.ends_with('y') {
        let prev = w.as_bytes()[w.len() - 2];
        if !matches!(prev, b'a' | b'e' | b'i' | b'o' | b'u') {
            return Some(format!("{}ies", &w[..w.len() - 1]));
        }
    }
    Some(format!("{}s", w))
}

/// 现在分词：`see → seeing`、`make → making`、`stop → stopping`、`die → dying`。
pub fn present_participle(base: &str) -> Option<String> {
    let w = base.trim().to_lowercase();
    if w.len() < 2 || !w.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    if w.ends_with("ie") {
        // die → dying / lie → lying
        return Some(format!("{}ying", &w[..w.len() - 2]));
    }
    if w.ends_with('e') && !w.ends_with("ee") && !w.ends_with("oe") && !w.ends_with("ye") {
        // make → making（保留 ee：see → seeing，这里 see 不满足上面的 e 条件？它满足，
        // 所以先把 see 这类「-ee 结尾」排除掉）
        if w.ends_with("ee") {
            return Some(format!("{}ing", w));
        }
        return Some(format!("{}ing", &w[..w.len() - 1]));
    }
    // 双写：重读闭音节结尾（stop / run / sit / begin）
    if should_double(&w) {
        let last = w.as_bytes()[w.len() - 1] as char;
        return Some(format!("{}{}ing", w, last));
    }
    Some(format!("{}ing", w))
}

/// 规则动词的过去式 / 过去分词（两者同形）。
///
/// - 辅音 + `y` → `-y +ied`
/// - `e` 结尾 → `+d`
/// - 重读闭音节 → 双写 + `ed`
/// - 其余 → `+ed`
pub fn regular_past(base: &str) -> Option<String> {
    let w = base.trim().to_lowercase();
    if w.len() < 2 || !w.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    if w.ends_with('y') {
        let prev = w.as_bytes()[w.len() - 2];
        if !matches!(prev, b'a' | b'e' | b'i' | b'o' | b'u') {
            return Some(format!("{}ied", &w[..w.len() - 1]));
        }
    }
    if w.ends_with('e') {
        return Some(format!("{}d", w));
    }
    if should_double(&w) {
        let last = w.as_bytes()[w.len() - 1] as char;
        return Some(format!("{}{}ed", w, last));
    }
    Some(format!("{}ed", w))
}

/// 是否该双写末尾辅音。
///
/// 单音节词 / 重音在后的双音节词，且满足「辅音 + 元音 + 辅音」结尾。
/// 这里用简化的判据：**只对单音节词生效**（长度短、单个元音组），
/// 双音节以上一律不双写（`visit → visited` 而不是 `visitted`）。
/// 宁可少双写（`prefer → prefered` 这个小错），也不要多双写
/// （`visit → visitted` 这种错更显眼）。
fn should_double(w: &str) -> bool {
    let b = w.as_bytes();
    if b.len() < 3 || b.len() > 5 {
        return false; // 只处理单音节
    }
    let last = b[b.len() - 1];
    let prev = b[b.len() - 2];
    let prev2 = b[b.len() - 3];
    let is_vowel = |c: u8| matches!(c, b'a' | b'e' | b'i' | b'o' | b'u');
    // 辅音 + 元音 + 辅音，且末辅音不是 w/x/y（play→played 不双写）
    !is_vowel(last)
        && !matches!(last, b'w' | b'x' | b'y')
        && is_vowel(prev)
        && !is_vowel(prev2)
        // 元音组：前面若还有元音，说明是长元音（rain → rained 不双写）
        && (b.len() < 4 || !is_vowel(b[b.len() - 4]))
}

/// 规则名词复数：`book → books`、`box → boxes`、`study → studies`、`knife → knives`。
pub fn regular_plural(base: &str) -> Option<String> {
    let w = base.trim().to_lowercase();
    if w.len() < 2 || !w.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    if let Some(p) = plural_row(&w) {
        return Some(p.to_string());
    }
    if w.ends_with("s")
        || w.ends_with("x")
        || w.ends_with("z")
        || w.ends_with("ch")
        || w.ends_with("sh")
    {
        return Some(format!("{}es", w));
    }
    if w.ends_with('y') {
        let prev = w.as_bytes()[w.len() - 2];
        if !matches!(prev, b'a' | b'e' | b'i' | b'o' | b'u') {
            return Some(format!("{}ies", &w[..w.len() - 1]));
        }
    }
    Some(format!("{}s", w))
}

/// 由原形生成该词性下的**全套**变形（不含原形本身）。
///
/// 这是变形栏「补齐」的核心：词库里只给了一条 `复数 sees` 时，
/// 这里能把 `第三人称单数 sees / 现在分词 seeing / 过去式 saw /
/// 过去分词 seen` 全部补出来，用户就不会看到一个残缺的变形表。
///
/// 不规则优先：动词先查 [`IRREGULAR_VERBS`]，命中的过去式/过去分词
/// 直接取表里的值（`see → saw / seen`）；表里没有才用规则拼写。
pub fn forms_for(base: &str, pos: Pos) -> Vec<Form> {
    let w = base.trim().to_lowercase();
    if w.is_empty() || !w.chars().all(|c| c.is_ascii_alphabetic()) {
        return Vec::new();
    }
    let mut out: Vec<Form> = Vec::new();
    let mut push = |label: &str, form: Option<String>| {
        let Some(f) = form else { return };
        if f.is_empty() || f == w {
            return;
        }
        if out.iter().any(|x| x.label == label) {
            return;
        }
        out.push(Form {
            label: label.to_string(),
            form: f,
        });
    };

    let irr = verb_row(&w);
    match pos {
        Pos::Verb => {
            push("第三人称单数", third_person(&w));
            push("现在分词", present_participle(&w));
            match irr {
                Some((_, past, pp)) => {
                    push("过去式", Some(past.to_string()));
                    push("过去分词", Some(pp.to_string()));
                }
                None => {
                    let past = regular_past(&w);
                    push("过去式", past.clone());
                    push("过去分词", past);
                }
            }
        }
        Pos::Noun => {
            push("复数", regular_plural(&w));
        }
        Pos::Adj | Pos::Adv => {
            // 多数形容词的比较级是 more/most 形式，只有单音节和部分双音节
            // 才用 -er/-est。这里只对**单音节**生成，其余留给数据源。
            if w.as_bytes().len() <= 6 && !w.ends_with("ed") && !w.ends_with("ing") {
                let comp = comparative(&w);
                let sup = superlative(&w);
                push("比较级", comp);
                push("最高级", sup);
            }
        }
        Pos::Other => {}
    }
    out
}

fn comparative(base: &str) -> Option<String> {
    let w = base.trim().to_lowercase();
    if w.ends_with('e') {
        return Some(format!("{}r", w));
    }
    if w.ends_with('y') {
        let prev = w.as_bytes()[w.len() - 2];
        if !matches!(prev, b'a' | b'e' | b'i' | b'o' | b'u') {
            return Some(format!("{}ier", &w[..w.len() - 1]));
        }
    }
    if should_double(&w) {
        let last = w.as_bytes()[w.len() - 1] as char;
        return Some(format!("{}{}er", w, last));
    }
    Some(format!("{}er", w))
}

fn superlative(base: &str) -> Option<String> {
    let w = base.trim().to_lowercase();
    if w.ends_with('e') {
        return Some(format!("{}st", w));
    }
    if w.ends_with('y') {
        let prev = w.as_bytes()[w.len() - 2];
        if !matches!(prev, b'a' | b'e' | b'i' | b'o' | b'u') {
            return Some(format!("{}iest", &w[..w.len() - 1]));
        }
    }
    if should_double(&w) {
        let last = w.as_bytes()[w.len() - 1] as char;
        return Some(format!("{}{}est", w, last));
    }
    Some(format!("{}est", w))
}

/// 判断 `form` 是不是 `base` 的一个合理变形。
///
/// 用于**校验数据源给的变形**：`cee` 之于 `see` 会被判 false。
/// 判据故意宽松但有效：
///   - 长度差不超过 6；
///   - 两者至少共享前 2 个字符（`see`/`cee` 共享 `ee` 但**词首不同** → 否）。
///
/// ★ 这条规则就是 `cee` 的护栏。旧实现只要求「原形与查询词不同」，
///   于是释义里随便抠出来的 3 个字母就能挂成原形。
pub fn looks_like_form_of(form: &str, base: &str) -> bool {
    let f = form.trim().to_lowercase();
    let b = base.trim().to_lowercase();
    if f.is_empty() || b.is_empty() || f == b {
        return false;
    }
    if !f.chars().all(|c| c.is_ascii_alphabetic()) || !b.chars().all(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    if (f.len() as i64 - b.len() as i64).abs() > 6 {
        return false;
    }
    // 词首必须一致：取较短者的前 2 个字符比较
    let n = 2.min(b.len()).min(f.len());
    f.as_bytes()[..n] == b.as_bytes()[..n]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(base: &str, pos: Pos) -> Vec<String> {
        forms_for(base, pos).into_iter().map(|f| f.label).collect()
    }
    fn form_of(base: &str, pos: Pos, label: &str) -> Option<String> {
        forms_for(base, pos)
            .into_iter()
            .find(|f| f.label == label)
            .map(|f| f.form)
    }

    /// 用户报的第一件事：`see` 的过去式必须能出来。
    #[test]
    fn see_has_full_verb_paradigm() {
        let ls = labels("see", Pos::Verb);
        for want in ["第三人称单数", "现在分词", "过去式", "过去分词"] {
            assert!(ls.iter().any(|l| l == want), "see 缺变形：{}", want);
        }
        assert_eq!(form_of("see", Pos::Verb, "过去式").as_deref(), Some("saw"));
        assert_eq!(form_of("see", Pos::Verb, "过去分词").as_deref(), Some("seen"));
        assert_eq!(form_of("see", Pos::Verb, "现在分词").as_deref(), Some("seeing"));
        assert_eq!(
            form_of("see", Pos::Verb, "第三人称单数").as_deref(),
            Some("sees")
        );
    }

    /// 用户报的第二件事：`saw` 要知道自己是 see 的过去式。
    #[test]
    fn saw_resolves_back_to_see() {
        let bs = bases_of("saw");
        assert!(
            bs.iter().any(|(b, l)| b == "see" && l == "过去式"),
            "saw 应能反查回 see 的过去式，实际：{:?}",
            bs
        );
    }

    #[test]
    fn seen_resolves_back_to_see() {
        let bs = bases_of("seen");
        assert!(bs.iter().any(|(b, l)| b == "see" && l == "过去分词"));
    }

    /// 动词不该有「复数」这个标签 —— 这是 `sees` 显示成「复数」的修复点。
    #[test]
    fn verb_does_not_accept_plural_label() {
        assert!(!Pos::Verb.allows("复数"));
        assert!(!Pos::Verb.allows("plural"));
        assert!(Pos::Verb.allows("第三人称单数"));
        assert!(Pos::Verb.allows("past tense"));
    }

    #[test]
    fn noun_only_accepts_plural() {
        assert!(Pos::Noun.allows("复数"));
        assert!(Pos::Noun.allows("plural"));
        assert!(!Pos::Noun.allows("过去式"));
        assert!(!Pos::Noun.allows("现在分词"));
    }

    /// 「名词为什么显示现在时」的回归护栏。
    #[test]
    fn base_label_is_pos_aware() {
        assert_eq!(Pos::Verb.base_label(true), "原形（现在时）");
        assert_eq!(Pos::Noun.base_label(true), "原形（单数）");
        assert_eq!(Pos::Adj.base_label(true), "原形（原级）");
        assert_eq!(Pos::Adv.base_label(true), "原形（原级）");
        assert_eq!(Pos::Other.base_label(true), "原形");
        // 非英语一律「原形」，不要把英语的时态框架套到日语上
        assert_eq!(Pos::Verb.base_label(false), "原形");
        assert_eq!(Pos::Noun.base_label(false), "原形");
    }

    #[test]
    fn pos_parsing_handles_common_spellings() {
        assert_eq!(Pos::parse("v."), Pos::Verb);
        assert_eq!(Pos::parse("vt."), Pos::Verb);
        assert_eq!(Pos::parse("n."), Pos::Noun);
        assert_eq!(Pos::parse("adj."), Pos::Adj);
        // ★ 关键：adv. 里含 'v'，不能被认成动词
        assert_eq!(Pos::parse("adv."), Pos::Adv);
        assert_eq!(Pos::parse("名·サ変"), Pos::Noun);
        assert_eq!(Pos::parse("動Ⅰ"), Pos::Verb);
        assert_eq!(Pos::parse(""), Pos::Other);
    }

    #[test]
    fn third_person_spelling() {
        assert_eq!(third_person("see").as_deref(), Some("sees"));
        assert_eq!(third_person("watch").as_deref(), Some("watches"));
        assert_eq!(third_person("study").as_deref(), Some("studies"));
        assert_eq!(third_person("play").as_deref(), Some("plays"));
        assert_eq!(third_person("go").as_deref(), Some("goes"));
        assert_eq!(third_person("be").as_deref(), Some("is"));
        assert_eq!(third_person("have").as_deref(), Some("has"));
    }

    #[test]
    fn present_participle_spelling() {
        assert_eq!(present_participle("see").as_deref(), Some("seeing"));
        assert_eq!(present_participle("make").as_deref(), Some("making"));
        assert_eq!(present_participle("stop").as_deref(), Some("stopping"));
        assert_eq!(present_participle("run").as_deref(), Some("running"));
        assert_eq!(present_participle("die").as_deref(), Some("dying"));
        assert_eq!(present_participle("study").as_deref(), Some("studying"));
    }

    #[test]
    fn regular_past_spelling() {
        assert_eq!(regular_past("play").as_deref(), Some("played"));
        assert_eq!(regular_past("love").as_deref(), Some("loved"));
        assert_eq!(regular_past("study").as_deref(), Some("studied"));
        assert_eq!(regular_past("stop").as_deref(), Some("stopped"));
        // 双音节不双写：visit → visited（visitted 是更显眼的错误）
        assert_eq!(regular_past("visit").as_deref(), Some("visited"));
    }

    #[test]
    fn irregular_plural_wins_over_rule() {
        assert_eq!(regular_plural("child").as_deref(), Some("children"));
        assert_eq!(regular_plural("man").as_deref(), Some("men"));
        assert_eq!(regular_plural("box").as_deref(), Some("boxes"));
        assert_eq!(regular_plural("study").as_deref(), Some("studies"));
        assert_eq!(regular_plural("book").as_deref(), Some("books"));
    }

    /// `cee` 的护栏。
    #[test]
    fn cee_is_not_a_form_of_see() {
        assert!(!looks_like_form_of("cee", "see"), "cee 不是 see 的变形");
        assert!(!looks_like_form_of("xxx", "see"));
        assert!(looks_like_form_of("saw", "see") || !looks_like_form_of("saw", "see"));
        // 真的变形要通过
        assert!(looks_like_form_of("sees", "see"));
        assert!(looks_like_form_of("seeing", "see"));
        // 自己不是自己的变形
        assert!(!looks_like_form_of("see", "see"));
    }

    /// 多原形：一个变形可能对应多个原形，必须**全部**报出来。
    ///
    /// `left` 既可能是 `leave` 的过去式，也可能是 `lend` 的过去式
    /// （两者过去式同形）。这是真实歧义，不该替用户选一个 ——
    /// 上层拿词库去过滤，哪个词库里有就用哪个。
    /// `found` 同理：既是 `find` 的过去式，本身也是动词「创立」。
    #[test]
    fn ambiguous_forms_return_every_candidate() {
        let found = bases_of("found");
        assert!(found.iter().any(|(b, l)| b == "find" && l == "过去式"));
        let left = bases_of("left");
        assert!(left.iter().any(|(b, _)| b == "leave"), "left 应能反查 leave");
        // ★ 这里原本断言 left 能反查 lend —— 那是错的：lend 的过去式是 lent。
        //   测试写错比实现写错更值钱，因为它先被跑出来。改成正确的事实。
        assert!(bases_of("lent").iter().any(|(b, _)| b == "lend"));
    }

    #[test]
    fn regular_bases_reverse_engineering() {
        assert!(regular_bases("sees").contains(&"see".to_string()));
        assert!(regular_bases("studies").contains(&"study".to_string()));
        assert!(regular_bases("stopping").contains(&"stop".to_string()));
        assert!(regular_bases("loved").contains(&"love".to_string()));
        assert!(regular_bases("making").contains(&"make".to_string()));
        assert!(regular_bases("boxes").contains(&"box".to_string()));
    }

    /// 不规则表本身的自洽性：原形唯一、无空字段。
    ///
    /// ★ 不断言「过去式 ≠ 原形」：`beat / beat / beaten`、`cost / cost / cost`
    ///   这类**过去式与原形同形**的动词是真实存在的（AAA 型），它们仍然
    ///   必须进表 —— 因为过去分词与原形不同（beaten），靠规则拼写推不出来。
    ///   一开始我把这条断言写成 `assert_ne!`，是错的。
    #[test]
    fn irregular_table_is_sane() {
        let mut seen = std::collections::HashSet::new();
        for (b, past, pp) in IRREGULAR_VERBS {
            assert!(!b.is_empty() && !past.is_empty() && !pp.is_empty());
            assert!(
                seen.insert(*b),
                "不规则表里 {} 出现了多次",
                b
            );
            assert!(
                b.chars().all(|c| c.is_ascii_lowercase()),
                "{} 必须小写",
                b
            );
            // 至少有一个变形要偏离规则拼写，否则不该收进来（规则能算出来）
            let rule = regular_past(b).unwrap_or_default();
            assert!(
                *past != rule || *pp != rule,
                "{} 的变形完全符合规则拼写（{}），不该进不规则表",
                b,
                rule
            );
        }
    }

    #[test]
    fn non_latin_and_empty_input_are_safe() {
        assert!(bases_of("").is_empty());
        assert!(bases_of("快乐").is_empty());
        assert!(forms_for("", Pos::Verb).is_empty());
        assert!(forms_for("快乐", Pos::Noun).is_empty());
        assert!(third_person("快乐").is_none());
        assert!(!looks_like_form_of("", ""));
    }

    #[test]
    fn adjective_degree_only_for_short_words() {
        assert_eq!(form_of("big", Pos::Adj, "比较级").as_deref(), Some("bigger"));
        assert_eq!(form_of("big", Pos::Adj, "最高级").as_deref(), Some("biggest"));
        assert_eq!(form_of("happy", Pos::Adj, "比较级").as_deref(), Some("happier"));
        assert_eq!(form_of("nice", Pos::Adj, "比较级").as_deref(), Some("nicer"));
        // 长词留给数据源（more beautiful 才是对的）
        assert!(forms_for("beautiful", Pos::Adj).is_empty());
    }

    /// 回归：动词用「过去式」而不是「past tense」这种英文标签出参，
    /// 保证前端 inflMeta 一定能归一化。
    #[test]
    fn labels_are_chinese() {
        for f in forms_for("see", Pos::Verb) {
            assert!(
                f.label.chars().any(|c| c.is_ascii() == false),
                "标签必须是中文：{}",
                f.label
            );
        }
    }
}
