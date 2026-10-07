//! 语音合成（TTS）—— 本地 Piper 引擎与语音包管理。
//!
//! 为什么不继续只用 WebView 的 `speechSynthesis`：它只能暴露**系统已安装**
//! 的语音，而 Windows 10 默认只带 SAPI5 老音色（David / Zira / Huihui），
//! 机械感很重。要拿到接近微软 Neural 的音质，必须自带引擎。
//!
//! 目录布局跟着「模型目录」设置走（与 llama.cpp 引擎共用一个根）：
//! ```text
//! <models_dir>/tts/
//!   piper/                       引擎：piper.exe + *.dll + espeak-ng-data/
//!   voices/<id>/<id>.onnx        语音模型
//!   voices/<id>/<id>.onnx.json   语音配置（采样率 / 音素表 / 说话人）
//! ```
//!
//! 资源默认**不随安装包分发**（英文默认语音除外），全部按需下载。
//! 下载地址有多条镜像：优先本项目自己的 GitHub Release，回退到
//! hf-mirror。GitHub 那条在未上传时就是 404，`download_with_progress`
//! 会自动跳到下一条，不需要额外分支。

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use once_cell::sync::Lazy;
use serde::Serialize;

/// 本项目仓库（语音包与引擎包作为 Release 资产发布在这里）。
pub const REPO: &str = "DedalusArtin/wordwise";
/// 语音包所在的 Release 标签。
pub const VOICE_TAG: &str = "tts-voices-v1";
/// Piper 引擎包所在的 Release 标签。
pub const ENGINE_TAG: &str = "tts-engine-v1";
/// 引擎包资产名。
pub const ENGINE_ASSET: &str = "piper_windows_amd64.zip";
/// 引擎包体积（用于进度显示；实际以响应头为准）。
pub const ENGINE_BYTES: u64 = 22_477_236;

/// GitHub 下载的镜像前缀。空串 = 直连。
///
/// 顺序即尝试顺序：**直连永远排第一**（能通时最快），后面是公开反代。
/// 实测 `gh-proxy.com` 在国内最稳，`ghfast.top` 次之。
pub const GH_MIRRORS: &[&str] = &[
    "https://gh-proxy.com",
    "https://ghproxy.net",
    "https://ghfast.top",
];

/// 语音模型的 HuggingFace 回退源（官方 + 国内镜像）。
pub const HF_MIRRORS: &[&str] = &[
    "https://hf-mirror.com/rhasspy/piper-voices/resolve/main",
    "https://huggingface.co/rhasspy/piper-voices/resolve/main",
];

/// 一条可选语音的静态描述。
#[derive(Debug, Clone, Serialize)]
pub struct VoiceSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// 词条语言（BCP-47 的**主语言**，如 `ja` / `zh`）—— 用于「按语言自动挑语音」
    pub lang: &'static str,
    /// 完整地区码（BCP-47，统一小写下划线，如 `ja_jp` / `zh_cn`）。
    /// 比 `lang` 精细一档：同一种 `es` 下要区分 `es_ES` 与 `es_MX` 时靠它。
    pub locale: &'static str,
    /// 口音：`us` / `gb` / `mx` / `br` / 空
    pub accent: &'static str,
    /// `female` / `male` / 空（Piper 官方元数据里没有性别字段，
    /// 空串表示「无法从公开信息确认」，不猜）
    pub gender: &'static str,
    /// `x_low` / `low` / `medium` / `high`
    pub quality: &'static str,
    /// `.onnx` 的字节数（界面显示体积、下载后校验）
    pub bytes: u64,
    /// HuggingFace 上的目录（回退源用）
    pub hf_dir: &'static str,
    /// 是否随安装包预置（构建时把文件塞进 vendor/，首次使用即生效）
    pub preset: bool,
    /// 已知短板的提示语（空串 = 无）。显示在设置页语音名下面。
    pub note: &'static str,
    /// 该语音**全部镜像**的 `.onnx` 地址，按优先级排列：
    /// 自建 Release（直连 → 各 GitHub 反代）→ hf-mirror → HuggingFace 官方。
    ///
    /// `.onnx.json` 不必再列一份：两边的资产命名都满足「onnx 名 + `.json`」，
    /// 见 [`file_urls`]。
    ///
    /// 由 [`build_voice_urls`] 在清单首次访问时统一生成，**不在每条里手抄
    /// 二十多份镜像地址** —— 换镜像只改一个函数，也不会出现「清单里写了 6 个
    /// 源、下载时只用 4 个」这种漂移。
    #[serde(default)]
    pub urls: Vec<String>,
}

