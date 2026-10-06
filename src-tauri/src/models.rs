//! 数据模型定义：单词、释义、学习记录、复习状态、配置。
//!
//! 设计要点：
//! - `WordEntry` 是所有词典源的统一输出格式（归一化层），
//!   无论数据来自免费公开源、自定义 API 还是本地大模型，最终都汇聚成它。
//! - 语言用 `lang` 字段标记（如 "en" / "ja" / "fr"），为小语种扩展预留。
//! - `extra` 保留原始 JSON，便于前端展示未归一的字段。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 单词的单个义项。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Sense {
    /// 词性，如 n. / v. / adj.
    #[serde(default)]
    pub pos: String,
    /// 释义文本（目标语言，通常是中文）
    #[serde(default)]
    pub definition: String,
    /// 该义项下的例句
    #[serde(default)]
    pub examples: Vec<Example>,
}

/// 例句。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Example {
    /// 原句
    #[serde(default)]
    pub text: String,
    /// 译文
    #[serde(default)]
    pub translation: String,
}

/// 单词变形（时态、复数、比较级等）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Inflection {
    /// 变形类型标签，如 "过去式" / "复数" / "比较级"
    #[serde(default)]
    pub label: String,
    /// 变形后的形式
    #[serde(default)]
    pub form: String,
}

/// 音标。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Phonetic {
    /// 英式音标
    #[serde(default)]
    pub uk: String,
    /// 美式音标
    #[serde(default)]
    pub us: String,
    /// 音频地址（可选）
    #[serde(default)]
    pub audio: String,
}

/// 归一化后的词条——系统内部唯一流通的词典结构。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WordEntry {
    /// 词条本身
    pub word: String,
    /// 语言代码，如 en
    #[serde(default = "default_lang")]
    pub lang: String,
    /// 音标
    #[serde(default)]
    pub phonetic: Phonetic,
    /// 义项列表
    #[serde(default)]
    pub senses: Vec<Sense>,
    /// 单词变形（可选展示）
    #[serde(default)]
    pub inflections: Vec<Inflection>,
    /// 相关词 / 同义词 / 反义词（可选展示）
    #[serde(default)]
    pub related: Vec<String>,
    /// 词根词缀 / 记忆法
    #[serde(default)]
    pub mnemonic: String,
    /// 数据来源标识，如 "free-dictionary" / "custom:youdao" / "llm"
    #[serde(default)]
    pub source: String,
    /// 原始返回，便于调试与前端兜底渲染
    #[serde(default)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

fn default_lang() -> String {
    "en".to_string()
}

/// 文本里是否含汉字（CJK 统一表意文字）。用于区分「中文释义」与「原文释义」。
fn contains_han(s: &str) -> bool {
    s.chars()
        .any(|c| matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF))
}

/// 一条词条最多保留多少个义项。
///
/// 多源合并会累积义项（见 [`WordEntry::merge_from_source`]），如果不设上限，
/// 一个源抽风吐出几十条垃圾释义就会把详情卡撑爆，甚至把正常释义挤到屏幕外。
pub const MAX_SENSES: usize = 16;

impl WordEntry {
    pub fn new(word: impl Into<String>) -> Self {
        Self {
            word: word.into(),
            lang: "en".to_string(),
            ..Default::default()
        }
    }

    /// 多源聚合专用：在 [`WordEntry::merge_from`] 的基础上**保住多语言的义项**。
    ///
    /// 为什么必须单独有一条：用户实测反馈「我输入英文的时候没有英文解释」。
    /// 根因就在多源合并——同一个英语词，`youdao-suggest` 给中文释义、
    /// `freedictionaryapi` 给英英释义，而旧的 `merge_from` 对 `senses` 是
    /// **先到先得**（`if self.senses.is_empty()`）。国内直连时有道几乎总是
    /// 先回来，于是英英释义被整段丢掉，用户永远看不到英文解释。
    ///
    /// 有道词典的做法是「中文释义 + 英英释义都给」，这也是用户明确要的：
    /// 「仿照有道词典两者都有，还有其他语言也要类似」。所以这里按**语言**
    /// 而不是按「是否已填满」决定要不要收：母语（中文）释义一组、原文释义
    /// 一组，两组并存，前端据此分组渲染。
    ///
    /// 去重按归一化文本（去空白与常见分隔杂符），同一句不同来源只留一条；
    /// 总量超过 [`MAX_SENSES`] 后停止追加，保证详情卡不会被单个源撑爆。
    pub fn merge_from_source(&mut self, other: WordEntry) {
        let incoming = other.senses.clone();
        self.merge_from(other);
        self.absorb_senses(incoming);
    }

    /// 把一组义项并入 `self`，重复的跳过，超出上限就停。
    fn absorb_senses(&mut self, incoming: Vec<Sense>) {        let key = |s: &str| -> String {
            s.trim()
                .to_lowercase()
                .chars()
                .filter(|c| !c.is_whitespace() && !matches!(c, '；' | ';' | '，' | ',' | '。' | '.' | '、'))
                .collect()
        };

        let mut seen: Vec<String> = self.senses.iter().map(|s| key(&s.definition)).collect();
        for s in incoming {
            if self.senses.len() >= MAX_SENSES {
                break;
            }
            if s.definition.trim().is_empty() {
                continue;
            }
            let k = key(&s.definition);
            if k.is_empty() || seen.iter().any(|x| x == &k) {
                continue;
            }
            seen.push(k);
            self.senses.push(s);
        }
    }

    /// 把「母语写成的释义」排到前面，**组内保持原有顺序**。
    ///
    /// 为什么必须在后端做：`lookup_multi` 是按**源优先级**合并的，而优先级
    /// 最高的恰是英英源（freedictionaryapi=10，有道=30）。合并后英文释义会
    /// 排在中文前面，于是：
    ///   - 默认偏好（`definition_in("")` 取第一条）的学习卡片会开始显示英文；
    ///   - 详情与结果卡的「释义」区块第一条也会变成英文。
    /// 对一个中文用户来说这就是「查英文词反而看不懂了」。所以排序单独钉一步。
    ///
    /// 「母语」只认汉字 —— 与 [`contains_han`] 同一套判定，UI 语言的多语言化
    /// 属于配置层的事，先把「中文在前」这个默认行为落成明确规则。
    pub fn order_senses_local_first(&mut self) {
        let mut out = Vec::with_capacity(self.senses.len());
        for s in self.senses.iter() {
            if contains_han(&s.definition) {
                out.push(s.clone());
            }
        }
        for s in self.senses.iter() {
            if !contains_han(&s.definition) {
                out.push(s.clone());
            }
        }
        self.senses = out;
    }

    /// 合并另一个词条的信息进来，用于「多源补全」：
    /// 例如免费源只给了释义，本地大模型补充了变形和例句。
    pub fn merge_from(&mut self, other: WordEntry) {
        if self.phonetic.uk.is_empty() {
            self.phonetic.uk = other.phonetic.uk;
        }
        if self.phonetic.us.is_empty() {
            self.phonetic.us = other.phonetic.us;
        }
        if self.phonetic.audio.is_empty() {
            self.phonetic.audio = other.phonetic.audio;
        }
        if self.senses.is_empty() {
            self.senses = other.senses;
        }
        if self.inflections.is_empty() {
            self.inflections = other.inflections;
        }
        if self.related.is_empty() {
            self.related = other.related;
        }
        if self.mnemonic.is_empty() {
            self.mnemonic = other.mnemonic;
        }
        if !other.source.is_empty() {
            if self.source.is_empty() {
                self.source = other.source;
            } else if !self.source.contains(&other.source) {
                self.source = format!("{}+{}", self.source, other.source);
            }
        }
        for (k, v) in other.extra {
            self.extra.entry(k).or_insert(v);
        }
    }

