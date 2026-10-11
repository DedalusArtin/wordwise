//! 本地大模型一键部署（需求：帮没有本地大模型的用户装一个能跑的小模型）。
//!
//! ## 整体形态
//!
//! ```text
//!   ① 推理引擎  llama.cpp 的 llama-server（一个目录，约 45 MB 解压后）
//!   ② 模型权重  Qwen3-1.7B Q4_K_M（约 1.1 GB）
//!   ③ 托管进程  应用负责起/停 llama-server，并把 base_url 指过去
//! ```
//!
//! 用户只需要点一次「一键部署」，之后 AI 讲解、翻译兜底、知识图谱发散
//! 就都有本地模型可用了，全程不依赖 LM Studio。
//!
//! ## ★ 实测出来的两个硬约束（决定了这里的设计）
//!
//! 1. **GitHub Release 在国内基本下不动。**
//!    实测 `github.com` 直连超时、走代理 502；只有 `gh-proxy.com` 这类
//!    第三方镜像能通，而且**被全局限速到几十 KB/s**（33 MB 的 Vulkan 包
//!    要十几分钟，并发 8 线程也一样）。所以引擎**优先随安装包分发**
//!    （见 `build.ps1`，会把 `vendor/llama` 打进安装目录），只有在
//!    安装目录里找不到时才退回联网下载，并且如实告诉用户会很慢。
//!
//! 2. **模型权重走魔搭（ModelScope）快得离谱。**
//!    实测 `modelscope.cn` 直连 14.5 MB/s（1.1 GB 约 80 秒），
//!    而 `hf-mirror.com` 只有 3.3 MB/s、`huggingface.co` 完全不通。
//!    所以模型下载**首选魔搭**，hf-mirror 只作为兜底镜像。
//!
//! ## 为什么用「外部进程」而不是把 llama.cpp 静态链进来
//!
//! 链进来会让 exe 体积暴涨几十 MB，还要在 MSVC 下引入 CMake + C++ 构建链，
//! 编译时间和失败率都不可控。而 llama-server 本身就暴露 OpenAI 兼容接口，
//! 与本项目已有的 [`crate::llm`] 完全对接 —— 托管一个进程的成本远低于
//! 自己维护一套推理后端。

use anyhow::{anyhow, Context, Result};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// llama.cpp 的版本标记。引擎包与模型清单都跟着它走，
/// 升级时只改这一处，界面上也能如实显示「用的是哪一版」。
pub const ENGINE_TAG: &str = "b11414";

/// 引擎包的镜像链（按速度排序，实测 gh-proxy 是唯一稳定可用的）。
pub const ENGINE_MIRRORS: [&str; 3] = [
    "https://gh-proxy.com/https://github.com/ggml-org/llama.cpp/releases/download",
    "https://ghfast.top/https://github.com/ggml-org/llama.cpp/releases/download",
    "https://ghproxy.net/https://github.com/ggml-org/llama.cpp/releases/download",
];

/// 平台对应的引擎包名。
///
/// 只用 **Vulkan 版**：它的压缩包里**同时带了全部 CPU 后端**
/// （`ggml-cpu-*.dll` 一整套），有 Vulkan 设备就自动加速、没有就回落 CPU。
/// 换句话说一份包覆盖两种情况，比同时分发 CPU 版 + Vulkan 版省一半体积。
pub fn engine_asset() -> String {
    // ★ 版本号必须从 ENGINE_TAG 派生，不要在这里再写一遍字面量：
    //   否则升级 llama.cpp 时会出现「常量改了、包名没改」的漂移，
    //   表现为一键部署去下载一个不存在的资产（404），而日志里显示的版本是对的。
    if cfg!(target_arch = "aarch64") {
        format!("llama-{ENGINE_TAG}-bin-win-vulkan-arm64.zip")
    } else {
        format!("llama-{ENGINE_TAG}-bin-win-vulkan-x64.zip")
    }
}

/// 一个可选模型的描述。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelSpec {
    pub id: String,
    pub name: String,
    /// 一句话说明，给用户选的时候看
    pub note: String,
    pub file: String,
    /// 体积（字节），前端显示用
    pub size_bytes: u64,
    /// 推荐档（界面默认选中）
    pub recommended: bool,
    /// 下载镜像链
    #[serde(skip)]
    pub urls: Vec<String>,
}