/// 精选语音清单的**原始定义**（`urls` 留给 [`VOICES`] 首次访问时填）。
///
/// 为什么分成两步：`urls` 要按镜像常量拼出来，而 `Vec<String>` 没法在
/// `const` 里拼接。所以这里只写「人工要维护的事实」（id / 语言 / 质量 /
/// 目录 / 体积 / 提示），镜像那一层全自动。
static VOICE_DEFS: &[VoiceSpec] = &[
    // ─────────────── 英语 ───────────────
    VoiceSpec {
        id: "en_US-amy-medium",
        label: "Amy · 美式女声",
        lang: "en",
        locale: "en_us",
        accent: "us",
        gender: "female",
        quality: "medium",
        bytes: 63_201_294,
        hf_dir: "en/en_US/amy/medium",
        preset: true, // 英文默认语音，随安装包预置
        note: "",
        urls: Vec::new(),
    },
    VoiceSpec {
        id: "en_GB-alba-medium",
        label: "Alba · 英式女声",
        lang: "en",
        locale: "en_gb",
        accent: "gb",
        gender: "female",
        quality: "medium",
        bytes: 63_201_294,
        hf_dir: "en/en_GB/alba/medium",
        preset: false,
        note: "",
        urls: Vec::new(),
    },
    VoiceSpec {
        id: "en_US-ryan-high",
        label: "Ryan · 美式男声（高音质）",
        lang: "en",
        locale: "en_us",
        accent: "us",
        gender: "male",
        quality: "high",
        bytes: 120_786_792,
        hf_dir: "en/en_US/ryan/high",
        preset: false,
        note: "体积最大（115 MB），音质最好",
        urls: Vec::new(),
    },
    VoiceSpec {
        id: "en_US-lessac-medium",
        label: "Lessac · 美式女声（经典）",
        lang: "en",
        locale: "en_us",
        accent: "us",
        gender: "female",
        quality: "medium",
        bytes: 63_201_294,
        hf_dir: "en/en_US/lessac/medium",
        preset: false,
        note: "",
        urls: Vec::new(),
    },
    VoiceSpec {
        id: "en_US-joe-medium",
        label: "Joe · 美式男声",
        lang: "en",
        locale: "en_us",
        accent: "us",
        gender: "male",
        quality: "medium",
        bytes: 63_201_294,
        hf_dir: "en/en_US/joe/medium",
        preset: false,
        note: "",
        urls: Vec::new(),
    },
    VoiceSpec {
        id: "en_GB-northern_english_male-medium",
        label: "Northern English · 英式男声",
        lang: "en",
        locale: "en_gb",
        accent: "gb",
        gender: "male",
        quality: "medium",
        bytes: 63_201_294,
        hf_dir: "en/en_GB/northern_english_male/medium",
        preset: false,
        note: "英格兰北部口音，与 Alba 的南部标准音不同",
        urls: Vec::new(),
    },
    // ─────────────── 中文 ───────────────
    // ★ 官方仓库里只有 zh_CN（`zh_TW` / `yue` 两个目录实测都是 404），
    //   所以「台湾/粤语语音」这条需求没有可交付的语音，见构建报告。
    VoiceSpec {
        id: "zh_CN-huayan-medium",
        label: "华言 · 中文女声",
        lang: "zh",
        locale: "zh_cn",
        accent: "",
        gender: "female",
        quality: "medium",
        bytes: 63_201_294,
        hf_dir: "zh/zh_CN/huayan/medium",
        preset: false,
        note: "",
        urls: Vec::new(),
    },
    VoiceSpec {
        id: "zh_CN-huayan-x_low",
        label: "华言 · 中文女声（小体积）",
        lang: "zh",
        locale: "zh_cn",
        accent: "",
        gender: "female",
        quality: "x_low",
        bytes: 20_628_813,
        hf_dir: "zh/zh_CN/huayan/x_low",
        preset: false,
        // 实测：合成「你好，这是朗读效果」会打印
        // `Missing 3 phoneme(s) from phoneme/id map!` —— 且换成中英文标点、
        // 去标点都同样缺失，说明是**小模型音素表本身不全**，不是文本的问题。
        // 部分汉字会被静默跳过，读音不准。如实告诉用户，别让人以为捡了便宜。
        note: "音素表较小，个别汉字可能读不出（追求准确请选上一档）",
        urls: Vec::new(),
    },
    // ─────────────── 德语 ───────────────
    VoiceSpec {
        id: "de_DE-thorsten-medium",
        label: "Thorsten · 德语男声",
        lang: "de",
        locale: "de_de",
        accent: "",
        gender: "male",
        quality: "medium",
        bytes: 63_201_294,
        hf_dir: "de/de_DE/thorsten/medium",
        preset: false,
        note: "",
        urls: Vec::new(),
    },
    VoiceSpec {
        id: "de_DE-eva_k-x_low",
        label: "Eva K · 德语女声（小体积）",
        lang: "de",
        locale: "de_de",
        accent: "",
        gender: "female",
        quality: "x_low",
        bytes: 20_628_813,
        hf_dir: "de/de_DE/eva_k/x_low",
        preset: false,
        note: "16 kHz 小模型（约 20 MB），音质比 medium 明显粗糙，胜在省地方",
        urls: Vec::new(),
    },
    // ─────────────── 法语 ───────────────
    VoiceSpec {
        id: "fr_FR-siwis-medium",
        label: "Siwis · 法语女声",
        lang: "fr",
        locale: "fr_fr",
        accent: "",
        gender: "female",
        quality: "medium",
        bytes: 63_201_294,
        hf_dir: "fr/fr_FR/siwis/medium",
        preset: false,
        note: "",
        urls: Vec::new(),
    },
    VoiceSpec {
        id: "fr_FR-tom-medium",
        label: "Tom · 法语男声",
        lang: "fr",
        locale: "fr_fr",
        accent: "",
        gender: "male",
        quality: "medium",
        bytes: 63_511_038,
        hf_dir: "fr/fr_FR/tom/medium",
        preset: false,
        note: "",
        urls: Vec::new(),
    },
    // ─────────────── 西班牙语 ───────────────
    VoiceSpec {
        id: "es_ES-davefx-medium",
        label: "DaveFX · 西班牙语（西班牙）男声",
        lang: "es",
        locale: "es_es",
        accent: "",
        gender: "male",
        quality: "medium",
        bytes: 63_201_294,
        hf_dir: "es/es_ES/davefx/medium",
        preset: false,
        note: "",
        urls: Vec::new(),
    },
    VoiceSpec {
        id: "es_MX-claude-high",
        label: "Claude · 西班牙语（墨西哥）男声",
        lang: "es",
        locale: "es_mx",
        accent: "mx",
        gender: "male",
        quality: "high",
        bytes: 63_122_309,
        hf_dir: "es/es_MX/claude/high",
        preset: false,
        note: "高音质档，体积与 medium 档几乎一样",
        urls: Vec::new(),
    },
    // ─────────────── 俄语 ───────────────
    VoiceSpec {
        id: "ru_RU-irina-medium",
        label: "Irina · 俄语女声",
        lang: "ru",
        locale: "ru_ru",
        accent: "",
        gender: "female",
        quality: "medium",
        bytes: 63_201_294,
        hf_dir: "ru/ru_RU/irina/medium",
        preset: false,
        note: "",
        urls: Vec::new(),
    },
    // ─────────────── 意大利语 ───────────────
    VoiceSpec {
        id: "it_IT-paola-medium",
        label: "Paola · 意大利语女声",
        lang: "it",
        locale: "it_it",
        accent: "",
        gender: "female",
        quality: "medium",
        bytes: 63_511_038,
        hf_dir: "it/it_IT/paola/medium",
        preset: false,
        note: "",
        urls: Vec::new(),
    },
    // ─────────────── 葡萄牙语 ───────────────
    VoiceSpec {
        id: "pt_BR-faber-medium",
        label: "Faber · 葡萄牙语（巴西）",
        lang: "pt",
        locale: "pt_br",
        accent: "br",
        // 官方元数据没有性别字段，公开资料里也查不到 Faber 的性别 —— 留空，
        // 宁可不显示也不写一个可能错的字。
        gender: "",
        quality: "medium",
        bytes: 63_201_294,
        hf_dir: "pt/pt_BR/faber/medium",
        preset: false,
        note: "",
        urls: Vec::new(),
    },
    VoiceSpec {
        id: "en_US-lessac-low",
        label: "Lessac · 美式女声（小体积）",
        lang: "en",
        locale: "en_us",
        accent: "us",
        gender: "female",
        quality: "low",
        bytes: 20_795_512,
        hf_dir: "en/en_US/lessac/low",
        preset: false,
        note: "小体积模型：下载快（约为 medium 的三分之一），音质略低，适合流量/磁盘紧张时",
        urls: Vec::new(),
    },
    VoiceSpec {
        id: "en_GB-alan-low",
        label: "Alan · 英式男声（小体积）",
        lang: "en",
        locale: "en_gb",
        accent: "gb",
        gender: "male",
        quality: "low",
        bytes: 20_913_408,
        hf_dir: "en/en_GB/alan/low",
        preset: false,
        note: "小体积模型：下载快，音质略低",
        urls: Vec::new(),
    },
];

/// 精选语音清单（只读；`urls` 在首次访问时填好）。
///
/// 收录标准：**官方仓库里有、地址实测可下、并且能和本项目随包引擎
/// （piper 1.2.0）一起用**。
///
/// 最后一条是关键 —— 下面这些语音在官方仓库里**存在且能下**，但模型用了
/// 新版音素格式（`.onnx.json` 的 `phoneme_id_map` 出现 `aɪ`、`ai` 这类多码点
/// 键），本项目的引擎会直接报
/// `"aɪ" is not a single codepoint (ids=161,)` 而**一个字都合成不出来**：
///   * `ja_JP-hi_fi_captain-medium`（日语，唯一的日语语音，MODEL_CARD 里也
///     明写 Requires piper 1.7.0 or higher）
///   * `ko_KR-kss-medium`（韩语，唯一的韩语语音）
///   * `it_IT-serena-medium`、`zh_CN-chaowen-medium`、`zh_CN-xiao_ya-medium`
/// 所以日语 / 韩语这一轮**没有可交付的语音**，宁可空着也不放一条
/// 「下得下来、读不出声」的进来。详见 `docs` 里的语音包说明与构建报告。
///
/// ★ 既有的 5 条 id **一个都没改名、没删除**：用户机器上可能已经下过，
///   配置（`TtsConfig.voice_local`）里也可能存着旧 id，动了就等于把人家
///   已经装好的语音变成未知项。
pub static VOICES: Lazy<Vec<VoiceSpec>> = Lazy::new(|| {
    VOICE_DEFS
        .iter()
        .cloned()
        .map(|mut s| {
            s.urls = build_voice_urls(s.id, s.hf_dir);
            s
        })
        .collect()
});