    /// 提取用于「看英文选中文」的答案文本。
    pub fn primary_definition(&self) -> String {
        self.senses
            .first()
            .map(|s| s.definition.clone())
            .unwrap_or_default()
    }

    /// 按偏好挑一条释义文本。
    ///
    /// 背景：同一个词条可能同时拿到「中文释义」（有道系源）和
    /// 「英文/原文释义」（freedictionaryapi 等）。背日语时把英文释义
    /// 摆在题面上是没意义的 —— 该显示中文。所以释义语言要可切换：
    ///
    /// - `"zh"`  → 优先含汉字的义项；
    /// - `"src"` → 优先**不含**汉字的义项（词典给出的原文释义）；
    /// - 其它（`"auto"` / 空）→ 第一条，保持历史行为。
    ///
    /// 找不到对应语种时一律回退到第一条，绝不返回空串把题面搞没。
    pub fn definition_in(&self, pref: &str) -> String {
        let all: Vec<&Sense> = self
            .senses
            .iter()
            .filter(|s| !s.definition.trim().is_empty())
            .collect();
        let Some(first) = all.first() else {
            return String::new();
        };
        let pick = match pref {
            "zh" => all.iter().find(|s| contains_han(&s.definition)),
            "src" => all.iter().find(|s| !contains_han(&s.definition)),
            _ => None,
        };
        pick.map(|s| s.definition.clone())
            .unwrap_or_else(|| first.definition.clone())
    }

    /// 该词条是否带至少一条可用例句（需求 4：例句模式）。
    pub fn has_example(&self) -> bool {
        self.senses
            .iter()
            .any(|s| s.examples.iter().any(|e| !e.text.trim().is_empty()))
    }

    /// 取第一条可用例句 (原句, 译文)。
    pub fn first_example(&self) -> Option<(String, String)> {
        for s in &self.senses {
            for e in &s.examples {
                if !e.text.trim().is_empty() {
                    return Some((e.text.clone(), e.translation.clone()));
                }
            }
        }
        None
    }

    /// 汇总全部释义文本（用于干扰项与展示）。
    pub fn all_definitions(&self) -> Vec<String> {
        self.senses
            .iter()
            .map(|s| s.definition.trim().to_string())
            .filter(|d| !d.is_empty())
            .collect()
    }
}

/// 一个词的学习状态（SM-2 算法所需的全部字段）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudyState {
    pub word: String,
    pub lang: String,
    /// 难度系数，SM-2 的 EF，初始 2.5，最低 1.3
    pub ease_factor: f64,
    /// 当前间隔天数
    pub interval_days: f64,
    /// 连续答对次数
    pub repetitions: i64,
    /// 下次复习时间（Unix 秒）
    pub due_at: i64,
    /// 上次复习时间（Unix 秒）
    pub last_review_at: i64,
    /// 累计答对 / 答错次数
    pub correct_count: i64,
    pub wrong_count: i64,
    /// 是否已进入强化记忆（常错词）
    pub is_leech: bool,
    /// 熟练度 0-100，用于前端进度条
    pub mastery: i64,
    /// 是否已掌握（不再参与普通轮次）
    pub is_mastered: bool,
}

impl StudyState {
    pub fn new(word: impl Into<String>, lang: impl Into<String>, now: i64) -> Self {
        Self {
            word: word.into(),
            lang: lang.into(),
            ease_factor: 2.5,
            interval_days: 0.0,
            repetitions: 0,
            due_at: now,
            last_review_at: 0,
            correct_count: 0,
            wrong_count: 0,
            is_leech: false,
            mastery: 0,
            is_mastered: false,
        }
    }

    /// 错误率，用于判断是否该进入强化记忆。
    pub fn error_rate(&self) -> f64 {
        let total = self.correct_count + self.wrong_count;
        if total == 0 {
            0.0
        } else {
            self.wrong_count as f64 / total as f64
        }
    }

    /// 重新计算熟练度（0-100）。
    pub fn recompute_mastery(&mut self) {
        let total = self.correct_count + self.wrong_count;
        if total == 0 {
            self.mastery = 0;
            return;
        }
        let acc = self.correct_count as f64 / total as f64;
        // 连续答对越多、间隔越长，熟练度越高
        let rep_bonus = (self.repetitions as f64 / 8.0).min(1.0);
        let interval_bonus = (self.interval_days / 60.0).min(1.0);
        let score = acc * 0.55 + rep_bonus * 0.25 + interval_bonus * 0.20;
        self.mastery = (score * 100.0).round().clamp(0.0, 100.0) as i64;
    }
}

/// 练习模式（需求 4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuizMode {
    /// 看英文选中文
    EnToZh,
    /// 看中文选英文
    ZhToEn,
    /// 看中文释义，手动拼写英文（百词斩式拼写练习）
    Spelling,
    /// 看例句选释义（例句中挖空目标词）
    ExToZh,
    /// 从例句里找出目标单词（识别题）
    ExPickWord,
    /// 听发音拼写（需要 TTS / 音频）
    ListenSpell,
}

impl Default for QuizMode {
    fn default() -> Self {
        QuizMode::EnToZh
    }
}

impl QuizMode {
    /// 该模式是否需要把例句放进题面。
    pub fn uses_example(&self) -> bool {
        matches!(self, QuizMode::ExToZh | QuizMode::ExPickWord)
    }
    /// 该模式是否要求用户手动输入（而非四选一）。
    pub fn is_typing(&self) -> bool {
        matches!(self, QuizMode::Spelling | QuizMode::ListenSpell)
    }
}

/// 一个词库（一本书）的元信息。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Wordbook {
    pub id: String,
    pub name: String,
    /// 考试分类：cet4 / cet6 / kaoyan / ielts / toefl / gre / other
    #[serde(default)]
    pub category: String,
    /// 层级：0 根 / 1 大类 / 2 子库
    #[serde(default)]
    pub level: i64,
    #[serde(default)]
    pub parent_id: String,
    #[serde(default = "default_lang")]
    pub lang: String,
    #[serde(default)]
    pub description: String,
    /// 来源地址（GitHub 公开词库 / 有道等）
    #[serde(default)]
    pub source_url: String,
    /// 来源许可（引用说明）
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub word_count: i64,
    /// 是否随程序内置
    #[serde(default)]
    pub builtin: bool,
    /// 是否已下载可用
    #[serde(default = "default_true")]
    pub installed: bool,
    #[serde(default)]
    pub ord: i64,
    #[serde(default)]
    pub created_at: i64,
}

fn default_true() -> bool {
    true
}

