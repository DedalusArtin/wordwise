//! 应用共享状态：数据库、配置、HTTP 客户端、当前学习会话。
//!
//! 用 `parking_lot::RwLock` 而非 tokio 的锁，因为大部分命令是短同步操作；
//! 网络相关命令在 async 函数里先取快照再释放锁，避免跨 await 持有。

use crate::db::Db;
use crate::models::{AppConfig, QuizMode};
use anyhow::Result;
use parking_lot::RwLock;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 当前正在进行的答题会话。
#[derive(Debug, Clone, Default)]
pub struct Session {
    /// 本轮出题队列
    pub queue: Vec<crate::models::WordEntry>,
    /// 当前题目下标
    pub index: usize,
    /// 本轮模式
    pub mode: Option<QuizMode>,
    /// 本轮已答对数
    pub correct: i32,
    /// 本轮已答错数
    pub wrong: i32,
    /// 本轮开始时间
    pub started_at: i64,
    /// 是否仅强化记忆词
    pub leech_only: bool,
    /// 本轮每个词已被「答错回插」的次数（键按小写归一）。
    ///
    /// 为什么要有上限：没有它的话，一个怎么都记不住的词会被无限回插，
    /// 用户就会卡在同一个词上出不去（死亡循环）。上限见 `commands` 里的
    /// `MAX_REQUEUE_PER_WORD`。
    pub requeue_counts: std::collections::HashMap<String, u32>,
    /// 题面释义使用哪种语言：`"zh"` 中文 / `"src"` 原文 / 空 = 不限
    ///
    /// 由前端按**所选词库的语言**下发（背日语教材时就该给中文释义），
    /// 会话期间保持不变，避免同一轮里题面语种忽中忽英。
    pub def_lang: String,
    /// 本会话的**学习语言**（词库语言），建队列时锁定（P1 修复）。
    ///
    /// ★ 为什么必须存在：出题（干扰项池）、判分（upsert_state）、
    ///   调度（srs::schedule）全都要用「这条词该记在哪个语言下」。
    ///   此前各命令各自 `lang.unwrap_or(target_lang)`，前端只要漏传一次
    ///   （背日语词库、target_lang=en 的组合下）状态就记成 (word, "en")，
    ///   `unscheduled_words_in_book` 的 JOIN 永远落空 —— 背过的词每轮
    ///   都当新词重新进队，词库永远背不完。
    ///
    /// 空串 = 老会话/未设置（程序内开始的会话一定有值），此时才允许
    /// 回退到调用方参数 / 配置。
    pub lang: String,
}

impl Session {
    pub fn is_active(&self) -> bool {
        !self.queue.is_empty() && self.index < self.queue.len()
    }

    pub fn current(&self) -> Option<&crate::models::WordEntry> {
        self.queue.get(self.index)
    }

    pub fn total(&self) -> usize {
        self.queue.len()
    }
}

/// 全局应用状态，交给 Tauri 托管。
pub struct AppState {
    pub db: Db,
    pub config: RwLock<AppConfig>,
    /// HTTP 客户端放在 RwLock 里：用户在设置页改完代理后可以原地重建，
    /// 不需要重启应用（代理失效/换端口是高频操作）。
    http: RwLock<reqwest::Client>,
    /// 当前生效的代理解析结果，供界面如实展示
    proxy: RwLock<crate::net::ProxyResolution>,
    pub session: RwLock<Session>,
    /// **复习会话**：与背诵会话彻底分开的两个槽位（需求 14）。
    ///
    /// 之前只有一个槽位，于是「开始今日复习」会把正在进行的背诵整体覆写，
    /// 反过来也一样：两个入口互相摧毁。更要命的是复习会被补足到
    /// `batch_size`，而入口按钮显示的是「今天到期 + 错词」的真实数量
    /// （比如 12），点进去却变成 20 —— 数字对不上。
    pub review: RwLock<Session>,
    /// 学习数据、模型、备份的落点
    pub data_dir: PathBuf,
    /// 上面这个目录是**怎么选出来的**（设置页要如实说明，
    /// 否则用户看到「我的数据怎么在 C 盘」时无从判断该改哪里）
    pub data_dir_source: DataDirSource,
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Result<Arc<Self>> {
        Self::with_source(data_dir, DataDirSource::Fallback)
    }