/// 内置模型清单。
///
/// 三档覆盖「超低配 / 主流 / 求稳」三种诉求。全部是 Q4_K_M：
/// 4bit 中等量化，在 1.5B 级别上质量损失很小，而体积只有 fp16 的 1/4。
pub fn models() -> Vec<ModelSpec> {
    let qwen3_dir = "https://www.modelscope.cn/models/unsloth/Qwen3-1.7B-GGUF/resolve/master";
    let qwen3_small = "https://www.modelscope.cn/models/unsloth/Qwen3-0.6B-GGUF/resolve/master";
    let q25 = "https://www.modelscope.cn/models/Qwen/Qwen2.5-1.5B-Instruct-GGUF/resolve/master";
    let hf_qwen3 = "https://hf-mirror.com/Qwen/Qwen3-1.7B-GGUF/resolve/main";
    let hf_qwen3s = "https://hf-mirror.com/Qwen/Qwen3-0.6B-GGUF/resolve/main";
    let hf_q25 = "https://hf-mirror.com/Qwen/Qwen2.5-1.5B-Instruct-GGUF/resolve/main";

    vec![
        ModelSpec {
            id: "qwen3-1.7b".into(),
            name: "Qwen3 1.7B（推荐）".into(),
            note: "中文与多语言理解好，讲词、造句、翻译兜底都够用。低配 CPU 上约 5-15 词/秒。".into(),
            file: "Qwen3-1.7B-Q4_K_M.gguf".into(),
            size_bytes: 1_107_400_000,
            recommended: true,
            urls: vec![
                format!("{qwen3_dir}/Qwen3-1.7B-Q4_K_M.gguf"),
                format!("{hf_qwen3}/Qwen3-1.7B-Q4_K_M.gguf"),
            ],
        },
        ModelSpec {
            id: "qwen3-0.6b".into(),
            name: "Qwen3 0.6B（极速）".into(),
            note: "只有 400 MB，老机器也能流畅跑，适合只想要「有个模型兜底」的场景。".into(),
            file: "Qwen3-0.6B-Q4_K_M.gguf".into(),
            size_bytes: 396_700_000,
            recommended: false,
            urls: vec![
                format!("{qwen3_small}/Qwen3-0.6B-Q4_K_M.gguf"),
                format!("{hf_qwen3s}/Qwen3-0.6B-Q4_K_M.gguf"),
            ],
        },
        ModelSpec {
            id: "qwen2.5-1.5b".into(),
            name: "Qwen2.5 1.5B（稳定）".into(),
            note: "上一代模型，指令遵循非常稳、不说怪话，体积与 1.7B 相当。".into(),
            file: "qwen2.5-1.5b-instruct-q4_k_m.gguf".into(),
            size_bytes: 1_117_300_000,
            recommended: false,
            urls: vec![
                format!("{q25}/qwen2.5-1.5b-instruct-q4_k_m.gguf"),
                format!("{hf_q25}/qwen2.5-1.5b-instruct-q4_k_m.gguf"),
            ],
        },
    ]
}

/// 按 id 找一个模型。
pub fn find_model(id: &str) -> Option<ModelSpec> {
    models().into_iter().find(|m| m.id == id)
}

/* ---------------- 路径 ---------------- */

/// 随安装包分发的引擎目录（exe 同级的 `vendor/llama`）。
pub fn bundled_engine_dir(app_dir: &Path) -> PathBuf {
    app_dir.join("vendor").join("llama")
}

/// 运行时下载到数据目录的引擎（便携版 / 用户删过文件时用）。
pub fn downloaded_engine_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("engine").join(ENGINE_TAG)
}

/// 模型目录的指针文件名。写在**数据目录**里（跟数据一起搬），
/// 而不是 exe 旁边 —— 数据目录变了，指针要跟着走。
pub const MODELS_POINTER: &str = "models_dir.txt";

/// 显式指定模型目录的环境变量（优先级高于指针文件）。
pub const MODELS_DIR_ENV: &str = "WORDWISE_MODELS_DIR";

/// 模型存放目录。
///
/// 三级：环境变量 → 数据目录里的 `models_dir.txt` → `<数据目录>/models`。
///
/// 为什么允许单独换：模型是**唯一可能上 GB 的东西**（单档 400MB~1.1GB，
/// 三档全下接近 2.6GB）。用户的 C 盘可能只够装程序、却不够放模型，
/// 而数据目录本身他未必想动 —— 「只挪模型」是真实且高频的诉求。
pub fn models_dir(data_dir: &Path) -> PathBuf {
    let env_dir = std::env::var_os(MODELS_DIR_ENV).map(PathBuf::from);
    models_dir_with(data_dir, env_dir.as_deref())
}

/// `models_dir` 的纯函数版本（入参可控 → 可单测）。
pub fn models_dir_with(data_dir: &Path, env_dir: Option<&Path>) -> PathBuf {
    if let Some(p) = env_dir {
        if !p.as_os_str().is_empty() {
            return p.to_path_buf();
        }
    }
    if let Some(p) = crate::state::read_pointer(&data_dir.join(MODELS_POINTER)) {
        return p;
    }
    data_dir.join("models")
}

/// 找一个可用的引擎目录。
///
/// 顺序：**随包的优先**。随包的已经解压好、不占额外下载，没有理由不用它。
pub fn find_engine(app_dir: &Path, data_dir: &Path) -> Option<PathBuf> {
    for dir in [bundled_engine_dir(app_dir), downloaded_engine_dir(data_dir)] {
        if dir.join(server_exe_name()).is_file() {
            return Some(dir);
        }
    }
    None
}

/// `llama-server` 的可执行文件名。
pub fn server_exe_name() -> &'static str {
    if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    }
}

/* ---------------- 下载 ---------------- */

/// 引擎或模型的下载/安装进度。
#[derive(Debug, Clone, serde::Serialize)]
pub struct Progress {
    /// `engine` / `model`
    pub kind: String,
    /// 处于哪个阶段：`download` / `unpack` / `done` / `failed`
    pub stage: String,
    pub got: u64,
    pub total: u64,
    /// 0~100，总长未知时为 -1（前端显示「不确定进度」）
    pub percent: f64,
    pub speed: String,
    pub message: String,
}