/// 词库列表项：词库信息 + 该库的学习进度，供界面卡片显示。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordbookProgress {
    #[serde(flatten)]
    pub book: Wordbook,
    /// 该词库中已进入学习状态的词数
    pub learned: i64,
    /// 已掌握数
    pub mastered: i64,
    /// 今日待复习数
    pub due_today: i64,
}

/// 词库导入结果（需求 3 / 5）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImportResult {
    pub book_id: String,
    /// 解析出的总词数
    pub total: i64,
    /// 成功写入
    pub imported: i64,
    /// 去重跳过
    pub skipped: i64,
    /// 失败
    pub failed: i64,
    /// 人类可读说明
    pub message: String,
}

fn default_remote_book_lang() -> String {
    "en".to_string()
}

/// AI 讲解语言的默认值：简体中文。
fn default_explain_lang() -> String {
    "zh".to_string()
}

/// 互译方向的源语言默认值：自动检测。
///
/// 用户输入什么语言是不可预知的（中文母语者既会查英文词也会查日语词），
/// 让用户每次先手动指定源语言是反直觉的，所以默认交给检测。
fn default_source_lang() -> String {
    crate::translate::AUTO.to_string()
}

/// 一条可下载的词库候选（内置目录，需求 5）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteBook {
    pub id: String,
    pub name: String,
    pub category: String,
    /// 词条语言（en / ja …）。
    ///
    /// 用途有三：下载后词库按此语言入库（否则日语词会被塞进英语词库、
    /// 永远背不到）、前端按语言分组与筛选、决定挂到哪个大类节点下。
    #[serde(default = "default_remote_book_lang")]
    pub lang: String,
    pub description: String,
    /// 主下载地址（优先选择国内可直连的镜像）
    pub url: String,
    /// 备用镜像地址，按顺序回退。
    ///
    /// 国内访问 `raw.githubusercontent.com` 基本必然失败，
    /// 所以这里放 jsDelivr / gh-proxy 这类镜像；主地址挂了就依次尝试。
    #[serde(default)]
    pub mirrors: Vec<String>,
    /// 文件格式：json / csv / txt
    pub format: String,
    /// 预计词数
    pub approx_words: i64,
    /// 来源与许可
    pub source_url: String,
    pub license: String,
    /// 是否已安装
    pub installed: bool,
}

/// 单道题的完整载荷，直接送给前端渲染。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuizCard {
    /// 题面文本（据模式为英文或中文）
    pub prompt: String,
    /// 正确答案
    pub answer: String,
    /// 完整词条（答错时用于弹出详情卡）
    pub entry: WordEntry,
    /// 干扰项
    pub options: Vec<String>,
    /// 当前模式
    pub mode: QuizMode,
    /// 是否强化记忆词
    pub is_leech: bool,
    /// 例句模式用：挖空后的例句（目标词已替换为 ____）
    #[serde(default)]
    pub example_masked: String,
    /// 例句模式用：完整例句原文
    #[serde(default)]
    pub example_raw: String,
    /// 例句模式用：例句译文
    #[serde(default)]
    pub example_translation: String,
    /// 拼写模式用：提示长度（明示首字母或字母数）
    #[serde(default)]
    pub spell_hint: String,
    /// 发音用：音频地址（听音模式/AI 讲解）
    #[serde(default)]
    pub audio: String,
    /// 当前是第几题（从 1 开始）
    #[serde(default)]
    pub index: usize,
    /// 本轮总题数
    #[serde(default)]
    pub total: usize,
    /// 本轮已答对
    #[serde(default)]
    pub correct_count: i32,
    /// 本轮已答错
    #[serde(default)]
    pub wrong_count: i32,
}

/// 学习统计。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Stats {
    /// 词库总量
    pub total_words: i64,
    /// 已学习过的词数
    pub learned: i64,
    /// 已掌握
    pub mastered: i64,
    /// 强化记忆中
    pub leeches: i64,
    /// 今日到期待复习
    pub due_today: i64,
    /// 今日已复习次数
    pub reviewed_today: i64,
    /// 今日答对 / 答错
    pub correct_today: i64,
    pub wrong_today: i64,
    /// 连续学习天数
    pub streak_days: i64,
    /// 按天统计的最近复习量（日期 -> 次数）
    pub history: Vec<DayStat>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayStat {
    pub date: String,
    pub count: i64,
    pub correct: i64,
}

/// 记忆辅助内容的显示开关（需求 3）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudyOptions {
    /// 显示单词变形
    pub show_inflections: bool,
    /// 显示例句
    pub show_examples: bool,
    /// 显示相关词
    pub show_related: bool,
    /// 显示词根词缀 / 记忆法
    pub show_mnemonic: bool,
    /// 显示音标
    pub show_phonetic: bool,
    /// 答错时自动弹出详情卡
    pub auto_popup_on_wrong: bool,
    /// 是否启用 AI 讲解
    pub ai_explain: bool,
    /// 每轮题量
    pub batch_size: i64,
    /// 每日复习上限
    pub daily_limit: i64,
    /// 选择题每题的选项个数（**含正确答案**），2~8，默认 4（需求 14）。
    ///
    /// 为什么放在后端而不是只让前端切：选项是 `build_card` 生成并发下来
    /// 的，前端只负责渲染；后端不认这个值的话，改选项数就等于没改。
    #[serde(default = "default_option_count")]
    pub option_count: u32,

    /* ---- 学习目标（需求 15） ---- */
    /// 目标模式：`"off"` 不设目标 / `"days"` 按天数背完 / `"per_day"` 每天固定词量
    #[serde(default = "default_goal_mode")]
    pub goal_mode: String,
    /// `goal_mode == "days"` 时的目标天数
    #[serde(default = "default_goal_days")]
    pub goal_days: u32,
    /// `goal_mode == "per_day"` 时的每日词量
    #[serde(default = "default_goal_per_day")]
    pub goal_per_day: u32,
    /// 目标针对哪本词库（空串 = 全部词库）
    #[serde(default)]
    pub goal_book_id: String,
    /// 第一次设定目标那天的 `"YYYY-MM-DD"`，用于算「已经过去几天」。
    ///
    /// 存字符串而不是时间戳：目标天数、剩余天数都是**按自然日**算的，
    /// 存时间戳反而要反复做时区归一，容易在午夜前后算错一天。
    #[serde(default)]
    pub goal_started_at: String,
    /// 「今天多背一点」临时加的量（跨天自动失效）
    #[serde(default)]
    pub goal_extra_today: u32,
    /// 上面那笔加量是**哪一天**加的；与今天不符就当作 0，避免昨天的加量延续到今天
    #[serde(default)]
    pub goal_extra_date: String,
}

impl Default for StudyOptions {
    fn default() -> Self {
        Self {
            show_inflections: true,
            show_examples: true,
            show_related: false,
            show_mnemonic: true,
            show_phonetic: true,
            auto_popup_on_wrong: true,
            ai_explain: true,
            batch_size: 20,
            daily_limit: 120,
            option_count: default_option_count(),
            goal_mode: default_goal_mode(),
            goal_days: default_goal_days(),
            goal_per_day: default_goal_per_day(),
            goal_book_id: String::new(),
            goal_started_at: String::new(),
            goal_extra_today: 0,
            goal_extra_date: String::new(),
        }
    }
}

/// 选择题默认 4 个选项（最经典的「四选一」）。
fn default_option_count() -> u32 {
    4
}