    /// 带上「目录是怎么来的」一起构造（正常启动路径走这个）。
    pub fn with_source(data_dir: PathBuf, data_dir_source: DataDirSource) -> Result<Arc<Self>> {
        // logger 必须第一个初始化：后面所有 log::info/warn 只有在
        // logger 就位后才会真正被记录（没有 logger 时 log crate 静默丢弃）。
        crate::logging::init(&data_dir);
        let db_path = data_dir.join("wordwise.db");
        let db = Db::open(&db_path)?;

        // 优先读数据库里的配置；若库里没有则用默认值并落盘
        let config = db.load_config().unwrap_or_default();

        // 一次性配置迁移：把内置词典源升级到最新定义（含失效地址修复），
        // 并在未启用代理时关掉需要代理的源。
        //
        // 为什么必须在启动时做：词典源是跟着配置一起持久化的，
        // 光改代码里的 `default_sources()` 只能影响新装用户。
        // 迁移只在版本落后时执行一次，不会覆盖用户后续的手工调整。
        let mut config = config;
        if crate::dict::builtin::migrate_config(&mut config) {
            log::info!(
                "配置已迁移到 v{}（内置词典源已刷新）",
                crate::models::CONFIG_VERSION
            );
            if let Err(e) = db.save_config(&config) {
                log::warn!("配置迁移写回失败（不影响本次运行）：{}", e);
            }
        }

        // 若数据库中没有自定义源，用内置源补齐
        if config.dict_sources.is_empty() {
            config.dict_sources = crate::dict::builtin::default_sources();
        }

        // 保证 127.0.0.1 永不被代理：LM Studio 必须直连
        crate::net::ensure_no_proxy_env();

        let (http, proxy) = crate::net::build_client_with(&config.network)?;
        // 启动时如实打一行日志，方便用户/我们判断「到底走没走代理」
        log::info!("WordWise 网络：{}", proxy.describe());

        // 一次性语言修复迁移（需求 16）：纠正历史上被错标语言的内置词库。
        // 放在这里而不是各命令里，是因为它必须在**任何读写词的命令之前**跑完，
        // 且桌面端与移动端都走这个构造函数，不会漏掉一端。
        // 迁移自身会打印修了几行；这里只在失败时提醒一句。
        if let Err(e) = db.fix_builtin_book_langs(&crate::dict::importer::builtin_book_langs()) {
            log::warn!("词库语言修复迁移失败（不影响本次运行）：{e}");
        }

        // 紧随其后：清洗「列的 lang 与 entry_json 里的 lang 不一致」的存量脏数据。
        // 顺序不能反 —— 先由上面那步把列改到权威语言，这一步才<｜hy_place▁holder▁no▁813｜>到一个
        // 已经正确的目标值去对齐 JSON；反过来会把 JSON 写回旧的错误语言。
        if let Err(e) = db.repair_entry_json_langs() {
            log::warn!("词条语言对齐失败（读取时会自行兜底）：{e}");
        }

        let state = Arc::new(Self {
            db,
            config: RwLock::new(config),
            http: RwLock::new(http),
            proxy: RwLock::new(proxy),
            session: RwLock::new(Session::default()),
            review: RwLock::new(Session::default()),
            data_dir,
            data_dir_source,
        });
        // 后台词库内容增强：空闲时用本地大模型把单薄词条补成统一详细的详解。
        // 引擎内部有自己的启动延迟与节流，这里只管把它拉起来。
        crate::enrich::spawn(state.clone());
        Ok(state)
    }

    /// 取 HTTP 客户端。reqwest 的 Client 内部是 Arc，克隆很廉价。
    pub fn http(&self) -> reqwest::Client {
        self.http.read().clone()
    }

