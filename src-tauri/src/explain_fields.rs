//! AI 讲解「六栏」→ 词条结构化字段的**确定性**提取与合并。
//!
//! ## 为什么要有这个模块
//!
//! 用户报的两件事，根子都在「并入词库」这条链路上：
//!
//!   1. **并入变成覆盖** —— `cmd_explain_to_entry` 原来是整条 `upsert_word`，
//!      用户已有的核心含义 / 记忆方法 / 搭配被一股脑冲掉；
//!   2. **例句丢了** —— 例句挂在哪条义项全凭模型心情，模型漏挂一条，
//!      界面就显示「暂时没有例句」。
//!
//! 所以这里做两件事，而且**都不把正确性押在模型身上**：
//!
//!   - **例句自己解析**：讲解 markdown 的「## 例句」栏目格式是提示词写死的
//!     （原句一行 + 下一行 `> ` 译文），可以直接确定性地抠出来做兜底池；
//!   - **分栏合并**：六栏各有各的规矩 —— 核心含义替换，其余五栏追加。
//!
//! ## 合并规则（用户需求原文逐条对应）
//!
//! | 栏目 | 落点 | 新数据可用 | 新数据为空 |
//! |---|---|---|---|
//! | 核心含义 | `senses` | 替换（但旧例句要捞回来） | 保留原数据 |
//! | 考点 | `exam_points` | 追加（去重） | 保留原数据 |
//! | 记忆方法 | `mnemonic` | 追加 | 保留原数据 |
//! | 常见搭配 | `collocations` | 追加（去重） | 保留原数据 |
//! | 例句 | `senses[].examples` | 按释义匹配后追加 | 保留原数据 |
//! | 易混辨析 | `confusables` | 追加（去重） | 保留原数据 |
//!
//! 「追加」一律带去重：同一个词反复并入，不能让记忆方法一段段堆成裹脚布。

use crate::models::{Collocation, Confusable, Example, Sense, WordEntry};

/// 归一化：小写 + 去首尾空白 + 压缩内部空白。用于去重与释义比对，
/// 免得模型多写一个空格就被判成两条。
fn norm(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

// ============================================================
// 一、从讲解 markdown 里确定性抠出例句与考点
// ============================================================

/// 「例句」栏目的几种标题写法。
///
/// 人设提示词迭代过（`llm/mod.rs` 里还有一份要求「例句与翻译」的老文案），
/// 逐份去对齐成本太高，这里统一兜住。
const EXAMPLE_HEADINGS: [&str; 3] = ["例句", "例句与翻译", "例句和翻译"];

/// 取讲解正文里某个二级标题下的内容（不含标题行本身）。
///
/// 遇到下一个 `##` / `#` 标题或文末就停。
fn section_body(markdown: &str, titles: &[&str]) -> Option<String> {
    let mut lines = markdown.lines().peekable();
    while let Some(line) = lines.next() {
        let t = line.trim();
        if !t.starts_with('#') {
            continue;
        }
        let heading = t.trim_start_matches('#').trim();
        if !titles.iter().any(|x| *x == heading) {
            continue;
        }
        let mut body = String::new();
        while let Some(next) = lines.peek() {
            let n = next.trim_end();
            if n.starts_with('#') {
                break;
            }
            body.push_str(n);
            body.push('\n');
            lines.next();
        }
        return Some(body);
    }
    None
}

/// 解析讲解正文「## 例句」栏，得到例句池。
///
/// 格式是提示词写死的：每条原句单独一行（允许 `- ` / `1. ` 前缀），
/// 紧随其后的下一行以 `> ` 开头作为译文。所以这里**不猜**：按行读，
/// 读到 `>` 就挂给上一条原句，读非空行就新起一条。
///
/// 解析不出来返回空 `Vec` —— 调用方会当作「没有兜底例句」，不会影响主流程。
pub fn parse_example_pool(markdown: &str) -> Vec<Example> {
    let body = match section_body(markdown, &EXAMPLE_HEADINGS) {
        Some(b) => b,
        None => return Vec::new(),
    };
    let mut out: Vec<Example> = Vec::new();
    for raw in body.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(trans) = line.strip_prefix('>') {
            // 译文行：挂给上一条还没译文的例句
            let t = trans.trim().to_string();
            if t.is_empty() {
                continue;
            }
            if let Some(last) = out.last_mut() {
                if last.translation.is_empty() {
                    last.translation = t;
                } else {
                    // 连续两行译文（模型偶尔会给双译）：拼起来比丢掉好
                    last.translation = format!("{} {}", last.translation, t);
                }
            }
            continue;
        }
        // 去掉列表前缀
        let mut text = line;
        for prefix in ["- ", "* ", "+ "] {
            if let Some(rest) = text.strip_prefix(prefix) {
                text = rest;
                break;
            }
        }
        // `1. ` 这类有序列表前缀
        if let Some(dot) = text.find(". ") {
            let head = &text[..dot];
            if !head.is_empty() && head.chars().all(|c| c.is_ascii_digit()) {
                text = &text[dot + 2..];
            }
        }
        let text = text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        out.push(Example { text, translation: String::new() });
    }
    out.retain(|e| !e.text.trim().is_empty());
    out
}