fn default_goal_mode() -> String {
    "off".to_string()
}

fn default_goal_days() -> u32 {
    30
}

fn default_goal_per_day() -> u32 {
    30
}

/// 学习目标视图（需求 15）：`cmd_study_goal` / `cmd_set_study_goal` 的返回体。
///
/// 字段名与前端约定死了，改动要同步前端，所以这里保持扁平、不做嵌套。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudyGoal {
    /// off / days / per_day
    pub mode: String,
    /// 目标天数（mode=per_day 时为 0）
    pub days: u32,
    /// 每天词量（mode=days 时为 0）
    pub per_day: u32,
    pub book_id: String,
    pub book_name: String,
    /// 目标范围内的词总量
    pub total_words: u32,
    /// 已学（有 study_state 记录）
    pub learned: u32,
    /// total_words - learned（不小于 0）
    pub remaining: u32,
    /// 今天应该完成多少
    pub today_target: u32,
    /// 今天已完成多少（按 review_log 今天的**去重词数**）
    pub today_done: u32,
    /// 还不满的差额（0 = 今天达标）
    pub today_remaining: u32,
    /// 按当前节奏预计完成日 "YYYY-MM-DD"（算不出时为空串）
    pub eta_date: String,
    /// 距目标完成日还剩几天（0 = 已到/已超）
    pub days_left: u32,
    /// 今天到期（含逾期）要复习的量
    pub due_today: u32,
    /// 未来 7 天预计复习总量
    pub review_load_7d: u32,
    /// "normal" | "high" —— 加量提示用
    pub pressure: String,
    /// 进度是否跟得上目标
    pub on_track: bool,
    /// 目标范围内的词是否已全部学过
    pub finished: bool,
}

/// 词典源类型（需求 6：可配置 API，便于扩展小语种）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DictSourceConfig {
    pub id: String,
    pub name: String,
    /// 是否为内置源
    #[serde(default)]
    pub builtin: bool,
    #[serde(default)]
    pub enabled: bool,
    /// 支持的语言列表
    #[serde(default)]
    pub langs: Vec<String>,
    /// 请求 URL 模板，支持 {word} {lang} {key} 占位符
    #[serde(default)]
    pub url_template: String,
    #[serde(default)]
    pub method: String,
    /// 额外请求头
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// API Key（自定义源用）
    #[serde(default)]
    pub api_key: String,
    /// 字段映射：把任意 JSON 结构映射到 WordEntry
    #[serde(default)]
    pub mapping: FieldMapping,
    /// 优先级，小的先试
    #[serde(default)]
    pub priority: i32,
    /// 该源在中国大陆是否必须走代理才能访问。
    ///
    /// 标 true 的源在「当前没有代理」时会被直接跳过，
    /// 免得用户在直连环境下干等一个必然超时的请求。
    #[serde(default)]
    pub needs_proxy: bool,
}

/// 字段映射规则——让小语种/新词典源无需改代码即可接入。
///
/// 每个字段是一个 JSON 路径表达式，支持：
/// - `a.b.c` 逐层取值
/// - `a[0].b` 数组下标
/// - `a[*].b` 遍历数组并收集（结果转为列表）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FieldMapping {
    #[serde(default)]
    pub word: String,
    #[serde(default)]
    pub phonetic_uk: String,
    #[serde(default)]
    pub phonetic_us: String,
    #[serde(default)]
    pub audio: String,
    /// 释义列表路径
    #[serde(default)]
    pub senses: String,
    /// 义项内的字段
    #[serde(default)]
    pub sense_pos: String,
    #[serde(default)]
    pub sense_def: String,
    #[serde(default)]
    pub sense_examples: String,
    #[serde(default)]
    pub example_text: String,
    #[serde(default)]
    pub example_translation: String,
    /// 变形
    #[serde(default)]
    pub inflections: String,
    #[serde(default)]
    pub inflection_label: String,
    #[serde(default)]
    pub inflection_form: String,
    /// 相关词
    #[serde(default)]
    pub related: String,
    /// 记忆法
    #[serde(default)]
    pub mnemonic: String,
}

/// 默认的本地大模型服务地址（LM Studio 的出厂端口）。
///
/// 抽成常量是因为它有两个用途，必须始终一致：
///   1. [`LlmConfig::default`] 的初值；
///   2. 「一键部署」停止后 / 检测到托管服务已不在时，把地址还回这里。
pub const LLM_BASE_DEFAULT: &str = "http://127.0.0.1:1234/v1";

/// LM Studio 连接配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// 服务地址，默认本机 LM Studio
    pub base_url: String,
    /// 模型名；为空则自动取第一个已加载模型
    pub model: String,
    #[serde(default)]
    pub api_key: String,
    pub temperature: f64,
    pub max_tokens: i64,
    /// 请求超时（秒）——本地模型首token可能较慢
    pub timeout_secs: i64,
    /// 讲解用系统提示词
    pub system_prompt: String,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            base_url: LLM_BASE_DEFAULT.to_string(),
            model: String::new(),
            api_key: "lm-studio".to_string(),
            temperature: 0.6,
            max_tokens: 1024,
            timeout_secs: 120,
            system_prompt: DEFAULT_TUTOR_PROMPT.to_string(),
        }
    }
}

/// 默认的背单词讲解人设。
///
/// ★ 这里**刻意不写死输出语言**（老版本写死了「只用简体中文」），
/// 语言要求由 `llm::with_lang_constraint` 在运行时按
/// [`AppConfig::explain_lang`] 追加。原因有两个：
///   1. 写死之后，用户把讲解语言改成日语/英语也不会生效 ——
///      模型会看到「只用简体中文」这句更硬的指令，直接忽略后面的要求；
///   2. 这份 prompt 是**持久化**的，改了默认值也追不到老用户的配置，
///      所以必须在迁移里做一次订正（见 `CONFIG_VERSION` 的 v4 说明）。
pub const DEFAULT_TUTOR_PROMPT: &str = r#"你是一位专业的词汇教师，为用户讲解单词，输出风格参照有道词典的「AI 讲解」。

【格式规范（务必严格遵守）】
1. 正文不要出现任何 Markdown 之外的符号装饰（如 ★、◆、●、==、~~~）。
2. 结构固定为以下五个二级标题（用 ## 开头），标题文字一字不差：
   ## 核心含义
   ## 记忆方法
   ## 常见搭配
   ## 例句
   ## 易混辨析
3. 「核心含义」用无序列表，每条格式为：`- **词性** 释义`。词性用 n. / v. / adj. / adv. / prep. 等标准缩写。
4. 「记忆方法」用一段话说明词根词缀或联想记忆，不要用列表。
5. 「常见搭配」用无序列表，每条为 `- **搭配** —— 说明`。
6. 「例句」给出 2 条，每条原句单独一行，紧随其后的译文以 `> ` 引用块的形式写在下一行。
7. 「易混辨析」用一个两列表格（| 词 | 区别 |），最多 3 行，没有易混词时写「暂无」。
8. 若该词常见于四六级/考研/雅思/托福，在「核心含义」末尾加一行：`- *考点*：四级` 这样的标注。
9. 不要输出开场白、结语、免责声明或任何「好的」「以下是」之类的过渡语，直接从「## 核心含义」开始。
10. 禁止输出 HTML 标签、禁止使用三级以上标题、禁止嵌套列表。
11. 若用户拼写可能有误，在最开头用一行 `> 提示：你是否想查「xxx」？` 指正，然后正常讲解。