    /// 当前生效的代理信息。
    pub fn proxy_info(&self) -> crate::net::ProxyResolution {
        self.proxy.read().clone()
    }

    /// 按最新配置重建 HTTP 客户端（改代理后调用）。
    pub fn reload_http(&self) -> Result<crate::net::ProxyResolution> {
        let cfg = self.cfg();
        let (client, resolved) = crate::net::build_client_with(&cfg.network)?;
        *self.http.write() = client;
        *self.proxy.write() = resolved.clone();
        Ok(resolved)
    }

    /// 取配置快照（克隆一份，避免长时间持锁）。
    pub fn cfg(&self) -> AppConfig {
        self.config.read().clone()
    }

    /// 按 `kind` 选择会话槽位：缺省 / `"study"` → 背诵槽；`"review"` → 复习槽。
    ///
    /// 把「选哪个槽」收敛成一个函数，是因为有 6 个命令都要做这个判断；
    /// 各写一遍 `if kind == "review"` 迟早有人写漏一处，那一处就会继续
    /// 串到另一个会话里去。
    ///
    /// 无法识别的取值一律当 `"study"`：宁可退回旧行为，也不要因为前端多传了
    /// 一个没约定的值就让命令失败。
    pub fn session_slot(&self, kind: Option<&str>) -> &RwLock<Session> {
        if kind == Some("review") {
            &self.review
        } else {
            &self.session
        }
    }

    /// 更新配置并持久化。
    pub fn update_config(&self, f: impl FnOnce(&mut AppConfig)) -> Result<()> {
        let snapshot = {
            let mut w = self.config.write();
            f(&mut w);
            w.clone()
        };
        self.db.save_config(&snapshot)?;
        Ok(())
    }
}

/* ============================================================
   数据目录解析
   ============================================================

   历史包袱：早期版本**无条件**把数据放 `%APPDATA%\WordWise`（C 盘）。
   后果是「把软件装到 D 盘」也拦不住模型（最大 1.1 GB / 个）写进 C 盘，
   用户明明有盘却看着系统盘被塞满 —— 而且他没有任何入口能改。

   现在改成「**跟着软件走**」为主、老用户原地不动为辅：

     ① `WORDWISE_DATA_DIR` 环境变量      显式指定，最高优先
     ② exe 同级 `portable.txt`           便携模式：自包含，拷走即走
     ③ exe 同级 `location.txt`           用户在设置页改过目录（指针文件）
     ④ 系统用户目录里已有 wordwise.db     ★ 老用户护城河：绝不搬家
     ⑤ exe 同级 `data\`                   ★ 新装默认：装到哪，数据就在哪
     ⑥ 系统用户目录                       兜底（exe 目录不可写，如 Program Files）

   ④ 必须排在 ⑤ 前面：否则老用户升级后数据目录会从 `%APPDATA%` 变成
   `<exe>\data`，新目录没有 `wordwise.db`，应用会当成新装 → 词库、进度、
   错词本全部「凭空消失」。这条比「默认不写 C 盘」重要得多。
*/

/// exe 同级、用于「锁定数据位置」的标记文件。
///
/// - `portable.txt`：**存在即表示便携模式**（数据跟程序走）。文件内容为
///   绝对路径时用它；空文件 = exe 同级 `data\`。
/// - `location.txt`：设置页改过数据目录时写下的指针。内容为空或
///   `default` 视为「没指定」，继续往下走。
pub const PORTABLE_FILE: &str = "portable.txt";
/// 见 `PORTABLE_FILE` 说明。
pub const LOCATION_FILE: &str = "location.txt";

