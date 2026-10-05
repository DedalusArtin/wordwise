/* ============================================================
   inflect.rs —— 规则变形生成（需求 5、18）
   当联网词典源与本地大模型都没有给出变形时，
   依据词性用英语构词规则推导常用变形，保证「变形」标签页始终有内容。
   ============================================================ */

use crate::models::Inflection;

/// 依据词性与单词本身推导变形。
/// `pos_hint` 可为空（此时对常见词尾做保守推断）。
pub fn derive(word: &str, pos_hint: &str, senses_pos: &[String]) -> Vec<Inflection> {
    let w = word.trim().to_lowercase();
    if w.is_empty() || !w.chars().all(|c| c.is_ascii_alphabetic() || c == '-' || c == ' ') {
        return Vec::new();
    }
    if w.contains(' ') || w.contains('-') {
        return Vec::new();
    }

    let pos = normalize_pos(pos_hint, senses_pos);
    let mut out: Vec<Inflection> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let push = |label: &str, form: String, out: &mut Vec<Inflection>, seen: &mut std::collections::BTreeSet<String>| {
        if form.is_empty() || form == w { return; }
        let k = format!("{}|{}", label, form);
        if seen.insert(k) {
            out.push(Inflection { label: label.to_string(), form });
        }
    };

    // 名词：复数
    if pos.contains('n') {
        push("复数", pluralize(&w), &mut out, &mut seen);
    }
    // 动词：过去式 / 过去分词 / 现在分词 / 第三人称单数
    if pos.contains('v') {
        let (past, pp) = past_forms(&w);
        push("过去式", past.clone(), &mut out, &mut seen);
        push("过去分词", pp, &mut out, &mut seen);
        push("现在分词", ing_form(&w), &mut out, &mut seen);
        push("第三人称单数", third_person(&w), &mut out, &mut seen);
    }
    // 形容词：比较级 / 最高级
    if pos.contains('a') || pos.contains('j') {
        if let Some((cmp, sup)) = comparatives(&w) {
            push("比较级", cmp, &mut out, &mut seen);
            push("最高级", sup, &mut out, &mut seen);
        }
    }
    // 副词：比较级 / 最高级
    if pos == "adv" {
        if let Some((cmp, sup)) = comparatives(&w) {
            push("比较级", format!("more {}", w), &mut out, &mut seen);
            push("最高级", format!("most {}", w), &mut out, &mut seen);
            let _ = (cmp, sup);
        }
    }

    // 无词性提示时做通用推断：动词变形最常用
    if out.is_empty() {
        let (past, pp) = past_forms(&w);
        push("过去式", past, &mut out, &mut seen);
        push("过去分词", pp, &mut out, &mut seen);
        push("现在分词", ing_form(&w), &mut out, &mut seen);
        push("复数", pluralize(&w), &mut out, &mut seen);
    }

    out
}

fn normalize_pos(pos_hint: &str, senses_pos: &[String]) -> String {
    let mut p = pos_hint.trim().to_lowercase();
    if p.is_empty() {
        for s in senses_pos {
            let t = s.trim().to_lowercase();
            if !t.is_empty() {
                p = t;
                break;
            }
        }
    }
    // 中文词性 → 英文缩写
    if p.contains("名") { return "n".into(); }
    if p.contains("动") { return "v".into(); }
    if p.contains("形容") { return "adj".into(); }
    if p.contains("副词") { return "adv".into(); }
    if p.starts_with("n") { return "n".into(); }
    if p.starts_with("v") { return "v".into(); }
    if p.starts_with("adj") || p.starts_with("a.") { return "adj".into(); }
    if p.starts_with("adv") { return "adv".into(); }
    p
}

const VOWELS: [char; 5] = ['a', 'e', 'i', 'o', 'u'];

fn is_vowel(c: char) -> bool { VOWELS.contains(&c) }

fn ends_with_consonant_y(w: &str) -> bool {
    let b: Vec<char> = w.chars().collect();
    b.len() >= 2 && b[b.len() - 1] == 'y' && !is_vowel(b[b.len() - 2])
}

/// 辅音结尾且为「重读闭音节」的短词需双写末尾字母。
fn needs_double(w: &str) -> bool {
    let b: Vec<char> = w.chars().collect();
    let n = b.len();
    if n < 3 { return false; }
    let last = b[n - 1];
    if is_vowel(last) || !last.is_ascii_alphabetic() { return false; }
    if !is_vowel(b[n - 2]) { return false; }
    // 单音节或末尾重读：以 CVC 结尾且最后三字母后无其它元音组，粗略判定
    let tail = &w[n.saturating_sub(3)..];
    let cc: Vec<char> = tail.chars().collect();
    if cc.len() == 3 && !is_vowel(cc[0]) && is_vowel(cc[1]) && !is_vowel(cc[2]) {
        // 排除 w/x/y 结尾（不双写）
        return last != 'w' && last != 'x' && last != 'y';
    }
    false
}