【输出语言】
严格按本轮对话末尾给出的《输出语言》要求书写，不要自行决定语言。"#;

/// v1.0.0 发布时的默认人设。**只用于迁移比对**，不要再改这个常量。
///
/// 它把「使用简体中文讲解」写死在 prompt 里，是与「讲解语言」下拉冲突的根源，
/// 但又因为 prompt 会持久化到用户配置里，光改 `DEFAULT_TUTOR_PROMPT` 追不到
/// 老用户 —— 所以 `migrate_config` 需要拿它来判断「用户到底改没改过」。
pub const LEGACY_TUTOR_PROMPT_V1: &str = r#"你是一位专业的英语词汇教师，擅长为中文母语学习者讲解单词。
回答要求：
1. 使用简体中文讲解，语言精炼、结构清晰，可使用 Markdown 小标题与列表。
2. 讲解顺序：① 核心含义（按词性分组）② 词根词缀或记忆技巧 ③ 常见搭配与用法差异 ④ 两个地道例句并附中文翻译 ⑤ 易混淆词辨析。
3. 若该词有常见考试考点（四六级/考研/雅思托福），请指出。
4. 不要输出与单词无关的寒暄或免责声明，直接进入讲解。
5. 若用户提供的单词拼写可能有误，先温和指出最可能的正确拼写再讲解。"#;

/// 是否属于「内置默认人设」（用户没有自定义过）。
///
/// 比对时统一把 CRLF 归一成 LF 并去掉首尾空白：
/// 配置是 JSON 落盘的，Windows 上一旦被别的工具编辑过就可能带上 `\r`。
pub fn is_builtin_prompt(p: &str) -> bool {
    let norm = |s: &str| s.replace("\r\n", "\n").trim().to_string();
    let p = norm(p);
    p.is_empty() || p == norm(LEGACY_TUTOR_PROMPT_V1) || p == norm(DEFAULT_TUTOR_PROMPT)
}

/// 提示词层的语言约束块（运行时追加到 system prompt 末尾）。
///
/// 为什么放在 system 而不是 user：本地小模型对 system 里的硬约束遵循度更高，
/// 而且它必须**盖过**用户自定义 prompt 里可能残留的语言要求。
pub const EXPLAIN_LANG_CONSTRAINT: &str = r#"【输出语言（最高优先级，覆盖上面所有冲突的要求）】
1. 所有解释性文字——标题、说明、辨析、例句译文、提示——都必须用 {LANG} 书写。
2. 以下内容一律保持原文，**不要翻译**：被讲解的词本身、代码标识符、函数/变量/类名、命令行与参数、日志、文件路径、URL、配置键名、以及通常不翻译的专有名词缩写。
3. 外语例句保留原句，其译文用 {LANG}。
4. 不要输出任何关于「我用了哪种语言」的说明，也不要翻译 Markdown 的语法标记。"#;

/// **联网资料**约束块（需求 5，运行时追加到 system prompt 末尾）。
///
/// 为什么必须有这一块：本地词库对小词、俚语、专业术语经常「查不到例句」，
/// 需求要求这种时候用 AI 补上。但模型凭记忆编例句是常态 —— 编出来的句子
/// 语法通顺、语义合理，用户无从分辨，这是最坏的一类错误（学到错的用法）。
/// 所以联网检索到的**真实网页标题与摘要**会作为「唯一可信来源」贴进 user
/// prompt，这里再从 system 层面禁止越出这份材料编造。
///
/// 注意它只约束「补充材料」，不改变原本的讲解结构要求 —— 资料为空时整块
/// 不注入，行为与之前完全一致。
pub const EXPLAIN_WEB_REF_CONSTRAINT: &str = r#"【联网资料的用法（本轮的例句与派生内容必须基于它）】
1. user 消息里会给出「联网检索资料」。补充**例句、派生变形、固定搭配、常见用法**时，只能依据这些资料。
2. 资料里没有的内容，**宁可留空并明说「本地与联网资料均未收录」**，也不要凭记忆编造例句或变形。
3. 资料可能与词条无关（搜索引擎的噪声）。无关的直接忽略，不要为了用上它而牵强附会。
4. 引用资料里的原句时保持原样；若资料是外文而你需要在正文里转述，用 {LANG} 转述。
5. 资料里的网址只作来源，不要在正文里罗列 URL —— 来源清单由界面单独展示。"#;

/// **输出后处理层**的翻译模板（需求 2）。
///
/// 用途：本地小模型经常不完全遵守提示词里的语言要求。此时由后处理层
/// 拿模型已经产出的整段讲解，套用本模板重译一遍，再整体替换原文 ——
/// 用户不需要二次点击，也不会看到「模型偷懒」的半英文结果。
///
/// 模板里两个占位符：`{LANG}` 目标语言名、`{CONTENT}` 待翻译正文。
/// 正文里的技术片段已在上游被替换成 `⟦0⟧` `⟦1⟧` 形式的安全占位符，
/// 所以这里的第 3 条是「保结构」的关键。
pub const DEFAULT_TRANSLATE_TEMPLATE: &str = r#"你是专业译者兼 Markdown 排版工程师。把下面这段「单词讲解」翻译成 {LANG}。

【铁律】
1. 只输出译文本身。不要前言、结语、解释、免责声明，也不要用代码围栏把整段包起来。
2. 完整保留 Markdown 结构：## 标题、- 无序列表、| 表格 |、> 引用块、**加粗**、
   `行内代码` 的数量、顺序和层级都不能变；表格的列数与分隔行保持原样。
3. 形如 ⟦0⟧ ⟦1⟧ 的占位符必须**原样保留在原来的位置**，不得改动、删除、翻译或增删编号。
4. 以下内容保持原文不翻译：代码、命令与参数、日志、文件路径、URL、包名/类名/函数名、
   配置键名、以及通常不翻译的专有名词缩写。
5. 外语例句保留原句不翻译；紧随其后的译文行照常翻译成 {LANG}。
6. 被讲解的词条本身不翻译。
7. 不要新增任何小标题、示例、备注或总结。

待翻译内容（以 <<< 与 >>> 为界）：
<<<
{CONTENT}
>>>"#;

