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
    /// 词条语言（用于「按语言自动挑语音」）
    pub lang: &'static str,
    /// 口音：`us` / `gb` / 空
    pub accent: &'static str,
    /// `female` / `male`
    pub gender: &'static str,
    /// `x_low` / `low` / `medium` / `high`
    pub quality: &'static str,
    /// `.onnx` 的字节数（界面显示体积、下载后校验）
    pub bytes: u64,
    /// HuggingFace 上的目录（回退源用）
    pub hf_dir: &'static str,
    /// 是否随安装包预置（构建时把文件塞进 resources，首次启动复制过来）
    pub preset: bool,
}

/// 精选语音清单。
///
/// 只挑**对背单词真正有用**的：英语两种口音各一个、质量最高的一个男声、
/// 中文标准女声（含一个 20MB 的小体积版，方便只想要中文的用户）。
/// 没有把所有 piper 语音都列进来 —— 几十条里绝大多数用户永远不会点。
pub const VOICES: &[VoiceSpec] = &[
    VoiceSpec {
        id: "en_US-amy-medium",
        label: "Amy · 美式女声",
        lang: "en",
        accent: "us",
        gender: "female",
        quality: "medium",
        bytes: 63_201_294,
        hf_dir: "en/en_US/amy/medium",
        preset: true, // 英文默认语音，随安装包预置
    },
    VoiceSpec {
        id: "en_GB-alba-medium",
        label: "Alba · 英式女声",
        lang: "en",
        accent: "gb",
        gender: "female",
        quality: "medium",
        bytes: 63_201_294,
        hf_dir: "en/en_GB/alba/medium",
        preset: false,
    },
    VoiceSpec {
        id: "en_US-ryan-high",
        label: "Ryan · 美式男声（高音质）",
        lang: "en",
        accent: "us",
        gender: "male",
        quality: "high",
        bytes: 120_786_792,
        hf_dir: "en/en_US/ryan/high",
        preset: false,
    },
    VoiceSpec {
        id: "zh_CN-huayan-medium",
        label: "华言 · 中文女声",
        lang: "zh",
        accent: "",
        gender: "female",
        quality: "medium",
        bytes: 63_201_294,
        hf_dir: "zh/zh_CN/huayan/medium",
        preset: false,
    },
    VoiceSpec {
        id: "zh_CN-huayan-x_low",
        label: "华言 · 中文女声（小体积 20MB）",
        lang: "zh",
        accent: "",
        gender: "female",
        quality: "x_low",
        bytes: 20_628_813,
        hf_dir: "zh/zh_CN/huayan/x_low",
        preset: false,
    },
];

/// 按 id 找语音描述。
pub fn spec(id: &str) -> Option<&'static VoiceSpec> {
    VOICES.iter().find(|v| v.id == id)
}