impl Progress {
    pub fn download(kind: &str, got: u64, total: u64, speed: &str, msg: &str) -> Self {
        let percent = if total > 0 {
            (got as f64 / total as f64 * 100.0).min(100.0)
        } else {
            -1.0
        };
        Self {
            kind: kind.into(),
            stage: "download".into(),
            got,
            total,
            percent,
            speed: speed.into(),
            message: msg.into(),
        }
    }
    pub fn stage(kind: &str, stage: &str, msg: &str) -> Self {
        Self {
            kind: kind.into(),
            stage: stage.into(),
            got: 0,
            total: 0,
            percent: -1.0,
            speed: String::new(),
            message: msg.into(),
        }
    }
}

/// 人读的速度文本。
pub fn human_bytes(n: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let f = n as f64;
    if f >= MB {
        format!("{:.0} MB", f / MB)
    } else {
        format!("{:.0} KB", f / 1024.0)
    }
}

/// 带进度回调的下载。
///
/// 语义上是「**尽力而为 + 可续传**」：
/// - 先写 `.part` 临时文件，下完才改名 —— 中途断掉不会留一个看起来
///   完整的坏文件，下次启动也就不会拿它去加载模型；
/// - 如果 `.part` 已经存在且服务器支持 Range，就从中断处续传。
pub async fn download_with_progress<F>(
    client: &reqwest::Client,
    urls: &[String],
    dest: &Path,
    kind: &str,
    cancel: Arc<AtomicBool>,
    mut on: F,
) -> Result<()>
where
    F: FnMut(Progress) + Send,
{
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let part = dest.with_extension("part");

    let mut last_err: Option<String> = None;
    for (i, url) in urls.iter().enumerate() {
        let mut start = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        on(Progress::download(
            kind,
            0,
            0,
            "",
            &format!("正在连接下载源 {}…", i + 1),
        ));

        let mut req = client.get(url);
        if start > 0 {
            req = req.header("Range", format!("bytes={start}-"));
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                last_err = Some(format!("下载源 {} 连接失败：{e}", i + 1));
                start = 0;
                let _ = std::fs::remove_file(&part);
                continue;
            }
        };
        if !resp.status().is_success() && resp.status().as_u16() != 206 {
            last_err = Some(format!("下载源 {} 返回 {}", i + 1, resp.status()));
            continue;
        }
        // 服务器不认 Range 时会返回 200 从头开始，此时必须丢弃旧的 .part
        if start > 0 && resp.status().as_u16() == 200 {
            start = 0;
            let _ = std::fs::remove_file(&part);
        }

        let total = resp.content_length().unwrap_or(0) + start;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(start > 0)
            .write(true)
            .truncate(start == 0)
            .open(&part)?;

        let mut got = start;
        let mut stream = resp;
        let t0 = std::time::Instant::now();
        let mut last_tick = std::time::Instant::now();
        // 中断标志：必须单独记，不能只看字节数。
        // 服务器没给 Content-Length 时 total==0，仅凭「下到的字节数」
        // 判断不出文件是否完整 —— 那样会把半个模型 rename 成正式文件，
        // 之后加载模型时报的错会离真正的原因非常远。
        let mut interrupted = false;

        loop {
            if cancel.load(Ordering::Relaxed) {
                let _ = file.flush();
                return Err(anyhow!("已取消"));
            }
            match stream.chunk().await {
                Ok(Some(chunk)) => {
                    file.write_all(&chunk)?;
                    got += chunk.len() as u64;
                    // 每 200ms 汇报一次：太频繁会把前端刷爆，太慢又像卡死
                    if last_tick.elapsed().as_millis() >= 200 {
                        let secs = t0.elapsed().as_secs_f64().max(0.001);
                        let bps = ((got - start) as f64 / secs) as u64;
                        on(Progress::download(
                            kind,
                            got,
                            total,
                            &format!("{}/s", human_bytes(bps)),
                            &format!("已下载 {} / {}", human_bytes(got), human_bytes(total)),
                        ));
                        last_tick = std::time::Instant::now();
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    let _ = file.flush();
                    last_err = Some(format!("下载中断：{e}（可重试续传）"));
                    interrupted = true;
                    break;
                }
            }
        }
        file.flush()?;
        drop(file);

        let done_len = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if interrupted {
            // 中断原因已经在上面写进 last_err，直接换下一个镜像重试
            continue;
        }
        if total > 0 && done_len < total {
            last_err = Some(format!(
                "下载未完成（{} / {}），可再点一次继续",
                human_bytes(done_len),
                human_bytes(total)
            ));
            continue;
        }

        std::fs::rename(&part, dest)?;
        on(Progress::download(kind, done_len, done_len, "", "下载完成"));
        return Ok(());
    }

    Err(anyhow!(
        "所有下载源都失败了：{}",
        last_err.unwrap_or_else(|| "未知原因".into())
    ))
}

