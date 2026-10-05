//! 应用共享状态：数据库、配置、HTTP 客户端、当前学习会话。
//!
//! 用 `parking_lot::RwLock` 而非 tokio 的锁，因为大部分命令是短同步操作；
//! 网络相关命令在 async 函数里先取快照再释放锁，避免跨 await 持有。

use crate::db::Db;
use crate::models::{AppConfig, QuizMode};
use anyhow::Result;
use parking_lot::RwLock;
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
    /// 导出目录（打包后指向用户可写目录）
    pub data_dir: std::path::PathBuf,
}

impl AppState {
    pub fn new(data_dir: std::path::PathBuf) -> Result<Arc<Self>> {
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

        Ok(Arc::new(Self {
            db,
            config: RwLock::new(config),
            http: RwLock::new(http),
            proxy: RwLock::new(proxy),
            session: RwLock::new(Session::default()),
            data_dir,
        }))
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

/// 应用数据目录：优先用传入的 workspace 目录，否则退回系统 AppData。
pub fn resolve_data_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("WORDWISE_DATA_DIR") {
        let p = std::path::PathBuf::from(dir);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    // Windows: %APPDATA%\WordWise
    if let Some(base) = dirs::data_dir() {
        return base.join("WordWise");
    }
    std::path::PathBuf::from(".").join("wordwise-data")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::WordEntry;

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
}