/// 按 id 找语音描述。
pub fn spec(id: &str) -> Option<&'static VoiceSpec> {
    VOICES.iter().find(|v| v.id == id)
}

/// 语言码归一化成**主语言**：`EN` / `en-US` / `en_US` → `en`。
pub(crate) fn lang_base(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .replace('-', "_")
        .split('_')
        .next()
        .unwrap_or("")
        .to_string()
}

/// 地区码归一化：`zh-CN` / `ZH_cn` → `zh_cn`（调用方没写地区时就是语言本身）。
fn lang_locale(s: &str) -> String {
    s.trim().to_lowercase().replace('-', "_")
}

/// 质量分：high > medium > low > x_low。
fn quality_score(q: &str) -> i32 {
    match q {
        "high" => 30,
        "medium" => 20,
        "low" => 10,
        _ => 0,
    }
}

/// 一条语音对「(主语言, 地区码, 口音)」的优先级档位；`None` = 不合适。
///
/// 档位越小越优先。0~2 都**要求语言一致**，3 才是跨语言兜底。
fn voice_tier(s: &VoiceSpec, base: &str, loc: &str, accent: &str) -> Option<i32> {
    if base.is_empty() {
        return Some(3); // 调用方没说要读哪国话 → 任意可用语音都能兜底
    }
    if s.lang != base {
        return None; // 语言不对：不跨语言硬凑（这正是「拿英语语音读日语」的根源）
    }
    if loc.contains('_') && s.locale.eq_ignore_ascii_case(loc) {
        return Some(0); // 地区码完全一致，比如请求 zh_cn 命中 zh_cn
    }
    if !accent.is_empty() && s.accent == accent {
        return Some(1); // 语言对 + 口音对，比如请求 en + us 命中 en_US
    }
    Some(2) // 只要语言对得上就行
}

/// 按语言（+口音）挑一条已安装的语音；**目标语言一条都没有时返回 `None`**。
///
/// 优先级：
///   0. 地区码完全一致（请求 `zh_CN`，库里有 `zh_CN`）
///   1. 同一语言 + 口音一致（请求 `en` + `us`，命中 `en_US-*`）
///   2. 同一语言（语言对得上就行）
///   3. 调用方**没给语言**（`lang` 为空）时的任意可用语音
///
/// 同档内按「质量 > 预置」再排；完全同分时取清单 [`VOICES`] 里靠前的那条
/// （所以清单顺序是有意义的，别随手重排）。
///
/// ★ 0~2 都要求语言一致，这是刻意的：以前「挑不中就随便拿第一条」会把英语
///   语音拿去读日语，用户听到的是一串怪音 —— 正是反馈里的「体验不佳」。
///   现在宁可返回 `None`，让上层（`commands::tts::cmd_tts_speak`）能据此
///   回退到系统 TTS 或在线朗读，也不要制造「能播但读错语言」的假象。
///   只有调用方**根本没指定语言**时（`lang` 为空）才允许拿任意一条兜底。
pub fn pick_voice_for_lang(lang: &str, accent: &str, installed: &[String]) -> Option<String> {
    let base = lang_base(lang);
    let loc = lang_locale(lang);
    let accent = accent.trim().to_lowercase();
    let mut best: Option<(&VoiceSpec, i32)> = None;
    for id in installed {
        let Some(s) = spec(id) else { continue };
        let Some(tier) = voice_tier(s, &base, &loc, &accent) else {
            continue;
        };
        // 档位越小越优先，所以权重取 (3 - tier)：档 0 最高、档 3（跨语言兜底）最低
        let score = (3 - tier) * 1000 + quality_score(s.quality) + if s.preset { 1 } else { 0 };
        if best.map(|(_, b)| score > b).unwrap_or(true) {
            best = Some((s, score));
        }
    }
    best.map(|(s, _)| s.id.to_string())
}

/// TTS 的根目录。
pub fn tts_root(models_dir: &Path) -> PathBuf {
    models_dir.join("tts")
}

/// 引擎目录。
pub fn engine_dir(models_dir: &Path) -> PathBuf {
    tts_root(models_dir).join("piper")
}

/// `piper.exe` 的路径。
pub fn piper_exe(models_dir: &Path) -> PathBuf {
    engine_dir(models_dir).join("piper.exe")
}

/// 语音包根目录。
pub fn voices_dir(models_dir: &Path) -> PathBuf {
    tts_root(models_dir).join("voices")
}

/// 单条语音的目录。
pub fn voice_dir(models_dir: &Path, id: &str) -> PathBuf {
    voices_dir(models_dir).join(id)
}

/// 单条语音的模型文件路径。
pub fn voice_onnx(models_dir: &Path, id: &str) -> PathBuf {
    voice_dir(models_dir, id).join(format!("{id}.onnx"))
}

/// 单条语音的配置文件路径。
pub fn voice_json(models_dir: &Path, id: &str) -> PathBuf {
    voice_dir(models_dir, id).join(format!("{id}.onnx.json"))
}

/// 引擎是否就绪（exe 在就算就绪；缺 dll 会在真正调用时暴露）。
pub fn engine_ready(models_dir: &Path) -> bool {
    piper_exe(models_dir).is_file()
}

/// 扫描已安装的语音 id（`onnx` 与 `json` 都在**且 onnx 足够大**才算装好）。
///
/// 为什么要两个文件都在：只下到一半时 `onnx` 是完整的但缺配置，
/// piper 启动会直接报错。把它们当作一个原子整体，界面才不会出现
/// 「显示已安装、一点就失败」的状态。
///
/// 为什么还要校验大小：下载是**断点续传**，中断会在磁盘上留下半个
/// `onnx`（几 MB）—— 只查「文件存在」的话，残缺文件会被当成已装好，
/// 界面显示「已安装」、合成时 piper 必败，用户就撞上「明明载入成功
/// 却发不出声」。真实的语音模型最小也有十几 MB，1 MB 以下必是残件。
/// 残件不算已装，下载流程（`cmd_tts_install_voice`）就能重新把它补全，
/// 而不是跳过（「文件存在 → continue」）永远留个坑。
pub fn installed_voices(models_dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let dir = voices_dir(models_dir);
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return out;
    };
    for e in rd.flatten() {
        if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let id = e.file_name().to_string_lossy().to_string();
        if voice_ready(models_dir, &id) {
            out.push(id);
        }
    }
    out.sort();
    out
}

/// 单条语音是否**真的**可用：两个文件都在，且 `onnx` 不是残件。
pub fn voice_ready(models_dir: &Path, id: &str) -> bool {
    let o = voice_onnx(models_dir, id);
    let j = voice_json(models_dir, id);
    if !o.is_file() || !j.is_file() {
        return false;
    }
    match std::fs::metadata(&o) {
        Ok(m) => m.len() >= MIN_ONNX_BYTES,
        Err(_) => false,
    }
}