/// 解压引擎包。
///
/// 只保留运行 `llama-server` 需要的文件，其余（cli / bench / perplexity
/// 等十来个可执行文件）统统丢掉 —— 它们是给开发者用的，白占用户几十 MB。
pub fn unpack_engine(zip_path: &Path, dest: &Path) -> Result<usize> {
    let f = std::fs::File::open(zip_path).context("打开引擎包失败")?;
    let mut zip = zip::ZipArchive::new(f).context("引擎包不是有效的 zip")?;
    std::fs::create_dir_all(dest)?;

    // llama-server.exe 是个瘦壳，真正的实现在这些文件里，缺一个都起不来
    let keep_exact = [
        "llama-server.exe",
        "llama-server-impl.dll",
        "llama.dll",
        "llama-common.dll",
        "ggml.dll",
        "ggml-base.dll",
        "libomp.dll",
        "ggml-vulkan.dll",
        "ggml-rpc.dll",
        "mtmd.dll",
    ];

    let mut n = 0usize;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        let Some(raw) = file.enclosed_name() else { continue };
        let name = raw
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        // 只留 server 相关 + 全部 ggml-cpu-*.dll
        // （后者是各代 CPU 的 SIMD 内核，运行时会自动挑一个加载，
        //   删掉就没法在对应 CPU 上跑，必须整套留着）
        let keep = keep_exact.contains(&name.as_str())
            || (name.starts_with("ggml-cpu-") && name.ends_with(".dll"));
        if !keep {
            continue;
        }
        let out = dest.join(&name);
        let mut w = std::fs::File::create(&out)?;
        std::io::copy(&mut file, &mut w)?;
        n += 1;
    }
    if n == 0 {
        return Err(anyhow!("引擎包里没有找到 llama-server，可能下载不完整"));
    }
    Ok(n)
}

/* ---------------- 子进程托管 ---------------- */

/// 正在托管的 llama-server。
struct ServerHandle {
    child: Child,
    port: u16,
    model: String,
}

static SERVER: Mutex<Option<ServerHandle>> = Mutex::new(None);

