//! 内置示例词库：让用户装完就能直接开始背，不必先导入 CSV。
//!
//! 覆盖英语高频核心词，含音标、释义、例句、变形，可直接用于双向背诵。
//! 其他语言返回一个最小集合，演示小语种扩展能力。

use crate::models::{Example, Inflection, Phonetic, Sense, WordEntry};

/// 构造一个词条的辅助函数。
#[allow(clippy::too_many_arguments)]
fn w(
    word: &str,
    uk: &str,
    us: &str,
    senses: Vec<(&str, &str, &str, &str)>,
    infl: Vec<(&str, &str)>,
    related: Vec<&str>,
    mnemonic: &str,
) -> WordEntry {
    WordEntry {
        word: word.to_string(),
        lang: "en".to_string(),
        phonetic: Phonetic {
            uk: uk.to_string(),
            us: us.to_string(),
            audio: String::new(),
        },
        senses: senses
            .into_iter()
            .map(|(pos, def, ex, exz)| Sense {
                pos: pos.to_string(),
                definition: def.to_string(),
                examples: if ex.is_empty() {
                    vec![]
                } else {
                    vec![Example {
                        text: ex.to_string(),
                        translation: exz.to_string(),
                    }]
                },
            })
            .collect(),
        inflections: infl
            .into_iter()
            .map(|(l, f)| Inflection {
                label: l.to_string(),
                form: f.to_string(),
            })
            .collect(),
        related: related.into_iter().map(String::from).collect(),
        mnemonic: mnemonic.to_string(),
        source: "builtin-seed".to_string(),
        extra: Default::default(),
    }
}

/// 返回指定语言的示例词库。
pub fn demo_words(lang: &str) -> Vec<WordEntry> {
    match lang {
        "en" => english(),
        "ja" => japanese(),
        _ => english(),
    }
}

