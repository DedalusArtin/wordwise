//! 朗读（TTS）命令层。
//!
//! 与 `local-llm` 那套是同一个思路：资源按需下载、进度实时上报、失败
//! 给出可读原因。区别在于 TTS 的**每次发声**都要经过后端（piper 是外部
//! 进程），所以这里还有一个合成命令。
//!
//! 事件：`tts://progress`（下载/解压）、`tts://done`（合成完成，供界面
//! 显示耗时与是否命中缓存）。

use crate::commands::localllm::app_dir;
use crate::localllm::{self, Progress};
use crate::models::TtsConfig;
use crate::state::AppState;
use crate::tts;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// 下载/解压进度事件名。
pub const EVT_PROGRESS: &str = "tts://progress";

/// 合成结果事件名。
pub const EVT_DONE: &str = "tts://done";

/// 下载取消标志。
///
/// 刻意与 `localllm` 的取消标志**分开**：两边可能同时在跑，共用一个
/// 标志会让「取消语音下载」顺手把模型下载也掐掉。
fn cancel_flag() -> Arc<AtomicBool> {
    static FLAG: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();
    FLAG.get_or_init(|| Arc::new(AtomicBool::new(false))).clone()
}

/// 当前语音模型所在目录（跟随「模型目录」设置）。
fn models_dir(state: &AppState) -> std::path::PathBuf {
    localllm::models_dir(&state.data_dir)
}

/// TTS 总体状态：引擎是否就绪、装了哪些语音、有哪些可选。
#[tauri::command(async)]
pub fn cmd_tts_status(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let dir = models_dir(&state);
    let exedir = app_dir();
    let cfg = state.cfg();

    // 「可用」= 用户下载的 ∪ 随安装包预置的。两者都直接原地读取，
    // 预置的那份不会复制过来，省掉首次启动 60 MB 的搬运。
    let installed = tts::available_voices(&exedir, &dir);
    let engine = tts::resolve_engine(&exedir, &dir);
    let engine_bundled = engine
        .as_ref()
        .map(|p| p.starts_with(tts::bundled_engine_dir(&exedir)))
        .unwrap_or(false);

    let voices: Vec<serde_json::Value> = tts::VOICES
        .iter()
        .map(|v| {
            let src = tts::voice_source(&exedir, &dir, v.id);
            serde_json::json!({
                "id": v.id,
                "label": v.label,
                "lang": v.lang,
                "accent": v.accent,
                "gender": v.gender,
                "quality": v.quality,
                "bytes": v.bytes,
                "size_text": localllm::human_bytes(v.bytes),
                "preset": v.preset,
                "note": v.note,
                "installed": src != "none",
                // downloaded / bundled / none —— 界面据此显示「已下载」还是「已预置」
                "source": src,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "engine_ready": engine.is_some(),
        "engine_bundled": engine_bundled,
        "engine_bytes": tts::ENGINE_BYTES,
        "engine_size_text": localllm::human_bytes(tts::ENGINE_BYTES),
        "engine_path": engine.map(|p| p.display().to_string()).unwrap_or_default(),
        "voices_dir": tts::voices_dir(&dir).display().to_string(),
        "models_dir": dir.display().to_string(),
        "installed": installed,
        "voices": voices,
        "config": cfg.tts,
        // ── 本平台能不能真的跑 Piper ──
        // Piper 和 llama-server 一样是外部二进制，Android 的 W^X 规则
        // 同样拦它。界面据此把「下载语音引擎 / 语音包」收起来，
        // 免得用户先下 60 MB，再发现永远合成不出来。
        "supported": piper_supported(),
        "unsupported_reason": piper_unsupported_reason(),
    }))
}

/// 本地神经语音（Piper）在本平台能不能跑。
#[cfg(not(target_os = "android"))]
pub fn piper_supported() -> bool {
    true
}
#[cfg(target_os = "android")]
pub fn piper_supported() -> bool {
    false
}

/// 不支持时给用户的解释。
#[cfg(not(target_os = "android"))]
pub fn piper_unsupported_reason() -> Option<&'static str> {
    None
}
#[cfg(target_os = "android")]
pub fn piper_unsupported_reason() -> Option<&'static str> {
    Some("Android 不允许 App 执行自带的外部二进制（W^X），Piper 神经语音在移动端不可用。朗读会走系统语音。")
}