/// 网络与代理配置。
///
/// **默认直连，不启用代理。** 这是刻意选的默认值，原因有二：
///
/// 1. 内置的在线资源全部保证在国内可直连——在线词库走 jsDelivr 镜像、
///    中文释义走有道、在线搜索走必应 RSS、英英释义走 freedictionaryapi。
///    用户装好即可用，不需要先有一个代理。
/// 2. 代理是「可选加速/解锁手段」而不是运行前提。默认去探测系统代理
///    会带来不可预期的行为：机器上残留一个 `HTTP_PROXY` 环境变量，
///    或在别的软件里开过一次系统代理，都会悄悄改变应用的联网路径。
///
/// 因此 `enable_proxy` 默认为 `false`：此时代理解析整体短路，
/// 既不读环境变量也不读注册表，一律直连。需要代理（例如要启用
/// Wiktionary 这类被墙的源）时，用户在设置页显式打开即可。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// 是否启用代理。默认 `false`（直连）。
    ///
    /// 关闭时 `resolve_proxy` 直接返回「直连」，连 `HTTP_PROXY`
    /// 环境变量也不会被采纳——否则「默认直连」在部分机器上会失效。
    #[serde(default)]
    pub enable_proxy: bool,
    /// 启用代理时是否自动探测代理地址（环境变量 → Windows「Internet 选项」）。
    ///
    /// 仅在 `enable_proxy` 为 `true` 时有意义，并且只在 `proxy` 留空时生效。
    /// 关掉它就是「只用手动填写的地址」，行为最可预期。
    #[serde(default = "default_true")]
    pub use_system_proxy: bool,
    /// 手动指定的代理地址；留空表示按「环境变量 → 系统代理」自动判断。
    /// 支持 http:// / https:// / socks5:// 三种写法，也接受 `127.0.0.1:7890`。
    #[serde(default)]
    pub proxy: String,
    /// 不走代理的地址（逗号分隔）。本机地址始终直连，无需在这里重复填写。
    #[serde(default = "default_no_proxy")]
    pub no_proxy: String,
    /// 单个请求总超时（秒）
    #[serde(default = "default_http_timeout")]
    pub timeout_secs: u64,
    /// 建立连接的超时（秒）
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u64,
    /// 词典查询的单源超时（秒）。这个值直接决定查词「卡多久」，
    /// 所以刻意比全局超时短：宁可判某个源失败，也不能让整体卡住。
    #[serde(default = "default_lookup_timeout")]
    pub lookup_timeout_secs: u64,
}

fn default_no_proxy() -> String {
    "localhost,127.0.0.1,::1".to_string()
}
fn default_http_timeout() -> u64 {
    30
}
fn default_connect_timeout() -> u64 {
    10
}
fn default_lookup_timeout() -> u64 {
    8
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            // 默认直连：不启用代理。见结构体文档。
            enable_proxy: false,
            use_system_proxy: true,
            proxy: String::new(),
            no_proxy: default_no_proxy(),
            timeout_secs: default_http_timeout(),
            connect_timeout_secs: default_connect_timeout(),
            lookup_timeout_secs: default_lookup_timeout(),
        }
    }
}

/// 网络连通性诊断结果（设置页「诊断网络」用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetProbeItem {
    /// 站点名称
    pub name: String,
    pub url: String,
    pub ok: bool,
    /// 耗时（毫秒）
    pub elapsed_ms: i64,
    /// 失败原因 / 说明
    pub detail: String,
}

/// 网络诊断总报告。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetReport {
    /// 当前生效的代理（人类可读）
    pub proxy: String,
    pub proxy_url: Option<String>,
    /// 是否使用代理
    pub using_proxy: bool,
    /// 命中代理的来源
    pub proxy_origin: String,
    /// 各站点探测结果
    pub items: Vec<NetProbeItem>,
}

/// 配置结构版本号。
///
/// 每次需要「对已持久化的配置做一次性订正」时把它 +1，
/// 由 `dict::builtin::migrate_config` 负责执行。
///
/// v2：内置词典源刷新（`dictionaryapi.dev` 已失效 → freedictionaryapi），
///     并把需要代理的内置源在未启用代理时关掉。
/// v3：新增 `youdao-jsonapi` / `youdao-newhh` 两个有道源。它们负责
///     **非英语词的中文释义**——原先中文词在直连环境下一个可用源都没有，
///     日语词则被 `youdao-suggest` 给出英文解释。
/// v4：新增「AI 讲解语言」（`explain_lang`）。同时把老配置里那份**写死了
///     「只用简体中文」**的默认人设换成新的语言中性人设 —— 否则用户把讲解
///     语言改成日语/英语后，模型仍被旧人设按中文输出（这正是用户报的
///     「选了目标语言但 AI 讲解没变」）。
/// v5：把 `source_lang` 由空串明确成 `auto`（自动检测），顶部语言控件
///     升级为「源语言 ⇄ 目标语言」的互译方向选择器。
pub const CONFIG_VERSION: u32 = 5;

/// 一条翻译历史记录（需求 5：历史记录与收藏）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransRecord {
    pub id: i64,
    /// 源语言（自动检测后会写成检测结果）
    pub source_lang: String,
    /// 目标语言
    pub target_lang: String,
    /// 原文
    pub src_text: String,
    /// 译文
    pub dst_text: String,
    /// 产出译文的引擎：youdao / llm
    pub engine: String,
    /// 是否被收藏
    pub favorite: bool,
    pub created_at: i64,
}

