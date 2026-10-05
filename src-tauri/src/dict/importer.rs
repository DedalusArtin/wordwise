/* ============================================================
   importer.rs —— 公开词库导入（需求 3 / 5）
   支持三种常见格式，均可通过「字段映射」适配不同来源：
     1. JSON      数组或 {"words":[...]} 包裹
     2. CSV/TSV   首行为表头（或纯词表）
     3. 纯文本     每行一个词（可带释义，用 Tab / 竖线分隔）
   外加：内置一份「可下载词库目录」，指向 GitHub 上的公开词库。
   ============================================================ */

use crate::models::{Example, RemoteBook, Sense, WordEntry};
use anyhow::Result;

/// 导入时如何从原始记录里取字段。
/// 每一项都是「候选路径/列名」列表，按顺序尝试，取到第一个非空值。
/// 这样同一份代码能适配 ECDICT、kajweb/dict、各种自制 CSV。
#[derive(Debug, Clone)]
pub struct ImportMapping {
    /// 单词列/字段
    pub word: Vec<String>,
    /// 音标
    pub phonetic: Vec<String>,
    /// 英文释义
    pub definition: Vec<String>,
    /// 中文释义
    pub translation: Vec<String>,
    /// 词性
    pub pos: Vec<String>,
    /// 例句
    pub example: Vec<String>,
    /// 例句译文
    pub example_translation: Vec<String>,
    /// 标签（如 cet4 / cet6 / ky），用于归类到词库
    pub tags: Vec<String>,
}