fn english() -> Vec<WordEntry> {
    vec![
        w("abandon", "/əˈbæn.dən/", "/əˈbæn.dən/",
            vec![
                ("v.", "放弃；抛弃；遗弃", "He abandoned his car in the snow.", "他把车丢弃在雪地里。"),
                ("n.", "放纵；无拘无束", "She danced with abandon.", "她尽情地跳舞。"),
            ],
            vec![("过去式", "abandoned"), ("过去分词", "abandoned"), ("现在分词", "abandoning")],
            vec!["desert", "forsake", "quit"],
            "a-band-on：一个乐队(band)在台上(on)被放弃"),
        w("ability", "/əˈbɪl.ə.ti/", "/əˈbɪl.ə.t̬i/",
            vec![("n.", "能力；才能", "She has the ability to solve hard problems.", "她有能力解决难题。")],
            vec![("复数", "abilities")],
            vec!["capability", "competence", "skill"],
            "able(能)+ity(名词后缀)=能力"),
        w("accurate", "/ˈæk.jə.rət/", "/ˈæk.jɚ.ət/",
            vec![("adj.", "准确的；精确的", "The report is accurate and detailed.", "这份报告准确而详尽。")],
            vec![("副词", "accurately"), ("名词", "accuracy"), ("比较级", "more accurate")],
            vec!["precise", "exact", "correct"],
            "ac(加强)+cur(关心)+ate=特别关心所以准确"),
        w("benefit", "/ˈben.ɪ.fɪt/", "/ˈben.ə.fɪt/",
            vec![
                ("n.", "好处；益处；福利", "Exercise has many benefits.", "运动有很多好处。"),
                ("v.", "有益于；受益", "Everyone benefits from clean air.", "人人都因清洁空气而受益。"),
            ],
            vec![("过去式", "benefited"), ("现在分词", "benefiting"), ("复数", "benefits")],
            vec!["advantage", "profit", "gain"],
            "bene(好)+fit(做)=做好事带来好处"),
        w("consider", "/kənˈsɪd.ər/", "/kənˈsɪd.ɚ/",
            vec![("v.", "考虑；认为；体谅", "Please consider my suggestion.", "请考虑我的建议。")],
            vec![("过去式", "considered"), ("名词", "consideration"), ("形容词", "considerate")],
            vec!["think", "regard", "contemplate"],
            "con(共同)+sider(星星)=一起看星星思考"),
        w("determine", "/dɪˈtɜː.mɪn/", "/dɪˈtɝː.mɪn/",
            vec![("v.", "决定；确定；查明", "She determined to study abroad.", "她决定出国留学。")],
            vec![("名词", "determination"), ("形容词", "determined")],
            vec!["decide", "resolve", "ascertain"],
            "de(加强)+termine(界限)=定下界限就是决定"),
        w("efficient", "/ɪˈfɪʃ.ənt/", "/ɪˈfɪʃ.ənt/",
            vec![("adj.", "高效的；效率高的", "This is a more efficient method.", "这是更高效的方法。")],
            vec![("名词", "efficiency"), ("副词", "efficiently"), ("反义", "inefficient")],
            vec!["effective", "productive"],
            "ef(出)+fic(做)+ient=能做出成果的"),
        w("generate", "/ˈdʒen.ə.reɪt/", "/ˈdʒen.ə.reɪt/",
            vec![("v.", "产生；生成；引起", "Solar panels generate electricity.", "太阳能板发电。")],
            vec![("名词", "generation"), ("名词", "generator")],
            vec!["produce", "create", "yield"],
            "gen(产生)+erate=不断产生"),
        w("hesitate", "/ˈhez.ɪ.teɪt/", "/ˈhez.ə.teɪt/",
            vec![("v.", "犹豫；踌躇", "Don't hesitate to ask me.", "别犹豫，尽管问我。")],
            vec![("名词", "hesitation"), ("形容词", "hesitant")],
            vec!["waver", "falter"],
            "hesit(粘住)+ate=被粘住迈不开步，犹豫"),
        w("improve", "/ɪmˈpruːv/", "/ɪmˈpruːv/",
            vec![("v.", "改进；提高；改善", "Practice will improve your English.", "练习会提高你的英语。")],
            vec![("名词", "improvement"), ("过去式", "improved")],
            vec!["enhance", "better", "upgrade"],
            "im(进入)+prove(证明)=进入证明阶段即改进"),
        w("justify", "/ˈdʒʌs.tɪ.faɪ/", "/ˈdʒʌs.tə.faɪ/",
            vec![("v.", "证明……正当；为……辩护", "Nothing can justify such rudeness.", "这种无礼毫无道理可言。")],
            vec![("名词", "justification"), ("形容词", "justified")],
            vec!["defend", "warrant"],
            "just(公正)+ify(使)=使其公正"),
        w("maintain", "/meɪnˈteɪn/", "/meɪnˈteɪn/",
            vec![("v.", "维持；保养；主张", "It costs a lot to maintain the house.", "保养这房子花费不菲。")],
            vec![("名词", "maintenance")],
            vec!["sustain", "preserve", "assert"],
            "main(主要)+tain(拿住)=牢牢抓住保持"),
        w("negotiate", "/nəˈɡəʊ.ʃi.eɪt/", "/nəˈɡoʊ.ʃi.eɪt/",
            vec![("v.", "谈判；协商；商定", "They negotiated a new contract.", "他们谈成了一份新合同。")],
            vec![("名词", "negotiation"), ("名词", "negotiator")],
            vec!["bargain", "discuss"],
            "neg(不)+oti(闲暇)+ate=没空闲，忙于谈判"),
        w("obvious", "/ˈɒb.vi.əs/", "/ˈɑːb.vi.əs/",
            vec![("adj.", "明显的；显而易见的", "The answer is obvious.", "答案很明显。")],
            vec![("副词", "obviously"), ("名词", "obviousness")],
            vec!["evident", "apparent", "clear"],
            "ob(在前)+vi(路)+ous=就在前面的路上，一眼可见"),
        w("persuade", "/pəˈsweɪd/", "/pɚˈsweɪd/",
            vec![("v.", "说服；劝说", "I persuaded him to stay.", "我说服他留下来。")],
            vec![("名词", "persuasion"), ("形容词", "persuasive")],
            vec!["convince", "coax"],
            "per(彻底)+suade(劝)=彻底劝服"),
        w("relevant", "/ˈrel.ə.vənt/", "/ˈrel.ə.vənt/",
            vec![("adj.", "相关的；切题的", "Please provide relevant documents.", "请提供相关文件。")],
            vec![("名词", "relevance"), ("反义", "irrelevant")],
            vec!["pertinent", "applicable"],
            "re(再)+lev(举起)+ant=再次提起即相关"),
        w("significant", "/sɪɡˈnɪf.ɪ.kənt/", "/sɪɡˈnɪf.ə.kənt/",
            vec![("adj.", "重要的；显著的", "There was a significant increase.", "出现了显著增长。")],
            vec![("名词", "significance"), ("副词", "significantly")],
            vec!["important", "notable", "substantial"],
            "sign(记号)+ific(做)+ant=值得做记号的，重要"),
        w("thorough", "/ˈθʌr.ə/", "/ˈθɜːr.oʊ/",
            vec![("adj.", "彻底的；周密的", "He gave the room a thorough cleaning.", "他把房间彻底打扫了一遍。")],
            vec![("副词", "thoroughly"), ("名词", "thoroughness")],
            vec!["complete", "exhaustive"],
            "through(穿过)+ough=从头穿到尾，彻底"),
        w("vulnerable", "/ˈvʌl.nər.ə.bəl/", "/ˈvʌl.nɚ.ə.bəl/",
            vec![("adj.", "脆弱的；易受伤害的", "Children are vulnerable to illness.", "孩子容易生病。")],
            vec![("名词", "vulnerability")],
            vec!["fragile", "susceptible"],
            "vulner(伤口)+able=易受伤的"),
        w("withstand", "/wɪðˈstænd/", "/wɪðˈstænd/",
            vec![("v.", "承受；抵挡", "The bridge can withstand strong winds.", "这座桥能抵御强风。")],
            vec![("过去式", "withstood")],
            vec!["resist", "endure", "bear"],
            "with(对抗)+stand(站)=顶着站住，承受"),
    ]
}

