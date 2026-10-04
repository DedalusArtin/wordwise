//! SQLite 存储层：词库、学习状态、错词本、复习日志、配置。
//!
//! 所有数据落在用户 AppData 目录下的 `wordwise.db`，随安装包分发、卸载可选保留。

use crate::models::{AppConfig, DictSourceConfig, StudyState, WordEntry};
use anyhow::{Context, Result};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 数据库句柄。用 Mutex 包住 Connection，满足 Tauri 状态的 Send + Sync 要求。
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
    path: PathBuf,
}

/// 词库中一条记录。
#[derive(Debug, Clone, serde::Serialize)]
pub struct WordRow {
    pub word: String,
    pub lang: String,
    pub entry: WordEntry,
    pub added_at: i64,
}

impl Db {
    /// 打开（或创建）数据库。
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(&path)
            .with_context(|| format!("无法打开数据库: {}", path.display()))?;

        // WAL 模式提升并发读写体验
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
            path,
        };
        db.migrate()?;
        Ok(db)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 建表。使用 IF NOT EXISTS 保证幂等，便于版本升级。
    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute_batch(
            r#"
            -- 词库：存归一化后的完整词条 JSON
            CREATE TABLE IF NOT EXISTS words (
                word        TEXT NOT NULL,
                lang        TEXT NOT NULL DEFAULT 'en',
                entry_json  TEXT NOT NULL,
                added_at    INTEGER NOT NULL,
                PRIMARY KEY (word, lang)
            );

            -- 学习状态：SM-2 调度所需字段
            CREATE TABLE IF NOT EXISTS study_state (
                word            TEXT NOT NULL,
                lang            TEXT NOT NULL DEFAULT 'en',
                ease_factor     REAL NOT NULL DEFAULT 2.5,
                interval_days   REAL NOT NULL DEFAULT 0,
                repetitions     INTEGER NOT NULL DEFAULT 0,
                due_at          INTEGER NOT NULL DEFAULT 0,
                last_review_at  INTEGER NOT NULL DEFAULT 0,
                correct_count   INTEGER NOT NULL DEFAULT 0,
                wrong_count     INTEGER NOT NULL DEFAULT 0,
                is_leech        INTEGER NOT NULL DEFAULT 0,
                mastery         INTEGER NOT NULL DEFAULT 0,
                is_mastered     INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (word, lang)
            );

            -- 复习日志：用于统计与遗忘曲线校准
            CREATE TABLE IF NOT EXISTS review_log (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                word        TEXT NOT NULL,
                lang        TEXT NOT NULL DEFAULT 'en',
                grade       TEXT NOT NULL,
                mode        TEXT NOT NULL,
                reviewed_at INTEGER NOT NULL,
                elapsed_ms  INTEGER NOT NULL DEFAULT 0
            );

            -- 配置：键值对存储
            CREATE TABLE IF NOT EXISTS config (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            -- 自定义词典源（需求 6）
            CREATE TABLE IF NOT EXISTS dict_sources (
                id       TEXT PRIMARY KEY,
                json     TEXT NOT NULL,
                ord      INTEGER NOT NULL DEFAULT 0
            );

            -- 词典缓存：避免重复联网
            CREATE TABLE IF NOT EXISTS dict_cache (
                word        TEXT NOT NULL,
                lang        TEXT NOT NULL DEFAULT 'en',
                source      TEXT NOT NULL DEFAULT '',
                entry_json  TEXT NOT NULL,
                cached_at   INTEGER NOT NULL,
                PRIMARY KEY (word, lang)
            );

            -- 搜索历史
            CREATE TABLE IF NOT EXISTS search_log (
                id     INTEGER PRIMARY KEY AUTOINCREMENT,
                query  TEXT NOT NULL,
                at     INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_state_due ON study_state(due_at);
            CREATE INDEX IF NOT EXISTS idx_state_leech ON study_state(is_leech);
            CREATE INDEX IF NOT EXISTS idx_log_at ON review_log(reviewed_at);
            "#,
        )?;
        Ok(())
    }

    // ---------- 词库 ----------

    /// 插入或更新词条（词库导入用）。
    pub fn upsert_word(&self, entry: &WordEntry, now: i64) -> Result<()> {
        let conn = self.conn.lock();
        let json = serde_json::to_string(entry)?;
        conn.execute(
            "INSERT INTO words(word, lang, entry_json, added_at) VALUES(?1,?2,?3,?4)
             ON CONFLICT(word, lang) DO UPDATE SET entry_json=excluded.entry_json",
            params![entry.word, entry.lang, json, now],
        )?;
        Ok(())
    }

    /// 批量导入，事务包裹，速度快得多。
    pub fn bulk_upsert_words(&self, entries: &[WordEntry], now: i64) -> Result<usize> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut n = 0;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO words(word, lang, entry_json, added_at) VALUES(?1,?2,?3,?4)
                 ON CONFLICT(word, lang) DO UPDATE SET entry_json=excluded.entry_json",
            )?;
            for e in entries {
                let json = serde_json::to_string(e)?;
                stmt.execute(params![e.word, e.lang, json, now])?;
                n += 1;
            }
        }
        tx.commit()?;
        Ok(n)
    }

    pub fn get_word(&self, word: &str, lang: &str) -> Result<Option<WordEntry>> {
        let conn = self.conn.lock();
        let row: Option<String> = conn
            .query_row(
                "SELECT entry_json FROM words WHERE word=?1 AND lang=?2",
                params![word, lang],
                |r| r.get(0),
            )
            .optional()?;
        match row {
            Some(j) => Ok(Some(serde_json::from_str(&j)?)),
            None => Ok(None),
        }
    }

    pub fn word_count(&self, lang: &str) -> Result<i64> {
        let conn = self.conn.lock();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM words WHERE lang=?1",
            params![lang],
            |r| r.get(0),
        )?;
        Ok(n)
    }

    /// 列出词库（分页）。
    pub fn list_words(&self, lang: &str, limit: i64, offset: i64) -> Result<Vec<WordRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT word, lang, entry_json, added_at FROM words
             WHERE lang=?1 ORDER BY added_at DESC, word ASC LIMIT ?2 OFFSET ?3",
        )?;
        let rows = stmt.query_map(params![lang, limit, offset], |r| {
            let word: String = r.get(0)?;
            let lang: String = r.get(1)?;
            let json: String = r.get(2)?;
            let added_at: i64 = r.get(3)?;
            Ok((word, lang, json, added_at))
        })?;

        let mut out = Vec::new();
        for row in rows {
            let (word, lang, json, added_at) = row?;
            let entry: WordEntry = serde_json::from_str(&json).unwrap_or_else(|_| WordEntry::new(&word));
            out.push(WordRow { word, lang, entry, added_at });
        }
        Ok(out)
    }

    /// 模糊搜索词库。
    pub fn search_words(&self, lang: &str, q: &str, limit: i64) -> Result<Vec<WordRow>> {
        let conn = self.conn.lock();
        // 转义 LIKE 的通配符，避免用户输入的 % 或 _ 被当作模式匹配
        let escaped = q
            .to_lowercase()
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("%{}%", escaped);
        let mut stmt = conn.prepare(
            "SELECT word, lang, entry_json, added_at FROM words
             WHERE lang=?1 AND lower(word) LIKE ?2 ESCAPE '\\'
             ORDER BY length(word), word LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![lang, pattern, limit], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (word, lang, json, added_at) = row?;
            let entry: WordEntry = serde_json::from_str(&json).unwrap_or_else(|_| WordEntry::new(&word));
            out.push(WordRow { word, lang, entry, added_at });
        }
        Ok(out)
    }

    /// 随机抽取词库中的若干词（用于生成干扰项、随机测试）。
    pub fn random_words(&self, lang: &str, limit: i64) -> Result<Vec<WordEntry>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT entry_json FROM words WHERE lang=?1 ORDER BY RANDOM() LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![lang, limit], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            if let Ok(e) = serde_json::from_str::<WordEntry>(&row?) {
                out.push(e);
            }
        }
        Ok(out)
    }

    pub fn delete_word(&self, word: &str, lang: &str) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM words WHERE word=?1 AND lang=?2", params![word, lang])?;
        tx.execute("DELETE FROM study_state WHERE word=?1 AND lang=?2", params![word, lang])?;
        tx.commit()?;
        Ok(())
    }

    // ---------- 学习状态 ----------

    pub fn upsert_state(&self, s: &StudyState) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            r#"INSERT INTO study_state
               (word,lang,ease_factor,interval_days,repetitions,due_at,last_review_at,
                correct_count,wrong_count,is_leech,mastery,is_mastered)
               VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
               ON CONFLICT(word,lang) DO UPDATE SET
                 ease_factor=excluded.ease_factor,
                 interval_days=excluded.interval_days,
                 repetitions=excluded.repetitions,
                 due_at=excluded.due_at,
                 last_review_at=excluded.last_review_at,
                 correct_count=excluded.correct_count,
                 wrong_count=excluded.wrong_count,
                 is_leech=excluded.is_leech,
                 mastery=excluded.mastery,
                 is_mastered=excluded.is_mastered"#,
            params![
                s.word,
                s.lang,
                s.ease_factor,
                s.interval_days,
                s.repetitions,
                s.due_at,
                s.last_review_at,
                s.correct_count,
                s.wrong_count,
                s.is_leech as i32,
                s.mastery,
                s.is_mastered as i32
            ],
        )?;
        Ok(())
    }

    pub fn get_state(&self, word: &str, lang: &str) -> Result<Option<StudyState>> {
        let conn = self.conn.lock();
        let r = conn
            .query_row(
                r#"SELECT word,lang,ease_factor,interval_days,repetitions,due_at,last_review_at,
                          correct_count,wrong_count,is_leech,mastery,is_mastered
                   FROM study_state WHERE word=?1 AND lang=?2"#,
                params![word, lang],
                row_to_state,
            )
            .optional()?;
        Ok(r)
    }

    /// 取到期的复习队列。
    ///
    /// 排序策略（需求 4）：
    /// 1. 强化记忆词优先（is_leech）
    /// 2. 已逾期的先复习
    /// 3. 逾期越久优先（due_at 越小越优先）
    pub fn due_states(&self, lang: &str, now: i64, limit: i64) -> Result<Vec<StudyState>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"SELECT word,lang,ease_factor,interval_days,repetitions,due_at,last_review_at,
                      correct_count,wrong_count,is_leech,mastery,is_mastered
               FROM study_state
               WHERE lang=?1 AND is_mastered=0 AND due_at<=?2
               ORDER BY is_leech DESC, due_at ASC
               LIMIT ?3"#,
        )?;
        let rows = stmt.query_map(params![lang, now, limit], row_to_state)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 取某一天到期的词（复习计划视图）。
    pub fn states_due_between(
        &self,
        lang: &str,
        start: i64,
        end: i64,
    ) -> Result<Vec<StudyState>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"SELECT word,lang,ease_factor,interval_days,repetitions,due_at,last_review_at,
                      correct_count,wrong_count,is_leech,mastery,is_mastered
               FROM study_state
               WHERE lang=?1 AND is_mastered=0 AND due_at>=?2 AND due_at<?3
               ORDER BY due_at ASC"#,
        )?;
        let rows = stmt.query_map(params![lang, start, end], row_to_state)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn all_states(&self, lang: &str) -> Result<Vec<StudyState>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"SELECT word,lang,ease_factor,interval_days,repetitions,due_at,last_review_at,
                      correct_count,wrong_count,is_leech,mastery,is_mastered
               FROM study_state WHERE lang=?1"#,
        )?;
        let rows = stmt.query_map(params![lang], row_to_state)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 常错词列表（错词本）。
    pub fn leech_states(&self, lang: &str, limit: i64) -> Result<Vec<StudyState>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"SELECT word,lang,ease_factor,interval_days,repetitions,due_at,last_review_at,
                      correct_count,wrong_count,is_leech,mastery,is_mastered
               FROM study_state
               WHERE lang=?1 AND is_leech=1
               ORDER BY wrong_count DESC, mastery ASC LIMIT ?2"#,
        )?;
        let rows = stmt.query_map(params![lang, limit], row_to_state)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 取学习状态最差的一批词（用于优先出题）。
    pub fn weakest_states(&self, lang: &str, limit: i64) -> Result<Vec<StudyState>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"SELECT word,lang,ease_factor,interval_days,repetitions,due_at,last_review_at,
                      correct_count,wrong_count,is_leech,mastery,is_mastered
               FROM study_state
               WHERE lang=?1 AND is_mastered=0
               ORDER BY mastery ASC, wrong_count DESC LIMIT ?2"#,
        )?;
        let rows = stmt.query_map(params![lang, limit], row_to_state)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    // ---------- 复习日志 ----------

    pub fn log_review(
        &self,
        word: &str,
        lang: &str,
        grade: &str,
        mode: &str,
        now: i64,
        elapsed_ms: i64,
    ) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO review_log(word,lang,grade,mode,reviewed_at,elapsed_ms)
             VALUES(?1,?2,?3,?4,?5,?6)",
            params![word, lang, grade, mode, now, elapsed_ms],
        )?;
        Ok(())
    }

    /// 今日答题统计。
    pub fn today_counts(&self, now: i64) -> Result<(i64, i64, i64)> {
        let (start, _) = crate::timeutil::day_bounds(now);
        let conn = self.conn.lock();
        let total: i64 = conn.query_row(
            "SELECT COUNT(*) FROM review_log WHERE reviewed_at>=?1 AND reviewed_at<?2",
            params![start, start + 86400],
            |r| r.get(0),
        )?;
        let correct: i64 = conn.query_row(
            "SELECT COUNT(*) FROM review_log WHERE reviewed_at>=?1 AND reviewed_at<?2 AND grade!='wrong'",
            params![start, start + 86400],
            |r| r.get(0),
        )?;
        let wrong = total - correct;
        Ok((total, correct, wrong))
    }

    /// 最近 N 天的每日复习量。
    pub fn daily_history(&self, days: i64, now: i64) -> Result<Vec<(String, i64, i64)>> {
        let (today_start, _) = crate::timeutil::day_bounds(now);
        let from = today_start - (days - 1) * 86400;
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT reviewed_at, grade FROM review_log WHERE reviewed_at>=?1",
        )?;
        let rows = stmt.query_map(params![from], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;

        use std::collections::BTreeMap;
        let mut map: BTreeMap<String, (i64, i64)> = BTreeMap::new();
        for r in rows {
            let (ts, grade) = r?;
            let key = crate::srs::day_key(ts);
            let e = map.entry(key).or_insert((0, 0));
            e.0 += 1;
            if grade != "wrong" {
                e.1 += 1;
            }
        }

        let mut out = Vec::new();
        for i in (0..days).rev() {
            let ts = today_start - i * 86400;
            let key = crate::srs::day_key(ts);
            let (c, ok) = map.get(&key).copied().unwrap_or((0, 0));
            out.push((key, c, ok));
        }
        Ok(out)
    }

    /// 计算连续学习天数（streak）。
    pub fn streak(&self, now: i64) -> Result<i64> {
        let (today_start, _) = crate::timeutil::day_bounds(now);
        let conn = self.conn.lock();
        let stmt = conn.prepare(
            "SELECT DISTINCT reviewed_at FROM review_log WHERE reviewed_at>=?1",
        )?;
        // 往前查 400 天足够
        let from = today_start - 400 * 86400;
        drop(stmt);

        let mut stmt = conn.prepare("SELECT reviewed_at FROM review_log WHERE reviewed_at>=?1")?;
        let rows = stmt.query_map(params![from], |r| r.get::<_, i64>(0))?;
        let mut days: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();
        for r in rows {
            let ts = r?;
            let (ds, _) = crate::timeutil::day_bounds(ts);
            days.insert(ds);
        }

        let mut streak = 0i64;
        let mut cursor = today_start;
        // 今天没学不打断连续（从昨天往前算）
        if !days.contains(&cursor) {
            cursor -= 86400;
        }
        while days.contains(&cursor) {
            streak += 1;
            cursor -= 86400;
        }
        Ok(streak)
    }

    /// 已开始学习的词数。
    pub fn learned_count(&self, lang: &str) -> Result<i64> {
        let conn = self.conn.lock();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM study_state WHERE lang=?1 AND (correct_count+wrong_count)>0",
            params![lang],
            |r| r.get(0),
        )?;
        Ok(n)
    }

    pub fn mastered_count(&self, lang: &str) -> Result<i64> {
        let conn = self.conn.lock();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM study_state WHERE lang=?1 AND is_mastered=1",
            params![lang],
            |r| r.get(0),
        )?;
        Ok(n)
    }

    pub fn leech_count(&self, lang: &str) -> Result<i64> {
        let conn = self.conn.lock();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM study_state WHERE lang=?1 AND is_leech=1",
            params![lang],
            |r| r.get(0),
        )?;
        Ok(n)
    }

    // ---------- 配置 ----------

    pub fn save_config(&self, cfg: &AppConfig) -> Result<()> {
        let conn = self.conn.lock();
        let json = serde_json::to_string(cfg)?;
        conn.execute(
            "INSERT INTO config(key,value) VALUES('app',?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![json],
        )?;
        Ok(())
    }

    pub fn load_config(&self) -> Result<AppConfig> {
        let conn = self.conn.lock();
        let v: Option<String> = conn
            .query_row("SELECT value FROM config WHERE key='app'", [], |r| r.get(0))
            .optional()?;
        drop(conn);
        match v {
            Some(j) => Ok(serde_json::from_str(&j).unwrap_or_default()),
            None => Ok(AppConfig::default()),
        }
    }

    // ---------- 自定义词典源 ----------

    pub fn save_sources(&self, sources: &[DictSourceConfig]) -> Result<()> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM dict_sources", [])?;
        {
            let mut stmt = tx.prepare("INSERT INTO dict_sources(id,json,ord) VALUES(?1,?2,?3)")?;
            for (i, s) in sources.iter().enumerate() {
                stmt.execute(params![s.id, serde_json::to_string(s)?, i as i64])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn load_sources(&self) -> Result<Vec<DictSourceConfig>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT json FROM dict_sources ORDER BY ord ASC")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            if let Ok(s) = serde_json::from_str::<DictSourceConfig>(&r?) {
                out.push(s);
            }
        }
        Ok(out)
    }

    // ---------- 词典缓存 ----------

    pub fn cache_entry(&self, e: &WordEntry, source: &str, now: i64) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO dict_cache(word,lang,source,entry_json,cached_at) VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(word,lang) DO UPDATE SET
               source=excluded.source, entry_json=excluded.entry_json, cached_at=excluded.cached_at",
            params![e.word, e.lang, source, serde_json::to_string(e)?, now],
        )?;
        Ok(())
    }

    /// 读缓存，超过 ttl_secs 视为过期。
    pub fn get_cached(&self, word: &str, lang: &str, ttl_secs: i64, now: i64) -> Result<Option<WordEntry>> {
        let conn = self.conn.lock();
        let r: Option<(String, i64)> = conn
            .query_row(
                "SELECT entry_json, cached_at FROM dict_cache WHERE word=?1 AND lang=?2",
                params![word, lang],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match r {
            Some((j, at)) if now - at < ttl_secs => Ok(serde_json::from_str(&j).ok()),
            _ => Ok(None),
        }
    }

    pub fn clear_cache(&self) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM dict_cache", [])?;
        Ok(())
    }

    // ---------- 搜索历史 ----------

    pub fn log_search(&self, query: &str, now: i64) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO search_log(query,at) VALUES(?1,?2)",
            params![query, now],
        )?;
        // 只保留最近 200 条
        conn.execute(
            "DELETE FROM search_log WHERE id NOT IN (SELECT id FROM search_log ORDER BY at DESC LIMIT 200)",
            [],
        )?;
        Ok(())
    }

    pub fn recent_searches(&self, limit: i64) -> Result<Vec<String>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT query FROM search_log ORDER BY at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 导出全部学习数据为 JSON（备份 / 跨设备迁移）。
    pub fn export_all(&self) -> Result<serde_json::Value> {
        let conn = self.conn.lock();
        let mut words = Vec::new();
        {
            let mut stmt = conn.prepare("SELECT entry_json FROM words")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for r in rows {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&r?) {
                    words.push(v);
                }
            }
        }
        let mut states = Vec::new();
        {
            let mut stmt = conn.prepare(
                r#"SELECT word,lang,ease_factor,interval_days,repetitions,due_at,last_review_at,
                          correct_count,wrong_count,is_leech,mastery,is_mastered FROM study_state"#,
            )?;
            let rows = stmt.query_map([], row_to_state)?;
            for r in rows {
                states.push(serde_json::to_value(r?)?);
            }
        }
        let cfg = self.load_config()?;
        Ok(serde_json::json!({
            "version": 1,
            "exported_at": crate::timeutil::now_ts(),
            "words": words,
            "states": states,
            "config": cfg,
        }))
    }
}

/// rusqlite row → StudyState
fn row_to_state(r: &rusqlite::Row) -> rusqlite::Result<StudyState> {
    Ok(StudyState {
        word: r.get(0)?,
        lang: r.get(1)?,
        ease_factor: r.get(2)?,
        interval_days: r.get(3)?,
        repetitions: r.get(4)?,
        due_at: r.get(5)?,
        last_review_at: r.get(6)?,
        correct_count: r.get(7)?,
        wrong_count: r.get(8)?,
        is_leech: r.get::<_, i32>(9)? != 0,
        mastery: r.get(10)?,
        is_mastered: r.get::<_, i32>(11)? != 0,
    })
}
