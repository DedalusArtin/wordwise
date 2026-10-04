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

impl WordEntry {
    pub fn new(word: impl Into<String>) -> Self {
        Self {
            word: word.into(),
            lang: "en".to_string(),
            ..Default::default()
        }
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

/// 练习模式：双向背诵。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuizMode {
    /// 看英文选中文
    EnToZh,
    /// 看中文选英文
    ZhToEn,
}

impl Default for QuizMode {
    fn default() -> Self {
        QuizMode::EnToZh
    }
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
        }
    }
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
            base_url: "http://127.0.0.1:1234/v1".to_string(),
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
pub const DEFAULT_TUTOR_PROMPT: &str = r#"你是一位专业的英语词汇教师，擅长为中文母语学习者讲解单词。
回答要求：
1. 使用简体中文讲解，语言精炼、结构清晰，可使用 Markdown 小标题与列表。
2. 讲解顺序：① 核心含义（按词性分组）② 词根词缀或记忆技巧 ③ 常见搭配与用法差异 ④ 两个地道例句并附中文翻译 ⑤ 易混淆词辨析。
3. 若该词有常见考试考点（四六级/考研/雅思托福），请指出。
4. 不要输出与单词无关的寒暄或免责声明，直接进入讲解。
5. 若用户提供的单词拼写可能有误，先温和指出最可能的正确拼写再讲解。"#;

/// 应用全局配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub llm: LlmConfig,
    pub study: StudyOptions,
    /// 当前学习的目标语言
    pub target_lang: String,
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
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            llm: LlmConfig::default(),
            study: StudyOptions::default(),
            target_lang: "en".to_string(),
            ui_lang: "zh-CN".to_string(),
            sidebar_always_on_top: true,
            sidebar_width: 380,
            srs: SrsConfig::default(),
            dict_sources: crate::dict::builtin::default_sources(),
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