/// 日语示例——演示小语种扩展（词条结构完全一致）。
fn japanese() -> Vec<WordEntry> {
    vec![
        WordEntry {
            word: "勉強".to_string(),
            lang: "ja".to_string(),
            phonetic: Phonetic {
                uk: "べんきょう".to_string(),
                us: String::new(),
                audio: String::new(),
            },
            senses: vec![Sense {
                pos: "名·サ変".to_string(),
                definition: "学习；用功".to_string(),
                examples: vec![Example {
                    text: "毎日日本語を勉強します。".to_string(),
                    translation: "每天学习日语。".to_string(),
                }],
            }],
            inflections: vec![],
            related: vec!["学習".to_string(), "学ぶ".to_string()],
            mnemonic: "「勉」努力+「強」勉强自己＝努力学习".to_string(),
            source: "builtin-seed".to_string(),
            extra: Default::default(),
        },
        WordEntry {
            word: "綺麗".to_string(),
            lang: "ja".to_string(),
            phonetic: Phonetic {
                uk: "きれい".to_string(),
                us: String::new(),
                audio: String::new(),
            },
            senses: vec![Sense {
                pos: "ナ形".to_string(),
                definition: "漂亮的；干净的".to_string(),
                examples: vec![Example {
                    text: "この花はとても綺麗です。".to_string(),
                    translation: "这朵花非常漂亮。".to_string(),
                }],
            }],
            inflections: vec![],
            related: vec!["美しい".to_string()],
            mnemonic: "綺(华美)+麗(美丽)".to_string(),
            source: "builtin-seed".to_string(),
            extra: Default::default(),
        },
        WordEntry {
            word: "頑張る".to_string(),
            lang: "ja".to_string(),
            phonetic: Phonetic {
                uk: "がんばる".to_string(),
                us: String::new(),
                audio: String::new(),
            },
            senses: vec![Sense {
                pos: "動Ⅰ".to_string(),
                definition: "努力；加油".to_string(),
                examples: vec![Example {
                    text: "試験のために頑張ります。".to_string(),
                    translation: "为了考试而努力。".to_string(),
                }],
            }],
            inflections: vec![Inflection {
                label: "ます形".to_string(),
                form: "頑張ります".to_string(),
            }],
            related: vec!["努力する".to_string()],
            mnemonic: "頑(顽固)+張(张)努力坚持".to_string(),
            source: "builtin-seed".to_string(),
            extra: Default::default(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_seed_is_substantial() {
        let v = english();
        assert!(v.len() >= 20, "示例词库应至少 20 词，实际 {}", v.len());
    }

    #[test]
    fn every_word_has_definition_and_phonetic() {
        for e in english() {
            assert!(!e.senses.is_empty(), "{} 缺少释义", e.word);
            assert!(!e.phonetic.uk.is_empty(), "{} 缺少音标", e.word);
            assert!(
                e.senses.iter().all(|s| !s.definition.is_empty()),
                "{} 存在空释义",
                e.word
            );
        }
    }

    #[test]
    fn words_are_lowercase_and_unique() {
        let v = english();
        let mut seen = std::collections::BTreeSet::new();
        for e in &v {
            assert_eq!(e.word, e.word.to_lowercase(), "{} 应为小写", e.word);
            assert!(seen.insert(e.word.clone()), "{} 重复", e.word);
        }
    }

    #[test]
    fn japanese_extendable() {
        let v = demo_words("ja");
        assert!(!v.is_empty());
        assert!(v.iter().all(|x| x.lang == "ja"));
    }

    #[test]
    fn demo_words_defaults_to_english() {
        assert_eq!(demo_words("xx").len(), english().len());
    }
}
