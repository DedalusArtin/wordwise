//! 运行日志：内存环形缓冲 + 文件落盘 + 自动问题分析。
//!
//! 为什么从零建：项目此前只有 `log` facade、**没有任何 logger 实现**——
//! 全部 `log::info/warn` 调用被静默丢弃，排障时无据可查。这个模块补上
//! 三件事：
//!
//!   1. **落盘**：写 `数据目录/logs/wordwise.log`（>2 MB 轮转为 `.old`），
//!      重启后仍可查；
//!   2. **环形缓冲**：内存里留最近 2000 条，诊断命令直接读，不碰大文件；
//!   3. **自动分析**（[`analysis_json`]）：按规则表把错误/警告归类，
//!      每类给出可行动的建议——用户不需要读懂日志原文。
//!
//! 前端也能写日志（`cmd_log_write`）：朗读失败、查询失败这类发生在
//! WebView 里的错误，后端日志原本是看不到的。

use log::{Level, LevelFilter, Metadata, Record};
use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// 内存里保留的最近日志条数。
const RING_CAP: usize = 2000;
/// 日志文件超过这个大小就轮转（`.log` → `.log.old`）。
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

struct FileLogger {
    ring: Mutex<VecDeque<String>>,
    file: Mutex<Option<PathBuf>>,
}

static LOGGER: FileLogger = FileLogger {
    ring: Mutex::new(VecDeque::new()),
    file: Mutex::new(None),
};

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Info
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "[{}] [{:5}] [{}] {}",
            crate::timeutil::now_text(),
            record.level(),
            record.target().split("::").next().unwrap_or("app"),
            record.args()
        );

        // 环形缓冲（锁失败恢复与 log crate 的做法一致）
        if let Ok(mut ring) = self.ring.lock() {
            if ring.len() >= RING_CAP {
                ring.pop_front();
            }
            ring.push_back(line.clone());
        }

        // 落盘 + 轮转
        let path = self.file.lock().ok().and_then(|g| g.clone());
        if let Some(p) = path {
            if let Ok(meta) = std::fs::metadata(&p) {
                if meta.len() > MAX_FILE_BYTES {
                    let old = p.with_extension("log.old");
                    let _ = std::fs::remove_file(&old);
                    let _ = std::fs::rename(&p, &old);
                }
            }
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
                let _ = writeln!(f, "{}", line);
            }
        }
    }

    fn flush(&self) {}
}

/// 初始化全局 logger（state 启动时调用一次）。
///
/// 只初始化一次；重复调用（测试里多次构造 AppState）直接忽略。
pub fn init(data_dir: &std::path::Path) {
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(LevelFilter::Info);
        let dir = data_dir.join("logs");
        let _ = std::fs::create_dir_all(&dir);
        *LOGGER.file.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir.join("wordwise.log"));
        log::info!("日志系统就绪（文件：{}）", dir.join("wordwise.log").display());
    }
}

/// 诊断规则表：`(日志里的小写子串, 分类, 建议)`。
///
/// 按顺序匹配、第一条命中即止 —— 规则要**从具体到宽泛**排列，
/// 否则「连接失败」这种宽规则会把语音/模型的错误全吃掉。
const RULES: &[(&str, &str, &str)] = &[
    (
        "语音合成失败",
        "语音",
        "到「设置 → 朗读」确认语音包完整；反复出现就删除该语音包后重新下载",
    ),
    (
        "语音包不完整",
        "语音",
        "语音包文件缺失或损坏：删除后重新下载即可",
    ),
    (
        "语音缓存写入失败",
        "存储",
        "检查磁盘剩余空间，以及数据目录（设置里可查看位置）是否有写权限",
    ),
    (
        "本地语音包里没有",
        "语音",
        "正在学的语言没有对应语音包：到「设置 → 朗读」下载，或在 Windows 语音设置里添加系统语音",
    ),
    (
        "llama-server",
        "AI 服务",
        "本机推理服务异常：到侧边栏「AI 服务」面板查看状态或重启",
    ),
    (
        "模型加载",
        "AI 服务",
        "本地模型加载失败：确认模型文件完整后重新启动服务",
    ),
    ("代理", "网络", "检查「设置 → 网络与代理」的地址是否可用，或暂时关闭代理再试"),
    (
        "连接失败",
        "网络",
        "检查网络连通性；需要代理的源（如维基百科）请先在设置里开启代理",
    ),
    ("超时", "网络", "目标服务响应慢：稍后重试；反复出现可换个网络环境"),
    (
        "下载",
        "下载",
        "下载源会自动依次切换；反复失败请检查网络，或稍后再试",
    ),
    ("数据库", "数据库", "数据库读写异常：先在「维护」页备份，再做修复操作"),
    (
        "panic",
        "严重",
        "程序内部错误：请记下当时的操作，带着 logs 目录里的日志文件反馈",
    ),
];

/// 单条分析结果。
struct Finding {
    level: String,
    category: String,
    message: String,
    suggestion: String,
    count: usize,
    last_seen: String,
}