/// onnx 模型的最小可信体积。Piper 官方最小的语音模型也有十几 MB，
/// 断点续传留下的残件远小于这个值。
pub const MIN_ONNX_BYTES: u64 = 1024 * 1024;

/// 一条语音需要下载的文件：`(目标文件名, HF 相对路径)`。
pub fn voice_files(s: &VoiceSpec) -> Vec<(String, String)> {
    vec![
        (format!("{}.onnx", s.id), format!("{}/{}.onnx", s.hf_dir, s.id)),
        (
            format!("{}.onnx.json", s.id),
            format!("{}/{}.onnx.json", s.hf_dir, s.id),
        ),
    ]
}

/* ============================================================
   随包资源（vendor/）
   ============================================================

   沿用 llama.cpp 引擎那套约定：随安装包分发的资源放 exe 同级的
   `vendor/` 下，`/vendor/` 已被 gitignore —— 二进制不进仓库，构建时
   由脚本下载，build 脚本检测到就复制进发行包。

   ```
   vendor/piper/piper.exe               语音引擎
   vendor/tts-voices/<id>/<id>.onnx     预置语音（一条一个目录，当前写法）
   vendor/voices/<id>.onnx              预置语音（早期写法，平铺）
   ```

   ★ 为什么有两个语音目录名：早期脚本把预置语音平铺放进 `vendor/voices/`
   （实机上就留着一份 `vendor/voices/zh_CN-huayan-x_low.onnx`），后来统一
   成 `vendor/tts-voices/<id>/`。只认其中一个，另一份就**永远扫不到** ——
   表现为「文件明明在硬盘上，设置页里就是没有这条语音」。所以这里两个都认，
   `build.ps1` 也两个都打包。

   查找顺序是「下载的优先，随包的兜底」。反过来不行：用户在设置页
   新下载一条语音，不该被安装包里的旧版本盖住。

   随包资源**不复制**到模型目录 —— 直接原地读取。程序装在 Program Files
   也不影响（我们只读），省掉首次启动复制 60 MB 的开销。
*/

/// 随包引擎目录。
pub fn bundled_engine_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("vendor").join("piper")
}

/// 随包语音根目录（当前写法：`vendor/tts-voices/<id>/<id>.onnx`）。
pub fn bundled_voices_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("vendor").join("tts-voices")
}

/// **所有**随包语音根目录，按优先级排列。
///
/// 第一项是当前写法，第二项是早期写法（平铺的 `<id>.onnx`）；
/// 实测两种布局在用户机器上会并存，所以两个都要扫。
pub fn bundled_voice_roots(app_dir: &Path) -> Vec<PathBuf> {
    vec![bundled_voices_dir(app_dir), app_dir.join("vendor").join("voices")]
}

/// 在**某一个**随包根目录里找一条语音的 `(onnx, json)`，两种布局都认：
///
/// ```text
/// ① <root>/<id>/<id>.onnx + <id>.onnx.json    目录式（tts-voices）
/// ② <root>/<id>.onnx       + <id>.onnx.json    平铺式（voices）
/// ```
///
/// 两种布局都要求 `onnx` 与 `json` **同时存在**：只下一个文件时 piper
/// 会启动失败，不能算「有这条语音」。
fn voice_pair_in(root: &Path, id: &str) -> Option<(PathBuf, PathBuf)> {
    let sub = root.join(id);
    let (o, j) = (
        sub.join(format!("{id}.onnx")),
        sub.join(format!("{id}.onnx.json")),
    );
    if o.is_file() && j.is_file() {
        return Some((o, j));
    }
    let (o, j) = (
        root.join(format!("{id}.onnx")),
        root.join(format!("{id}.onnx.json")),
    );
    if o.is_file() && j.is_file() {
        return Some((o, j));
    }
    None
}

/// 在所有随包根目录里解析一条语音（不含用户下载的那份）。
pub fn resolve_bundled_voice(app_dir: &Path, id: &str) -> Option<(PathBuf, PathBuf)> {
    bundled_voice_roots(app_dir)
        .into_iter()
        .find_map(|root| voice_pair_in(&root, id))
}

/// 列出**某一个**随包根目录下的语音 id（目录式 + 平铺式都认）。
fn list_voices_in(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(root) else {
        return out;
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        // 目录式 → 目录名就是 id；平铺式 → 从 `<id>.onnx` 反推
        // （`<id>.onnx.json` 不匹配 `.onnx` 后缀，不会被误认）
        let id = if is_dir {
            name.clone()
        } else {
            match name.strip_suffix(".onnx") {
                Some(s) => s.to_string(),
                None => continue,
            }
        };
        if voice_pair_in(root, &id).is_some() {
            out.push(id);
        }
    }
    out.sort();
    out
}

/// 解析可用的 `piper.exe`：先看模型目录（用户下载的），再看随包的。
pub fn resolve_engine(app_dir: &Path, models_dir: &Path) -> Option<PathBuf> {
    let downloaded = piper_exe(models_dir);
    if downloaded.is_file() {
        return Some(downloaded);
    }
    let bundled = bundled_engine_dir(app_dir).join("piper.exe");
    if bundled.is_file() {
        return Some(bundled);
    }
    None
}

/// 解析某条语音的 `(onnx, json)`：同样是下载优先、随包兜底。
pub fn resolve_voice(app_dir: &Path, models_dir: &Path, id: &str) -> Option<(PathBuf, PathBuf)> {
    // ★ 必须走 voice_ready（含大小校验）：残缺的 onnx 交给 piper 只会
    //   启动失败，宁可回落随包那份，也不制造「合成必败」的假可用。
    if voice_ready(models_dir, id) {
        return Some((voice_onnx(models_dir, id), voice_json(models_dir, id)));
    }
    resolve_bundled_voice(app_dir, id)
}