fn pluralize(w: &str) -> String {
    let irr = [("man", "men"), ("woman", "women"), ("child", "children"),
               ("foot", "feet"), ("tooth", "teeth"), ("goose", "geese"),
               ("mouse", "mice"), ("person", "people")];
    for (s, p) in irr { if w == s { return p.into(); } }
    if w.ends_with("s") || w.ends_with("x") || w.ends_with("z")
        || w.ends_with("ch") || w.ends_with("sh") {
        return format!("{}es", w);
    }
    if ends_with_consonant_y(w) {
        return format!("{}ies", &w[..w.len() - 1]);
    }
    if w.ends_with("f") { return format!("{}ves", &w[..w.len() - 1]); }
    if w.ends_with("fe") { return format!("{}ves", &w[..w.len() - 2]); }
    format!("{}s", w)
}

/// 返回 (过去式, 过去分词)
fn past_forms(w: &str) -> (String, String) {
    let irr: [(&str, &str, &str); 16] = [
        ("be", "was", "been"), ("have", "had", "had"), ("do", "did", "done"),
        ("go", "went", "gone"), ("make", "made", "made"), ("take", "took", "taken"),
        ("come", "came", "come"), ("get", "got", "gotten"), ("give", "gave", "given"),
        ("see", "saw", "seen"), ("know", "knew", "known"), ("write", "wrote", "written"),
        ("speak", "spoke", "spoken"), ("run", "ran", "run"), ("eat", "ate", "eaten"),
        ("begin", "began", "begun"),
    ];
    for (a, b, c) in irr { if w == a { return (b.into(), c.into()); } }

    if w.ends_with('e') {
        let s = format!("{}d", w);
        return (s.clone(), s);
    }
    if ends_with_consonant_y(w) {
        let s = format!("{}ied", &w[..w.len() - 1]);
        return (s.clone(), s);
    }
    if needs_double(w) {
        let s = format!("{}{}ed", w, w.chars().last().unwrap());
        return (s.clone(), s);
    }
    let s = format!("{}ed", w);
    (s.clone(), s)
}

fn ing_form(w: &str) -> String {
    if w.ends_with("ie") {
        return format!("{}ying", &w[..w.len() - 2]);
    }
    if w.ends_with('e') && !w.ends_with("ee") && !w.ends_with("oe") && !w.ends_with("ye") {
        return format!("{}ing", &w[..w.len() - 1]);
    }
    if needs_double(w) && w.len() <= 5 {
        return format!("{}{}ing", w, w.chars().last().unwrap());
    }
    format!("{}ing", w)
}

fn third_person(w: &str) -> String {
    if w == "be" { return "is".into(); }
    if w == "have" { return "has".into(); }
    if w.ends_with("s") || w.ends_with("x") || w.ends_with("z")
        || w.ends_with("ch") || w.ends_with("sh") || w.ends_with('o') {
        return format!("{}es", w);
    }
    if ends_with_consonant_y(w) {
        return format!("{}ies", &w[..w.len() - 1]);
    }
    format!("{}s", w)
}

fn comparatives(w: &str) -> Option<(String, String)> {
    let irr = [("good", "better", "best"), ("bad", "worse", "worst"),
               ("many", "more", "most"), ("much", "more", "most"),
               ("little", "less", "least"), ("far", "further", "furthest")];
    for (a, b, c) in irr { if w == a { return Some((b.into(), c.into())); } }
    // 长词用 more/most
    let syllables = w.chars().filter(|c| is_vowel(*c)).count();
    if w.len() > 7 || syllables > 2 {
        return Some((format!("more {}", w), format!("most {}", w)));
    }
    if ends_with_consonant_y(w) {
        return Some((format!("{}ier", &w[..w.len() - 1]), format!("{}iest", &w[..w.len() - 1])));
    }
    if w.ends_with('e') {
        return Some((format!("{}r", w), format!("{}st", w)));
    }
    if needs_double(w) {
        let d = w.chars().last().unwrap();
        return Some((format!("{}{}er", w, d), format!("{}{}est", w, d)));
    }
    Some((format!("{}er", w), format!("{}est", w)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forms(w: &str, pos: &str) -> Vec<String> {
        derive(w, pos, &[]).into_iter().map(|x| x.form).collect()
    }

    #[test]
    fn noun_plural() {
        assert!(forms("apple", "n").contains(&"apples".to_string()));
        assert!(forms("box", "n").contains(&"boxes".to_string()));
        assert!(forms("city", "n").contains(&"cities".to_string()));
        assert!(forms("child", "n").contains(&"children".to_string()));
    }

    #[test]
    fn verb_forms() {
        let v = forms("study", "v");
        assert!(v.contains(&"studied".to_string()), "{:?}", v);
        assert!(v.contains(&"studying".to_string()), "{:?}", v);
        assert!(v.contains(&"studies".to_string()), "{:?}", v);
    }

    #[test]
    fn irregular_verb() {
        let v = forms("go", "v");
        assert!(v.contains(&"went".to_string()));
        assert!(v.contains(&"gone".to_string()));
        assert!(v.contains(&"going".to_string()));
    }

    #[test]
    fn adjective_comparative() {
        let a = forms("big", "adj");
        assert!(a.contains(&"bigger".to_string()), "{:?}", a);
        assert!(a.contains(&"biggest".to_string()), "{:?}", a);
    }

    #[test]
    fn cjk_or_space_returns_empty() {
        assert!(derive("苹果", "n", &[]).is_empty());
        assert!(derive("take off", "v", &[]).is_empty());
    }
}