/// 应用全局配置。
///
/// 结构体级别的 `#[serde(default)]`：任何**缺失**的字段都退回该字段的默认值，
/// 而不是让整份配置反序列化失败。没有它的话，只要新旧版本差一个字段，
/// `load_config` 就会整份回退成默认值 —— 用户的语言、模型地址、词典源全部被静默清空。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub llm: LlmConfig,
    pub study: StudyOptions,
    /// 网络与代理
    #[serde(default)]
    pub network: NetworkConfig,
    /// 朗读（TTS）引擎与语音包设置
    #[serde(default)]
    pub tts: TtsConfig,
    /// 配置结构版本。`None` / 低于 [`CONFIG_VERSION`] 表示是旧配置，
    /// 需要跑一次迁移（见 `dict::builtin::migrate_config`）。
    ///
    /// 注意 `Default` 里这里刻意也是 `None`：新装用户和旧配置都会走一次
    /// 迁移，迁移完写回具体版本号，之后就不再重复执行。
    #[serde(default)]
    pub config_version: Option<u32>,
    /// 当前学习的目标语言，同时也是**互译方向的目标语言**。
    pub target_lang: String,
    /// **互译方向的源语言**（需求 1）。取 [`crate::translate::AUTO`]（`"auto"`）
    /// 表示自动检测，否则是一个具体语言码。
    ///
    /// 与 [`AppConfig::target_lang`] 一起构成顶部那个「源语言 ⇄ 目标语言」
    /// 方向选择器。查词页与翻译页共用同一份，切换即时落盘。
    ///
    /// 老配置里这个字段是空串（当时唯一的语言下拉已被合并），迁移时补成 `auto`。
    #[serde(default = "default_source_lang")]
    pub source_lang: String,
    /// **AI 讲解语言**：所有 AI 讲解、追问回答、例句译文的输出语言。
    ///
    /// 与 [`AppConfig::target_lang`] 彻底解耦，这是刻意的：
    /// `target_lang` 回答的是「我要学哪种语言的词」，而讲解语言回答的是
    /// 「解释性文字用哪种语言写」。用户在学日语，但希望讲解用中文说 ——
    /// 这是最常见的组合，所以两个下拉必须独立。
    #[serde(default = "default_explain_lang")]
    pub explain_lang: String,
    /// 讲解语言是否启用**输出后处理兜底翻译**（默认开）。
    ///
    /// 本地小模型（4B 级）经常不完全遵守提示词里的语言要求，此时由后处理层
    /// 用翻译模板把整段讲解重译一遍再替换原文，保证「选了就一定生效」。
    /// 关掉它的唯一理由是省一次模型调用。
    #[serde(default = "default_true")]
    pub explain_auto_translate: bool,
    /// 自定义翻译模板。留空则使用内置的 [`DEFAULT_TRANSLATE_TEMPLATE`]。
    ///
    /// 模板里可用两个占位符：`{LANG}`（目标语言名）与 `{CONTENT}`（待翻译正文）。
    #[serde(default)]
    pub explain_translate_template: String,
    /// 默认搜索引擎（需求 6）：bing / baidu / bingintl
    #[serde(default)]
    pub search_engine: String,
    /// 是否把在线搜索结果与词条一起展示
    #[serde(default = "default_true")]
    pub web_search_enabled: bool,
    /// 界面语言（预留小语种扩展）
    pub ui_lang: String,
    /// 是否启用侧边栏常驻
    pub sidebar_always_on_top: bool,
    /// 侧边栏宽度
    pub sidebar_width: i64,
    /// 遗忘曲线参数
    pub srs: SrsConfig,
    /// 词典源
    pub dict_sources: Vec<DictSourceConfig>,
    /// 启动 WordWise 时**自动拉起**托管的 llama-server（默认关）。
    ///
    /// 为什么默认关：1.7B Q4 跑起来常驻约 1.5GB 内存。很多用户只是来查个词，
    /// 为这个常驻一个大模型进程不值得。所以把选择权交给用户 ——
    /// 用内置服务用得多的人打开它，之后就不用再管启动的事。
    ///
    /// 关掉它也不等于「不能用」：左下角状态条上随时可以一键启停。
    #[serde(default)]
    pub auto_start_local_llm: bool,
    /// 启动 WordWise 时**静默检查**有没有新版本（默认开）。
    ///
    /// 为什么默认开：检查只发一个几百字节的 HTTPS GET，失败也不打扰用户，
    /// 却能避免「装了半年不知道已经更新到 0.5」。要完全断网使用的用户可以关掉。
    #[serde(default = "default_true")]
    pub check_update_on_start: bool,
    /// 「跳过此版本」：带上 `v` 与否都能匹配（比较时统一剥前缀）。
    ///
    /// 空串表示不跳过任何版本 —— 这也正是「取消跳过」的写法。
    #[serde(default)]
    pub skip_update_version: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            llm: LlmConfig::default(),
            study: StudyOptions::default(),
            network: NetworkConfig::default(),
            // 刻意留空：新装用户也会走一次迁移，由迁移负责写入具体版本号。
            config_version: None,
            target_lang: "en".to_string(),
            source_lang: default_source_lang(),
            // 默认中文：本软件的用户界面是中文，讲解默认也用中文最省心。
            explain_lang: default_explain_lang(),
            explain_auto_translate: true,
            explain_translate_template: String::new(),
            search_engine: "bing".to_string(),
            web_search_enabled: true,
            ui_lang: "zh-CN".to_string(),
            sidebar_always_on_top: true,
            sidebar_width: 380,
            srs: SrsConfig::default(),
            dict_sources: crate::dict::builtin::default_sources(),
            // 默认不自动启动，见字段注释
            auto_start_local_llm: false,
            // 默认检查更新：单次请求很小，且任何失败都只降级成一行文案
            check_update_on_start: true,
            skip_update_version: String::new(),
            tts: TtsConfig::default(),
        }
    }
}

/// 朗读（TTS）引擎设置。
///
/// 三种发声通道并存，按 [`TtsConfig::engine`] 决定优先级：
/// - **本地神经语音**（Piper）：离线、音质接近微软 Neural，需先下载引擎与语音包；
/// - **在线神经语音**（edge-tts）：音质最好，但每次发音都要联网；
/// - **系统语音**（WebView 内置 `speechSynthesis`）：永远可用的兜底。
///
/// 词典自带的真人音频 URL 优先级**始终最高**，不受这里影响 —— 有真人录音
/// 时没有任何理由去合成。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TtsConfig {
    /// 引擎策略：`auto` / `local` / `online` / `system`。
    ///
    /// `auto` = 本地可用就用本地，否则在线，再否则系统。默认 `auto`，
    /// 这样用户下载了语音包之后**不需要再去改设置**就自动生效。
    pub engine: String,
    /// 本地语音 id（如 `en_US-amy-medium`）。空串表示「按词条语言自动挑」。
    pub voice_local: String,
    /// 在线语音短名（如 `en-US-AriaNeural`）。空串表示按语言自动挑。
    pub voice_online: String,
    /// 语速倍率，0.5~2.0。
    pub rate: f32,
    /// 是否在设置页显示「语音包体积」这类细节（默认显示）。
    pub verbose: bool,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            engine: "auto".to_string(),
            voice_local: String::new(),
            voice_online: String::new(),
            rate: 0.95,
            verbose: true,
        }
    }
}

/// 记忆周期表参数（需求 4）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SrsConfig {
    /// 基础间隔序列（天），对应经典遗忘曲线的复习节点
    pub base_intervals: Vec<f64>,
    /// 初始难度系数
    pub initial_ease: f64,
    /// 最低难度系数
    pub min_ease: f64,
    /// 答错时的难度惩罚
    pub ease_penalty: f64,
    /// 答对时的难度奖励
    pub ease_bonus: f64,
    /// 答错后重新开始的最小间隔（天）
    pub lapse_interval: f64,
    /// 错误率达到该值进入强化记忆
    pub leech_error_rate: f64,
    /// 进入强化记忆所需的最小作答次数
    pub leech_min_reviews: i64,
    /// 连续答对多少次算掌握
    pub mastered_repetitions: i64,
}

