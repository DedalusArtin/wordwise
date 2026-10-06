//! 本地大模型一键部署的命令层。
//!
//! 界面上的「一键部署」在这里拆成可观察的三步：装引擎 → 下模型 → 起服务。
//! 每一步都通过 `local-llm://progress` 事件把进度推给前端，用户不会面对
//! 一个转了几分钟的圈却不知道在干什么。

use crate::localllm::{self, Progress};
use crate::state::AppState;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// 进度事件名。
pub const EVT_PROGRESS: &str = "local-llm://progress";

/// exe 所在目录（随安装包分发的引擎就在它的 `vendor/llama` 下）。
pub(crate) fn app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|x| x.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 当前部署状态。
#[tauri::command]
pub fn cmd_local_llm_status(
    state: State<'_, Arc<AppState>>,
    _app: AppHandle,
) -> Result<serde_json::Value, String> {
    let data_dir = state.data_dir.clone();
    let dir = app_dir();

    let bundled = localllm::bundled_engine_dir(&dir);
    let downloaded = localllm::downloaded_engine_dir(&data_dir);
    let engine = localllm::find_engine(&dir, &data_dir);

    // 模型目录里实际存在哪些文件（用清单去对，避免把临时文件也算进去）
    let mdir = localllm::models_dir(&data_dir);
    let installed: Vec<String> = localllm::models()
        .iter()
        .filter(|m| mdir.join(&m.file).is_file())
        .map(|m| m.id.clone())
        .collect();

    // 已下载模型的真实体积
    let mut installed_detail = Vec::new();
    for m in localllm::models() {
        let p = mdir.join(&m.file);
        if let Ok(meta) = std::fs::metadata(&p) {
            installed_detail.push(serde_json::json!({
                "id": m.id,
                "path": p.display().to_string(),
                "size_text": localllm::human_bytes(meta.len()),
            }));
        }
    }

    let (running, port, model) = localllm::server_state();
    let cfg = state.cfg();
    // 当前 AI 用的是不是我们托管的这个服务
    let using_ours = running && cfg.llm.base_url.contains(&format!("127.0.0.1:{port}"));

    Ok(serde_json::json!({
        "engine": {
            "ready": engine.is_some(),
            "dir": engine.as_ref().map(|p| p.display().to_string()),
            "bundled_dir": bundled.display().to_string(),
            "bundled_ready": bundled.join(localllm::server_exe_name()).is_file(),
            "downloaded_dir": downloaded.display().to_string(),
            "tag": localllm::ENGINE_TAG,
            "asset": localllm::engine_asset(),
            "exe": localllm::server_exe_name(),
        },
        "models": {
            "dir": mdir.display().to_string(),
            "installed": installed,
            "detail": installed_detail,
        },
        "server": {
            "running": running,
            "port": port,
            "model": model,
            "using_ours": using_ours,
            "base_url": cfg.llm.base_url,
        },
        "threads": localllm::default_threads(),
        "cpu": std::thread::available_parallelism().map(|x| x.get()).unwrap_or(0),
        "app_dir": dir.display().to_string(),
        // 左下角弹层里的「启动时自动拉起」开关要读它
        "auto_start": cfg.auto_start_local_llm,
        // 引擎没有随包分发时，只能走镜像下载 —— 提前把预期告诉用户
        "engine_needs_download": !bundled.join(localllm::server_exe_name()).is_file(),
        // ── 本平台到底能不能跑本地模型 ──
        // 见 `mobile_unsupported_reason()` 里那句解释：这不是「没装好」，
        // 而是平台不允许。状态里写明，界面才能把按钮直接置灰。
        "supported": local_model_supported(),
        "unsupported_reason": local_model_unsupported_reason(),
    }))
}

/// 本地模型在本平台能不能真的跑起来。
#[cfg(not(target_os = "android"))]
pub fn local_model_supported() -> bool {
    true
}