impl Default for ImportMapping {
    /// 宽松默认：覆盖 ECDICT 与常见自制词表的命名习惯。
    fn default() -> Self {
        let v = |s: &str| vec![s.to_string()];
        Self {
            // `expression` 是 Anki 系词表的惯用列名（日语 JLPT 卡组都是这个）
            word: ["word", "Word", "单词", "headWord", "head_word", "en", "term", "name", "expression", "词条"]
                .iter().map(|s| s.to_string()).collect(),
            // 日语/韩语词表用 reading / kana 存读音，等价于音标的位置
            phonetic: ["phonetic", "usphone", "ukphone", "音标", "pron", "pronunciation", "reading", "kana", "假名"]
                .iter().map(|s| s.to_string()).collect(),
            definition: ["definition", "def", "meaning_en", "english", "释义", "explain_en", "meanings"]
                .iter().map(|s| s.to_string()).collect(),
            translation: ["translation", "trans", "meaning", "meaning_cn", "中文", "释义_cn", "explain", "gloss", "meanings"]
                .iter().map(|s| s.to_string()).collect(),
            pos: v("pos"),
            example: ["example", "sentence", "例句", "example_en"]
                .iter().map(|s| s.to_string()).collect(),
            example_translation: ["example_translation", "sentence_cn", "例句译文", "example_cn"]
                .iter().map(|s| s.to_string()).collect(),
            tags: ["tag", "tags", "category", "level", "标签", "分类", "book"]
                .iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// 判断一个 token 是否「像一个词条」。
///
/// 这一步是必需的：公开词表里普遍混着标题行、`(共 4615 词)` 这样的说明行、
/// 以及 `A`/`B`/`C` 这样的分节字母。老实现不做校验，于是这些噪声全部变成了
/// 「单词」被写进词库，用户看到一堆莫名其妙的词条。
fn looks_like_word(s: &str, lang: &str) -> bool {
    if s.is_empty() || s.chars().count() > 40 {
        return false;
    }
    let mut has_letter = false;
    for c in s.chars() {
        if c.is_alphabetic() {
            // 英语词表里混进 CJK 一定是噪声（标题 / 说明）
            if lang == "en" && !c.is_ascii_alphabetic() {
                return false;
            }
            has_letter = true;
        } else if !matches!(c, '-' | '\'' | '\u{2019}' | '.' | '\u{30fb}') {
            // 词里只可能出现连字符、撇号、点（mother-in-law / don't / U.S.）
            return false;
        }
    }
    if !has_letter {
        return false;
    }
    // 单个大写字母是词表的分节标题（A / B / C …），不是单词；
    // 但小写 "a" 是正经单词，必须保留。
    if s.chars().count() == 1 && s.chars().next().map(|c| c.is_ascii_uppercase()) == Some(true) {
        return false;
    }
    true
}

/// 是否包含汉字（用于识别「整行中文」的标题行）。
fn has_han(s: &str) -> bool {
    s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

/// 按「连续 2 个及以上空白」切分，用于 `word           [音标]         词性.释义`
/// 这类对齐排版的词表（托福、COCA、牛津等公开词表普遍是这种格式）。
///
/// 只按 ASCII 空白切分，不会切开多字节字符。
fn split_multi_space(line: &str) -> Vec<String> {
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut parts: Vec<String> = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < n {
        if bytes[i].is_ascii_whitespace() {
            let ws_start = i;
            while i < n && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            let run = i - ws_start;
            // 制表符一律视为分隔；空格则要连续 2 个以上才算
            if run >= 2 || bytes[ws_start] == b'\t' {
                parts.push(line[start..ws_start].trim().to_string());
                start = i;
            }
        } else {
            i += 1;
        }
    }
    parts.push(line[start..].trim().to_string());
    parts.retain(|p| !p.is_empty());
    parts
}

/// 把一行拆成字段。按「显式分隔符优先、空格兜底」的顺序尝试。
fn split_fields(line: &str) -> Vec<String> {
    // 1) 制表符 / 连续空格（覆盖绝大多数对齐排版的词表）
    let by_space = split_multi_space(line);
    if by_space.len() >= 2 {
        return by_space;
    }
    // 2) 竖线
    if line.contains('|') {
        let parts: Vec<String> = line.split('|').map(|s| s.trim().to_string()).collect();
        if parts.len() >= 2 {
            return parts;
        }
    }
    // 3) 逗号（且整行没有空格，如 `apple,苹果`）
    if line.contains(',') && !line.contains(' ') {
        let parts: Vec<String> = line.split(',').map(|s| s.trim().to_string()).collect();
        if parts.len() >= 2 {
            return parts;
        }
    }
    // 4) 兜底：`abandon [əˈbændən] vt.丢弃` —— 只有单个空格，
    //    此时「第一个空白前的 token」是单词，其余整体作为音标+释义。
    if let Some(idx) = line.find(char::is_whitespace) {
        let head = line[..idx].trim();
        let tail = line[idx..].trim();
        if !head.is_empty() && !tail.is_empty() {
            return vec![head.to_string(), tail.to_string()];
        }
    }
    vec![line.to_string()]
}

/// 从一行纯文本里切出「词」与「可选释义/音标」。
/// 支持：
/// - `apple`
/// - `apple\t苹果`
/// - `apple|苹果|/ˈæp.əl/`
/// - `apple,苹果`
/// - `abandon [əˈbændən] vt.放弃`
/// - `abandon           [ə'bændən]            vt.  放弃,沉溺`
pub fn parse_text_line(line: &str, lang: &str) -> Option<(String, String, String, Vec<String>)> {
    // 去 BOM：CET4 / 中考试类词表的首行都带 UTF-8 BOM
    let line = line.trim().trim_start_matches('\u{feff}').trim();
    if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
        return None;
    }

    let parts = split_fields(line);
    let word = parts.first()?.trim().to_string();
    if !looks_like_word(&word, lang) {
        return None;
    }

    let rest = &parts[1..];
    let mut phonetic = String::new();
    let mut gloss_parts: Vec<String> = Vec::new();
    let mut tags: Vec<String> = Vec::new();

    for p in rest {
        if p.is_empty() {
            continue;
        }
        // 音标形态：/.../ 或 [...]
        if (p.starts_with('/') && p.ends_with('/')) || (p.starts_with('[') && p.ends_with(']')) {
            let inner = p[1..p.len() - 1].trim();
            if !inner.is_empty() && phonetic.is_empty() {
                phonetic = format!("[{}]", inner);
            }
            continue;
        }
        // 等级标签：cet4 / cet6 / ky / gre / ielts ...
        let low = p.to_lowercase();
        if matches!(low.as_str(),
            "cet4" | "cet6" | "cet-4" | "cet-6" | "ky" | "kaoyan" | "考研"
            | "ielts" | "toefl" | "gre" | "gaokao" | "高考" | "四级" | "六级") {
            tags.push(low);
            continue;
        }
        // 含「::」的复合列单独处理（见下方），不并进普通释义
        if p.contains("::") {
            continue;
        }
        gloss_parts.push(p.clone());
    }

    // 剩下的字段全部并进释义。
    // 必须并起来而不是只取第一个：托福/COCA 这类词表用多空格对齐，
    // 而 `vt.  放弃` 里面本身就带两个空格，会被切成两段。
    //
    // 例外：出现「::」复合列时（雅思等 TSV 把 `词性::释义::同义词` 挤在一列），
    // 只取其中的释义段，其余列（音标、同义词）一律丢弃 ——
    // 否则释义会长成 `'sɛnsəbl 'sensɪb(ə)l adj::可感觉的；明智的::...`，
    // 根本没法看。
    let colon_gloss: Vec<String> = rest
        .iter()
        .filter(|p| p.contains("::"))
        .map(|p| pick_gloss_segment(p))
        .filter(|s| !s.is_empty())
        .collect();
    let mut gloss = if colon_gloss.is_empty() {
        gloss_parts.join(" ")
    } else {
        colon_gloss.join(" ")
    };

    // 有些词表把音标和释义挤在同一个字段里（`abandon [əˈbændən] vt.放弃`）：
    // 这时要把音标抽出来，**并且**从释义里去掉，否则释义会带着一串音标。
    if phonetic.is_empty() {
        let (p, g) = split_bracket_phonetic(&gloss);
        if let Some(p) = p {
            phonetic = p;
            gloss = g;
        }
    }

    // 去掉遗留的前导括号说明，如 `(an) art. 一（个、件……）`
    let gloss = trim_leading_paren(&gloss);

    // 整行中文、又既没音标也没释义 —— 这是词表的标题/说明行（如
    // 「大学英语四级大纲单词表」「(共 4615 词)」），不是词条。
    if gloss.is_empty() && phonetic.is_empty() && has_han(&word) {
        return None;
    }

    Some((word, phonetic, gloss, tags))
}

/// 从形如 `[əˈbændən] vt.放弃` 的串里抽出音标，返回
/// `(音标, 去掉音标后的剩余文本)`。
fn split_bracket_phonetic(s: &str) -> (Option<String>, String) {
    let original = s.trim().to_string();
    let Some(start) = s.find('[') else {
        return (None, original);
    };
    let Some(end_rel) = s[start..].find(']') else {
        return (None, original);
    };
    let inner = s[start + 1..start + end_rel].trim();
    if inner.is_empty() {
        return (None, original);
    }
    let before = s[..start].trim();
    let after = s[start + end_rel + 1..].trim();
    let mut cleaned = before.to_string();
    if !after.is_empty() {
        if !cleaned.is_empty() {
            cleaned.push(' ');
        }
        cleaned.push_str(after);
    }
    (Some(format!("[{}]", inner)), cleaned.trim().to_string())
}

/// 从 `adj::可感觉的；明智的::reasonable, praiseworthy` 这类
/// **用「::」分段的复合字段**里取出释义。
///
/// 雅思等 TSV 词表会把「词性 / 释义 / 同义词」挤在同一列，整段塞进释义的话
/// 用户看到的是一坨混着音标和同义词的文字。取「第一个含中日韩字符的段」
/// 就是释义；万一没有（比如纯英文同义列），退回第二段。
fn pick_gloss_segment(p: &str) -> String {
    if !p.contains("::") {
        return p.to_string();
    }
    let is_cjk = |c: char| {
        ('\u{4e00}'..='\u{9fff}').contains(&c)      // 汉字
            || ('\u{3040}'..='\u{30ff}').contains(&c)   // 日文假名
            || ('\u{ac00}'..='\u{d7af}').contains(&c)   // 韩文谚文
    };
    let segs: Vec<&str> = p
        .split("::")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if let Some(s) = segs.iter().find(|s| s.chars().any(is_cjk)) {
        return (*s).to_string();
    }
    segs.get(1).or_else(|| segs.first()).unwrap_or(&"").to_string()
}

/// 去掉释义开头的 `(an)` / `(pl.)` 这类补充说明。
fn trim_leading_paren(s: &str) -> String {
    let t = s.trim();
    if !(t.starts_with('(') || t.starts_with('（')) {
        return t.to_string();
    }
    let close = if t.starts_with('(') { ')' } else { '）' };
    match t.find(close) {
        Some(i) if i < 12 => t[i + close.len_utf8()..].trim().to_string(),
        _ => t.to_string(),
    }
}

/// 把一条记录转成 WordEntry（不含变形，变形交由上层用规则补）。
fn build_entry(
    word: &str,
    phonetic: &str,
    def_en: &str,
    def_cn: &str,
    pos: &str,
    example: &str,
    example_cn: &str,
    lang: &str,
    ordinal: usize,
) -> WordEntry {
    let mut e = WordEntry::new(word);
    e.lang = lang.to_string();
    if !phonetic.is_empty() {
        e.phonetic.uk = phonetic.to_string();
        e.phonetic.us = phonetic.to_string();
    }

    let def = if !def_cn.trim().is_empty() { def_cn.trim() } else { def_en.trim() };
    if !def.is_empty() {
        // 一条记录里可能用分号分隔多个义项，拆开更利于展示
        let senses: Vec<Sense> = def
            .split([';', '；'])
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .enumerate()
            .map(|(i, d)| {
                let mut sense = Sense {
                    pos: if i == 0 { pos.trim().to_string() } else { String::new() },
                    definition: d.to_string(),
                    examples: Vec::new(),
                };
                if i == 0 && !example.trim().is_empty() {
                    sense.examples.push(Example {
                        text: example.trim().to_string(),
                        translation: example_cn.trim().to_string(),
                    });
                }
                sense
            })
            .collect();
        e.senses = senses;
    } else if !example.trim().is_empty() {
        e.senses.push(Sense {
            pos: pos.trim().to_string(),
            definition: String::new(),
            examples: vec![Example {
                text: example.trim().to_string(),
                translation: example_cn.trim().to_string(),
            }],
        });
    }

    let _ = ordinal;
    e
}

/// 从 JSON 值里按候选键取名（大小写不敏感）。
fn pick<'a>(obj: &'a serde_json::Map<String, serde_json::Value>, keys: &[String]) -> String {
    for k in keys {
        // 精确命中
        if let Some(v) = obj.get(k) { if let Some(s) = as_text(v) { return s; } }
        // 大小写不敏感命中
        for (kk, vv) in obj.iter() {
            if kk.eq_ignore_ascii_case(k) {
                if let Some(s) = as_text(vv) { return s; }
            }
        }
    }
    String::new()
}

/// 与 `pick` 类似，但**数组会被整段拼起来**（用「；」分隔），而不是只取第一项。
///
/// 为什么需要它：OpenJLPT 这类词库的释义是数组
/// （`"meanings": ["to meet", "to see"]`），只取第一项会静默丢掉其余义项，
/// 用户看到的就是「这个词怎么只有一个意思」。拼起来的分隔符用「；」，
/// 正好能被 `build_entry` 拆成多个 sense 逐条展示。
fn pick_join<'a>(obj: &'a serde_json::Map<String, serde_json::Value>, keys: &[String]) -> String {
    for k in keys {
        let v = obj
            .get(k)
            .or_else(|| obj.iter().find(|(kk, _)| kk.eq_ignore_ascii_case(k)).map(|(_, vv)| vv));
        let Some(v) = v else { continue };
        let s = match v {
            serde_json::Value::String(s) => s.trim().to_string(),
            serde_json::Value::Array(a) => a
                .iter()
                .filter_map(as_text)
                .collect::<Vec<_>>()
                .join("；"),
            other => as_text(other).unwrap_or_default(),
        };
        if !s.trim().is_empty() {
            return s;
        }
    }
    String::new()
}

fn as_text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Array(a) => a.iter().find_map(as_text),
        _ => None,
    }
}

