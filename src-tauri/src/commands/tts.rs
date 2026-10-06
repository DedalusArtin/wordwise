//! 朗读（TTS）命令层。
//!
//! 与 `local-llm` 那套是同一个思路：资源按需下载、进度实时上报、失败
//! 给出可读原因。区别在于 TTS 的**每次发声**都要经过后端（piper 是外部
//! 进程），所以这里还有一个合成命令。
//!
//! 事件：`tts://progress`（下载/解压）、`tts://done`（合成完成，供界面
//! 显示耗时与是否命中缓存）。

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
#[tauri::command]
pub fn cmd_tts_status(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let dir = models_dir(&state);
    let cfg = state.cfg();

    let installed = tts::installed_voices(&dir);
    let voices: Vec<serde_json::Value> = tts::VOICES
        .iter()
        .map(|v| {
            let is_in = installed.iter().any(|i| i == v.id);
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
                "installed": is_in,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "engine_ready": tts::engine_ready(&dir),
        "engine_bytes": tts::ENGINE_BYTES,
        "engine_size_text": localllm::human_bytes(tts::ENGINE_BYTES),
        "engine_path": tts::piper_exe(&dir).display().to_string(),
        "voices_dir": tts::voices_dir(&dir).display().to_string(),
        "models_dir": dir.display().to_string(),
        "installed": installed,
        "voices": voices,
        "config": cfg.tts,
    }))
}

/// 安装 Piper 引擎（下载 zip → 解压到 `<models_dir>/tts/`）。
#[tauri::command]
pub async fn cmd_tts_install_engine(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    let dir = models_dir(&state);
    if tts::engine_ready(&dir) {
        return Ok(serde_json::json!({
            "ok": true,
            "already": true,
            "message": "语音引擎已就绪，无需重复下载",
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

    if !tts::engine_ready(&dir) {
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

/// 删除一条已安装的语音包。
#[tauri::command]
pub fn cmd_tts_remove_voice(
    state: State<'_, Arc<AppState>>,
    voice_id: String,
) -> Result<serde_json::Value, String> {
    let dir = models_dir(&state);
    let vdir = tts::voice_dir(&dir, &voice_id);
    // 只允许删清单里认识的 id，避免任意路径删除
    if tts::spec(&voice_id).is_none() {
        return Err(format!("未知的语音：{voice_id}"));
    }
    if vdir.is_dir() {
        std::fs::remove_dir_all(&vdir).map_err(err)?;
    }
    Ok(serde_json::json!({ "ok": true, "message": "已删除" }))
}

/// 取消进行中的下载。
#[tauri::command]
pub fn cmd_tts_cancel() {
    cancel_flag().store(true, Ordering::Relaxed);
}

/// 读取朗读偏好。
#[tauri::command]
pub fn cmd_tts_prefs(state: State<'_, Arc<AppState>>) -> TtsConfig {
    state.cfg().tts
}

/// 写入朗读偏好（只有这个面板会改，单独一个命令比整份 save_config 稳）。
#[tauri::command]
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
    if !tts::engine_ready(&dir) {
        return Err("语音引擎未安装".to_string());
    }

    let cfg = state.cfg();
    let installed = tts::installed_voices(&dir);
    if installed.is_empty() {
        return Err("还没有安装任何语音包".to_string());
    }

    // 选定语音：用户指定 > 按语言自动挑 > 第一条已装的
    let l = lang.unwrap_or_default();
    let a = accent.unwrap_or_default();
    let chosen = if !cfg.tts.voice_local.is_empty() && installed.contains(&cfg.tts.voice_local) {
        cfg.tts.voice_local.clone()
    } else {
        tts::pick_voice_for_lang(&l, &a, &installed)
            .or_else(|| installed.first().cloned())
            .ok_or_else(|| "没有可用的语音包".to_string())?
    };

    let rate = cfg.tts.rate.clamp(0.5, 2.0);
    let cache = tts::cache_dir(&state.data_dir);
    let key = tts::cache_key(&chosen, &text, rate);
    let wav_path = cache.join(format!("{key}.wav"));

    let t0 = std::time::Instant::now();

    // 缓存命中：连 piper 都不用起
    if let Ok(bytes) = std::fs::read(&wav_path) {
        if bytes.len() > 64 {
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

    let exe = tts::piper_exe(&dir);
    let model = tts::voice_onnx(&dir, &chosen);
    let text2 = text.clone();
    let wav = tokio::task::spawn_blocking(move || tts::synth_blocking(&exe, &model, &text2, rate))
        .await
        .map_err(|e| format!("合成任务失败：{e}"))?
        .map_err(err)?;

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

/// 清空 TTS 缓存（设置页的「清理缓存」）。
#[tauri::command]
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