/// Android：**不能**。
///
/// 根因是 Android 从 API 29 起禁止 App 执行自己数据目录里的外部二进制
/// （W^X，即「可写就不可执行」）。llama-server 是 `std::process::Command`
/// 拉起的独立可执行文件，无论把它放在应用的哪个私有目录下，都会在执行那
/// 一步被系统拒绝 —— 这不是权限没申请、也不是参数没调好，是平台规则。
///
/// 所以 APK 版本的「模型能力」必然低于桌面版：只能用在线 API，本机模型
/// 这条路在移动端是封死的。与其让用户点「一键部署」然后收到一串看不懂的
/// spawn 失败，不如在状态里就说清楚。
#[cfg(target_os = "android")]
pub fn local_model_supported() -> bool {
    false
}

/// 不支持时给用户的解释（支持时为 `None`）。
#[cfg(not(target_os = "android"))]
pub fn local_model_unsupported_reason() -> Option<&'static str> {
    None
}
#[cfg(target_os = "android")]
pub fn local_model_unsupported_reason() -> Option<&'static str> {
    Some("Android 不允许 App 执行自带的外部二进制（W^X），本地模型在移动端无法启动。请改用在线 API。")
}

/// 移动端早退：本地模型在 Android 上根本起不来（见 `local_model_supported`）。
///
/// 放在命令入口而不是放在 spawn 之后：spawn 失败时用户看到的是
/// 「Permission denied (os error 13)」这种和真实原因隔着三层的报错，
/// 而这里能直接说「平台不支持，请改用在线 API」。
fn reject_if_unsupported() -> Result<(), String> {
    if !local_model_supported() {
        return Err(local_model_unsupported_reason()
            .unwrap_or("本平台不支持本地模型")
            .to_string());
    }
    Ok(())
}

/// 可选模型清单。
#[tauri::command]
pub fn cmd_local_llm_models() -> Vec<localllm::ModelSpec> {
    localllm::models()
}

/// 取消进行中的下载。
#[tauri::command]
pub fn cmd_local_llm_cancel() {
    cancel_flag().store(true, Ordering::Relaxed);
}

/// 安装引擎：优先用随包的，没有才下载解压安装到数据目录。
#[tauri::command]
pub async fn cmd_local_llm_install_engine(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    reject_if_unsupported()?;
    let data_dir = state.data_dir.clone();
    let dir = app_dir();

    // 随包已有 → 直接可用，一步都不用等
    if let Some(e) = localllm::find_engine(&dir, &data_dir) {
        let bundled = e.starts_with(localllm::bundled_engine_dir(&dir));
        return Ok(serde_json::json!({
            "ok": true,
            "already": true,
            "path": e.display().to_string(),
            "from_bundle": bundled,
            "message": if bundled {
                "引擎已随程序安装，无需下载".to_string()
            } else {
                "引擎已就绪".to_string()
            },
        }));
    }

    cancel_flag().store(false, Ordering::Relaxed);
    let _ = app.emit(EVT_PROGRESS, Progress::stage("engine", "download", "准备下载推理引擎…"));

    let asset = localllm::engine_asset();
    let urls: Vec<String> = localllm::ENGINE_MIRRORS
        .iter()
        .map(|m| format!("{m}/{}/{}", localllm::ENGINE_TAG, asset))
        .collect();

    let dest_dir = localllm::downloaded_engine_dir(&data_dir);
    let zip_path = dest_dir.join(asset);

    let client = state.http();
    let app2 = app.clone();
    localllm::download_with_progress(&client, &urls, &zip_path, "engine", cancel_flag(), move |p| {
        let _ = app2.emit(EVT_PROGRESS, p);
    })
    .await
    .map_err(err)?;

    let _ = app.emit(EVT_PROGRESS, Progress::stage("engine", "unpack", "正在解压引擎…"));
    let n = localllm::unpack_engine(&zip_path, &dest_dir).map_err(err)?;
    let _ = std::fs::remove_file(&zip_path);

    let _ = app.emit(
        EVT_PROGRESS,
        Progress::stage("engine", "done", &format!("引擎就绪（{n} 个文件）")),
    );

    Ok(serde_json::json!({
        "ok": true,
        "already": false,
        "path": dest_dir.display().to_string(),
        "files": n,
        "message": format!("引擎安装完成，共 {n} 个文件"),
    }))
}