/// 解析「考点」标注。
///
/// 提示词要求写在核心含义末尾：`- *考点*：四级`。这种行格式固定，
/// 确定性解析比让模型再输出一遍 JSON 字段可靠得多 —— 尤其在本地小模型上。
///
/// 同时兼容几种常见写法：`*考点*：四级` / `考点：四级` / `【考点】四级`。
pub fn parse_exam_points(markdown: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in markdown.lines() {
        let line = raw.trim();
        let value = if let Some(rest) = line.split_once("：").or_else(|| line.split_once(": ")) {
            let (head, tail) = rest;
            let head = head.trim().trim_matches('*').trim_matches('`').trim();
            if head.contains("考点") {
                Some(tail.trim())
            } else {
                None
            }
        } else if let Some(idx) = line.find("考点") {
            // `【考点】四级` 这类没有冒号的写法
            let after = &line[idx + "考点".len()..];
            let after = after.trim_start_matches(['】', ']', '）', ')', '》', '>']).trim();
            if after.is_empty() {
                None
            } else {
                Some(after)
            }
        } else {
            None
        };
        let Some(value) = value else { continue };
        let value = value.trim_matches(['*', '`', '。', '，', ',']).trim();
        if value.is_empty() || value.len() > 30 {
            continue;
        }
        for part in value.split(['、', '/', '，', ',']) {
            let p = part.trim().to_string();
            if p.is_empty() {
                continue;
            }
            if !out.iter().any(|x| x == &p) {
                out.push(p);
            }
        }
    }
    out
}

// ============================================================
// 二、例句 ↔ 释义 的匹配与分发
// ============================================================

/// 在本来就有的义项里，按「释义」找最匹配的那一条。
///
/// 顺序：
///   1. 释义归一化后完全相等（最强证据）；
///   2. 释义去掉 punctuation 后相等；
///   3. 词性相同且其中一方的释义被另一方包含（模型常把长释义截短）。
fn find_sense<'a>(senses: &'a [Sense], pos: &str, def: &str) -> Option<&'a Sense> {
    let want = norm(def);
    if want.is_empty() {
        return None;
    }
    if let Some(s) = senses.iter().find(|s| norm(&s.definition) == want) {
        return Some(s);
    }
    let strip = |c: char| !(c.is_alphanumeric() || c as u32 > 0x2FFF);
    let want2: String = want.chars().filter(|c| !strip(*c)).collect();
    if let Some(s) = senses.iter().find(|s| {
        let d: String = norm(&s.definition).chars().filter(|c| !strip(*c)).collect();
        !d.is_empty() && d == want2
    }) {
        return Some(s);
    }
    let pos = pos.trim();
    if !pos.is_empty() {
        if let Some(s) = senses.iter().find(|s| {
            s.pos.trim() == pos
                && !s.examples.is_empty()
                && (norm(&s.definition).contains(&want) || want.contains(&norm(&s.definition)))
        }) {
            return Some(s);
        }
    }
    None
}