/// 系统里所有同名托管进程的 pid（含**不是本进程启动的**）。
///
/// ★ 为什么要走到系统层面去数：`SERVER` 只是**本进程内**的一张表。
///   应用崩溃、被任务管理器结束、或开发时热重启，这张表就随进程一起没了，
///   而上一轮起出来的 `llama-server` 还在跑、还占着显存 ——
///   它是孤儿，谁都管不着。实机上一次调试就攒了 4 个，
///   每个都在显存里躺着 ≥1.5 GB，把 12 GB 的卡占到 83%。
///
/// 所以「清理」不能只清自己那一份，必须以进程表为准扫一遍。
fn managed_pids() -> Vec<u32> {
    let exe = server_exe_name();
    let mut out = Vec::new();
    #[cfg(windows)]
    {
        // ★ 用 tasklist 而不是 WMIC：WMIC 在 Win11 上已被移除（deprecated），
        //   而 tasklist 从 XP 到 Win11 都在，且不需要额外权限。
        //   CSV 无表头格式：`"映像名称","PID","会话名","会话#","内存使用"`
        let out_raw = Command::new("tasklist")
            .args(["/FI", &format!("IMAGENAME eq {exe}"), "/FO", "CSV", "/NH"])
            .output();
        if let Ok(o) = out_raw {
            let text = String::from_utf8_lossy(&o.stdout);
            for line in text.lines() {
                let line = line.trim();
                // 没有匹配时 tasklist 会输出一句中文提示而不是 CSV，跳过
                if !line.starts_with('"') {
                    continue;
                }
                // 字段 2 是 PID（字段 1 是映像名，可能带逗号，所以按引号切）
                let parts: Vec<&str> = line.split("\",\"").collect();
                if parts.len() < 2 {
                    continue;
                }
                if let Ok(pid) = parts[1].trim_matches('"').parse::<u32>() {
                    out.push(pid);
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        // Unix 下 pgrep 比 ps 解析更稳；拿不到就退化成「只有自己那一份」
        if let Ok(o) = Command::new("pgrep").args(["-f", &exe]).output() {
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                if let Ok(pid) = line.trim().parse::<u32>() {
                    out.push(pid);
                }
            }
        }
    }
    out
}

/// 抓一次系统进程表，返回 `pid -> ppid` 的映射。
///
/// ★ 只抓**一次**。早先的写法是「每个候选 pid 起一个 PowerShell 去问父进程在不在」，
/// 一个进程冷启 300–600 ms，N 个进程就是 N 次串行等待，启动路径上白白卡住一两秒，
/// 而且把「查权限」这种事重复做了 N 遍（PowerShell 的 CIM/WMI 在受限上下文里可能被拒）。
/// 改成一次性全表扫描后，Rust 侧做集合运算即可，成本与进程数无关。
///
/// 返回 `None` 表示**表没抓到**（命令失败/解析为空）。调用方必须把 `None` 当成
/// 「判不了」，走保守分支（不杀），绝不能当成「没有孤儿」或「全是孤儿」。
fn process_table() -> Option<HashMap<u32, u32>> {
    #[cfg(windows)]
    {
        // PowerShell 一次性吐出 "pid,ppid" 逐行。
        // 不用 Get-CimInstance 单查（N 次调用）；也不依赖 wmic（Win11 已移除）。
        let out = Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-CimInstance Win32_Process | ForEach-Object { \"$($_.ProcessId),$($_.ParentProcessId)\" }",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output();
        let o = match out {
            Ok(o) if o.status.success() => o,
            _ => return None,
        };
        let text = String::from_utf8_lossy(&o.stdout);
        let mut map = HashMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let mut it = line.split(',');
            if let (Some(a), Some(b)) = (it.next(), it.next()) {
                if let (Ok(pid), Ok(ppid)) = (a.trim().parse::<u32>(), b.trim().parse::<u32>()) {
                    map.insert(pid, ppid);
                }
            }
        }
        if map.is_empty() {
            None // 抓到了命令但一行都没解析出来 → 视为失败，别当成「系统没有进程」
        } else {
            Some(map)
        }
    }
    #[cfg(not(windows))]
    {
        // Unix：`ps -e -o pid=,ppid=` 一次拿全表。
        let out = Command::new("ps").args(["-e", "-o", "pid=,ppid="]).output();
        let o = match out {
            Ok(o) if o.status.success() => o,
            _ => return None,
        };
        let text = String::from_utf8_lossy(&o.stdout);
        let mut map = HashMap::new();
        for line in text.lines() {
            let mut it = line.split_whitespace();
            if let (Some(a), Some(b)) = (it.next(), it.next()) {
                if let (Ok(pid), Ok(ppid)) = (a.parse::<u32>(), b.parse::<u32>()) {
                    map.insert(pid, ppid);
                }
            }
        }
        if map.is_empty() {
            None
        } else {
            Some(map)
        }
    }
}

/// 给定进程表，判定哪些 pid 是孤儿。
///
/// 与 [`orphan_pids`] 分离出来，是为了让它成为**纯函数**：不碰系统、不 spawn，
/// 可以直接喂人造进程表做断言，把「判据」本身钉死。
///
/// 判据（三条全中才算孤儿）：
///   1. `pid` 要在表里（已经退出的进程不占资源，不用我们动手）；
///   2. `ppid` 不能是 0 或自身（内核/根进程，判不了 → 按有主处理）；
///   3. `ppid` 不能出现在表里（父进程还在 → 有主 → 别碰）。
fn orphans_from_table(table: &HashMap<u32, u32>, pids: &[u32]) -> Vec<u32> {
    let mut out = Vec::new();
    for &pid in pids {
        let Some(&ppid) = table.get(&pid) else {
            continue; // 1. 已退出
        };
        if ppid == 0 || ppid == pid {
            continue; // 2. 判不了
        }
        if table.contains_key(&ppid) {
            continue; // 3. 有主
        }
        out.push(pid);
    }
    out
}

/// 判断一批 pid 里哪些是**孤儿**（父进程已经不在了）。
///
/// ★ 「父进程还在不在」是「该不该杀」的**唯一判据**，比「是不是我们起的」更准：
///   - 父进程还在 → 它有主，可能是**另一个正在运行的 WordWise 实例**
///     （用户开了两份，或开发时开着调试实例），杀它就是砸别人的饭碗；
///   - 父进程没了 → 它是孤儿，原主人已经走了，谁都不会再来认领它，
///     显存却一直被占着。这种才该收。
///
/// 表抓不到时返回空集：调用方据此跳过本轮，不做任何击杀。
fn orphan_pids(pids: &[u32]) -> Vec<u32> {
    let table = match process_table() {
        Some(t) => t,
        None => {
            log::warn!("拿不到系统进程表，本轮跳过孤儿回收（宁可少收，不可误杀）");
            return Vec::new();
        }
    };
    orphans_from_table(&table, pids)
}

/// 结束一个 pid；返回是否真的发过信号。
fn kill_pid(pid: u32) -> bool {
    #[cfg(windows)]
    {
        // /T 连同子进程一起（llama-server 在某些配置下会再拉一个 worker）
        return Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F", "/T"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    }
    #[cfg(not(windows))]
    {
        return Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
    }
}

/// 扫掉系统里**所有**托管进程（含孤儿），返回清掉的个数。
///
/// 这是 `stop_server` 的「彻底版」：不信任内存记录，直接以进程表为准。
/// 启动前与退出时都该跑它。
///
/// ★ 只收**孤儿**，不碰有主的进程。
///
///   一个 llama-server 可能是：
///   - 孤儿（父进程已退出）→ 没人管了，显存白占，**收**；
///   - 有主（父进程还活着）→ 那是**另一个正在运行的 WordWise 实例**
///     在用它，杀了就是砸别人的饭碗，**不动**。
///
///   这个判据让「多开应用」和「崩溃残留」两种情况都能被正确处理，
///   而不用去猜「是不是我们起的」。
pub fn sweep_orphans() -> usize {
    let me = std::process::id();
    let candidates: Vec<u32> = managed_pids().into_iter().filter(|&p| p != me).collect();
    if candidates.is_empty() {
        return 0;
    }
    // 一次全表扫描定孤儿；表抓不到就一个都不杀。
    let orphans = orphan_pids(&candidates);
    let mut killed = 0usize;
    for pid in orphans {
        if kill_pid(pid) {
            killed += 1;
            log::info!("回收孤儿托管进程 llama-server (pid {pid})");
        }
    }
    killed
}

/// 托管服务占用的端口段（含下界、不含上界）。
///
/// 与 [`pick_port`] 的范围必须一致：判断「这个 base_url 是不是我们自己写进去的」
/// 全靠它。
pub const MANAGED_PORT_MIN: u16 = 18080;
pub const MANAGED_PORT_MAX: u16 = 18180;

/// `base_url` 是否指向我们托管的服务。
///
/// 用于两件事：① 启动前判断「有没有必要记下原地址」；
/// ② 启动时自愈「地址悬在一个已经停掉的托管端口上」。
pub fn is_managed_base(base: &str) -> bool {
    for host in ["127.0.0.1:", "localhost:"] {
        if let Some(rest) = base.split(host).nth(1) {
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(p) = digits.parse::<u16>() {
                if (MANAGED_PORT_MIN..MANAGED_PORT_MAX).contains(&p) {
                    return true;
                }
            }
        }
    }
    false
}

/// 「一键部署接管之前，AI 原来指向哪里」。
///
/// 没有这份快照的话：用户本来连着 LM Studio(127.0.0.1:1234)，点一次
/// 「启动」再点「停止」，`base_url` 就永久停在 18080 —— 服务已经停了，
/// AI 讲解也跟着废掉，而设置页上显示的地址看起来还挺正常，
/// 用户根本想不到是自己点了那个按钮造成的。
#[derive(Debug, Clone)]
pub struct LlmCfgSnapshot {
    pub base_url: String,
    pub model: String,
    pub timeout_secs: i64,
}

static PREV_LLM: Mutex<Option<LlmCfgSnapshot>> = Mutex::new(None);

/// 记下「启动托管服务之前」的 AI 配置。
///
/// 幂等：已经有快照就不再覆盖。否则「换个模型重启一下」会把 18080 自己
/// 记成原值，停止后还原了个寂寞。
pub fn remember_prev_llm(snap: LlmCfgSnapshot) {
    let mut g = PREV_LLM.lock();
    if g.is_none() {
        *g = Some(snap);
    }
}

/// 取回并清空快照（停止托管服务时用）。
pub fn take_prev_llm() -> Option<LlmCfgSnapshot> {
    PREV_LLM.lock().take()
}

/// 找一个空闲端口。
///
/// 从 18080 往上试：这个区段不容易撞上别的软件，而且即使被占用也能往后挪，
/// 不会像固定端口那样「装完却起不来」。
pub fn pick_port() -> Result<u16> {
    for p in MANAGED_PORT_MIN..MANAGED_PORT_MAX {
        if std::net::TcpListener::bind(("127.0.0.1", p)).is_ok() {
            return Ok(p);
        }
    }
    Err(anyhow!(
        "没有找到可用端口（{MANAGED_PORT_MIN}-{} 都被占用了）",
        MANAGED_PORT_MAX - 1
    ))
}

/// 一个 CPU 上跑多少线程。
///
/// 留一半给系统和其他程序：低配机器上把 8 个核全占满，界面会卡到没法用。
pub fn default_threads() -> usize {
    let n = std::thread::available_parallelism()
        .map(|x| x.get())
        .unwrap_or(4);
    (n / 2).clamp(2, 8)
}

/// 启动 llama-server。
///
/// `ngl` 是「放到 GPU 上的层数」，999 = 全部。用 Vulkan 后端时它才生效；
/// 没有可用 GPU 时 llama.cpp 会自己回落到 CPU，不会报错。
pub fn start_server(
    engine: &Path,
    model_path: &Path,
    threads: usize,
    context: usize,
) -> Result<u16> {
    // ★ 先彻底清场，再起新的。
    //
    //   只调 `stop_server()` 是不够的 —— 它只杀本进程 `SERVER` 里记着的那一个。
    //   上一轮应用退出/崩溃留下的孤儿它根本不知道，于是每重启一次就多一个
    //   常驻显存的 llama-server（实机 4 小时攒了 4 个、吃掉 6.4 GB 显存）。
    //   这里以**系统进程表**为准扫一遍，起新进程之前保证旧的一个不剩。
    stop_server();
    let swept = sweep_orphans();
    if swept > 0 {
        // 进程退出、显存释放不是瞬时的：给驱动一点时间回收，
        // 否则可能出现「旧的还没释放、新的又要分配」而失败。
        std::thread::sleep(std::time::Duration::from_millis(600));
        log::info!("启动前清掉 {swept} 个残留的托管进程");
    }

    let exe = engine.join(server_exe_name());
    if !exe.is_file() {
        return Err(anyhow!("引擎不完整：找不到 {}", exe.display()));
    }
    if !model_path.is_file() {
        return Err(anyhow!("模型文件不存在：{}", model_path.display()));
    }

    let port = pick_port()?;
    let mut cmd = Command::new(&exe);
    cmd.current_dir(engine)
        .arg("-m")
        .arg(model_path)
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .arg("-t")
        .arg(threads.to_string())
        .arg("-c")
        .arg(context.to_string())
        // GPU 层数：写成 999 让 llama.cpp 自己决定（Vulkan 后端会全放上去）
        .arg("-ngl")
        .arg("999")
        // 关掉日志刷屏，Windows 上子进程继承控制台会让 exe 多出一个黑窗口
        .arg("--no-webui")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW：不加这个，GUI 程序启动子进程会弹一个黑色控制台
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let child = cmd
        .spawn()
        .with_context(|| format!("启动 {} 失败", exe.display()))?;

    let model_name = model_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    *SERVER.lock() = Some(ServerHandle {
        child,
        port,
        model: model_name,
    });
    Ok(port)
}

/// 停止托管的服务（幂等）。
///
/// 只处理**本进程记着的那一份**。想连孤儿一起清，用 [`sweep_orphans`]——
/// 或直接调 [`shutdown`]，它就是「两份都做」。
pub fn stop_server() {
    let mut guard = SERVER.lock();
    if let Some(mut h) = guard.take() {
        let _ = h.child.kill();
        let _ = h.child.wait();
    }
}

/// 当前托管状态：`(是否在跑, 端口, 模型名)`。
///
/// 「在跑」的判据是**子进程还活着**，不是「配置里写了地址」——
/// 用户手动把服务杀掉之后，界面必须如实反映。
pub fn server_state() -> (bool, u16, String) {
    let mut guard = SERVER.lock();
    let Some(h) = guard.as_mut() else {
        return (false, 0, String::new());
    };
    match h.child.try_wait() {
        Ok(Some(_)) => {
            let port = h.port;
            let model = h.model.clone();
            *guard = None;
            (false, port, model)
        }
        Ok(None) => (true, h.port, h.model.clone()),
        Err(_) => (false, h.port, h.model.clone()),
    }
}

/// 进程退出时清理：不能让 llama-server 变成孤儿进程常驻内存。
///
/// 两份都要做：先收自己那一份（正常路径），再扫一遍孤儿（崩溃/强杀后的残留）。
/// 只靠前者的那版代码在实机上留了 4 个进程吃满显存 —— 退出路径并不可靠，
/// 所以这里**不以内存记录为准**，而是以系统进程表为准。
pub fn shutdown() {
    stop_server();
    let n = sweep_orphans();
    if n > 0 {
        log::info!("退出时回收了 {n} 个托管进程");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_asset_matches_arch() {
        let a = engine_asset();
        assert!(a.starts_with("llama-b"));
        assert!(a.ends_with(".zip"));
        // ★ 这条才是真正的回归守卫：包名里的版本号必须和 ENGINE_TAG 一致。
        //   少了它，engine_asset() 里写死一个旧版本号也能通过测试，
        //   而线上会去下载一个 404 的资产。
        assert!(
            a.contains(ENGINE_TAG),
            "引擎包名 {a} 里没有当前的 ENGINE_TAG（{ENGINE_TAG}），两者已经漂移"
        );
        if cfg!(target_arch = "aarch64") {
            assert!(a.contains("arm64"));
        } else {
            assert!(a.contains("x64"));
        }
    }

    #[test]
    fn models_have_mirrors_and_sizes() {
        let list = models();
        assert!(list.len() >= 3, "至少要有三档模型");
        assert_eq!(
            list.iter().filter(|m| m.recommended).count(),
            1,
            "推荐档只能有一个，否则界面不知道该默认选谁"
        );
        for m in &list {
            assert!(m.urls.len() >= 2, "{} 缺少备用镜像", m.id);
            assert!(m.size_bytes > 100_000_000, "{} 体积不合理", m.id);
            assert!(m.file.ends_with(".gguf"));
            // 首选必须是魔搭：实测它是唯一能跑到 10 MB/s 以上的源
            assert!(
                m.urls[0].contains("modelscope.cn"),
                "{} 的首选下载源不是魔搭",
                m.id
            );
        }
    }

    #[test]
    fn find_model_by_id() {
        assert!(find_model("qwen3-1.7b").is_some());
        assert!(find_model("nope").is_none());
    }

    #[test]
    fn human_bytes_readable() {
        assert_eq!(human_bytes(1024 * 1024), "1 MB");
        assert_eq!(human_bytes(512 * 1024), "512 KB");
    }

    #[test]
    fn threads_leave_headroom() {
        let t = default_threads();
        assert!((2..=8).contains(&t), "线程数应在 2~8 之间，实际 {t}");
    }

    #[test]
    fn progress_percent_handles_unknown_total() {
        let p = Progress::download("model", 100, 0, "", "");
        assert_eq!(p.percent, -1.0, "总长未知时应为 -1，前端据此显示不定进度");
        let p2 = Progress::download("model", 50, 100, "", "");
        assert!((p2.percent - 50.0).abs() < 0.01);
    }

    #[test]
    fn server_state_is_false_when_never_started() {
        // 注意：不能在这里 start，会真的拉起进程
        stop_server();
        let (running, _, _) = server_state();
        assert!(!running);
    }

    #[test]
    fn bundled_engine_prefers_app_dir() {
        let tmp = std::env::temp_dir();
        let p = bundled_engine_dir(&tmp);
        assert!(p.ends_with(Path::new("vendor").join("llama")));
    }

    #[test]
    fn models_dir_follows_the_pointer_file() {
        let tmp = std::env::temp_dir().join(format!("wordwise-modelsdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // 默认：<数据目录>\models
        assert_eq!(models_dir_with(&tmp, None), tmp.join("models"));

        // 指针文件优先（用户把模型单独挪到了别的盘）
        std::fs::write(tmp.join(MODELS_POINTER), "D:\\WW-Models").unwrap();
        assert_eq!(models_dir_with(&tmp, None), PathBuf::from("D:\\WW-Models"));

        // 环境变量再压一层
        assert_eq!(
            models_dir_with(&tmp, Some(Path::new("E:\\Env-Models"))),
            PathBuf::from("E:\\Env-Models")
        );
        // 空的环境变量等于没设，不能把模型目录解析成空路径
        assert_eq!(
            models_dir_with(&tmp, Some(Path::new(""))),
            PathBuf::from("D:\\WW-Models")
        );

        // 写 `default` = 恢复默认
        std::fs::write(tmp.join(MODELS_POINTER), "default\n").unwrap();
        assert_eq!(models_dir_with(&tmp, None), tmp.join("models"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn managed_base_recognizes_our_ports_only() {
        // 我们自己写进去的
        assert!(is_managed_base("http://127.0.0.1:18080/v1"));
        assert!(is_managed_base("http://127.0.0.1:18179/v1"));
        assert!(is_managed_base("http://localhost:18100/v1"));

        // LM Studio 的默认端口绝不能被当成我们的 —— 否则「启动前记下原地址」
        // 这步会被跳过，停止后就还原不回 1234。
        assert!(!is_managed_base("http://127.0.0.1:1234/v1"));
        // 端口段边界：上界不含
        assert!(!is_managed_base("http://127.0.0.1:18180/v1"));
        assert!(!is_managed_base("http://127.0.0.1:18079/v1"));
        // 远端地址、畸形输入一律不算
        assert!(!is_managed_base("https://api.openai.com/v1"));
        assert!(!is_managed_base("http://127.0.0.1:abc/v1"));
        assert!(!is_managed_base("http://127.0.0.1:/v1"));
        assert!(!is_managed_base(""));
    }

    #[test]
    fn prev_llm_snapshot_is_taken_once_and_cleared() {
        // 静态量在测试进程里是共享的，先清干净
        let _ = take_prev_llm();

        let first = LlmCfgSnapshot {
            base_url: "http://127.0.0.1:1234/v1".into(),
            model: "qwen3-8b".into(),
            timeout_secs: 120,
        };
        remember_prev_llm(first);
        // 第二次（换模型重启）不能把 18080 自己记成原值
        remember_prev_llm(LlmCfgSnapshot {
            base_url: "http://127.0.0.1:18080/v1".into(),
            model: String::new(),
            timeout_secs: 180,
        });

        let got = take_prev_llm().expect("快照应该还在");
        assert_eq!(got.base_url, "http://127.0.0.1:1234/v1");
        assert_eq!(got.model, "qwen3-8b");
        assert_eq!(got.timeout_secs, 120);

        // 取过一次就没了，避免「停止」被点两次时重复还原
        assert!(take_prev_llm().is_none());
    }

    /// 造一张人造进程表：pid -> ppid。
    fn table(rows: &[(u32, u32)]) -> HashMap<u32, u32> {
        rows.iter().copied().collect()
    }

    /// ★ 核心回归：有主的 llama-server 绝不能被收。
    ///
    /// 场景：用户同时开了两个 WordWise（或开发时开着调试实例），
    /// 另一个实例托管的 llama-server 父进程还活着。上一版「按名字全杀」
    /// 会把它的服务端一并杀掉，等于砸别人的饭碗。
    #[test]
    fn 有主的托管进程不会被回收() {
        let t = table(&[(1000, 500), (500, 1)]);
        // 1000 是 llama-server，父 500（另一个 wordwise）还活着 → 不收
        let got = orphans_from_table(&t, &[1000]);
        assert!(got.is_empty(), "父进程还活着就不能杀，实际收掉了 {got:?}");
    }

    /// 父进程已经退出 → 孤儿，该收。
    ///
    /// 场景：上一轮 WordWise 崩溃/被强杀，它起的 llama-server 没人管了，
    /// 但显存一直占着（实机 4 个进程吃掉 6.4 GB）。
    #[test]
    fn 父进程消失的托管进程会被回收() {
        // 1000 在表里，父 500 不在表里（已退出）
        let t = table(&[(1000, 500), (777, 1)]);
        let got = orphans_from_table(&t, &[1000]);
        assert_eq!(got, vec![1000]);
    }

    /// 已经退出的进程不用我们动手（它压根不占资源了）。
    #[test]
    fn 已退出的进程不进回收名单() {
        let t = table(&[(500, 1)]);
        // 1000 不在表里 → 早就没了
        let got = orphans_from_table(&t, &[1000]);
        assert!(got.is_empty());
    }

    /// 父 pid 为 0 或指向自身 → 判不了，按有主处理（宁可少收，不可误杀）。
    #[test]
    fn 判不了父进程时一律不杀() {
        let t = table(&[(1000, 0), (2000, 2000)]);
        assert!(orphans_from_table(&t, &[1000]).is_empty(), "ppid=0 不该杀");
        assert!(
            orphans_from_table(&t, &[2000]).is_empty(),
            "ppid 指向自身不该杀"
        );
    }

    /// 混合场景：一批候选里只挑出真孤儿，其余不动。
    #[test]
    fn 一批候选里只挑真孤儿() {
        let t = table(&[
            (100, 9),  // 孤儿（9 不在表里）
            (200, 50), // 有主（50 在表里）
            (50, 1),   // 另一个 wordwise
            (300, 100), // 父 100 是孤儿但不是我们这轮的候选，它自己还没退出 → 有主
        ]);
        let mut got = orphans_from_table(&t, &[100, 200, 300]);
        got.sort_unstable();
        assert_eq!(got, vec![100], "只有 100 是孤儿：300 的父 100 仍在表里");
    }
}