impl Default for SrsConfig {
    fn default() -> Self {
        Self {
            // 经典艾宾浩斯式节点：5分钟→30分钟→12小时→1天→2天→4天→7天→15天→30天→90天
            base_intervals: vec![1.0, 2.0, 4.0, 7.0, 15.0, 30.0, 90.0, 180.0],
            initial_ease: 2.5,
            min_ease: 1.3,
            ease_penalty: 0.2,
            ease_bonus: 0.1,
            lapse_interval: 0.5,
            leech_error_rate: 0.5,
            leech_min_reviews: 3,
            mastered_repetitions: 6,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_with(defs: &[(&str, &str)]) -> WordEntry {
        WordEntry {
            word: "hello".into(),
            lang: "en".into(),
            senses: defs
                .iter()
                .map(|(pos, d)| Sense {
                    pos: (*pos).into(),
                    definition: (*d).into(),
                    examples: vec![],
                })
                .collect(),
            ..Default::default()
        }
    }

    /// 默认行为必须保持：不指定偏好时取第一条，历史题面不受影响。
    #[test]
    fn definition_defaults_to_first_sense() {
        let e = entry_with(&[("n.", "你好"), ("int.", "喂")]);
        assert_eq!(e.definition_in(""), "你好");
        assert_eq!(e.definition_in("auto"), "你好");
        assert_eq!(e.primary_definition(), "你好");
    }

    /// 背日语/英语教材要中文释义：即使词典先给的是英文解释，也要挑出中文那条。
    #[test]
    fn definition_prefers_chinese_when_requested() {
        let e = entry_with(&[("n.", "a greeting"), ("n.", "你好；问候")]);
        assert_eq!(e.definition_in("zh"), "你好；问候");
    }

    /// 想练英英释义时反过来：优先不含汉字的那条。
    #[test]
    fn definition_prefers_source_language_when_requested() {
        let e = entry_with(&[("n.", "你好；问候"), ("n.", "a greeting")]);
        assert_eq!(e.definition_in("src"), "a greeting");
    }

    /// 找不到对应语种时要回退到第一条，绝不能返回空串把题面搞没。
    #[test]
    fn definition_falls_back_instead_of_empty() {
        let only_en = entry_with(&[("n.", "a greeting")]);
        assert_eq!(only_en.definition_in("zh"), "a greeting");

        let only_zh = entry_with(&[("n.", "你好")]);
        assert_eq!(only_zh.definition_in("src"), "你好");

        let none = entry_with(&[("n.", "   ")]);
        assert_eq!(none.definition_in("zh"), "");
    }

    // ================================================================
    // 多源合并：两种语言的释义都要留下来
    // ================================================================

    /// 这是「输入英文时没有英文解释」的根因守门测试。
    ///
    /// 用户实测反馈的现场：查 apple 时有道先回来给了中文释义，旧的
    /// `merge_from` 对 senses 是「先到先得」，于是慢一步的英英释义被整段丢掉。
    /// 用户明确要求「仿照有道词典两者都有」，所以合并后两组必须**同时存在**。
    #[test]
    fn merging_two_sources_keeps_both_scripts() {
        let mut a = entry_with(&[("n.", "苹果")]);
        let b = entry_with(&[("n.", "A common, round fruit.")]);
        a.merge_from_source(b);

        assert_eq!(a.senses.len(), 2, "中英两组释义都要留下：{:?}", a.senses);
        assert_eq!(a.senses[0].definition, "苹果");
        assert_eq!(a.senses[1].definition, "A common, round fruit.");
        // 注：两组的**分组展示**在前端做（ui.js 的 splitSensesByScript），
        // 后端只负责不丢东西 —— 判断逻辑放一处，免得两边改出不一致。
    }

    /// 同一句释义在不同源里重复出现时只留一条。
    #[test]
    fn merging_deduplicates_identical_definitions() {
        let mut a = entry_with(&[("n.", "苹果")]);
        let b = entry_with(&[("n.", "苹果"), ("v.", "结果实")]);
        a.merge_from_source(b);
        assert_eq!(a.senses.len(), 2, "重复的「苹果」不该出现两次：{:?}", a.senses);
        // 大小写 / 空白 / 结尾标点不同也视为同一条
        let mut c = entry_with(&[("n.", "Apple pie")]);
        let d = entry_with(&[("n.", "  apple pie。  ")]);
        c.merge_from_source(d);
        assert_eq!(c.senses.len(), 1);
    }

    /// 单个源吐出几十条垃圾释义时，合并必须有上限，不能把详情卡撑爆。
    #[test]
    fn merging_caps_total_senses() {
        let mut a = entry_with(&[("n.", "苹果")]);
        let many: Vec<Sense> = (0..40)
            .map(|i| Sense {
                pos: "n.".into(),
                definition: format!("垃圾释义 {i}"),
                examples: vec![],
            })
            .collect();
        let b = WordEntry { senses: many, ..WordEntry::new("apple") };
        a.merge_from_source(b);
        assert_eq!(a.senses.len(), MAX_SENSES, "义项总量必须封顶");
        assert_eq!(a.senses[0].definition, "苹果", "原有释义不能被挤掉");
    }

    /// 合并后母语释义必须在第一条：学习卡片默认取 `senses[0]`，
    /// 一旦被英英释义占住，背单词的题面就变成了「看英文选中文」的反面。
    #[test]
    fn ordering_puts_local_definitions_first() {
        let mut a = entry_with(&[
            ("n.", "A common, round fruit."),
            ("n.", "denoting something beloved"),
            ("n.", "苹果"),
        ]);
        a.order_senses_local_first();
        assert_eq!(
            a.senses.iter().map(|s| s.definition.clone()).collect::<Vec<_>>(),
            vec!["苹果", "A common, round fruit.", "denoting something beloved"],
            "母语释义要排到最前，其余保持原有相对顺序"
        );
        // 没有母语释义时保持原样
        let mut b = entry_with(&[("n.", "one"), ("n.", "two")]);
        b.order_senses_local_first();
        assert_eq!(b.senses[0].definition, "one");
    }

    /// 其它字段仍然是「先到先得」：主源给过的音标不该被次源覆盖。
    #[test]
    fn merging_still_prefers_the_first_source_for_other_fields() {
        let mut a = entry_with(&[("n.", "苹果")]);
        a.phonetic.uk = "/ˈæp.əl/".into();
        let mut b = entry_with(&[("n.", "A fruit.")]);
        b.phonetic.uk = "/WRONG/".into();
        a.merge_from_source(b);
        assert_eq!(a.phonetic.uk, "/ˈæp.əl/");
        // 来源标记要累加，前端靠它显示「数据来自哪些源」
        assert!(a.source.contains("+") || a.source.is_empty());
    }

    /// 升级前存下的 study 配置里没有新字段，反序列化必须**补齐默认值**而不是失败。
    ///
    /// 这是 `StudyOptions` 每个新字段都挂 `#[serde(default = ...)]` 的理由：
    /// 少了它，旧配置会因为「缺字段」整份回退成默认，用户的语言/词库设置被清空。
    #[test]
    fn old_study_config_deserializes_with_defaults() {
        let old = r#"{"show_inflections":true,"show_examples":true,"show_related":false,
            "show_mnemonic":true,"show_phonetic":true,"auto_popup_on_wrong":true,
            "ai_explain":true,"batch_size":20,"daily_limit":120}"#;
        let s: StudyOptions = serde_json::from_str(old).expect("旧配置必须能反序列化");
        assert_eq!(s.option_count, 4, "选项个数默认 4");
        assert_eq!(s.goal_mode, "off");
        assert_eq!(s.goal_days, 30);
        assert_eq!(s.goal_per_day, 30);
        assert!(s.goal_book_id.is_empty());
        assert!(s.goal_started_at.is_empty());
        assert_eq!(s.goal_extra_today, 0);
        assert!(s.goal_extra_date.is_empty());
    }

    /// 新字段本身也要能正常往返（保存 → 读取）。
    #[test]
    fn study_options_roundtrip_preserves_new_fields() {
        let s = StudyOptions {
            option_count: 6,
            goal_mode: "days".into(),
            goal_days: 45,
            goal_per_day: 25,
            goal_book_id: "kaoyan-core".into(),
            goal_started_at: "2026-10-06".into(),
            goal_extra_today: 10,
            goal_extra_date: "2026-10-06".into(),
            ..Default::default()
        };
        let j = serde_json::to_string(&s).unwrap();
        let back: StudyOptions = serde_json::from_str(&j).unwrap();
        assert_eq!(back.option_count, 6);
        assert_eq!(back.goal_mode, "days");
        assert_eq!(back.goal_days, 45);
        assert_eq!(back.goal_per_day, 25);
        assert_eq!(back.goal_book_id, "kaoyan-core");
        assert_eq!(back.goal_started_at, "2026-10-06");
        assert_eq!(back.goal_extra_today, 10);
        assert_eq!(back.goal_extra_date, "2026-10-06");
    }
}