/// 把例句池分发给缺例句的义项。
///
/// 为什么是「轮转」而不是全给第一条：一个词通常 2~3 条义项，例句池也是 2 条，
/// 全塞给第一条会让第二条永远空着 —— 这正是用户看到的「暂时没有例句」。
/// 先保证**每条义项都有一条**，池子还有余量再回头补第二条。
fn distribute_examples(senses: &mut [Sense], pool: &[Example]) {
    if senses.is_empty() {
        return;
    }
    let mut used: Vec<String> = senses
        .iter()
        .flat_map(|s| s.examples.iter())
        .map(|e| norm(&e.text))
        .collect();

    // 只有有释义的义项才配例句：没释义的占位义项挂例句反而误导
    let targets: Vec<usize> = (0..senses.len())
        .filter(|i| !senses[*i].definition.trim().is_empty())
        .collect();
    if targets.is_empty() {
        return;
    }

    for round in 0..pool.len() {
        let mut progressed = false;
        for idx in &targets {
            if senses[*idx].examples.len() > round {
                continue;
            }
            let Some(cand) = pool.iter().find(|e| {
                !e.text.trim().is_empty() && !used.iter().any(|u| *u == norm(&e.text))
            }) else {
                break;
            };
            used.push(norm(&cand.text));
            senses[*idx].examples.push(cand.clone());
            progressed = true;
        }
        if !progressed {
            break;
        }
    }
}

// ============================================================
// 三、分栏合并
// ============================================================

/// 合并结果统计（用于 toast / 日志，让用户知道到底动了几处）。
#[derive(Debug, Default, Clone, Copy)]
pub struct MergeReport {
    /// 核心含义是否被新数据替换
    pub senses_replaced: bool,
    /// 补上的例句条数
    pub examples_added: usize,
    /// 新增的记忆方法段落数（0 或 1）
    pub mnemonic_added: bool,
    /// 新增的考点条数
    pub exam_points_added: usize,
    /// 新增的常见搭配条数
    pub collocations_added: usize,
    /// 新增的易混辨析条数
    pub confusables_added: usize,
}

impl MergeReport {
    /// 有没有任何实质变化（全 0 说明这次并入白点了）。
    pub fn is_empty(&self) -> bool {
        !self.senses_replaced
            && self.examples_added == 0
            && !self.mnemonic_added
            && self.exam_points_added == 0
            && self.collocations_added == 0
            && self.confusables_added == 0
    }
}

/// 追加一段记忆方法：空 → 直接写；已有 → 去重后换行追加。
///
/// 「有可以使用的数据那么一定要增加上去」，但同一个词反复并入不能无限堆，
/// 所以以**归一化后的子串包含**作为去重判据：重复的那段直接跳过。
fn append_mnemonic(base: &mut String, fresh: &str) -> bool {
    let f = fresh.trim();
    if f.is_empty() {
        return false;
    }
    if base.trim().is_empty() {
        *base = f.to_string();
        return true;
    }
    if norm(base).contains(&norm(f)) {
        return false;
    }
    *base = format!("{}\n\n{}", base.trim(), f);
    true
}

fn append_exam_points(base: &mut Vec<String>, extra: &[String]) -> usize {
    let mut n = 0;
    for p in extra {
        let p = p.trim().to_string();
        if p.is_empty() {
            continue;
        }
        let key = norm(&p);
        if base.iter().any(|x| norm(x) == key) {
            continue;
        }
        base.push(p);
        n += 1;
    }
    n
}

fn append_collocations(base: &mut Vec<Collocation>, extra: &[Collocation]) -> usize {
    let mut n = 0;
    for c in extra {
        if !c.is_usable() {
            continue;
        }
        let key = c.dedup_key();
        if base.iter().any(|x| x.dedup_key() == key) {
            continue;
        }
        base.push(c.clone());
        n += 1;
    }
    n
}

fn append_confusables(base: &mut Vec<Confusable>, extra: &[Confusable]) -> usize {
    let mut n = 0;
    for c in extra {
        if !c.is_usable() {
            continue;
        }
        let key = c.dedup_key();
        if base.iter().any(|x| x.dedup_key() == key) {
            continue;
        }
        base.push(c.clone());
        n += 1;
    }
    n
}