/// 移动端早退：装了也用不了，别让用户白下几十 MB。
fn reject_if_unsupported() -> Result<(), String> {
    if !piper_supported() {
        return Err(piper_unsupported_reason()
            .unwrap_or("本平台不支持本地神经语音")
            .to_string());
    }
    Ok(())
}

/// 安装 Piper 引擎（下载 zip → 解压到 `<models_dir>/tts/`）。
#[tauri::command]
pub async fn cmd_tts_install_engine(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    reject_if_unsupported()?;
    let dir = models_dir(&state);
    let exedir = app_dir();
    if let Some(e) = tts::resolve_engine(&exedir, &dir) {
        let bundled = e.starts_with(tts::bundled_engine_dir(&exedir));
        return Ok(serde_json::json!({
            "ok": true,
            "already": true,
            "message": if bundled {
                "语音引擎已随程序安装，无需下载"
            } else {
                "语音引擎已就绪，无需重复下载"
            },
        }));
    }

    let flag = cancel_flag();
    flag.store(false, Ordering::Relaxed);
    let _ = app.emit(
        EVT_PROGRESS,
        Progress::stage("engine", "download", "准备下载语音引擎…"),
    );

    let root = tts::tts_root(&dir);
    std::fs::create_dir_all(&root).map_err(err)?;
    // zip 下在 tts/ 下，解压完再删 —— 放 tts/ 而不是 tts/piper/，
    // 因为 piper 的包里顶层就已经是 piper/ 目录了。
    let zip_path = root.join("piper-engine.zip");

    let urls = tts::engine_urls();
    let client = state.http();
    let app2 = app.clone();
    localllm::download_with_progress(&client, &urls, &zip_path, "engine", flag, move |p| {
        let _ = app2.emit(EVT_PROGRESS, p);
    })
    .await
    .map_err(err)?;

    let _ = app.emit(
        EVT_PROGRESS,
        Progress::stage("engine", "unpack", "正在解压语音引擎…"),
    );
    let n = tts::unpack_engine(&zip_path, &root).map_err(err)?;
    let _ = std::fs::remove_file(&zip_path);

    if tts::resolve_engine(&app_dir(), &dir).is_none() {
        return Err(format!(
            "解压完成（{n} 个文件）但没找到 piper.exe，引擎包结构可能变了"
        ));
    }

    let _ = app.emit(
        EVT_PROGRESS,
        Progress::stage("engine", "done", &format!("语音引擎就绪（{n} 个文件）")),
    );
    Ok(serde_json::json!({
        "ok": true,
        "already": false,
        "files": n,
        "message": format!("语音引擎安装完成（{n} 个文件）"),
    }))
}