/// 按语言（+口音）挑一条已安装的语音；挑不到返回 None。
///
/// 打分：语言必须一致；口音一致 +30；medium/high 比 x_low 更受青睐。
pub fn pick_voice_for_lang(lang: &str, accent: &str, installed: &[String]) -> Option<String> {
    let base = lang.trim().to_lowercase();
    let base = base.split(['-', '_']).next().unwrap_or("");
    let mut best: Option<(&VoiceSpec, i32)> = None;
    for id in installed {
        let Some(s) = spec(id) else { continue };
        if s.lang != base {
            continue;
        }
        let mut score = 0;
        if !accent.is_empty() && s.accent == accent {
            score += 30;
        }
        score += match s.quality {
            "high" => 20,
            "medium" => 10,
            _ => 0,
        };
        if s.preset {
            score += 1; // 同分时优先预置的那条
        }
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

/// 扫描已安装的语音 id（`onnx` 与 `json` 都在才算装好）。
///
/// 为什么要两个文件都在：只下到一半时 `onnx` 是完整的但缺配置，
/// piper 启动会直接报错。把它们当作一个原子整体，界面才不会出现
/// 「显示已安装、一点就失败」的状态。
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
        if voice_onnx(models_dir, &id).is_file() && voice_json(models_dir, &id).is_file() {
            out.push(id);
        }
    }
    out.sort();
    out
}

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
   vendor/tts-voices/<id>/<id>.onnx     预置语音（当前只有英文）
   ```

   查找顺序是「下载的优先，随包的兜底」。反过来不行：用户在设置页
   新下载一条语音，不该被安装包里的旧版本盖住。

   随包资源**不复制**到模型目录 —— 直接原地读取。程序装在 Program Files
   也不影响（我们只读），省掉首次启动复制 60 MB 的开销。
*/

/// 随包引擎目录。
pub fn bundled_engine_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("vendor").join("piper")
}

/// 随包语音根目录。
pub fn bundled_voices_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("vendor").join("tts-voices")
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
    let (o, j) = (voice_onnx(models_dir, id), voice_json(models_dir, id));
    if o.is_file() && j.is_file() {
        return Some((o, j));
    }
    let b = bundled_voices_dir(app_dir).join(id);
    let (o2, j2) = (b.join(format!("{id}.onnx")), b.join(format!("{id}.onnx.json")));
    if o2.is_file() && j2.is_file() {
        return Some((o2, j2));
    }
    None
}

/// 当前**可用**（不管来自下载还是随包）的语音 id 列表。
pub fn available_voices(app_dir: &Path, models_dir: &Path) -> Vec<String> {
    let mut out = installed_voices(models_dir);
    let bundled = bundled_voices_dir(app_dir);
    if let Ok(rd) = std::fs::read_dir(&bundled) {
        for e in rd.flatten() {
            if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let id = e.file_name().to_string_lossy().to_string();
            if out.contains(&id) {
                continue;
            }
            let b = bundled.join(&id);
            if b.join(format!("{id}.onnx")).is_file() && b.join(format!("{id}.onnx.json")).is_file()
            {
                out.push(id);
            }
        }
    }
    out.sort();
    out
}

/// 这条语音从哪来：`downloaded` / `bundled` / `none`（界面据此显示标签）。
pub fn voice_source(app_dir: &Path, models_dir: &Path, id: &str) -> &'static str {
    if voice_onnx(models_dir, id).is_file() && voice_json(models_dir, id).is_file() {
        return "downloaded";
    }
    let b = bundled_voices_dir(app_dir).join(id);
    if b.join(format!("{id}.onnx")).is_file() && b.join(format!("{id}.onnx.json")).is_file() {
        return "bundled";
    }
    "none"
}

/// 删除语音时，随包的那份**不能删**（它在安装目录里，删了下次更新又回来；
/// 而且用户可能只是想「不要它」，那应该在设置里禁用而不是删文件）。
/// 这个函数回答「删除操作删的到底是哪一份」。
pub fn is_downloaded_voice(models_dir: &Path, id: &str) -> bool {
    voice_onnx(models_dir, id).is_file() && voice_json(models_dir, id).is_file()
}

/// 某个文件的候选下载地址（自建 Release 优先，HF 回退）。
pub fn file_urls(file: &str, hf_path: &str) -> Vec<String> {
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
/// 单次调用在 medium 模型上约 100~400ms，首次会多花一次模型加载时间。
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
        for v in VOICES {
            assert!(seen.insert(v.id), "语音 id 重复：{}", v.id);
            assert!(!v.label.is_empty());
            let files = voice_files(v);
            assert_eq!(files.len(), 2, "每条语音都该有 onnx + json 两个文件");
            for (name, path) in &files {
                assert!(name.starts_with(v.id), "文件名该以 id 开头：{name}");
                assert!(path.starts_with(v.hf_dir), "HF 路径该落在 hf_dir 下：{path}");
            }
        }
    }

    #[test]
    fn exactly_one_voice_is_preset() {
        let n = VOICES.iter().filter(|v| v.preset).count();
        assert_eq!(n, 1, "预置语音只能有一条（英文默认），否则安装包会失控地变大");
        let p = VOICES.iter().find(|v| v.preset).unwrap();
        assert_eq!(p.lang, "en", "预置的应该是英文语音");
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
        // 英式 → 只有 alba 是英式
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

    #[test]
    fn installed_detection_requires_both_files() {
        let tmp = std::env::temp_dir().join(format!("ww-tts-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        // 只放 onnx、不放 json → 不算装好
        let d = voice_dir(&tmp, "en_US-amy-medium");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(voice_onnx(&tmp, "en_US-amy-medium"), b"x").unwrap();
        assert!(installed_voices(&tmp).is_empty(), "缺 json 不该算已安装");
        // 补上 json → 装好
        std::fs::write(voice_json(&tmp, "en_US-amy-medium"), b"{}").unwrap();
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