/// 数据目录落在哪一级 —— 设置页要如实告诉用户「为什么是这个目录」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DataDirSource {
    /// ① 环境变量 `WORDWISE_DATA_DIR`
    Env,
    /// ② exe 同级 `portable.txt`（便携模式，自包含）
    Portable,
    /// ③ exe 同级 `location.txt`（设置页改过）
    Pointer,
    /// ④ 系统用户目录里已有学习数据（老用户，原地不动）
    Existing,
    /// ⑤ exe 同级 `data\`（新装默认）
    BesideExe,
    /// ⑥ 系统用户目录（兜底）
    Fallback,
    /// ⑦ 由宿主系统分配的应用私有目录（Android / iOS）
    ///
    /// 移动端没有「exe 同级」这种概念：`std::env::current_exe()` 在 Android 上
    /// 返回的是 `/system/bin/app_process*`，`dirs::data_dir()` 也不可靠。
    /// 唯一正确的是 Tauri 在 setup 阶段给出的 `app.path().app_data_dir()`。
    System,
}

impl DataDirSource {
    /// 给用户看的一句话说明。
    pub fn label(self) -> &'static str {
        match self {
            DataDirSource::Env => "环境变量 WORDWISE_DATA_DIR 指定",
            DataDirSource::Portable => "便携模式（exe 同级有 portable.txt）",
            DataDirSource::Pointer => "你在设置页指定的目录",
            DataDirSource::Existing => "沿用系统用户目录里已有的学习数据",
            DataDirSource::BesideExe => "软件所在目录（默认，跟着程序走）",
            DataDirSource::Fallback => "系统用户目录（兜底）",
            DataDirSource::System => "应用私有目录（由系统分配）",
        }
    }

    /// 数据是不是**跟着软件目录**走的。
    ///
    /// 界面据此给出「删掉软件目录会连数据一起删掉」的提醒 —— 这正是
    /// 「默认不写 C 盘」的代价，必须说清楚，不能只享受好处。
    pub fn travels_with_app(self) -> bool {
        matches!(self, DataDirSource::Portable | DataDirSource::BesideExe)
    }
}

/// 读一个指针文件里写的绝对路径。空内容 / `default` / 读不到 → `None`。
///
/// 做成 `pub` 是因为模型目录也用同一套指针文件机制
/// （`localllm::models_dir` 读数据目录里的 `models_dir.txt`）。
pub fn read_pointer(path: &Path) -> Option<PathBuf> {
    let raw = std::fs::read_to_string(path).ok()?;
    parse_pointer_text(&raw)
}

/// 指针文件内容的解析规则（抽出来单独测，不用碰文件系统）。
fn parse_pointer_text(raw: &str) -> Option<PathBuf> {
    // 用户可能用记事本存成带 BOM 的 UTF-8，也可能顺手写了个 `default`
    let s = raw.trim_start_matches('\u{feff}').trim();
    if s.is_empty() || s.eq_ignore_ascii_case("default") {
        return None;
    }
    let p = PathBuf::from(s);
    if p.as_os_str().is_empty() {
        None
    } else {
        Some(p)
    }
}

/// 便携标记：exe 同级有 `portable.txt` 就是便携模式。
///
/// 返回 `(目标目录, 是否是用户显式写的路径)`：
/// - **空文件 / `default`** → `(exe 同级 data\, false)`。便携包解压出来就是
///   靠这个空文件把数据钉在程序目录里的，所以「空」必须算有效。
/// - **写了绝对路径** → `(那个路径, true)`。
///
/// 那个 `bool` 决定「不可写时怎么办」：用户显式指定的路径要**照做**
/// （否则他指定的位置被静默忽略，比报错更难排查）；而 `exe\data` 只是
/// 我们定的约定，装到 `Program Files` 这类只读位置时应当直接放弃它、
/// 继续往下找可写的地方。
fn portable_target(exe_dir: &Path) -> Option<(PathBuf, bool)> {
    let marker = exe_dir.join(PORTABLE_FILE);
    if !marker.is_file() {
        return None;
    }
    match read_pointer(&marker) {
        Some(p) => Some((p, true)),
        None => Some((exe_dir.join("data"), false)),
    }
}