/// 下载一条语音包（onnx + onnx.json 两个文件）。
#[tauri::command]
pub async fn cmd_tts_install_voice(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
    voice_id: String,
) -> Result<serde_json::Value, String> {
    reject_if_unsupported()?;
    let spec = tts::spec(&voice_id).ok_or_else(|| format!("未知的语音：{voice_id}"))?;
    let dir = models_dir(&state);
    let vdir = tts::voice_dir(&dir, &voice_id);
    std::fs::create_dir_all(&vdir).map_err(err)?;

    let flag = cancel_flag();
    flag.store(false, Ordering::Relaxed);

    let client = state.http();
    let files = tts::voice_files(spec);
    let total = files.len();
    for (i, (name, hf_path)) in files.iter().enumerate() {
        let dest = vdir.join(name);
        if dest.is_file() {
            continue; // 已下好的那个文件不重复下（两个文件分别续传）
        }
        let _ = app.emit(
            EVT_PROGRESS,
            Progress::stage(
                "voice",
                "download",
                &format!("正在下载 {}（{}/{}）", spec.label, i + 1, total),
            ),
        );
        let urls = tts::file_urls(name, hf_path);
        let app2 = app.clone();
        localllm::download_with_progress(&client, &urls, &dest, "voice", flag.clone(), move |p| {
            let _ = app2.emit(EVT_PROGRESS, p);
        })
        .await
        .map_err(err)?;
    }

    // 两个文件都到位才算装好；少一个就清掉整个目录，避免留下
    // 「界面显示已安装、一点就报错」的半成品状态。
    if !tts::installed_voices(&dir).iter().any(|i| i == &voice_id) {
        let _ = std::fs::remove_dir_all(&vdir);
        return Err("下载未完成，已回滚。请重试或换个下载源。".to_string());
    }

    let _ = app.emit(
        EVT_PROGRESS,
        Progress::stage("voice", "done", &format!("{} 已安装", spec.label)),
    );
    Ok(serde_json::json!({
        "ok": true,
        "id": voice_id,
        "message": format!("{} 已安装", spec.label),
    }))
}

/// 删除一条已下载的语音包。
///
/// 随安装包预置的那份**删不掉**：它在程序目录里，删了下次覆盖安装又回来，
/// 而且通常没有写权限。这种情况如实说明，而不是报一个看不懂的错误。
#[tauri::command(async)]
pub fn cmd_tts_remove_voice(
    state: State<'_, Arc<AppState>>,
    voice_id: String,
) -> Result<serde_json::Value, String> {
    let dir = models_dir(&state);
    // 只允许删清单里认识的 id，避免任意路径删除
    if tts::spec(&voice_id).is_none() {
        return Err(format!("未知的语音：{voice_id}"));
    }
    if !tts::is_downloaded_voice(&dir, &voice_id) {
        return Ok(serde_json::json!({
            "ok": true,
            "removed": false,
            "message": "这条语音是随程序预置的，不需要也无法删除",
        }));
    }
    std::fs::remove_dir_all(tts::voice_dir(&dir, &voice_id)).map_err(err)?;
    Ok(serde_json::json!({
        "ok": true,
        "removed": true,
        "message": "已删除（预置的语音仍然可用）",
    }))
}

/// 取消进行中的下载。
#[tauri::command(async)]
pub fn cmd_tts_cancel() {
    cancel_flag().store(true, Ordering::Relaxed);
}

/// 读取朗读偏好。
#[tauri::command(async)]
pub fn cmd_tts_prefs(state: State<'_, Arc<AppState>>) -> TtsConfig {
    state.cfg().tts
}

/// 写入朗读偏好（只有这个面板会改，单独一个命令比整份 save_config 稳）。
#[tauri::command(async)]
pub fn cmd_set_tts_prefs(
    state: State<'_, Arc<AppState>>,
    engine: Option<String>,
    voice_local: Option<String>,
    voice_online: Option<String>,
    rate: Option<f32>,
) -> Result<serde_json::Value, String> {
    state
        .update_config(|cfg| {
            if let Some(e) = engine {
                let e = e.trim().to_lowercase();
                if ["auto", "local", "online", "system"].contains(&e.as_str()) {
                    cfg.tts.engine = e;
                }
            }
            if let Some(v) = voice_local {
                // 允许空串（= 自动挑），但非空的必须是清单里认识的
                if v.trim().is_empty() || tts::spec(&v).is_some() {
                    cfg.tts.voice_local = v.trim().to_string();
                }
            }
            if let Some(v) = voice_online {
                cfg.tts.voice_online = v.trim().to_string();
            }
            if let Some(r) = rate {
                if r.is_finite() {
                    cfg.tts.rate = r.clamp(0.5, 2.0);
                }
            }
        })
        .map_err(err)?;
    Ok(serde_json::json!({ "ok": true }))
}