/// 解析 JSON 文本。
pub fn parse_json(text: &str, m: &ImportMapping, lang: &str) -> Result<Vec<WordEntry>> {
    let v: serde_json::Value = serde_json::from_str(text)?;
    // 可能是数组，也可能包在常见字段里
    let arr = if let Some(a) = v.as_array() {
        a.clone()
    } else if let Some(o) = v.as_object() {
        let mut found = None;
        for k in ["words", "data", "list", "items", "entries", "result", "results"] {
            if let Some(a) = o.get(k).and_then(|x| x.as_array()) {
                found = Some(a.clone());
                break;
            }
        }
        // 也支持 { "apple": "苹果", "banana": "香蕉" } 这种字典形式
        if found.is_none() {
            let mut out = Vec::new();
            for (k, val) in o.iter() {
                if let Some(s) = as_text(val) {
                    out.push(build_entry(k, "", "", &s, "", "", "", lang, 0));
                }
            }
            return Ok(out);
        }
        found.unwrap()
    } else {
        return Ok(Vec::new());
    };

    let mut out = Vec::new();
    for (i, item) in arr.iter().enumerate() {
        match item {
            // 纯字符串数组：["apple","banana"]
            serde_json::Value::String(s) => {
                if !s.trim().is_empty() {
                    out.push(build_entry(s.trim(), "", "", "", "", "", "", lang, i));
                }
            }
            serde_json::Value::Object(o) => {
                let word = pick(o, &m.word);
                if word.trim().is_empty() { continue; }
                let e = build_entry(
                    word.trim(),
                    &pick(o, &m.phonetic),
                    // 释义可能是一组（OpenJLPT 的 meanings），要整段拼起来
                    &pick_join(o, &m.definition),
                    &pick_join(o, &m.translation),
                    &pick(o, &m.pos),
                    &pick(o, &m.example),
                    &pick(o, &m.example_translation),
                    lang,
                    i,
                );
                out.push(e);
            }
            _ => {}
        }
    }
    Ok(out)
}

/// 支持引号的字段切分（RFC4180 简化版）。
///
/// 老实现直接 `line.split(',')`，于是 `"放弃, 抛弃",v.` 这种带逗号的释义
/// 会被切碎、列错位，导进来的释义全是乱的。公开词表（尤其 ECDICT 系）
/// 几乎都用引号包裹含逗号/换行的字段，所以必须按引号规则切。
fn split_quoted(line: &str, delim: char) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                // 连续两个引号表示一个字面引号
                if chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                cur.push(c);
            }
        } else if c == '"' {
            in_quotes = true;
        } else if c == delim {
            out.push(cur.trim().to_string());
            cur = String::new();
        } else {
            cur.push(c);
        }
    }
    out.push(cur.trim().to_string());
    out
}

/// 解析 CSV/TSV（按首行表头映射列）。
pub fn parse_csv(text: &str, m: &ImportMapping, lang: &str) -> Result<Vec<WordEntry>> {
    // 去 BOM，否则首列列名会带上 \u{feff} 而匹配不上
    let text = text.trim_start_matches('\u{feff}');
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header = match lines.next() {
        Some(h) => h.trim_start_matches('\u{feff}').to_string(),
        None => return Ok(Vec::new()),
    };
    // 判断分隔符：制表符优先，其次逗号
    let delim = if header.contains('\t') { '\t' } else { ',' };
    let cols: Vec<String> = split_quoted(&header, delim);

    // 首行到底是「表头」还是「第一条数据」？
    //
    // 老实现只看 `cols.len() > 1`，于是像 `abandon,v./n.放弃；放纵` 这种
    // **无表头的两列词表**会把第一个单词当成列名吃掉，而且列名对不上
    // 任何已知字段 → 整份文件的释义全丢（红宝书 GRE、自制两列 CSV 都是
    // 这个格式）。这里改为「先看列名是否像表头」：
    //   - 命中已知列名（word/translation/释义…）→ 是表头；
    //   - 列数 ≥ 3 → 仍然当表头（多列无表头的情况极少，保持旧行为更稳）；
    //   - 否则按纯文本行解析，第一行老老实实当第一个单词。
    let has_header = looks_like_header(&header) || cols.len() > 2;
    if !has_header {
        // 退化为纯文本行解析
        return parse_text(text, m, lang);
    }

    let find_col = |cands: &[String]| -> Option<usize> {
        for c in cands {
            for (i, h) in cols.iter().enumerate() {
                if h.eq_ignore_ascii_case(c) {
                    return Some(i);
                }
            }
        }
        None
    };
    let ci_word = find_col(&m.word).unwrap_or(0);
    let ci_ph = find_col(&m.phonetic);
    let ci_def = find_col(&m.definition);
    let ci_cn = find_col(&m.translation);
    let ci_pos = find_col(&m.pos);
    let ci_ex = find_col(&m.example);
    let ci_excn = find_col(&m.example_translation);

    let get = |f: &[String], i: Option<usize>| -> String {
        i.and_then(|x| f.get(x)).cloned().unwrap_or_default()
    };

    let mut out = Vec::new();
    for (n, line) in lines.enumerate() {
        let f = split_quoted(line, delim);
        let word = get(&f, Some(ci_word));
        if word.is_empty() {
            continue;
        }
        out.push(build_entry(
            &word,
            &get(&f, ci_ph),
            &get(&f, ci_def),
            &get(&f, ci_cn),
            &get(&f, ci_pos),
            &get(&f, ci_ex),
            &get(&f, ci_excn),
            lang,
            n,
        ));
    }
    Ok(out)
}