/// 目录能不能写。
///
/// 会顺手把目录建出来（反正紧接着就要用），然后写一个探针文件再删掉。
/// 只判断「目录存在」是不够的：`C:\Program Files\...` 目录存在但只读，
/// 那样会在第一次存词时抛出一个很晚、很难懂的错误。
fn is_writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".wordwise-write-test");
    match std::fs::write(&probe, b"ok") {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// 数据目录解析的**纯函数**版本：环境变量 / exe 位置 / 系统数据目录
/// 全部做成入参，这样单测能在临时目录里确定性地覆盖每一条分支。
pub fn resolve_data_dir_with(
    exe_dir: Option<&Path>,
    env_dir: Option<&Path>,
    appdata_base: Option<&Path>,
) -> (PathBuf, DataDirSource) {
    // ① 环境变量：脚本 / 高级用户显式指定
    if let Some(p) = env_dir {
        if !p.as_os_str().is_empty() {
            return (p.to_path_buf(), DataDirSource::Env);
        }
    }

    // ② 便携标记（隐式约定要先确认目录可写，见 `portable_target` 的说明）
    if let Some(exe) = exe_dir {
        if let Some((p, explicit)) = portable_target(exe) {
            if explicit || is_writable(&p) {
                return (p, DataDirSource::Portable);
            }
        }
    }

    // ③ 设置页留下的指针
    if let Some(exe) = exe_dir {
        if let Some(p) = read_pointer(&exe.join(LOCATION_FILE)) {
            return (p, DataDirSource::Pointer);
        }
    }

    let appdata = appdata_base.map(|b| b.join("WordWise"));

    // ④ 老用户护城河 —— 必须在 ⑤ 之前，见本段开头的说明
    if let Some(ad) = &appdata {
        if ad.join("wordwise.db").is_file() {
            return (ad.clone(), DataDirSource::Existing);
        }
    }

    // ⑤ 新装默认：exe 同级 data\（装到 D 盘，数据就落 D 盘）
    if let Some(exe) = exe_dir {
        let beside = exe.join("data");
        if is_writable(&beside) {
            return (beside, DataDirSource::BesideExe);
        }
    }

    // ⑥ 兜底
    if let Some(ad) = appdata {
        return (ad, DataDirSource::Fallback);
    }
    (PathBuf::from(".").join("wordwise-data"), DataDirSource::Fallback)
}

/// 当前进程的可执行文件所在目录。
pub fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|x| x.to_path_buf()))
}

/// 应用数据目录（只关心路径的场景用这个）。
pub fn resolve_data_dir() -> PathBuf {
    resolve_data_dir_ex().0
}