/// 把「AI 讲解整理出的词条」按六栏规矩合并进已有词条。
///
/// `markdown` 是讲解原文：例句与考点从这里确定性解析，作为模型的兜底 ——
/// 本地小模型在 JSON 里漏字段是常态，而 markdown 是它必须产出的东西。
///
/// `local_exam_points` 是**本地词库归属**推出来的考点（见
/// [`crate::db::Db::word_book_refs`]），优先级高于模型：词库里明明写着这个词属于
/// 「四级核心词汇」，就不该让模型去猜。
pub fn merge_explain_fields(
    base: WordEntry,
    fresh: &WordEntry,
    markdown: &str,
    local_exam_points: &[String],
) -> (WordEntry, MergeReport) {
    let mut out = base.clone();
    let mut rep = MergeReport::default();

    let pool = parse_example_pool(markdown);
    let md_points = parse_exam_points(markdown);

    // ---- ① 核心含义：新数据可用则替换，否则保留 ----
    let usable_fresh_senses: Vec<Sense> = fresh
        .senses
        .iter()
        .filter(|s| !s.definition.trim().is_empty())
        .cloned()
        .collect();
    if !usable_fresh_senses.is_empty() {
        // 替换释义，但**旧义项身上已有的例句要捞回来**：例句属于追加项，
        // 不能因为换了一版释义就把用户已经看到的例句冲掉。
        //
        // ★ 匹配策略是「先按语义匹配，匹配不上就按位置兜底」。
        //   只按语义匹配是不够的：新模型给释义换了说法时
        //   （旧「旧释义」→ 新「苹果」，同一个 n. 但文字毫无重叠），
        //   `find_sense` 一定匹配不上，旧例句就被静默丢掉了 ——
        //   这正是用户报的「例句未添加」的反面：悄悄丢数据更难被发现。
        //   位置兜底的原则是「例句属于这个词，不属于某条具体释义」：
        //   义项数没变时按索引一一对应，够用且可预期。
        let mut merged = usable_fresh_senses;
        let old_senses = out.senses.clone();
        // ① 先按语义匹配（同一个词性 + 释义高度重叠）
        for s in merged.iter_mut() {
            if !s.examples.is_empty() {
                continue;
            }
            if let Some(old) = find_sense(&old_senses, &s.pos, &s.definition) {
                if !old.examples.is_empty() {
                    s.examples = old.examples.clone();
                }
            }
        }
        // ② 匹配不上的再按位置兜底
        if merged.len() == old_senses.len() {
            // 义项数没变：索引一一对应，语义匹配漏掉的按位置补
            for (i, s) in merged.iter_mut().enumerate() {
                if s.examples.is_empty() && !old_senses[i].examples.is_empty() {
                    s.examples = old_senses[i].examples.clone();
                }
            }
        } else {
            // 义项数变了（模型合并/拆分了义项）：位置对不上，就把语义匹配
            // 之后仍然「无人认领」的旧例句汇总到第一条义项，
            // 后面的 distribute_examples 会按序重新分配，一条都不丢。
            let mut claimed: Vec<String> = Vec::new();
            for s in merged.iter() {
                for e in &s.examples {
                    claimed.push(norm(&e.text));
                }
            }
            let mut orphan: Vec<Example> = Vec::new();
            for o in &old_senses {
                for e in &o.examples {
                    if !claimed.iter().any(|c| c == &norm(&e.text)) {
                        orphan.push(e.clone());
                    }
                }
            }
            if !orphan.is_empty() {
                if let Some(first) = merged.first_mut() {
                    first.examples.extend(orphan);
                }
            }
        }
        out.senses = merged;
        rep.senses_replaced = true;
    }
    // fresh 没给可用释义 → out.senses 保持原样（保留原数据）

    // ---- ② 例句：给仍缺例句的义项按序补齐 ----
    let before: usize = out.senses.iter().map(|s| s.examples.len()).sum();
    distribute_examples(&mut out.senses, &pool);
    let after: usize = out.senses.iter().map(|s| s.examples.len()).sum();
    rep.examples_added = after.saturating_sub(before);

    // ---- ③ 其余五栏：有数据就追加 ----
    rep.mnemonic_added = append_mnemonic(&mut out.mnemonic, &fresh.mnemonic);
    rep.exam_points_added += append_exam_points(&mut out.exam_points, local_exam_points);
    rep.exam_points_added += append_exam_points(&mut out.exam_points, &md_points);
    rep.exam_points_added += append_exam_points(&mut out.exam_points, &fresh.exam_points);
    rep.collocations_added = append_collocations(&mut out.collocations, &fresh.collocations);
    rep.confusables_added = append_confusables(&mut out.confusables, &fresh.confusables);

    // ---- ④ 其余字段沿用既有「只填空」纪律 ----
    //     这些栏目用户没提要求，但整条 upsert 会把它们清空，所以必须补回来。
    if out.phonetic.uk.trim().is_empty() && !fresh.phonetic.uk.trim().is_empty() {
        out.phonetic.uk = fresh.phonetic.uk.clone();
    }
    if out.phonetic.us.trim().is_empty() && !fresh.phonetic.us.trim().is_empty() {
        out.phonetic.us = fresh.phonetic.us.clone();
    }
    if out.phonetic.audio.trim().is_empty() && !fresh.phonetic.audio.trim().is_empty() {
        out.phonetic.audio = fresh.phonetic.audio.clone();
    }
    if out.inflections.is_empty() && !fresh.inflections.is_empty() {
        out.inflections = fresh.inflections.clone();
    }
    if out.related.is_empty() && !fresh.related.is_empty() {
        out.related = fresh.related.clone();
    }

    (out, rep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Collocation, Confusable, Example, Inflection, Sense, WordEntry};

    fn mk(word: &str) -> WordEntry {
        WordEntry::new(word)
    }

    #[test]
    fn parser_reads_examples_with_quote_translation() {
        let md = "## 核心含义\n- **n.** 苹果\n\n## 例句\n- An apple a day keeps the doctor away.\n> 一天一苹果，医生远离我。\n- She ate an apple.\n> 她吃了一个苹果。\n\n## 易混辨析\n| 词 | 区别 |\n";
        let pool = parse_example_pool(md);
        assert_eq!(pool.len(), 2, "两条例句应都被解析：{:?}", pool);
        assert_eq!(pool[0].text, "An apple a day keeps the doctor away.");
        assert_eq!(pool[0].translation, "一天一苹果，医生远离我。");
        assert_eq!(pool[1].text, "She ate an apple.");
        assert_eq!(pool[1].translation, "她吃了一个苹果。");
    }

    #[test]
    fn parser_tolerates_ordered_list_and_missing_translation() {
        let md = "## 例句\n1. First sentence here.\n> 第一句。\nSecond sentence without bullet.\n";
        let pool = parse_example_pool(md);
        assert_eq!(pool.len(), 2);
        assert_eq!(pool[0].text, "First sentence here.");
        assert_eq!(pool[1].text, "Second sentence without bullet.");
        assert!(pool[1].translation.is_empty());
    }

    #[test]
    fn parser_reads_exam_points() {
        let md = "## 核心含义\n- **n.** 苹果\n- *考点*：四级\n";
        let pts = parse_exam_points(md);
        assert!(pts.iter().any(|p| p == "四级"), "考点应解析出来：{:?}", pts);
    }

    #[test]
    fn parser_splits_multi_exam_points() {
        let md = "- *考点*：四级、考研\n";
        let pts = parse_exam_points(md);
        assert_eq!(pts, vec!["四级".to_string(), "考研".to_string()]);
    }

    #[test]
    fn core_meaning_replaced_when_fresh_usable() {
        let mut base = mk("apple");
        base.senses = vec![Sense {
            pos: "n.".into(),
            definition: "旧释义".into(),
            examples: vec![Example { text: "old ex".into(), translation: "旧例句".into() }],
        }];
        let fresh = WordEntry {
            senses: vec![Sense { pos: "n.".into(), definition: "苹果".into(), examples: vec![] }],
            ..mk("apple")
        };
        let (out, rep) = merge_explain_fields(base, &fresh, "", &[]);
        assert!(rep.senses_replaced);
        assert_eq!(out.senses[0].definition, "苹果");
        // ★ 旧例句必须被捞回来：换了释义不该把已有例句冲掉
        assert_eq!(out.senses[0].examples.len(), 1, "旧例句被替换掉了");
        assert_eq!(out.senses[0].examples[0].text, "old ex");
    }

    #[test]
    fn core_meaning_kept_when_fresh_empty() {
        let mut base = mk("apple");
        base.senses = vec![Sense { pos: "n.".into(), definition: "旧释义".into(), examples: vec![] }];
        base.mnemonic = "旧记忆法".into();
        let fresh = mk("apple"); // 什么都没有
        let (out, rep) = merge_explain_fields(base, &fresh, "", &[]);
        assert!(!rep.senses_replaced);
        assert_eq!(out.senses[0].definition, "旧释义", "没可用新数据时必须保留原数据");
        assert_eq!(out.mnemonic, "旧记忆法");
    }

    #[test]
    fn examples_distributed_to_every_sense() {
        // ★ 用户的 bug：模型给了 2 条例句，但全挂在第 1 条义项，第 2 条显示「没有例句」
        let mut base = mk("bear");
        base.senses = vec![
            Sense { pos: "n.".into(), definition: "熊".into(), examples: vec![] },
            Sense { pos: "v.".into(), definition: "忍受".into(), examples: vec![] },
        ];
        let md = "## 例句\n- The bear is huge.\n> 那只熊很大。\n- I can't bear it.\n> 我受不了了。\n";
        let fresh = WordEntry {
            senses: vec![
                Sense { pos: "n.".into(), definition: "熊".into(), examples: vec![] },
                Sense { pos: "v.".into(), definition: "忍受".into(), examples: vec![] },
            ],
            ..mk("bear")
        };
        let (out, rep) = merge_explain_fields(base, &fresh, md, &[]);
        assert_eq!(rep.examples_added, 2);
        assert_eq!(out.senses[0].examples.len(), 1, "第 1 条义项应有例句");
        assert_eq!(out.senses[1].examples.len(), 1, "第 2 条义项也必须有例句");
        assert_eq!(out.senses[1].examples[0].text, "I can't bear it.");
    }

    #[test]
    fn five_columns_append_and_dedupe() {
        let mut base = mk("apple");
        base.mnemonic = "旧方法".into();
        base.exam_points = vec!["四级".into()];
        base.collocations = vec![Collocation { phrase: "apple pie".into(), note: "苹果派".into() }];
        base.confusables = vec![Confusable { word: "apply".into(), note: "不是苹果".into() }];

        let fresh = WordEntry {
            mnemonic: "新方法".into(),
            exam_points: vec!["四级".into(), "考研".into()], // 四级重复
            collocations: vec![Collocation { phrase: "apple pie".into(), note: "苹果派".into() }], // 重复
            confusables: vec![Confusable { word: "ape".into(), note: "猿".into() }],
            ..mk("apple")
        };
        let (out, rep) = merge_explain_fields(base, &fresh, "", &[]);
        assert!(rep.mnemonic_added);
        assert!(out.mnemonic.contains("旧方法") && out.mnemonic.contains("新方法"));
        assert_eq!(out.exam_points, vec!["四级".to_string(), "考研".to_string()]);
        assert_eq!(rep.exam_points_added, 1, "重复的四级不该再追加");
        assert_eq!(out.collocations.len(), 1, "重复搭配不该再追加");
        assert_eq!(out.confusables.len(), 2);
        assert_eq!(rep.collocations_added, 0);
        assert_eq!(rep.confusables_added, 1);
    }

    #[test]
    fn repeated_merge_does_not_grow_unbounded() {
        let mut base = mk("apple");
        let fresh = WordEntry { mnemonic: "同一段".into(), ..mk("apple") };
        for _ in 0..3 {
            let (next, _) = merge_explain_fields(base.clone(), &fresh, "", &[]);
            base = next;
        }
        assert_eq!(base.mnemonic, "同一段", "重复并入不应堆重复内容");
        // 核 心含义 /例句 等也不该有变化
        let (_, rep) = merge_explain_fields(base.clone(), &fresh, "", &[]);
        assert!(rep.is_empty(), "第二次并入应判定为无变化：{:?}", rep);
    }

    #[test]
    fn local_exam_points_win_over_nothing() {
        let base = mk("abandon");
        let fresh = mk("abandon");
        let (out, rep) = merge_explain_fields(base, &fresh, "", &["考研".to_string()]);
        assert_eq!(out.exam_points, vec!["考研".to_string()]);
        assert_eq!(rep.exam_points_added, 1);
    }

    #[test]
    fn inflections_and_related_are_fill_only() {
        let mut base = mk("run");
        base.inflections = vec![Inflection { label: "过去式".into(), form: "ran".into() }];
        base.related = vec!["sprint".into()];
        let mut fresh = mk("run");
        fresh.inflections = vec![Inflection { label: "复数".into(), form: "runs".into() }];
        fresh.related = vec!["jog".into()];
        let (out, _) = merge_explain_fields(base, &fresh, "", &[]);
        assert_eq!(out.inflections.len(), 1, "已有变形不应被覆盖");
        assert_eq!(out.inflections[0].form, "ran");
        assert_eq!(out.related, vec!["sprint".to_string()]);
    }
}