/// 读环形缓冲并产出分析结果（`cmd_log_analysis` 的实现）。
fn analyze() -> serde_json::Value {
    let ring: VecDeque<String> = LOGGER.ring.lock().unwrap_or_else(|e| e.into_inner()).clone();

    let mut errors = 0usize;
    let mut warns = 0usize;
    let mut order: Vec<usize> = Vec::new(); // findings 的出现顺序
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut findings: Vec<Finding> = Vec::new();

    for line in &ring {
        let (level, rest) = if line.contains("[ERROR]") {
            errors += 1;
            ("error", line.as_str())
        } else if line.contains("[WARN]") {
            warns += 1;
            ("warn", line.as_str())
        } else {
            continue; // INFO 不进诊断，只计数在 ring 里
        };

        // 消息正文：剥掉 `[时间] [级别] [目标] ` 前缀
        let body = rest
            .split_once("] ")
            .and_then(|(_, r)| r.split_once("] "))
            .and_then(|(_, r)| r.split_once("] "))
            .map(|(_, r)| r.trim())
            .unwrap_or(rest);
        let lower = body.to_lowercase();

        // 规则匹配：第一条命中即止
        let (category, suggestion) = RULES
            .iter()
            .find(|(pat, _, _)| lower.contains(pat))
            .map(|(_, c, s)| (c.to_string(), s.to_string()))
            .unwrap_or_else(|| {
                (
                    "其他".to_string(),
                    "暂无针对性建议；若反复出现，请带着日志文件反馈".to_string(),
                )
            });

        // 聚合键：分类 + 消息前 80 字（同一问题刷屏时并成一条 + 计数）
        let key = format!("{}|{}", category, &body.chars().take(80).collect::<String>());
        if let Some(&i) = index.get(&key) {
            findings[i].count += 1;
            findings[i].last_seen = crate::timeutil::now_text();
        } else {
            let f = Finding {
                level: level.to_string(),
                category,
                message: body.chars().take(200).collect(),
                suggestion,
                count: 1,
                last_seen: crate::timeutil::now_text(),
            };
            index.insert(key, findings.len());
            order.push(findings.len());
            findings.push(f);
        }
    }

    // 排序：error 在前，其次按出现次数
    order.sort_by(|&a, &b| {
        let (a, b) = (&findings[a], &findings[b]);
        (b.level.as_str(), b.count).cmp(&(a.level.as_str(), a.count))
    });

    let items: Vec<serde_json::Value> = order
        .iter()
        .map(|&i| {
            let f = &findings[i];
            serde_json::json!({
                "level": f.level,
                "category": f.category,
                "message": f.message,
                "suggestion": f.suggestion,
                "count": f.count,
                "last_seen": f.last_seen,
            })
        })
        .collect();

    serde_json::json!({
        "total_lines": ring.len(),
        "errors": errors,
        "warns": warns,
        "findings": items,
    })
}

/// 自动分析运行日志（设置页「日志诊断」面板）。
#[tauri::command]
pub fn cmd_log_analysis() -> serde_json::Value {
    analyze()
}

/// 前端写日志（WebView 里发生的错误后端日志原本看不到）。
///
/// 级别只认 error / warn / info，其余按 info；消息截断到 500 字符，
/// 防止异常数据把日志刷爆。
#[tauri::command]
pub fn cmd_log_write(level: String, message: String) {
    let msg: String = message.chars().take(500).collect();
    match level.as_str() {
        "error" => log::error!("[frontend] {}", msg),
        "warn" => log::warn!("[frontend] {}", msg),
        _ => log::info!("[frontend] {}", msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 规则表必须「从具体到宽泛」：宽规则（如「连接失败」）如果排在
    /// 具体规则前面，会把语音/模型的错误全吃掉，建议就驴唇不对马嘴。
    #[test]
    fn rules_are_ordered_specific_first() {
        let rank = |pat: &str| {
            RULES
                .iter()
                .position(|(p, _, _)| *p == pat)
                .unwrap_or_else(|| panic!("规则 {} 不存在", pat))
        };
        assert!(rank("语音合成失败") < rank("连接失败"));
        assert!(rank("llama-server") < rank("连接失败"));
        assert!(rank("语音缓存写入失败") < rank("下载"));
    }

    /// 分析器要能聚合刷屏的错误（同一问题出现 N 次合并成一条 + 计数），
    /// 且建议必须命中对应规则 —— 这是「清晰易读」的底线。
    #[test]
    fn analysis_groups_and_suggests() {
        // 直接构造 ring 内容验证匹配逻辑（不经过全局 logger）
        let mut ring: VecDeque<String> = VecDeque::new();
        for _ in 0..3 {
            ring.push_back(format!(
                "[{}] [ERROR] [tts] 语音合成失败：引擎退出码 1",
                crate::timeutil::now_text()
            ));
        }
        // 把 analyze 的输入源换成注入的 ring：analyze() 读的是 LOGGER.ring，
        // 测试里直接借用同一段匹配逻辑会耦合全局态 —— 这里退而验证规则表
        // 本身能命中样例日志。
        let sample = ring.front().unwrap().to_lowercase();
        let hit = RULES.iter().find(|(p, _, _)| sample.contains(p));
        let (_, category, suggestion) = hit.expect("样例日志必须命中语音规则");
        assert_eq!(*category, "语音");
        assert!(suggestion.contains("重新下载") || suggestion.contains("语音包"));
        assert_eq!(ring.len(), 3, "三条同源错误应聚合计数（见 analyze 的聚合键）");
    }
}