/// 应用数据目录 + 它的来源（设置页要展示「为什么是这个目录」）。
pub fn resolve_data_dir_ex() -> (PathBuf, DataDirSource) {
    let env_dir = std::env::var_os("WORDWISE_DATA_DIR").map(PathBuf::from);
    let exe = exe_dir();
    let appdata = dirs::data_dir();
    resolve_data_dir_with(exe.as_deref(), env_dir.as_deref(), appdata.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::WordEntry;
    use std::fs;

    #[test]
    fn session_lifecycle() {
        let mut s = Session::default();
        assert!(!s.is_active());
        s.queue.push(WordEntry::new("apple"));
        assert!(s.is_active());
        assert_eq!(s.total(), 1);
        assert_eq!(s.current().unwrap().word, "apple");
        s.index = 1;
        assert!(!s.is_active());
    }

    /* ---------------- 数据目录解析 ---------------- */

    /// 每个用例一个独立临时目录（并行跑测试不会互相踩）。
    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "wordwise-statedir-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn pointer_text_ignores_blank_and_default() {
        assert_eq!(parse_pointer_text(""), None);
        assert_eq!(parse_pointer_text("   \r\n\t "), None);
        assert_eq!(parse_pointer_text("default"), None);
        assert_eq!(parse_pointer_text("DEFAULT"), None);
        // 记事本存的 UTF-8 会带 BOM
        assert_eq!(
            parse_pointer_text("\u{feff}D:\\Data"),
            Some(PathBuf::from("D:\\Data"))
        );
        assert_eq!(parse_pointer_text("  D:\\Data  "), Some(PathBuf::from("D:\\Data")));
    }

    #[test]
    fn env_var_wins_over_everything() {
        let base = tmp("env-wins");
        let exe = base.join("exe");
        let appdata = base.join("appdata");
        fs::create_dir_all(&exe).unwrap();
        // 同时存在便携标记 + 老数据 + 环境变量，环境变量必须赢
        fs::write(exe.join(PORTABLE_FILE), "").unwrap();
        fs::create_dir_all(appdata.join("WordWise")).unwrap();
        fs::write(appdata.join("WordWise").join("wordwise.db"), b"x").unwrap();

        let want = base.join("from-env");
        let (got, src) = resolve_data_dir_with(
            Some(&exe),
            Some(&want),
            Some(&appdata),
        );
        assert_eq!(got, want);
        assert_eq!(src, DataDirSource::Env);

        // 空字符串的环境变量等于没设，不能把数据目录解析成「当前目录」
        let (got2, src2) = resolve_data_dir_with(Some(&exe), Some(Path::new("")), Some(&appdata));
        assert_eq!(src2, DataDirSource::Portable);
        assert_eq!(got2, exe.join("data"));
    }

    #[test]
    fn portable_marker_pins_data_next_to_exe() {
        let base = tmp("portable");
        let exe = base.join("exe");
        fs::create_dir_all(&exe).unwrap();
        let appdata = base.join("appdata");
        // 即使系统目录里已经有学习数据，便携标记也要赢：那是用户**明确**的选择
        fs::create_dir_all(appdata.join("WordWise")).unwrap();
        fs::write(appdata.join("WordWise").join("wordwise.db"), b"x").unwrap();

        // 空文件 = 「就是便携模式」，数据放 exe 同级 data\
        fs::write(exe.join(PORTABLE_FILE), "\r\n").unwrap();
        let (got, src) = resolve_data_dir_with(Some(&exe), None, Some(&appdata));
        assert_eq!(src, DataDirSource::Portable);
        assert_eq!(got, exe.join("data"));
        assert!(src.travels_with_app(), "便携模式的数据是跟着软件走的");

        // 文件里写了绝对路径就用它
        fs::write(exe.join(PORTABLE_FILE), "D:\\WordWiseData").unwrap();
        let (got2, src2) = resolve_data_dir_with(Some(&exe), None, Some(&appdata));
        assert_eq!(src2, DataDirSource::Portable);
        assert_eq!(got2, PathBuf::from("D:\\WordWiseData"));
    }

    #[test]
    fn portable_marker_falls_through_when_exe_data_cannot_be_written() {
        // 模拟「装到 Program Files 这类只读位置」：exe 同级 `data` 这个位置
        // 被一个同名文件占着，目录建不出来。此时不能硬用它 ——
        // 否则会在第一次打开数据库时抛出一个很晚、很难懂的错误。
        let base = tmp("portable-blocked");
        let exe = base.join("exe");
        fs::create_dir_all(&exe).unwrap();
        fs::write(exe.join(PORTABLE_FILE), "").unwrap();
        fs::write(exe.join("data"), b"I am a file, not a directory").unwrap();
        let appdata = base.join("appdata");

        let (got, src) = resolve_data_dir_with(Some(&exe), None, Some(&appdata));
        assert_eq!(src, DataDirSource::Fallback);
        assert_eq!(got, appdata.join("WordWise"));
    }

    #[test]
    fn explicit_portable_path_is_honoured_even_before_it_exists() {
        // 用户/构建脚本明确写了路径 → 照做，不因为「目录还不存在」就忽略
        let base = tmp("portable-explicit");
        let exe = base.join("exe");
        fs::create_dir_all(&exe).unwrap();
        let want = base.join("elsewhere");
        fs::write(exe.join(PORTABLE_FILE), want.display().to_string()).unwrap();
        let appdata = base.join("appdata");

        let (got, src) = resolve_data_dir_with(Some(&exe), None, Some(&appdata));
        assert_eq!(src, DataDirSource::Portable);
        assert_eq!(got, want);
    }

    #[test]
    fn location_pointer_is_used_when_present() {
        let base = tmp("pointer");
        let exe = base.join("exe");
        fs::create_dir_all(&exe).unwrap();
        let appdata = base.join("appdata");

        fs::write(exe.join(LOCATION_FILE), "E:\\WW-Data\n").unwrap();
        let (got, src) = resolve_data_dir_with(Some(&exe), None, Some(&appdata));
        assert_eq!(src, DataDirSource::Pointer);
        assert_eq!(got, PathBuf::from("E:\\WW-Data"));

        // 「恢复默认」后指针文件里是 default → 不能把数据目录变成一个字面量目录名
        fs::write(exe.join(LOCATION_FILE), "default").unwrap();
        let (got2, src2) = resolve_data_dir_with(Some(&exe), None, Some(&appdata));
        assert_ne!(src2, DataDirSource::Pointer);
        assert_ne!(got2, PathBuf::from("default"));
    }

    #[test]
    fn fresh_install_prefers_beside_exe() {
        let base = tmp("fresh");
        let exe = base.join("exe");
        fs::create_dir_all(&exe).unwrap();
        // 系统目录里**没有**学习数据 = 新装
        let appdata = base.join("appdata");
        fs::create_dir_all(&appdata).unwrap();

        let (got, src) = resolve_data_dir_with(Some(&exe), None, Some(&appdata));
        assert_eq!(src, DataDirSource::BesideExe);
        assert_eq!(got, exe.join("data"), "新装默认跟着软件走，不写 C 盘");
        assert!(src.travels_with_app());
        assert!(got.is_dir(), "判可写时会把目录建出来");
    }

    #[test]
    fn existing_appdata_data_is_never_moved() {
        // ★ 这条是「老用户升级后词库凭空消失」的守卫。
        let base = tmp("existing");
        let exe = base.join("exe");
        fs::create_dir_all(&exe).unwrap();
        let appdata = base.join("appdata");
        let ad = appdata.join("WordWise");
        fs::create_dir_all(&ad).unwrap();
        fs::write(ad.join("wordwise.db"), b"sqlite").unwrap();

        let (got, src) = resolve_data_dir_with(Some(&exe), None, Some(&appdata));
        assert_eq!(src, DataDirSource::Existing);
        assert_eq!(got, ad, "老用户沿用 %APPDATA%\\WordWise，不搬家");
        assert!(!src.travels_with_app());
        assert!(
            !exe.join("data").exists(),
            "既然沿用了老目录，就不该顺手在 exe 旁边建 data\\"
        );
    }

    #[test]
    fn empty_appdata_dir_does_not_count_as_existing_data() {
        // 目录存在但没有 wordwise.db ≠ 有学习数据，不能据此把新装用户钉在 C 盘
        let base = tmp("empty-appdata");
        let exe = base.join("exe");
        fs::create_dir_all(&exe).unwrap();
        let appdata = base.join("appdata");
        fs::create_dir_all(appdata.join("WordWise")).unwrap();

        let (_, src) = resolve_data_dir_with(Some(&exe), None, Some(&appdata));
        assert_eq!(src, DataDirSource::BesideExe);
    }

    #[test]
    fn falls_back_when_there_is_no_exe_dir() {
        let base = tmp("no-exe");
        let appdata = base.join("appdata");
        fs::create_dir_all(&appdata).unwrap();
        let (got, src) = resolve_data_dir_with(None, None, Some(&appdata));
        assert_eq!(src, DataDirSource::Fallback);
        assert_eq!(got, appdata.join("WordWise"));

        let (got2, src2) = resolve_data_dir_with(None, None, None);
        assert_eq!(src2, DataDirSource::Fallback);
        assert_eq!(got2, PathBuf::from(".").join("wordwise-data"));
    }

    #[test]
    fn every_source_has_a_human_label() {
        for s in [
            DataDirSource::Env,
            DataDirSource::Portable,
            DataDirSource::Pointer,
            DataDirSource::Existing,
            DataDirSource::BesideExe,
            DataDirSource::Fallback,
        ] {
            assert!(!s.label().is_empty(), "{s:?} 必须给用户一句人话");
        }
    }
}