/// 合成一段语音，返回 base64 WAV。
///
/// `lang` / `accent` 用于在用户没指定语音时按词条语言自动挑一条。
#[tauri::command]
pub async fn cmd_tts_speak(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
    text: String,
    lang: Option<String>,
    accent: Option<String>,
) -> Result<tts::SynthOut, String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err("没有可朗读的内容".to_string());
    }
    if text.chars().count() > 400 {
        return Err("文本过长（最多 400 字），请分段朗读".to_string());
    }

    let dir = models_dir(&state);
    let exedir = app_dir();
    let Some(exe) = tts::resolve_engine(&exedir, &dir) else {
        return Err("语音引擎未安装".to_string());
    };

    let cfg = state.cfg();
    let installed = tts::available_voices(&exedir, &dir);
    if installed.is_empty() {
        return Err("还没有可用的语音包".to_string());
    }

    // 选定语音：用户指定 > 按语言自动挑。
    //
    // ★ 这里**刻意不做**「挑不中就随便拿第一条已装语音」的兜底：
    //   第一条通常就是英语语音，拿它去读日语/韩语会念出一串怪音，
    //   用户还以为是「软件发音不准」。`pick_voice_for_lang` 已经改成
    //   绝不跨语言降级（日语/韩语在 Piper 官方没有与随包引擎兼容的语音，
    //   会返回 `None`），此时如实报错，前端会据此回退到**系统语音**
    //   （Windows/Android 自带日韩语音），效果远好于错语言合成。
    let l = lang.unwrap_or_default();
    let a = accent.unwrap_or_default();
    let chosen = if !cfg.tts.voice_local.is_empty() && installed.contains(&cfg.tts.voice_local) {
        cfg.tts.voice_local.clone()
    } else {
        tts::pick_voice_for_lang(&l, &a, &installed).ok_or_else(|| {
            // 语言名要说人话：`en` 这种代码直接塞进提示，用户看不懂也不会
            // 联想到「去哪儿装」。这里换成「英语」并顺带指向解决办法。
            let lang_name = if l.trim().is_empty() {
                "该语言".to_string()
            } else {
                crate::translate::lang_name(&crate::tts::lang_base(l.trim()))
            };
            format!("本地语音包里没有{lang_name}语音，已改用系统语音；可在「设置 → 朗读」下载对应语音包")
        })?
    };

    let rate = cfg.tts.rate.clamp(0.5, 2.0);
    let cache = tts::cache_dir(&state.data_dir);
    // ★ 键里不带目标采样率：缓存只存语音包原生率，磁盘占用才是 1 倍
    let key = tts::cache_key(&chosen, &text, rate);
    let wav_path = cache.join(format!("{key}.wav"));

    let t0 = std::time::Instant::now();

    // 缓存命中：连 piper 都不用起
    if let Ok(bytes) = std::fs::read(&wav_path) {
        if bytes.len() > 64 {
            // ★ 旧缓存可能是 piper 流式写坏的（尺寸字段与实际不符）——
            //   命中后先规范化，修好的版本回写缓存，坏缓存由此自愈。
            let bytes = tts::normalize_wav(&bytes);
            let _ = std::fs::write(&wav_path, &bytes);
            let out = tts::SynthOut {
                audio: encode_wav(&bytes),
                sample_rate: tts::wav_sample_rate(&bytes),
                bytes: bytes.len(),
                cached: true,
                voice: chosen,
                elapsed_ms: t0.elapsed().as_millis() as u64,
            };
            let _ = app.emit(EVT_DONE, &out);
            return Ok(out);
        }
    }

    let (model, _json) = tts::resolve_voice(&exedir, &dir, &chosen)
        .ok_or_else(|| format!("语音包 {chosen} 不完整"))?;
    let text2 = text.clone();
    let wav = tokio::task::spawn_blocking(move || tts::synth_blocking(&exe, &model, &text2, rate))
        .await
        .map_err(|e| format!("合成任务失败：{e}"))?
        .map_err(err)?;

    // ★ 规范化后再落盘：piper 流式输出的尺寸字段可能与实际差上百字节，
    //   直接缓存的话，下次命中还是播不出来（重新下载语音包也无解——
    //   坏产物会原样再生成）。规范化后尺寸字段与数据严格一致。
    let wav = tts::normalize_wav(&wav);

    // 写缓存（失败不影响这次播放）
    let _ = std::fs::create_dir_all(&cache);
    if std::fs::write(&wav_path, &wav).is_err() {
        log::warn!("TTS 缓存写入失败：{}", wav_path.display());
    }

    let out = tts::SynthOut {
        audio: encode_wav(&wav),
        sample_rate: tts::wav_sample_rate(&wav),
        bytes: wav.len(),
        cached: false,
        voice: chosen,
        elapsed_ms: t0.elapsed().as_millis() as u64,
    };
    let _ = app.emit(EVT_DONE, &out);
    Ok(out)
}