/// 当前**可用**（不管来自下载还是随包）的语音 id 列表。
///
/// 随包那部分会把 [`bundled_voice_roots`] 里的每个根目录都扫一遍，
/// 目录式与平铺式布局都能识别 —— 所以早期放错位置的
/// `vendor/voices/zh_CN-huayan-x_low.onnx` 也会出现在界面里。
pub fn available_voices(app_dir: &Path, models_dir: &Path) -> Vec<String> {
    let mut out = installed_voices(models_dir);
    for root in bundled_voice_roots(app_dir) {
        for id in list_voices_in(&root) {
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out.sort();
    out
}

/// 这条语音从哪来：`downloaded` / `bundled` / `none`（界面据此显示标签）。
pub fn voice_source(app_dir: &Path, models_dir: &Path, id: &str) -> &'static str {
    if voice_ready(models_dir, id) {
        return "downloaded";
    }
    if resolve_bundled_voice(app_dir, id).is_some() {
        return "bundled";
    }
    "none"
}

/// 删除语音时，随包的那份**不能删**（它在安装目录里，删了下次更新又回来；
/// 而且用户可能只是想「不要它」，那应该在设置里禁用而不是删文件）。
/// 这个函数回答「删除操作删的到底是哪一份」。
pub fn is_downloaded_voice(models_dir: &Path, id: &str) -> bool {
    voice_ready(models_dir, id)
}

/// 一条语音在**所有镜像**上的 `.onnx` 地址，按优先级排列。
///
/// 顺序即尝试顺序（[`localllm::download_with_progress`] 会依次试，前一个
/// 失败自动换下一个）：
///   ① 本项目自建 Release（直连 → 各 GitHub 反代）—— 上传后最快；
///      还没上传时前几条就是 404，下载器会自动跳过；
///   ② hf-mirror.com —— 国内实测可用（新增语音实测 200/206）；
///   ③ huggingface.co 官方 —— 境外/直连环境。
///
/// `.onnx.json` 不单独存一份列表：两种源的资产命名都满足「onnx 名 + `.json`」，
/// 由 [`file_urls`] 按需加后缀。
pub fn build_voice_urls(id: &str, hf_dir: &str) -> Vec<String> {
    let onnx = format!("{id}.onnx");
    let mut v = Vec::with_capacity(GH_MIRRORS.len() + HF_MIRRORS.len() + 1);
    let direct = format!("https://github.com/{REPO}/releases/download/{VOICE_TAG}/{onnx}");
    v.push(direct.clone());
    for m in GH_MIRRORS {
        v.push(format!("{m}/{direct}"));
    }
    for m in HF_MIRRORS {
        v.push(format!("{m}/{hf_dir}/{onnx}"));
    }
    v
}

/// `en_US-amy-medium.onnx` / `en_US-amy-medium.onnx.json` → `en_US-amy-medium`。
fn voice_id_of_file(file: &str) -> &str {
    file.strip_suffix(".onnx.json")
        .or_else(|| file.strip_suffix(".onnx"))
        .unwrap_or(file)
}

/// 某个文件的候选下载地址（多镜像，按优先级）。
///
/// 清单**内**的语音直接用 [`build_voice_urls`] 生成的那份列表 —— 单一来源，
/// 不会出现「清单里 6 个镜像、下载时只用 4 个」的漂移；
/// 清单外的文件（理论上不会有）按同一套规则现场拼一份兜底。
pub fn file_urls(file: &str, hf_path: &str) -> Vec<String> {
    if let Some(s) = spec(voice_id_of_file(file)) {
        let want_json = file.ends_with(".json");
        return s
            .urls
            .iter()
            .map(|u| if want_json { format!("{u}.json") } else { u.clone() })
            .collect();
    }
    let mut v = Vec::new();
    // ① 本项目 Release（直连排第一，然后各镜像）
    let direct = format!("https://github.com/{REPO}/releases/download/{VOICE_TAG}/{file}");
    v.push(direct.clone());
    for m in GH_MIRRORS {
        v.push(format!("{m}/{direct}"));
    }
    // ② HuggingFace 官方与镜像
    for m in HF_MIRRORS {
        v.push(format!("{m}/{hf_path}"));
    }
    v
}

/// 引擎包（zip）的候选下载地址。
pub fn engine_urls() -> Vec<String> {
    let direct = format!("https://github.com/{REPO}/releases/download/{ENGINE_TAG}/{ENGINE_ASSET}");
    let mut v = vec![direct.clone()];
    for m in GH_MIRRORS {
        v.push(format!("{m}/{direct}"));
    }
    // 回退到 piper 官方 release（2023.11.14-2 是最后一个带 Windows 二进制的版本）
    let official = "https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_windows_amd64.zip";
    v.push(official.to_string());
    for m in GH_MIRRORS {
        v.push(format!("{m}/{official}"));
    }
    v
}

/// 解压引擎包到 `<models_dir>/tts/`。
///
/// piper 的 zip 里顶层就是 `piper/` 目录，所以解压到 `tts/` 而不是
/// `tts/piper/` —— 否则会变成 `tts/piper/piper/piper.exe`。
pub fn unpack_engine(zip_path: &Path, dest_root: &Path) -> Result<usize> {
    let f = std::fs::File::open(zip_path).context("打开引擎包失败")?;
    let mut zip = zip::ZipArchive::new(f).context("引擎包不是有效的 zip")?;
    std::fs::create_dir_all(dest_root)?;
    let mut n = 0usize;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else {
            continue; // 防目录穿越
        };
        let out = dest_root.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut w = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut w)?;
        n += 1;
    }
    Ok(n)
}

/// 合成结果。
#[derive(Debug, Clone, Serialize)]
pub struct SynthOut {
    /// base64 编码的 WAV（前端拼成 `data:` URL 直接播）
    pub audio: String,
    /// 采样率，前端做进度/时长参考
    pub sample_rate: u32,
    /// 音频字节数
    pub bytes: usize,
    /// 是否命中缓存
    pub cached: bool,
    /// 实际使用的语音 id
    pub voice: String,
    /// 耗时（毫秒）
    pub elapsed_ms: u64,
}

/// 缓存目录：放在数据目录下（不是模型目录），因为它是**可再生**的派生数据，
/// 用户切换模型目录时没必要跟着搬。
pub fn cache_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("tts-cache")
}

/// 缓存键：语音 + 文本 + 语速，三者任一不同都要重新合成。
pub fn cache_key(voice: &str, text: &str, rate: f32) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(voice.as_bytes());
    h.update(b"\x1f");
    h.update(text.as_bytes());
    h.update(b"\x1f");
    h.update(format!("{rate:.3}").as_bytes());
    let d = h.finalize();
    d.iter().map(|b| format!("{b:02x}")).collect::<String>()
}

/// 调用 piper 合成一段语音，返回 WAV 字节。
///
/// **阻塞**：piper 是外部进程，调用方要放进 `spawn_blocking`。
/// 实测（medium 模型）：单个词约 30~60ms，首次多花一次模型加载（约 0.2s）。
///
/// 关于 `--output_file -`：实测 piper 会把 WAV **干净地**写进 stdout
/// （首 12 字节就是 `RIFF....WAVE`），所有日志都走 stderr —— 所以直接读
/// stdout 是安全的，不需要临时文件。如果哪天日志混进来了，读取处会因为
/// 头部不是 RIFF 而报错，而不是悄悄播出一段噪音（`wav_sample_rate` 也会返回 0）。
pub fn synth_blocking(exe: &Path, model: &Path, text: &str, rate: f32) -> Result<Vec<u8>> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    if !exe.is_file() {
        return Err(anyhow!("语音引擎还没安装（缺少 {}）", exe.display()));
    }
    if !model.is_file() {
        return Err(anyhow!("语音包不完整（缺少 {}）", model.display()));
    }

    // 语速 → length_scale：piper 里这个值越大读得越慢，所以是**倒数**关系。
    let r = rate.clamp(0.5, 2.0);
    let length_scale = 1.0 / r;

    let mut cmd = Command::new(exe);
    cmd.arg("--model")
        .arg(model)
        .arg("--length_scale")
        .arg(format!("{length_scale:.3}"))
        .arg("--output_file")
        .arg("-") // stdout
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // 不弹黑框：piper 是控制台程序，不加这个标志每次发音都会闪一个窗口。
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd.spawn().context("启动语音引擎失败")?;
    {
        let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("无法写入语音引擎"))?;
        stdin.write_all(text.as_bytes())?;
        // 关掉 stdin 让 piper 知道输入结束，否则它会一直等
    }

    let out = child.wait_with_output().context("语音引擎执行失败")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!("语音合成失败：{}", err.trim().chars().take(300).collect::<String>()));
    }
    if out.stdout.len() < 64 {
        return Err(anyhow!("语音引擎没有输出音频（文本可能是空的或不受支持）"));
    }
    Ok(out.stdout)
}