/// 需要取消时该怎么办：这里暴露一个能被闭包读到的标志。
///
/// 单独抽出来是因为 `download_with_progress` 要求 `Arc<AtomicBool>`，
/// 而取消标志又必须能被跨命令的 `cmd_local_llm_cancel` 设置 ——
/// 两者得指向同一个对象。
fn cancel_flag() -> Arc<AtomicBool> {
    static FLAG: std::sync::OnceLock<Arc<AtomicBool>> = std::sync::OnceLock::new();
    FLAG.get_or_init(|| Arc::new(AtomicBool::new(false))).clone()
}

/// 下载一个模型。
#[tauri::command]
pub async fn cmd_local_llm_install_model(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
    model_id: String,
) -> Result<serde_json::Value, String> {
    let spec = localllm::find_model(&model_id)
        .ok_or_else(|| format!("未知的模型：{model_id}"))?;
    let dir = localllm::models_dir(&state.data_dir);
    let dest = dir.join(&spec.file);

    if dest.is_file() {
        return Ok(serde_json::json!({
            "ok": true,
            "already": true,
            "path": dest.display().to_string(),
            "message": format!("{} 已下载，无需重复下载", spec.name),
        }));
    }

    let flag = cancel_flag();
    flag.store(false, Ordering::Relaxed);
    let _ = app.emit(
        EVT_PROGRESS,
        Progress::stage("model", "download", &format!("开始下载 {}", spec.name)),
    );

    let client = state.http();
    let app2 = app.clone();
    localllm::download_with_progress(&client, &spec.urls, &dest, "model", flag, move |p| {
        let _ = app2.emit(EVT_PROGRESS, p);
    })
    .await
    .map_err(err)?;

    let sz = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    let _ = app.emit(
        EVT_PROGRESS,
        Progress::stage("model", "done", &format!("{} 下载完成", spec.name)),
    );

    Ok(serde_json::json!({
        "ok": true,
        "already": false,
        "path": dest.display().to_string(),
        "size_text": localllm::human_bytes(sz),
        "message": format!("{} 下载完成（{}）", spec.name, localllm::human_bytes(sz)),
    }))
}

/// 启动托管服务的核心逻辑。
///
/// 抽出来是为了给「开机自动启动」复用 —— 那条路径没有 `State` 也没有前端，
/// 但需要完全一样的行为（找引擎 → 选模型 → 起服务 → 把 AI 指过去）。
fn start_managed(
    state: &AppState,
    model_id: Option<&str>,
) -> Result<serde_json::Value, String> {
    let data_dir = state.data_dir.clone();
    let dir = app_dir();
    let engine = localllm::find_engine(&dir, &data_dir)
        .ok_or_else(|| "还没安装推理引擎。可在「设置 → 本地大模型一键部署」里装".to_string())?;

    // 没指定就用「清单里第一个已下载的模型」。
    // 清单本身按 recommended 排在前面，所以这里不需要再判 recommended。
    let mdir = localllm::models_dir(&data_dir);
    let spec = match model_id {
        Some(id) => localllm::find_model(id).ok_or_else(|| format!("未知模型：{id}"))?,
        None => localllm::models()
            .into_iter()
            .find(|m| mdir.join(&m.file).is_file())
            .ok_or_else(|| "还没有下载任何模型。可在「设置 → 本地大模型一键部署」里下载".to_string())?,
    };
    let model_path = mdir.join(&spec.file);
    if !model_path.is_file() {
        return Err(format!("{} 还没有下载完成", spec.name));
    }

    let threads = localllm::default_threads();
    let port = localllm::start_server(&engine, &model_path, threads, 4096).map_err(err)?;

    // 接管之前先记下用户原来的 AI 地址，停止时要还回去。
    // 已经在托管状态（换模型重启）就别覆盖 —— 那样会把 18080 自己记成原值。
    let cur = state.cfg();
    if !localllm::is_managed_base(&cur.llm.base_url) {
        localllm::remember_prev_llm(localllm::LlmCfgSnapshot {
            base_url: cur.llm.base_url.clone(),
            model: cur.llm.model.clone(),
            timeout_secs: cur.llm.timeout_secs,
        });
    }

    // 指向本地服务。注意只改 base_url / model / timeout，不动用户的温度等偏好。
    let base = format!("http://127.0.0.1:{port}/v1");
    state
        .update_config(|c| {
            c.llm.base_url = base.clone();
            // llama-server 会忽略 model 字段，但留空更容易被别处当成
            // 「自动选第一个」而走到错误分支，写死更省心。
            c.llm.model = spec.file.clone();
            // 本地小模型首 token 慢，超时给足；低配机器上加载 1.1GB 要几秒
            c.llm.timeout_secs = 180;
        })
        .map_err(err)?;

    Ok(serde_json::json!({
        "ok": true,
        "port": port,
        "base_url": base,
        "model": spec.name,
        "threads": threads,
        "message": format!("{} 已启动（127.0.0.1:{port}，{} 线程）", spec.name, threads),
    }))
}

