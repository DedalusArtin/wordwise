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
    stop_server();

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
pub fn shutdown() {
    stop_server();
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
}