/// 解析纯文本（每行一个词）。
///
/// 会用 `parse_text_line` 做一次「像不像词条」的校验，把标题行、
/// 分节字母、说明行之类的噪声挡在词库外面。
pub fn parse_text(text: &str, _m: &ImportMapping, lang: &str) -> Result<Vec<WordEntry>> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if let Some((word, phon, gloss, _tags)) = parse_text_line(line, lang) {
            out.push(build_entry(&word, &phon, "", &gloss, "", "", "", lang, i));
        }
    }
    Ok(out)
}

/// 按扩展名/内容自动选择解析器。
pub fn parse_auto(text: &str, m: &ImportMapping, lang: &str, hint: &str) -> Result<Vec<WordEntry>> {
    let trimmed = text.trim_start();
    let by_ext = hint.to_lowercase();
    if by_ext.ends_with(".json") || trimmed.starts_with('{') || trimmed.starts_with('[') {
        return parse_json(text, m, lang);
    }
    if by_ext.ends_with(".csv") || by_ext.ends_with(".tsv") {
        return parse_csv(text, m, lang);
    }
    // 无提示时按内容猜：
    //   首行字段命中已知列名（word/translation/词条…）→ 有表头的 CSV，
    //   其余一律走「逐行文本」解析。
    //
    // 这里刻意**不**用「含 Tab 就当 CSV」这条规则：`word\t中文释义` 这种
    // 两列词表（考研、高中、初中公开词表都是这个格式）并没有表头，
    // 一旦当成 CSV，首行会被当成表头吃掉，且因为列名对不上而丢掉释义。
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    if looks_like_header(first) {
        return parse_csv(text, m, lang);
    }
    parse_text(text, m, lang)
}

/// 判断首行是否是「表头」：把逗号/制表符/竖线切开的字段，
/// 逐个与已知候选列名比对，命中一半以上即认定是表头。
fn looks_like_header(line: &str) -> bool {
    let seps = [',', '\t', '|'];
    let Some(sep) = seps.iter().find(|s| line.contains(**s)) else {
        return false;
    };
    let cols: Vec<String> = line
        .split(*sep)
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    if cols.len() < 2 {
        return false;
    }
    // 已知列名（与 ImportMapping 默认候选保持一致）
    const KNOWN: &[&str] = &[
        "word", "单词", "headword", "head_word", "term", "en", "name", "词条",
        "expression", "reading", "kana", "romaji",
        "translation", "trans", "meaning", "meanings", "meaning_cn", "中文", "释义", "gloss", "释义_cn",
        "definition", "def", "english", "explain", "explain_en",
        "phonetic", "usphone", "ukphone", "音标", "pron", "pronunciation",
        "pos", "example", "examples", "sentence", "例句", "tag", "tags", "category", "level", "标签", "分类", "book",
    ];
    let hit = cols
        .iter()
        .filter(|c| KNOWN.iter().any(|k| c == k))
        .count();
    // 至少 1 列命中，且命中数不少于列数的一半
    hit >= 1 && hit * 2 >= cols.len()
}