/// 启动托管服务，并把 AI 配置指向它。
#[tauri::command]
pub fn cmd_local_llm_start(
    state: State<'_, Arc<AppState>>,
    model_id: Option<String>,
) -> Result<serde_json::Value, String> {
    reject_if_unsupported()?;
    start_managed(&state, model_id.as_deref())
}

/// 启动时的自愈：AI 地址指着托管端口，但托管服务并没在跑。
///
/// 出现这种情况的正常途径有两条：
///   ① 上次退出时服务还在跑（退出会杀掉进程，地址却留在配置里）；
///   ② 自动启动开着但这次起失败了。
/// 不修的话设置页上会显示一个「看着挺正常」的地址，而 AI 讲解静默失效，
/// 用户只会以为「AI 坏了」。
fn heal_stale_base_url(state: &AppState) {
    if localllm::server_state().0 {
        return;
    }
    if !localllm::is_managed_base(&state.cfg().llm.base_url) {
        return;
    }
    match state.update_config(|c| c.llm.base_url = crate::models::LLM_BASE_DEFAULT.to_string()) {
        Ok(_) => log::info!("AI 地址指向已停止的托管服务，已还原为默认地址"),
        Err(e) => log::warn!("还原 AI 地址失败：{e}"),
    }
}

/// 应用启动时把本地模型服务安排好：该自动拉起的拉起，该自愈的自愈。
///
/// 为什么自动启动放在**后台线程 + 延迟 3 秒**：
/// - 模型加载本身要几秒（1.1GB 读盘），放在 setup 里会把主窗口卡在白屏；
/// - 延迟一下是因为用户刚双击图标时系统正在忙，这时候抢 IO 会显得更慢。
///
/// 任何一步失败都只记日志、不打扰用户 —— 自动启动是「锦上添花」，
/// 失败了用户手动点一下就行，弹个错误框反而莫名其妙。
pub fn spawn_autostart(state: Arc<AppState>) {
    if !state.cfg().auto_start_local_llm {
        // 没开自动启动：至少把可能残留的死地址修回来
        heal_stale_base_url(&state);
        return;
    }
    // 已经在跑就别重复起（比如上一份进程没退干净，或用户手动起过）
    if localllm::server_state().0 {
        return;
    }
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(3));
        match start_managed(&state, None) {
            Ok(v) => log::info!(
                "已自动启动本地模型服务：{}",
                v.get("message").and_then(|m| m.as_str()).unwrap_or("")
            ),
            Err(e) => {
                log::info!("跳过本地模型自动启动：{}", e);
                // 起不来就别让 AI 地址悬在一个没人监听的端口上
                heal_stale_base_url(&state);
            }
        }
    });
}