/// 从 WAV 字节里读采样率（第 24~27 字节，小端）。
/// 读不出来返回 0，不影响播放。
pub fn wav_sample_rate(bytes: &[u8]) -> u32 {
    if bytes.len() < 28 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return 0;
    }
    u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]])
}

/// 下载编排的取消句柄（与 localllm 的**刻意不复用**：两边可能同时跑，
/// 共用一个标志会让「取消语音下载」顺带把模型下载也掐掉）。
pub type CancelFlag = Arc<AtomicBool>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_ids_are_unique_and_files_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for v in VOICES.iter() {
            assert!(seen.insert(v.id), "语音 id 重复：{}", v.id);
            assert!(!v.label.is_empty());
            assert!(!v.lang.is_empty(), "{} 缺 lang", v.id);
            assert!(!v.locale.is_empty(), "{} 缺 locale", v.id);
            assert!(
                v.locale.starts_with(&format!("{}_", v.lang)),
                "{} 的 locale（{}）该以 lang（{}）开头",
                v.id,
                v.locale,
                v.lang
            );
            let files = voice_files(v);
            assert_eq!(files.len(), 2, "每条语音都该有 onnx + json 两个文件");
            for (name, path) in &files {
                assert!(name.starts_with(v.id), "文件名该以 id 开头：{name}");
                assert!(path.starts_with(v.hf_dir), "HF 路径该落在 hf_dir 下：{path}");
            }
        }
    }

    /// 语音表的硬承诺：**既有 5 条 id 一个都不能改名/删除**。
    /// 用户机器上可能已经下过，配置里也可能存着旧 id。
    #[test]
    fn legacy_voice_ids_are_never_renamed_or_removed() {
        for id in [
            "en_US-amy-medium",
            "en_GB-alba-medium",
            "en_US-ryan-high",
            "zh_CN-huayan-medium",
            "zh_CN-huayan-x_low",
        ] {
            assert!(spec(id).is_some(), "老语音 id 不见了：{id}");
        }
    }

    /// 新增的多语言覆盖，逐条点名（少一条就说明清单被动过）。
    #[test]
    fn multilingual_voices_are_present() {
        let want: &[(&str, &str)] = &[
            ("de_DE-thorsten-medium", "de"),
            ("de_DE-eva_k-x_low", "de"),
            ("fr_FR-siwis-medium", "fr"),
            ("fr_FR-tom-medium", "fr"),
            ("es_ES-davefx-medium", "es"),
            ("es_MX-claude-high", "es"),
            ("ru_RU-irina-medium", "ru"),
            ("it_IT-paola-medium", "it"),
            ("pt_BR-faber-medium", "pt"),
            ("en_US-lessac-medium", "en"),
            ("en_US-joe-medium", "en"),
            ("en_GB-northern_english_male-medium", "en"),
        ];
        for (id, lang) in want {
            let s = spec(id).unwrap_or_else(|| panic!("缺语音 {id}"));
            assert_eq!(s.lang, *lang, "{id} 的语言标错了");
        }
    }

    /// 每条语音都要有**多个**镜像，且每个镜像的地址形状正确。
    /// 这是「下载不会因为单点失败而卡死」的静态保证。
    #[test]
    fn every_voice_has_multiple_working_mirrors() {
        for v in VOICES.iter() {
            assert!(
                v.urls.len() >= 3,
                "{} 的镜像太少（{} 个），单点失败就下载不了",
                v.id,
                v.urls.len()
            );
            assert!(
                v.urls[0].starts_with("https://github.com/DedalusArtin/wordwise/releases/download/"),
                "{} 的第一顺位该是自建 Release：{}",
                v.id,
                v.urls[0]
            );
            assert!(v.urls.iter().any(|u| u.contains("hf-mirror.com")), "{} 缺 hf-mirror", v.id);
            assert!(
                v.urls.iter().any(|u| u.contains("huggingface.co")),
                "{} 缺 HuggingFace 官方兜底",
                v.id
            );
            // 每个地址都要能对上这条语音自己的 hf_dir 与 id
            for u in v.urls.iter().filter(|u| u.contains("piper-voices")) {
                assert!(u.contains(&v.hf_dir), "{} 的镜像地址与 hf_dir 不符：{u}", v.id);
                assert!(u.ends_with(&format!("/{}.onnx", v.id)), "{} 的镜像地址没指向自己：{u}", v.id);
            }
            // 每条都要正好包含「直连在前、反代在后」的顺序
            assert!(v.urls[1].contains("gh-proxy.com"), "{} 的第二顺位该是 gh-proxy", v.id);
        }
    }

    /// 清单里的 `urls` 与实际下载用的 [`file_urls`] 必须完全一致 ——
    /// 否则「界面/清单显示的镜像」和「真正去下载的镜像」会各写一份。
    #[test]
    fn file_urls_agree_with_spec_urls() {
        for v in VOICES.iter() {
            let onnx = format!("{}.onnx", v.id);
            let hf = format!("{}/{}.onnx", v.hf_dir, v.id);
            assert_eq!(file_urls(&onnx, &hf), v.urls, "{} 的 onnx 地址不一致", v.id);
            let json = format!("{}.onnx.json", v.id);
            let hf_json = format!("{}/{}.onnx.json", v.hf_dir, v.id);
            let got = file_urls(&json, &hf_json);
            let want: Vec<String> = v.urls.iter().map(|u| format!("{u}.json")).collect();
            assert_eq!(got, want, "{} 的 json 地址不一致", v.id);
        }
    }

    /// 目录扫描：`vendor/tts-voices/<id>/` 与 `vendor/voices/<id>.onnx`
    /// 两种布局都要能认出来（实机上两种并存过）。
    #[test]
    fn bundled_scan_accepts_both_layouts() {
        let app = std::env::temp_dir().join(format!("ww-tts-vendor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&app);
        let models = app.join("models");

        // ① 目录式：vendor/tts-voices/<id>/<id>.onnx
        let dirw = app.join("vendor").join("tts-voices").join("en_US-amy-medium");
        std::fs::create_dir_all(&dirw).unwrap();
        std::fs::write(dirw.join("en_US-amy-medium.onnx"), b"x").unwrap();
        std::fs::write(dirw.join("en_US-amy-medium.onnx.json"), b"{}").unwrap();

        // ② 平铺式：vendor/voices/<id>.onnx（实机上就是这么一份中文模型）
        let flat = app.join("vendor").join("voices");
        std::fs::create_dir_all(&flat).unwrap();
        std::fs::write(flat.join("zh_CN-huayan-x_low.onnx"), b"x").unwrap();
        std::fs::write(flat.join("zh_CN-huayan-x_low.onnx.json"), b"{}").unwrap();

        // ③ 半成品：只有 onnx 没有 json → 两种布局都不该被采纳
        std::fs::write(flat.join("fr_FR-siwis-medium.onnx"), b"x").unwrap();

        assert_eq!(
            available_voices(&app, &models),
            vec!["en_US-amy-medium".to_string(), "zh_CN-huayan-x_low".to_string()],
            "两种布局都该被扫出来，半成品不该"
        );
        assert!(resolve_voice(&app, &models, "en_US-amy-medium").is_some(), "目录式没解析到");
        assert!(resolve_voice(&app, &models, "zh_CN-huayan-x_low").is_some(), "平铺式没解析到");
        assert!(resolve_voice(&app, &models, "fr_FR-siwis-medium").is_none(), "半成品不该能用");
        assert_eq!(voice_source(&app, &models, "zh_CN-huayan-x_low"), "bundled");
        assert_eq!(voice_source(&app, &models, "fr_FR-siwis-medium"), "none");
        // 下载的那份优先于随包的
        let dl = voice_dir(&models, "zh_CN-huayan-x_low");
        std::fs::create_dir_all(&dl).unwrap();
        write_fake_onnx(&models, "zh_CN-huayan-x_low");
        assert_eq!(voice_source(&app, &models, "zh_CN-huayan-x_low"), "downloaded");
        let _ = std::fs::remove_dir_all(&app);
    }

    /// 本机实测兜底：仓库里 `vendor/voices/` 下那份平铺的中文模型
    /// 真的能被 [`available_voices`] 列出来（vendor 不入库，没有就跳过）。
    #[test]
    fn real_repo_vendor_flat_voice_is_discovered() {
        let app = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("src-tauri 的上一级就是项目根")
            .to_path_buf();
        if !app.join("vendor").join("voices").is_dir() {
            return; // CI / 干净克隆：vendor 是 gitignore 的，跳过
        }
        let models = std::env::temp_dir().join("ww-tts-no-such-models-dir");
        let avail = available_voices(&app, &models);
        assert!(
            avail.iter().any(|i| i == "zh_CN-huayan-x_low"),
            "vendor/voices 下平铺的中文模型没被扫到：{avail:?}"
        );
        assert_eq!(voice_source(&app, &models, "zh_CN-huayan-x_low"), "bundled");
    }

    #[test]
    fn exactly_one_voice_is_preset() {
        let n = VOICES.iter().filter(|v| v.preset).count();
        assert_eq!(n, 1, "预置语音只能有一条（英文默认），否则安装包会失控地变大");
        let p = VOICES.iter().find(|v| v.preset).unwrap();
        assert_eq!(p.lang, "en", "预置的应该是英文语音");
    }

    /// 小体积语音有实测到的短板，必须如实标注 —— 否则用户为了省 40MB 选了它，
    /// 遇到读不出的字只会以为是程序坏了。
    #[test]
    fn small_models_carry_an_honest_note() {
        let small = spec("zh_CN-huayan-x_low").unwrap();
        assert!(!small.note.is_empty(), "x_low 实测有音素缺失，必须给出提示");
        assert!(small.note.contains("音素"), "提示要说清是音素表的问题，而不是笼统地贬低");
        // 中等质量及以上不该出现「读不准」措辞
        for v in VOICES.iter().filter(|v| v.quality != "x_low") {
            assert!(
                !v.note.contains("读不"),
                "{} 不该被标成读不准：{}",
                v.id,
                v.note
            );
        }
    }

    #[test]
    fn urls_cover_self_release_and_hf_fallback() {
        let urls = file_urls("en_US-amy-medium.onnx", "en/en_US/amy/medium/en_US-amy-medium.onnx");
        assert!(urls[0].starts_with("https://github.com/DedalusArtin/wordwise/releases/download/"),
            "自带 Release 必须是第一顺位");
        assert!(urls.iter().any(|u| u.contains("hf-mirror.com")), "缺 HF 镜像回退");
        assert!(urls.iter().any(|u| u.contains("gh-proxy.com")), "缺 GitHub 反代回退");
        // 直连永远在镜像之前
        let direct = urls.iter().position(|u| u.contains("github.com/DedalusArtin") && !u.contains("gh-") && !u.contains("ghproxy") && !u.contains("ghfast"));
        let mirror = urls.iter().position(|u| u.contains("gh-proxy.com"));
        assert!(direct < mirror, "直连该排在反代之前");
    }

    /// 镜像优先级必须是：自建 Release → 国内 HF 镜像 → HF 官方。
    /// 顺序错了会「先撞 404 再等超时」，用户看到的是下载半天不动。
    #[test]
    fn mirror_priority_is_self_then_hf_mirror_then_official() {
        let urls = build_voice_urls("de_DE-thorsten-medium", "de/de_DE/thorsten/medium");
        let pos_self = urls.iter().position(|u| u.contains("DedalusArtin/wordwise")).unwrap();
        let pos_mirror = urls.iter().position(|u| u.contains("hf-mirror.com")).unwrap();
        let pos_official = urls
            .iter()
            .position(|u| u.contains("huggingface.co/rhasspy"))
            .unwrap();
        assert!(pos_self < pos_mirror, "自建 Release 要排在 hf-mirror 之前");
        assert!(pos_mirror < pos_official, "国内镜像要排在 HF 官方之前（官方在国内常年不通）");
        // hf-mirror 与官方指向的**同一个** HF 路径
        assert!(urls[pos_mirror].ends_with("/de/de_DE/thorsten/medium/de_DE-thorsten-medium.onnx"));
        assert!(urls[pos_official].ends_with("/de/de_DE/thorsten/medium/de_DE-thorsten-medium.onnx"));
    }

    /// `file_urls` 对**清单外**的文件也要能兜底（不能因为查不到 spec 就返回空表）。
    #[test]
    fn file_urls_falls_back_for_unknown_files() {
        let urls = file_urls("xx_XX-nobody-medium.onnx", "xx/xx_XX/nobody/medium/xx_XX-nobody-medium.onnx");
        assert!(urls.len() >= 3, "清单外的文件也该拿到完整镜像列表：{urls:?}");
        assert!(urls[0].ends_with("/xx_XX-nobody-medium.onnx"));
        assert!(urls.iter().any(|u| u.contains("hf-mirror.com")));
        // id 反推要能处理两种后缀
        assert_eq!(voice_id_of_file("en_US-amy-medium.onnx"), "en_US-amy-medium");
        assert_eq!(voice_id_of_file("en_US-amy-medium.onnx.json"), "en_US-amy-medium");
    }

    #[test]
    fn engine_urls_include_official_fallback() {
        let urls = engine_urls();
        assert!(urls[0].contains("tts-engine-v1"), "优先拿自己发布的引擎包");
        assert!(urls.iter().any(|u| u.contains("rhasspy/piper")), "缺 piper 官方回退");
    }

    #[test]
    fn pick_voice_matches_language_and_prefers_accent() {
        let installed: Vec<String> = VOICES.iter().map(|v| v.id.to_string()).collect();
        // 美式 → 在美式里挑（ryan-high 分最高：high + 口音一致）
        assert_eq!(
            pick_voice_for_lang("en", "us", &installed).as_deref(),
            Some("en_US-ryan-high")
        );
        // 英式 → 两条 gb 同分（alba / northern_english_male），取清单里靠前的 alba
        assert_eq!(
            pick_voice_for_lang("en", "gb", &installed).as_deref(),
            Some("en_GB-alba-medium")
        );
        // 中文：medium 优于 x_low
        assert_eq!(
            pick_voice_for_lang("zh", "", &installed).as_deref(),
            Some("zh_CN-huayan-medium")
        );
        // 没有对应语言的语音 → None（调用方据此回退）
        assert_eq!(pick_voice_for_lang("ja", "", &installed), None);
        assert_eq!(pick_voice_for_lang("en", "us", &[]), None);
    }

    /// 新增语言要能按语言（和地区码）挑出来。
    #[test]
    fn pick_voice_covers_new_languages() {
        let installed: Vec<String> = VOICES.iter().map(|v| v.id.to_string()).collect();
        // 只有一条 medium 的语言，直接点中
        for (lang, want) in [
            ("de", "de_DE-thorsten-medium"),
            ("ru", "ru_RU-irina-medium"),
            ("it", "it_IT-paola-medium"),
            ("pt", "pt_BR-faber-medium"),
        ] {
            assert_eq!(
                pick_voice_for_lang(lang, "", &installed).as_deref(),
                Some(want),
                "{lang} 挑错了"
            );
        }
        // 西班牙语：high 的 claude 在质量上压过 medium 的 davefx
        assert_eq!(
            pick_voice_for_lang("es", "", &installed).as_deref(),
            Some("es_MX-claude-high")
        );
        // 给了地区码 → 同语言里优先地区一致的那条
        assert_eq!(
            pick_voice_for_lang("es_ES", "", &installed).as_deref(),
            Some("es_ES-davefx-medium")
        );
        assert_eq!(
            pick_voice_for_lang("zh-CN", "", &installed).as_deref(),
            Some("zh_CN-huayan-medium")
        );
        // 地区码大小写/连字符写法都认
        assert_eq!(
            pick_voice_for_lang("ES-mx", "", &installed).as_deref(),
            Some("es_MX-claude-high")
        );
    }

    /// ★ 本轮最重要的一条：**绝不跨语言兜底**。
    /// 目标语言一条语音都没有时必须返回 None（上层据此回退系统 TTS），
    /// 而不是随便拿英语语音去读日语 —— 那正是用户抱怨的「体验不佳」。
    #[test]
    fn never_falls_back_across_languages() {
        let installed: Vec<String> = VOICES.iter().map(|v| v.id.to_string()).collect();
        // 日语 / 韩语这一轮没有可用语音（官方那两条要 piper ≥1.7，随包引擎跑不了）
        assert_eq!(pick_voice_for_lang("ja", "", &installed), None);
        assert_eq!(pick_voice_for_lang("ja_JP", "", &installed), None);
        assert_eq!(pick_voice_for_lang("ko", "", &installed), None);
        assert_eq!(pick_voice_for_lang("ko_KR", "", &installed), None);
        // 只有英语可用时，读日语依然返回 None，绝不退化成英语
        let only_en = vec!["en_US-amy-medium".to_string()];
        assert_eq!(pick_voice_for_lang("ja", "", &only_en), None);
        assert_eq!(pick_voice_for_lang("de", "", &only_en), None);
        // 但「调用方没指定语言」（空串）时允许拿任意一条兜底
        assert_eq!(pick_voice_for_lang("", "", &only_en).as_deref(), Some("en_US-amy-medium"));
        assert_eq!(pick_voice_for_lang("   ", "", &only_en).as_deref(), Some("en_US-amy-medium"));
    }

    /// 语言码归一化：大小写、连字符/下划线混写都要认。
    #[test]
    fn language_codes_are_normalized() {
        assert_eq!(lang_base("EN"), "en");
        assert_eq!(lang_base("en-US"), "en");
        assert_eq!(lang_base(" en_gb "), "en");
        assert_eq!(lang_base("zh"), "zh");
        assert_eq!(lang_base(""), "");
        assert_eq!(lang_locale("zh-CN"), "zh_cn");
        assert_eq!(lang_locale("ES_mx"), "es_mx");
        assert_eq!(lang_locale("zh"), "zh");
    }


    /// 写一个**超过 MIN_ONNX_BYTES** 的假 onnx —— installed 判定含大小校验
    /// （防断点续传的残件），几字节的假文件不再算数。
    fn write_fake_onnx(dir: &Path, id: &str) {
        let d = voice_dir(dir, id);
        std::fs::create_dir_all(&d).unwrap();
        let mut data = vec![0u8; MIN_ONNX_BYTES as usize + 1];
        data[0] = b'x';
        std::fs::write(voice_onnx(dir, id), &data).unwrap();
        std::fs::write(voice_json(dir, id), b"{}").unwrap();
    }

    #[test]
    fn installed_detection_requires_both_files() {
        let tmp = std::env::temp_dir().join(format!("ww-tts-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        // 只放 onnx、不放 json → 不算装好
        let d = voice_dir(&tmp, "en_US-amy-medium");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(voice_onnx(&tmp, "en_US-amy-medium"), b"x").unwrap();
        assert!(installed_voices(&tmp).is_empty(), "缺 json 不该算已安装");
        // 补上 json，但 onnx 只有几字节（断点续传的残件）→ 仍不算装好
        std::fs::write(voice_json(&tmp, "en_US-amy-medium"), b"{}").unwrap();
        assert!(installed_voices(&tmp).is_empty(), "onnx 小于 MIN_ONNX_BYTES 的残件不该算已安装");
        // 写足体积 → 装好
        write_fake_onnx(&tmp, "en_US-amy-medium");
        assert_eq!(installed_voices(&tmp), vec!["en_US-amy-medium".to_string()]);
        // 目录不存在时是空表而不是 panic
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(installed_voices(&tmp).is_empty());
        assert!(!engine_ready(&tmp));
    }

    #[test]
    fn cache_key_separates_voice_text_and_rate() {
        let a = cache_key("en_US-amy-medium", "reality", 0.95);
        assert_eq!(a, cache_key("en_US-amy-medium", "reality", 0.95), "同参数必须同键");
        assert_ne!(a, cache_key("en_GB-alba-medium", "reality", 0.95), "换语音要换键");
        assert_ne!(a, cache_key("en_US-amy-medium", "reality ", 0.95), "文本差一个空格也要换键");
        assert_ne!(a, cache_key("en_US-amy-medium", "reality", 0.8), "换语速要换键");
        assert_eq!(a.len(), 64, "sha256 十六进制串长度");
    }

    #[test]
    fn wav_sample_rate_is_read_or_zero() {
        // 构造一个最小 WAV 头
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&0u32.to_le_bytes());
        w.extend_from_slice(b"WAVE");
        w.extend_from_slice(b"fmt ");
        w.extend_from_slice(&16u32.to_le_bytes());      // 16..20 chunk 长度
        w.extend_from_slice(&1u16.to_le_bytes());       // 20..22 音频格式 = PCM
        w.extend_from_slice(&1u16.to_le_bytes());       // 22..24 声道数 = 1
        w.extend_from_slice(&22050u32.to_le_bytes());   // 24..28 采样率
        assert_eq!(wav_sample_rate(&w), 22050);
        // 垃圾输入不 panic
        assert_eq!(wav_sample_rate(b"not a wav at all"), 0);
        assert_eq!(wav_sample_rate(&[]), 0);
    }

    #[test]
    fn synth_reports_missing_engine_instead_of_panicking() {
        let tmp = std::env::temp_dir();
        let e = synth_blocking(
            &tmp.join("no-such-piper.exe"),
            &tmp.join("no-such.onnx"),
            "hello",
            1.0,
        );
        assert!(e.is_err());
        let msg = format!("{}", e.unwrap_err());
        assert!(msg.contains("引擎"), "缺引擎要给出可读提示，而不是裸 IO 错误：{msg}");
    }

    #[test]
    fn length_scale_rate_clamp_is_sane() {
        // 语速被夹在 0.5~2.0，所以 length_scale 落在 0.5~2.0
        for r in [0.1f32, 0.5, 1.0, 2.0, 99.0] {
            let ls = 1.0 / r.clamp(0.5, 2.0);
            assert!((0.5..=2.0).contains(&ls), "语速 {r} 换算出越界的 length_scale {ls}");
        }
    }
}