/// 内置的「可下载词库」目录（需求 5）。
///
/// 这份目录修过两个致命问题：
///
/// 1. **全部指向 raw.githubusercontent.com** —— 国内直连必然失败，
///    这就是「开着代理也下不动」的直接原因。现在改为「jsDelivr CDN 优先」
///    （国内可直连、有缓存加速），gh-proxy 与 GitHub 原文作为回退。
/// 2. **部分文件名根本不存在** —— 原先的 KAOYAN_edited.txt / IELTS_edited.txt /
///    TOEFL_edited.txt / GRE_edited.txt / GAOKAO_edited.txt 在源仓库里都是 404，
///    点一次错一次。现在全部换成仓库里真实存在、且解析器能正确识别的文件。
///
/// 另：ECDICT（63MB 的 ecdict.csv）已从目录移除。它超过 jsDelivr 的单文件上限，
/// 下载耗时以分钟计，对背单词场景收益很低，保留它只会制造一次必然失败的体验。
///
/// 目录里的每一项都带 `lang`（词库本身的语言），下载时按它入库，
/// 而不是按「用户当前在学的语言」——否则在学英语时下载日语词库，
/// 整本词库会被标成英语，搜索和背诵都会串味。
/// 目前覆盖：英语（四级 ~ GRE / 雅思 / 牛津 / COCA）与日语（JLPT N5~N1）。
pub fn remote_catalog() -> Vec<RemoteBook> {
    // jsDelivr 的多个后端。
    //
    // jsDelivr 有若干个 CNAME 指向**不同的 CDN 供应商**（Fastly / Gcore /
    // Cloudflare），因此它们是彼此的独立故障域——一个后端的连接被干扰，
    // 另外两个仍可能正常。这比「换一个第三方 GitHub 代理」有用得多：
    // 实测（2026-10，直连）所有走 raw.githubusercontent 透传的代理
    // （gh-proxy.com / ghproxy.net / ghfast.top / hub.gitmirror.com）
    // **全部 0/3 失败**——它们自己也要去 fetch 被墙的 raw，于是把失败一起继承过来。
    // 而 jsDelivr 是从自家缓存分发，实测 fastly 3/3、cdn 2/3、gcore 2/3。
    //
    // 顺序：先用最标准的 cdn，再依次回退到另外两个后端。
    // 三者的实测成功率差异在 n=3 的样本下不具统计意义，真正的收益来自
    // 「多个独立后端 + 失败后重试」（见 cmd_download_book 的两轮扫描）。
    const JSDELIVR_HOSTS: [&str; 3] = [
        "cdn.jsdelivr.net",
        "fastly.jsdelivr.net",
        "gcore.jsdelivr.net",
    ];

    // 仓库写成 `owner/name@分支`；jsDelivr / raw 都按分支取文件。
    //
    // 为什么要显式带分支：这两个仓库默认的都是 `master`，但多语言词库
    // （OpenJLPT 等）的默认分支是 `main`。写死 `@master` 会让整份日语词库
    // 全部 404 —— 表现就是「点了下载半天没动静，最后报下载失败」。
    fn split_repo(repo: &str) -> (&str, &str) {
        match repo.split_once('@') {
            Some((r, b)) => (r, b),
            None => (repo, "master"),
        }
    }

    // 指定 jsDelivr 后端；文件名需百分号编码（含中文/空格的路径）
    fn jsd_at(host: &str, repo: &str, path: &str) -> String {
        let (r, b) = split_repo(repo);
        format!(
            "https://{}/gh/{}@{}/{}",
            host,
            r,
            b,
            urlencoding::encode(path)
        )
    }
    // gh-proxy：对 raw 的完整透传镜像，没有单文件大小限制。
    // 直连时基本不可用（见上），但对**开了代理**的用户是个有效补充。
    fn ghproxy(repo: &str, path: &str) -> String {
        let (r, b) = split_repo(repo);
        format!(
            "https://gh-proxy.com/https://raw.githubusercontent.com/{}/{}/{}",
            r,
            b,
            urlencoding::encode(path)
        )
    }
    fn raw(repo: &str, path: &str) -> String {
        let (r, b) = split_repo(repo);
        format!(
            "https://raw.githubusercontent.com/{}/{}/{}",
            r,
            b,
            urlencoding::encode(path)
        )
    }

    // 生成一条目录项：主地址用 jsDelivr 主后端，回退顺序为
    // 其余 jsDelivr 后端 → gh-proxy → GitHub 原文。
    //
    // `lang` 是该词库**本身**的语言（en / ja …）。下载时以它为准写入词条语言，
    // 不能用「用户当前在学的语言」：否则在学英语时导入日语词库，
    // 7800 个日语词会被当成英语词入库，搜索与背诵全都串味。
    let mk = |id: &str,
              name: &str,
              cat: &str,
              desc: &str,
              repo: &str,
              path: &str,
              fmt: &str,
              words: i64,
              lang: &str,
              src: &str,
              lic: &str| RemoteBook {
        id: id.into(),
        name: name.into(),
        category: cat.into(),
        description: desc.into(),
        lang: lang.into(),
        url: jsd_at(JSDELIVR_HOSTS[0], repo, path),
        mirrors: vec![
            jsd_at(JSDELIVR_HOSTS[1], repo, path),
            jsd_at(JSDELIVR_HOSTS[2], repo, path),
            ghproxy(repo, path),
            raw(repo, path),
        ],
        format: fmt.into(),
        approx_words: words,
        source_url: src.into(),
        license: lic.into(),
        installed: false,
    };

    const MAHA: &str = "mahavivo/english-wordlists@master";
    const KYLE: &str = "KyleBing/english-vocabulary@master";
    // OpenJLPT：JLPT N5~N1 词汇，CC-BY-SA-4.0，默认分支是 main
    const OPENJLPT: &str = "evanclan/OpenJLPT@main";

    vec![
        mk(
            "cet4-core", "四级核心词汇", "cet4",
            "大学英语四级大纲词汇，含音标与中文释义",
            MAHA, "CET4_edited.txt", "txt", 4615, "en",
            "https://github.com/mahavivo/english-wordlists", "MIT",
        ),
        mk(
            "cet6-core", "六级核心词汇", "cet6",
            "大学英语六级大纲词汇，含音标与中文释义",
            MAHA, "CET6_edited.txt", "txt", 2300, "en",
            "https://github.com/mahavivo/english-wordlists", "MIT",
        ),
        mk(
            "cet46-common", "四六级高频核心（合并）", "cet4",
            "四六级共用高频词，只有词条，适合快速过一遍",
            MAHA, "CET_4+6_edited.txt", "txt", 2200, "en",
            "https://github.com/mahavivo/english-wordlists", "MIT",
        ),
        mk(
            "kaoyan-core", "考研核心词汇", "kaoyan",
            "考研英语大纲词汇，含中文释义",
            KYLE, "5 考研-乱序.txt", "txt", 5500, "en",
            "https://github.com/KyleBing/english-vocabulary", "见源仓库",
        ),
        mk(
            "toefl-core", "托福核心词汇", "toefl",
            "TOEFL 高频词汇，含音标与中文释义",
            MAHA, "TOEFL.txt", "txt", 4200, "en",
            "https://github.com/mahavivo/english-wordlists", "MIT",
        ),
        mk(
            "gre-core", "GRE 核心词汇", "gre",
            "GRE 高频词汇，含音标与中文释义",
            MAHA, "GRE_8000_Words.txt", "txt", 8000, "en",
            "https://github.com/mahavivo/english-wordlists", "MIT",
        ),
        mk(
            "gaokao-core", "高中 / 高考核心词汇", "gaokao",
            "高中英语大纲词汇（纯词表，适合快速自测）",
            MAHA, "Highschool_edited.txt", "txt", 3500, "en",
            "https://github.com/mahavivo/english-wordlists", "MIT",
        ),
        mk(
            "gaokao-full", "高中词汇（含释义）", "gaokao",
            "高中词汇乱序版，每个词都带中文释义",
            KYLE, "2 高中-乱序.txt", "txt", 3500, "en",
            "https://github.com/KyleBing/english-vocabulary", "见源仓库",
        ),
        mk(
            "zhongkao-core", "初中 / 中考词汇", "other",
            "初中英语大纲词汇，含中文释义",
            KYLE, "1 初中-乱序.txt", "txt", 1600, "en",
            "https://github.com/KyleBing/english-vocabulary", "见源仓库",
        ),
        mk(
            "oald8-core", "牛津高阶英汉（精选）",
            "other",
            "Oxford Advanced Learner's Dictionary 精简版，英英释义 + 中文对照",
            MAHA, "OALD8_abridged_edited.txt", "txt", 30000, "en",
            "https://github.com/mahavivo/english-wordlists", "MIT",
        ),
        mk(
            "coca-common", "COCA 高频常用词", "other",
            "美国当代英语语料库高频词，含音标与词性",
            MAHA, "COCA_abridged.txt", "txt", 20000, "en",
            "https://github.com/mahavivo/english-wordlists", "MIT",
        ),
        mk(
            "tem48-core", "英语专业四八级词汇", "other",
            "英语专业四级 / 八级大纲词汇",
            MAHA, "英语专业四八级词汇表.txt", "txt", 12000, "en",
            "https://github.com/mahavivo/english-wordlists", "MIT",
        ),

        /* ---- 以下为后续补录：覆盖原本空缺的雅思 / SAT，并新增日语 JLPT ---- */

        mk(
            "kaoyan-npee", "考研大纲词汇（NPEE）", "kaoyan",
            "全国硕士研究生入学考试英语大纲词表，含音标与中文释义",
            MAHA, "NPEE_Wordlist.txt", "txt", 5400, "en",
            "https://github.com/mahavivo/english-wordlists", "MIT",
        ),
        mk(
            "ielts-core", "雅思核心词汇", "ielts",
            "IELTS 高频词汇，乱序排列，含中文释义与同义词",
            KYLE, "full_line_tsv/full/乱序/雅思.txt", "txt", 3400, "en",
            "https://github.com/KyleBing/english-vocabulary", "见源仓库",
        ),
        mk(
            "cet4-shuffle", "四级词汇（乱序 · 含释义）", "cet4",
            "四级大纲词乱序版，每词带中文释义，适合打乱顺序自测",
            KYLE, "3 四级-乱序.txt", "txt", 7500, "en",
            "https://github.com/KyleBing/english-vocabulary", "见源仓库",
        ),
        mk(
            "cet6-shuffle", "六级词汇（乱序 · 含释义）", "cet6",
            "六级大纲词乱序版，每词带中文释义",
            KYLE, "4 六级-乱序.txt", "txt", 5600, "en",
            "https://github.com/KyleBing/english-vocabulary", "见源仓库",
        ),
        mk(
            "gre-abridged", "GRE 高频精简版", "gre",
            "GRE 常考词精简版，含音标与中文释义，适合时间不多时先过一遍",
            MAHA, "GRE_abridged.txt", "txt", 4300, "en",
            "https://github.com/mahavivo/english-wordlists", "MIT",
        ),
        mk(
            "gre-hongbaoshu", "GRE 红宝书（含释义）", "gre",
            "《GRE 词汇精选》红宝书词表，两列 CSV：单词 + 中文释义",
            MAHA, "红宝书 GRE词汇精选.csv", "csv", 6200, "en",
            "https://github.com/mahavivo/english-wordlists", "MIT",
        ),
        mk(
            "sat-core", "SAT 词汇", "other",
            "SAT 常考词汇，含中文释义",
            KYLE, "7 SAT-乱序.txt", "txt", 8800, "en",
            "https://github.com/KyleBing/english-vocabulary", "见源仓库",
        ),
        mk(
            "primary-en", "小学英语大纲词汇", "other",
            "小学阶段英语大纲词表，词量很小，适合入门或给孩子用",
            MAHA, "小学英语大纲词汇.txt", "txt", 440, "en",
            "https://github.com/mahavivo/english-wordlists", "MIT",
        ),
        mk(
            "tw-highschool", "台湾高中英文参考词汇", "other",
            "台湾高中英文参考词汇表（學測 4000 字 + 指考 7000 字），繁体释义",
            MAHA, "台灣高中英文參考詞彙表.txt", "txt", 5800, "en",
            "https://github.com/mahavivo/english-wordlists", "MIT",
        ),

        /* ---- 日语：JLPT N5 → N1 ----
           来源 OpenJLPT（CC-BY-SA-4.0），JSON 结构：
           word / reading / romaji / meanings[] / pos[] / level / examples[]。
           这是目录里第一批非英语词库，下载时会按 lang=ja 入库。 */
        mk(
            "jlpt-n5", "JLPT N5 词汇", "jlpt",
            "日语能力考试 N5 词汇（最入门），含假名读音与英文释义",
            OPENJLPT, "data/json/vocab/n5.json", "json", 675, "ja",
            "https://github.com/evanclan/OpenJLPT", "CC-BY-SA-4.0",
        ),
        mk(
            "jlpt-n4", "JLPT N4 词汇", "jlpt",
            "日语能力考试 N4 词汇，含假名读音与英文释义",
            OPENJLPT, "data/json/vocab/n4.json", "json", 630, "ja",
            "https://github.com/evanclan/OpenJLPT", "CC-BY-SA-4.0",
        ),
        mk(
            "jlpt-n3", "JLPT N3 词汇", "jlpt",
            "日语能力考试 N3 词汇（初中级过渡），含假名读音与英文释义",
            OPENJLPT, "data/json/vocab/n3.json", "json", 1660, "ja",
            "https://github.com/evanclan/OpenJLPT", "CC-BY-SA-4.0",
        ),
        mk(
            "jlpt-n2", "JLPT N2 词汇", "jlpt",
            "日语能力考试 N2 词汇（中高级），含假名读音与英文释义",
            OPENJLPT, "data/json/vocab/n2.json", "json", 1780, "ja",
            "https://github.com/evanclan/OpenJLPT", "CC-BY-SA-4.0",
        ),
        mk(
            "jlpt-n1", "JLPT N1 词汇", "jlpt",
            "日语能力考试 N1 词汇（最高级），含假名读音与英文释义",
            OPENJLPT, "data/json/vocab/n1.json", "json", 3070, "ja",
            "https://github.com/evanclan/OpenJLPT", "CC-BY-SA-4.0",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_line_variants() {
        assert_eq!(parse_text_line("apple", "en").unwrap().0, "apple");
        let (w, _, g, _) = parse_text_line("apple\t苹果\t/ˈæp.əl/", "en").unwrap();
        assert_eq!(w, "apple");
        assert_eq!(g, "苹果");
        assert_eq!(parse_text_line("# comment", "en"), None);
        assert_eq!(parse_text_line("   ", "en"), None);
    }

    /// 公开词表最常见的两种排版都要能拆出「词 / 音标 / 释义」。
    #[test]
    fn text_line_real_wordlists() {
        // 四级：单空格 + 音标 + 词性.释义
        let (w, p, g, _) =
            parse_text_line("abandon [əˈbændən] vt.丢弃；放弃，抛弃", "en").unwrap();
        assert_eq!(w, "abandon");
        assert_eq!(p, "[əˈbændən]");
        assert_eq!(g, "vt.丢弃；放弃，抛弃");

        // 托福：多空格对齐排版；`vt.  放弃` 里的双空格不能把释义切断
        let (w, p, g, _) =
            parse_text_line("abandon           [ə'bændən]            vt.  放弃,沉溺", "en").unwrap();
        assert_eq!(w, "abandon");
        assert_eq!(p, "[ə'bændən]");
        assert_eq!(g, "vt. 放弃,沉溺");

        // 考研：制表符 + 释义
        let (w, p, g, _) = parse_text_line("revolt\tn. 反抗；造反，起义 v. 起义", "en").unwrap();
        assert_eq!(w, "revolt");
        assert!(p.is_empty());
        assert_eq!(g, "n. 反抗；造反，起义 v. 起义");

        // 牛津：音标后接词性而非缩写（无多余空格）
        let (w, p, g, _) =
            parse_text_line("aardvark [ˈɑːrdvɑːrk] noun 土豚（非洲食蟻獸）", "en").unwrap();
        assert_eq!(w, "aardvark");
        assert_eq!(p, "[ˈɑːrdvɑːrk]");
        assert_eq!(g, "noun 土豚（非洲食蟻獸）");

        // COCA：空音标块要忽略，别把 "[]" 当成音标
        let (w, p, g, _) =
            parse_text_line("others             []                   pron. 其他人", "en").unwrap();
        assert_eq!(w, "others");
        assert!(p.is_empty());
        assert_eq!(g, "pron. 其他人");

        // 中考：词后有括号补充说明
        let (w, p, g, _) =
            parse_text_line("a (an) [ə, eɪ(ən)] art. 一（个、件……）", "en").unwrap();
        assert_eq!(w, "a");
        assert_eq!(p, "[ə, eɪ(ən)]");
        assert_eq!(g, "art. 一（个、件……）");
    }

    /// 雅思 TSV 把「词性 / 释义 / 同义词」挤进同一列
    /// （`adj::可感觉的；明智的::reasonable, praiseworthy`），
    /// 释义必须只取中文那一段，不能连音标和同义词一起塞进去。
    #[test]
    fn ielts_double_colon_gloss() {
        let (w, _p, g, _) = parse_text_line(
            "sensible\t'sɛnsəbl\t'sensɪb(ə)l\tadj::可感觉的；明智的::reasonable, praiseworthy",
            "en",
        )
        .unwrap();
        assert_eq!(w, "sensible");
        assert_eq!(g, "可感觉的；明智的", "释义不该带音标/同义词：{}", g);
    }

    /// 词表里的噪声行不能变成单词。
    #[test]
    fn text_line_rejects_noise() {
        // CET4 的 BOM 标题行
        assert_eq!(parse_text_line("\u{feff}大学英语四级大纲单词表", "en"), None);
        // 说明行
        assert_eq!(parse_text_line("(共 4615 词)", "en"), None);
        // 分节字母
        assert_eq!(parse_text_line("A", "en"), None);
        // 但小写 a 是正经单词
        assert_eq!(parse_text_line("a", "en").unwrap().0, "a");
        // 分隔线
        assert_eq!(parse_text_line("————————————————", "en"), None);
    }

    #[test]
    fn cet4_sample_parses_cleanly() {
        let txt = "\u{feff}大学英语四级大纲单词表\n(共 4615 词)\n\nA\n\na art.一(个)；每一(个)\nabandon [əˈbændən] vt.丢弃；放弃，抛弃\nability [əˈbiliti] n.能力；能耐，本领\n";
        let e = parse_text(txt, &ImportMapping::default(), "en").unwrap();
        assert_eq!(e.len(), 3, "标题行/说明行/分节字母都不该成为词条");
        assert_eq!(e[0].word, "a");
        assert_eq!(e[1].word, "abandon");
        // 音标要被抽出来，且不能残留在释义里
        assert_eq!(e[1].phonetic.uk, "[əˈbændən]");
        // 释义里的分号会被拆成多个义项，这是刻意设计（便于逐条展示）
        let defs: Vec<&str> = e[1].senses.iter().map(|s| s.definition.as_str()).collect();
        assert_eq!(defs, vec!["vt.丢弃", "放弃，抛弃"]);
    }

    #[test]
    fn csv_quoted_field_with_comma() {
        let txt = "word,phonetic,translation\nabandon,[əˈbændən],\"v. 放弃,抛弃\"\n";
        let e = parse_csv(txt, &ImportMapping::default(), "en").unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].word, "abandon");
        assert_eq!(e[0].senses[0].definition, "v. 放弃,抛弃");
    }

    #[test]
    fn quoted_splitter_handles_escaped_quotes() {
        let f = split_quoted(r#"a,"b,1",c"#, ',');
        assert_eq!(f, vec!["a", "b,1", "c"]);
        let f = split_quoted(r#""say ""hi""",z"#, ',');
        assert_eq!(f, vec![r#"say "hi""#, "z"]);
    }

    #[test]
    fn json_array_of_strings() {
        let e = parse_json(r#"["apple","banana"]"#, &ImportMapping::default(), "en").unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].word, "apple");
    }

    #[test]
    fn json_object_array() {
        let txt = r#"[{"word":"apple","translation":"苹果","phonetic":"/ˈæp.əl/"}]"#;
        let e = parse_json(txt, &ImportMapping::default(), "en").unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].senses[0].definition, "苹果");
        assert_eq!(e[0].phonetic.uk, "/ˈæp.əl/");
    }

    #[test]
    fn json_wrapped_and_dict_form() {
        let e = parse_json(r#"{"words":[{"word":"go"}]}"#, &ImportMapping::default(), "en").unwrap();
        assert_eq!(e.len(), 1);
        let d = parse_json(r#"{"apple":"苹果","go":"去"}"#, &ImportMapping::default(), "en").unwrap();
        assert_eq!(d.len(), 2);
    }

    #[test]
    fn csv_with_header() {
        let txt = "word,translation\napple,苹果\nbanana,香蕉\n";
        let e = parse_csv(txt, &ImportMapping::default(), "en").unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!(e[1].senses[0].definition, "香蕉");
    }

    #[test]
    fn text_plain() {
        let e = parse_text("apple\nbanana\ncherry\n", &ImportMapping::default(), "en").unwrap();
        assert_eq!(e.len(), 3);
    }

    #[test]
    fn auto_detect() {
        assert_eq!(parse_auto("[{\"word\":\"a\"}]", &ImportMapping::default(), "en", "").unwrap().len(), 1);
        assert_eq!(parse_auto("word,trans\nx,y\n", &ImportMapping::default(), "en", "").unwrap().len(), 1);
        assert_eq!(parse_auto("x\ny\n", &ImportMapping::default(), "en", "").unwrap().len(), 2);
    }

    /// 针对**真实公开词表**的冒烟验证。
    ///
    /// 单元测试只能覆盖「我抄下来的那几行」，这个测试直接吃原始文件，
    /// 能发现「真实排版比想象中更脏」这类问题。
    /// 样本需先手动下载到某目录（见 `docs/DICT_SOURCES.md`），然后：
    ///   WORDWISE_SAMPLES=<目录> cargo test --lib
    #[test]
    fn real_wordlists_smoke() {
        let Ok(dir) = std::env::var("WORDWISE_SAMPLES") else {
            return; // 没准备样本就跳过，不影响常规测试
        };
        let cases: &[(&str, usize)] = &[
            ("cet4.txt", 4000),
            ("toefl.txt", 3000),
            ("ky.txt", 8000),
            ("coca.txt", 4000),
            ("oald.txt", 9000),
        ];
        for (file, expect_min) in cases {
            let path = std::path::Path::new(&dir).join(file);
            let Ok(text) = std::fs::read_to_string(&path) else {
                panic!("缺少样本文件：{}", path.display());
            };
            let entries = parse_auto(&text, &ImportMapping::default(), "en", "txt").unwrap();
            assert!(
                entries.len() >= *expect_min,
                "{} 只解析出 {} 条（期望至少 {}）",
                file,
                entries.len(),
                expect_min
            );
            for e in &entries {
                assert!(looks_like_word(&e.word, "en"), "{} 出现非法词条：{:?}", file, e.word);
            }
            // 释义覆盖率：公开词表里绝大多数词都带释义，太少说明字段切错了
            let with_def = entries.iter().filter(|e| !e.senses.is_empty()).count();
            assert!(
                with_def * 10 >= entries.len() * 9,
                "{} 释义覆盖率过低：{}/{}",
                file,
                with_def,
                entries.len()
            );
        }
    }

    #[test]
    fn catalog_is_wellformed() {
        let c = remote_catalog();
        assert!(c.len() >= 5);
        let mut ids: Vec<&str> = c.iter().map(|b| b.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), c.len(), "词库 id 必须唯一");
        for b in &c {
            assert!(!b.id.is_empty() && !b.name.is_empty());
            assert!(b.url.starts_with("https://"), "主地址必须是 https：{}", b.url);
            // 国内直连 raw.githubusercontent 必然失败，所以一定要有备用镜像
            assert!(!b.mirrors.is_empty(), "词库 {} 缺少备用镜像", b.id);
            for m in &b.mirrors {
                assert!(m.starts_with("https://"));
            }
            assert!(!b.source_url.is_empty() && !b.license.is_empty());
        }
    }

    /// 词库下载必须有多条**国内可直连**的链路。
    ///
    /// 这条测试守的是用户最初报的「网络问题不能下载」：
    /// 只要主地址挂在被墙的域名上、或者备用镜像只有一条，就很容易复现。
    #[test]
    fn catalog_mirrors_are_cn_reachable_and_independent() {
        for b in remote_catalog() {
            let all: Vec<&str> = std::iter::once(b.url.as_str())
                .chain(b.mirrors.iter().map(|s| s.as_str()))
                .collect();

            // 主地址不能是被墙的 raw
            assert!(
                !b.url.contains("raw.githubusercontent.com"),
                "词库 {} 的主地址指向了国内不可直连的 raw：{}",
                b.id,
                b.url
            );

            // 经 jsDelivr 分发的链路至少要覆盖 3 个不同后端
            // （cdn / fastly / gcore 是三家不同 CDN，互为独立故障域）
            let hosts: std::collections::BTreeSet<String> = all
                .iter()
                .filter_map(|u| u.split('/').nth(2).map(|h| h.to_string()))
                .collect();
            let jsd_hosts: Vec<&String> =
                hosts.iter().filter(|h| h.ends_with("jsdelivr.net")).collect();
            assert!(
                jsd_hosts.len() >= 3,
                "词库 {} 的 jsDelivr 后端只有 {} 个，故障域太少：{:?}",
                b.id,
                jsd_hosts.len(),
                jsd_hosts
            );

            // 三条 jsDelivr 链路的路径必须完全一致，否则会下到不同文件
            let paths: std::collections::BTreeSet<String> = all
                .iter()
                .filter(|u| u.contains("jsdelivr.net"))
                .filter_map(|u| u.split_once("/gh/").map(|(_, p)| p.to_string()))
                .collect();
            assert_eq!(
                paths.len(),
                1,
                "词库 {} 的各 jsDelivr 镜像路径不一致：{:?}",
                b.id,
                paths
            );
        }
    }
}

#[cfg(test)]
mod catalog_tests {
    use super::*;

    /// 无表头的两列 CSV（红宝书 GRE / 自制词表）不能被当成「有表头」，
    /// 否则第一个单词会被吃成列名、整份文件的释义全丢。
    #[test]
    fn headerless_two_column_csv_keeps_first_row() {
        let text = "abandon,v./n.放弃；放纵\nabash,v.使害羞，使尴尬\nabate,v.减轻，减少\n";
        let m = ImportMapping::default();
        let got = parse_csv(text, &m, "en").unwrap();
        assert_eq!(got.len(), 3, "首行不该被当成表头丢掉");
        assert_eq!(got[0].word, "abandon");
        assert!(
            got[0].senses.iter().any(|s| s.definition.contains("放弃")),
            "首行的释义要保留下来，实际：{:?}",
            got[0].senses
        );
    }

    /// 有表头的 CSV 仍要正常识别（表头不能被当成单词导入）。
    #[test]
    fn real_header_csv_is_detected() {
        let text = "word,translation,phonetic\napple,苹果,/ˈæp.əl/\nbanana,香蕉,\n";
        let m = ImportMapping::default();
        let got = parse_csv(text, &m, "en").unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].word, "apple");
        assert!(got.iter().all(|e| e.word != "word"), "表头不能被当成单词");
    }

    /// Anki 系（JLPT）的表头 expression / reading / meaning 要能被识别。
    #[test]
    fn anki_style_header_is_detected() {
        let text = "expression,reading,meaning,tags,guid\n会う,あう,\"to meet, to see\",JLPT,x1\n青,あお,blue,JLPT,x2\n";
        let m = ImportMapping::default();
        let got = parse_csv(text, &m, "ja").unwrap();
        assert_eq!(got.len(), 2, "表头行不该被当成词条");
        assert_eq!(got[0].word, "会う");
        assert_eq!(got[0].phonetic.uk, "あう", "reading 应作为读音");
        assert!(got[0].senses.iter().any(|s| s.definition.contains("to meet")));
    }

    /// OpenJLPT 的 JSON 结构：word / reading / meanings / pos 都是数组或嵌套。
    #[test]
    fn openjlpt_json_shape() {
        let text = r#"[
          {
            "id": "ada066edfd",
            "word": "会う",
            "reading": "あう",
            "romaji": "au",
            "meanings": ["to meet"],
            "level": "N5",
            "pos": ["v5u", "vi"],
            "jmdict_id": 1198180
          }
        ]"#;
        let m = ImportMapping::default();
        let got = parse_auto(text, &m, "ja", "json").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].word, "会う");
        assert_eq!(got[0].lang, "ja");
        assert_eq!(got[0].phonetic.uk, "あう");
        assert!(got[0].senses.iter().any(|s| s.definition.contains("to meet")));
    }

    /// OpenJLPT 的 `meanings` 是数组：每个义项都要保留，
    /// 只取第一项的话用户会以为这个词只有一个意思。
    #[test]
    fn openjlpt_meanings_are_all_kept() {
        let text = r#"[{"word":"会う","reading":"あう","meanings":["to meet","to see"]}]"#;
        let got = parse_auto(text, &ImportMapping::default(), "ja", "json").unwrap();
        let defs: Vec<&str> = got[0].senses.iter().map(|s| s.definition.as_str()).collect();
        assert_eq!(defs, vec!["to meet", "to see"], "义项被截断：{:?}", defs);
    }

    /// 目录必须给每本词库标注语言，而且要真的有非英语词库 ——
    /// 「支持多语言」不能只停留在设置项里。
    #[test]
    fn catalog_declares_language_and_has_japanese() {
        let c = remote_catalog();
        for b in &c {
            assert!(!b.lang.is_empty(), "词库 {} 没有标注语言", b.id);
            assert!(
                matches!(b.lang.as_str(), "en" | "ja"),
                "词库 {} 的语言超出了前端支持范围：{}",
                b.id,
                b.lang
            );
        }
        let jp: Vec<&RemoteBook> = c.iter().filter(|b| b.lang == "ja").collect();
        assert!(jp.len() >= 5, "日语词库太少（应覆盖 N5~N1）：{}", jp.len());
        for b in jp {
            assert_eq!(b.category, "jlpt", "日语词库 {} 的分类不对", b.id);
            // OpenJLPT 的默认分支是 main，写成 master 会整片 404
            assert!(b.url.contains("@main/"), "分支写错：{}", b.url);
            for m in &b.mirrors {
                if m.contains("jsdelivr.net") {
                    assert!(m.contains("@main/"), "镜像分支写错：{}", m);
                }
            }
        }
        // 英语词库仍在 master 分支上（老仓库）
        for b in c.iter().filter(|b| b.lang == "en") {
            assert!(b.url.contains("@master/"), "分支写错：{}", b.url);
        }
    }

    /// 日语词要能通过 looks_like_word（不能像英语那样把 CJK 全判成噪声）。
    #[test]
    fn japanese_words_survive_word_shaping() {
        assert_eq!(parse_text_line("ああ\t啊\t", "ja").map(|x| x.0), Some("ああ".into()));
        assert_eq!(parse_text_line("会う\tto meet", "ja").map(|x| x.0), Some("会う".into()));
        // 英语词表里出现 CJK 仍然算噪声
        assert_eq!(parse_text_line("ああ\t啊", "en"), None);
    }
}