/// 开关「启动时自动拉起本地模型服务」。
///
/// 单独一个命令而不是让前端整体保存配置：设置页以外的入口（左下角弹层）
/// 也要能改它，走整份 `cmd_save_config` 容易把别处的改动覆盖掉。
#[tauri::command]
pub fn cmd_set_local_llm_auto(
    state: State<'_, Arc<AppState>>,
    enabled: bool,
) -> Result<serde_json::Value, String> {
    state
        .update_config(|c| c.auto_start_local_llm = enabled)
        .map_err(err)?;
    Ok(serde_json::json!({
        "ok": true,
        "enabled": enabled,
        "message": if enabled {
            "下次启动 WordWise 会自动拉起本地模型服务"
        } else {
            "已关闭自动启动"
        },
    }))
}

/// 停止托管服务，并把 AI 配置还回「被接管之前」的样子。
#[tauri::command]
pub fn cmd_local_llm_stop(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    localllm::stop_server();

    // 不还回去的话，用户本来是连 LM Studio 的，点一次「启动→停止」之后
    // AI 地址就永久停在 18080：服务已停，AI 讲解静默失效。
    let restored = if let Some(p) = localllm::take_prev_llm() {
        state
            .update_config(|c| {
                c.llm.base_url = p.base_url.clone();
                c.llm.model = p.model.clone();
                c.llm.timeout_secs = p.timeout_secs;
            })
            .map_err(err)?;
        true
    } else {
        false
    };

    Ok(serde_json::json!({
        "ok": true,
        "restored": restored,
        "message": if restored {
            "本地模型服务已停止，AI 地址已还原"
        } else {
            "本地模型服务已停止"
        },
    }))
}

/// 删除一个已下载的模型（腾空间）。
#[tauri::command]
pub fn cmd_local_llm_remove_model(
    state: State<'_, Arc<AppState>>,
    model_id: String,
) -> Result<String, String> {
    let spec = localllm::find_model(&model_id).ok_or_else(|| format!("未知模型：{model_id}"))?;
    let p = localllm::models_dir(&state.data_dir).join(&spec.file);

    // 正在被这个服务加载时不允许直接删：Windows 上文件被占用会删不掉，
    // 删一半还会把模型文件搞坏。先停服务再删。
    let (running, _, cur_model) = localllm::server_state();
    if running && cur_model == spec.file.replace(".gguf", "") {
        return Err("这个模型正在使用中，请先停止本地模型服务".into());
    }
    if !p.is_file() {
        return Ok(format!("{} 本来就没有下载", spec.name));
    }
    std::fs::remove_file(&p).map_err(err)?;
    Ok(format!("已删除 {}", spec.name))
}

/// 探测本地服务是否真的可用（起进程后立刻调用，界面据此显示「已就绪」）。
#[tauri::command]
pub async fn cmd_local_llm_probe(state: State<'_, Arc<AppState>>) -> Result<serde_json::Value, String> {
    let (running, port, model) = localllm::server_state();
    if !running {
        return Ok(serde_json::json!({ "ok": false, "message": "本地模型服务未运行" }));
    }
    let client = state.http();
    let url = format!("http://127.0.0.1:{port}/health");
    // 模型加载要点时间，给 5 秒；加载中 llama-server 返回 503，那也算「起来了」
    let mut ok = false;
    let mut msg = String::new();
    for _ in 0..10 {
        match client.get(&url).timeout(std::time::Duration::from_secs(5)).send().await {
            Ok(r) if r.status().is_success() => {
                ok = true;
                msg = format!("{model} 已就绪");
                break;
            }
            Ok(r) => {
                msg = format!("服务已启动，正在加载模型（HTTP {}）", r.status());
            }
            Err(e) => {
                msg = format!("等待服务响应：{e}");
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    }
    Ok(serde_json::json!({ "ok": ok, "port": port, "model": model, "message": msg }))
}

/// 应用退出时清理托管的子进程。
///
/// 不做这一步，用户关掉 WordWise 之后 llama-server 会变成孤儿进程继续
/// 占着 1GB 内存和几个 CPU 核心 —— 在低配机器上这几乎是致命的。
pub fn cleanup_on_exit() {
    localllm::shutdown();
}