/// 枚举本机音频输出设备（读 Windows 注册表 MMDevices，绕过 WebView2 的
/// enumerateDevices 权限限制——那边拿不到设备真名，本命令能拿到全部
/// 渲染终结点的名字与状态）。DeviceState：1=活动 2=禁用 4=未插入 8=拔出。
#[tauri::command(async)]
pub fn cmd_audio_devices() -> Result<serde_json::Value, String> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;

    const FRIENDLY: &str = "{a45c254e-df1c-4efd-8020-67d146a850e0},2";
    let hk = RegKey::predef(HKEY_LOCAL_MACHINE);
    let root = hk
        .open_subkey_with_flags(
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render",
            winreg::enums::KEY_READ,
        )
        .map_err(|e| format!("读注册表失败：{e}"))?;

    let mut devs: Vec<serde_json::Value> = Vec::new();
    for guid in root.enum_keys().flatten() {
        let ep = match root.open_subkey_with_flags(&guid, winreg::enums::KEY_READ) {
            Ok(k) => k,
            Err(_) => continue,
        };
        let state: u32 = ep.get_value("DeviceState").unwrap_or(0);
        let name: String = ep
            .open_subkey_with_flags("Properties", winreg::enums::KEY_READ)
            .and_then(|p| p.get_value(FRIENDLY))
            .unwrap_or_else(|_| format!("未知设备（{guid}）"));
        // 只报状态 1（活动）与 4（未插入，插上就能用）；禁用/拔出的噪音不进列表
        if state != 1 && state != 4 {
            continue;
        }
        devs.push(serde_json::json!({
            "name": name,
            "active": state == 1,
        }));
    }
    // 活动的排前面
    devs.sort_by(|a, b| {
        let ka = if a["active"].as_bool().unwrap_or(false) { 0 } else { 1 };
        let kb = if b["active"].as_bool().unwrap_or(false) { 0 } else { 1 };
        ka.cmp(&kb).then(a["name"].as_str().cmp(&b["name"].as_str()))
    });
    Ok(serde_json::json!({ "devices": devs }))
}

/// 清空 TTS 缓存（设置页的「清理缓存」）。
#[tauri::command(async)]
pub fn cmd_tts_clear_cache(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let dir = tts::cache_dir(&state.data_dir);
    let mut n = 0usize;
    let mut bytes = 0u64;
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) == Some("wav") {
                bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
                if std::fs::remove_file(&p).is_ok() {
                    n += 1;
                }
            }
        }
    }
    Ok(serde_json::json!({
        "ok": true,
        "removed": n,
        "freed": localllm::human_bytes(bytes),
    }))
}

/// WAV → `data:audio/wav;base64,…`
fn encode_wav(bytes: &[u8]) -> String {
    use base64::Engine as _;
    format!(
        "data:audio/wav;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_url_has_wav_mime_prefix() {
        let s = encode_wav(b"RIFF....WAVE");
        assert!(s.starts_with("data:audio/wav;base64,"), "前端按前缀判断能否直接播");
        assert!(s.len() > 22);
    }
}
