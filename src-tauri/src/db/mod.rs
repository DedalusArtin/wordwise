//! SQLite 存储层：词库、学习状态、错词本、复习日志、配置。
//!
//! 所有数据落在用户 AppData 目录下的 `wordwise.db`，随安装包分发、卸载可选保留。

use crate::models::{AppConfig, DictSourceConfig, StudyState, WordEntry, Wordbook};
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

            -- ============ 词库分级（需求 5 / 20） ============
            -- 一个「词库」是一本书（四级核心词、考研词汇、雅思…），
            -- 有自己的元信息与归属（考试分类 / 是否内置 / 来源引用）。
            CREATE TABLE IF NOT EXISTS wordbooks (
                id           TEXT PRIMARY KEY,          -- 稳定标识，如 cet4 / kaoyan / ielts
                name         TEXT NOT NULL,             -- 显示名，如「四级核心词汇」
                category     TEXT NOT NULL DEFAULT '',  -- 考试分类：cet4 / cet6 / kaoyan / ielts / toefl / gre / other
                level        INTEGER NOT NULL DEFAULT 0,-- 层级：0 根 / 1 考试大类 / 2 子词库
                parent_id    TEXT NOT NULL DEFAULT '',  -- 上级词库 id（空表示顶层）
                lang         TEXT NOT NULL DEFAULT 'en',
                description  TEXT NOT NULL DEFAULT '',
                source_url   TEXT NOT NULL DEFAULT '',  -- 来源地址（GitHub 公开词库等）
                license      TEXT NOT NULL DEFAULT '',  -- 来源许可说明（引用所需）
                word_count   INTEGER NOT NULL DEFAULT 0,
                builtin      INTEGER NOT NULL DEFAULT 0,-- 是否随程序内置
                installed    INTEGER NOT NULL DEFAULT 1,-- 是否已下载/可用
                ord          INTEGER NOT NULL DEFAULT 0,-- 排序
                created_at   INTEGER NOT NULL DEFAULT 0
            );

            -- 词库 ↔ 单词 的归属关系（多对多：同一个词可属于四级也算考研词）
            CREATE TABLE IF NOT EXISTS wordbook_words (
                book_id  TEXT NOT NULL,
                word     TEXT NOT NULL,
                lang     TEXT NOT NULL DEFAULT 'en',
                ord      INTEGER NOT NULL DEFAULT 0,   -- 词库内顺序
                PRIMARY KEY (book_id, word, lang)
            );

            -- 导入记录：便于「查看已导入的词库」与失败重试
            CREATE TABLE IF NOT EXISTS import_log (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                book_id     TEXT NOT NULL DEFAULT '',
                source      TEXT NOT NULL DEFAULT '',   -- 来源标识 / URL / 文件名
                total       INTEGER NOT NULL DEFAULT 0,
                imported    INTEGER NOT NULL DEFAULT 0,
                skipped     INTEGER NOT NULL DEFAULT 0,
                failed      INTEGER NOT NULL DEFAULT 0,
                message     TEXT NOT NULL DEFAULT '',
                at          INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_book_cat     ON wordbooks(category);
            CREATE INDEX IF NOT EXISTS idx_book_parent  ON wordbooks(parent_id);
            CREATE INDEX IF NOT EXISTS idx_bw_book      ON wordbook_words(book_id);
            CREATE INDEX IF NOT EXISTS idx_bw_word      ON wordbook_words(word, lang);

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

    /// 模糊搜索词库：**同时匹配单词与释义**。
    ///
    /// 为什么必须搜释义：单词本身的拼写是英文/日文，而用户脑子里记的是中文。
    /// 老实现只 `lower(word) LIKE`，于是搜「苹果」一条都出不来——这正是
    /// 「单词列表的搜索只能搜索英文，搜索中文搜索不了」的原因。
    ///
    /// 释义存在 `entry_json` 里，用 `json_each(entry_json, '$.senses')` 展开
    /// 义项逐个比对 `definition`。**不直接对 `entry_json` 做 LIKE**：
    /// JSON 里满是 `"word"` / `"definition"` 这类键名，搜「word」会把
    /// 整库都匹配上。
    ///
    /// 兜底：极端情况下（手改过的库、被截断的 JSON）`json_each` 会报错，
    /// 这时退回「只搜单词」的老查询，宁可少搜到也不能整个搜索报错。
    ///
    /// `lang` 传 `None` 表示**跨语言搜索**。单词列表顶部的搜索框就是这个语义：
    /// 用户的库里可能同时有英语、日语词条，搜「开心」时期望把所有语言里
    /// 释义含「开心」的都找出来；若死板地只搜「当前在学的语言」，日语词的
    /// 中文释义就永远搜不到（也就是「其他语言也搜不了」）。
    pub fn search_words(&self, lang: Option<&str>, q: &str, limit: i64) -> Result<Vec<WordRow>> {
        let conn = self.conn.lock();
        // 转义 LIKE 的通配符，避免用户输入的 % 或 _ 被当作模式匹配
        let escaped = q
            .to_lowercase()
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("%{}%", escaped);

        const SELECT_COLS: &str = "SELECT word, lang, entry_json, added_at FROM words";
        // 跨语言搜索时不要 `lang` 条件；指定语言时限定。用命名参数，
        // 免得「有没有 lang」导致后续占位符编号整体错位。
        let lang_clause = if lang.is_some() { "lang = :l AND" } else { "" };

        // 排序：单词本身命中的排前面（用户更多是在找词），再按长度/字母序
        let rich = format!(
            "{SELECT_COLS}
             WHERE {lang_clause} (
               lower(word) LIKE :p ESCAPE '\\'
               OR EXISTS (
                 SELECT 1 FROM json_each(entry_json, '$.senses') j
                 WHERE lower(COALESCE(json_extract(j.value, '$.definition'), '')) LIKE :p ESCAPE '\\'
               )
             )
             ORDER BY (lower(word) LIKE :p ESCAPE '\\') DESC, length(word), word
             LIMIT :n"
        );
        let simple = format!(
            "{SELECT_COLS}
             WHERE {lang_clause} lower(word) LIKE :p ESCAPE '\\'
             ORDER BY length(word), word LIMIT :n"
        );

        fn row_tuple(r: &rusqlite::Row) -> rusqlite::Result<(String, String, String, i64)> {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
            ))
        }

        fn collect(
            conn: &rusqlite::Connection,
            sql: &str,
            lang: Option<&str>,
            pattern: &str,
            limit: i64,
        ) -> Result<Vec<WordRow>> {
            let mut stmt = conn.prepare(sql)?;
            let rows = match lang {
                Some(l) => stmt.query_map(
                    rusqlite::named_params! { ":l": l, ":p": pattern, ":n": limit },
                    row_tuple,
                )?,
                None => stmt.query_map(
                    rusqlite::named_params! { ":p": pattern, ":n": limit },
                    row_tuple,
                )?,
            };
            let mut out = Vec::new();
            for row in rows {
                let (word, lang, json, added_at) = row?;
                let entry: WordEntry =
                    serde_json::from_str(&json).unwrap_or_else(|_| WordEntry::new(&word));
                out.push(WordRow { word, lang, entry, added_at });
            }
            Ok(out)
        }

        match collect(&conn, &rich, lang, &pattern, limit) {
            Ok(v) => Ok(v),
            Err(_) => collect(&conn, &simple, lang, &pattern, limit),
        }
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
            Some(j) => match serde_json::from_str::<AppConfig>(&j) {
                Ok(cfg) => Ok(cfg),
                Err(e) => {
                    // 配置解析失败时不要静默吞掉：先留一份备份再回退默认值，
                    // 否则用户只会看到「设置全没了」而完全不知道原因。
                    let backup = self.config_backup_path();
                    if let Err(be) = std::fs::write(&backup, &j) {
                        log::warn!("配置解析失败且备份失败（{}）：{}", be, e);
                    } else {
                        log::warn!(
                            "配置解析失败（{}），已备份到 {}，本次使用默认配置",
                            e,
                            backup.display()
                        );
                    }
                    Ok(AppConfig::default())
                }
            },
            None => Ok(AppConfig::default()),
        }
    }

    /// 配置备份文件路径（仅在配置损坏时写入）。
    fn config_backup_path(&self) -> std::path::PathBuf {
        self.path
            .parent()
            .map(|p| p.join("config.broken.json"))
            .unwrap_or_else(|| std::path::PathBuf::from("config.broken.json"))
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

    // ============================================================
    //  词库分级（需求 5 / 20）
    // ============================================================

    /// 插入或更新一个词库。
    pub fn upsert_wordbook(&self, b: &Wordbook) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            r#"INSERT INTO wordbooks
               (id,name,category,level,parent_id,lang,description,source_url,license,
                word_count,builtin,installed,ord,created_at)
               VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
               ON CONFLICT(id) DO UPDATE SET
                 name=excluded.name, category=excluded.category, level=excluded.level,
                 parent_id=excluded.parent_id, lang=excluded.lang,
                 description=excluded.description, source_url=excluded.source_url,
                 license=excluded.license, word_count=excluded.word_count,
                 builtin=excluded.builtin, installed=excluded.installed,
                 ord=excluded.ord"#,
            rusqlite::params![
                b.id, b.name, b.category, b.level, b.parent_id, b.lang,
                b.description, b.source_url, b.license, b.word_count,
                b.builtin as i32, b.installed as i32, b.ord,
                if b.created_at == 0 { crate::timeutil::now_ts() } else { b.created_at },
            ],
        )?;
        Ok(())
    }

    /// 列出全部词库（按 层级 → 排序 排列）。
    pub fn list_wordbooks(&self, lang: Option<&str>) -> Result<Vec<Wordbook>> {
        let conn = self.conn.lock();
        let mut out = Vec::new();
        match lang {
            Some(l) => {
                let mut stmt = conn.prepare(
                    r#"SELECT id,name,category,level,parent_id,lang,description,source_url,
                              license,word_count,builtin,installed,ord,created_at
                       FROM wordbooks WHERE lang=?1 ORDER BY level, ord, name"#,
                )?;
                let rows = stmt.query_map([l], row_to_wordbook)?;
                for r in rows { out.push(r?); }
            }
            None => {
                let mut stmt = conn.prepare(
                    r#"SELECT id,name,category,level,parent_id,lang,description,source_url,
                              license,word_count,builtin,installed,ord,created_at
                       FROM wordbooks ORDER BY level, ord, name"#,
                )?;
                let rows = stmt.query_map([], row_to_wordbook)?;
                for r in rows { out.push(r?); }
            }
        }
        Ok(out)
    }

    /// 取单个词库。
    pub fn get_wordbook(&self, id: &str) -> Result<Option<Wordbook>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"SELECT id,name,category,level,parent_id,lang,description,source_url,
                      license,word_count,builtin,installed,ord,created_at
               FROM wordbooks WHERE id=?1"#,
        )?;
        let mut rows = stmt.query_map([id], row_to_wordbook)?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// 删除词库（连同归属关系；单词本体保留，可能被别的词库引用）。
    pub fn delete_wordbook(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM wordbook_words WHERE book_id=?1", [id])?;
        conn.execute("DELETE FROM wordbooks WHERE id=?1", [id])?;
        Ok(())
    }

    /// 把一批单词挂到某个词库下（幂等：重复的词跳过）。
    /// 返回 (新增数, 跳过数)。
    pub fn add_words_to_book(
        &self,
        book_id: &str,
        words: &[String],
        lang: &str,
    ) -> Result<(i64, i64)> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut added = 0i64;
        let mut skipped = 0i64;
        {
            let mut ins = tx.prepare(
                r#"INSERT OR IGNORE INTO wordbook_words (book_id,word,lang,ord)
                   VALUES (?1,?2,?3,?4)"#,
            )?;
            for (i, w) in words.iter().enumerate() {
                let n = ins.execute(rusqlite::params![book_id, w, lang, i as i64])?;
                if n > 0 { added += 1 } else { skipped += 1 }
            }
        }
        // 回填词数
        let cnt: i64 = tx.query_row(
            "SELECT COUNT(*) FROM wordbook_words WHERE book_id=?1",
            [book_id],
            |r| r.get(0),
        )?;
        tx.execute(
            "UPDATE wordbooks SET word_count=?2, installed=1 WHERE id=?1",
            rusqlite::params![book_id, cnt],
        )?;
        tx.commit()?;
        Ok((added, skipped))
    }

    /// 某词库中的词条（分页）。
    pub fn words_in_book(
        &self,
        book_id: &str,
        lang: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<WordRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"SELECT w.word, w.lang, w.entry_json, w.added_at
               FROM wordbook_words bw
               JOIN words w ON w.word = bw.word AND w.lang = bw.lang
               WHERE bw.book_id = ?1 AND bw.lang = ?2
               ORDER BY bw.ord
               LIMIT ?3 OFFSET ?4"#,
        )?;
        let rows = stmt.query_map(rusqlite::params![book_id, lang, limit, offset], |r| {
            let word: String = r.get(0)?;
            let lang: String = r.get(1)?;
            let json: String = r.get(2)?;
            let added_at: i64 = r.get(3)?;
            Ok((word, lang, json, added_at))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (word, lang, json, added_at) = row?;
            let entry: WordEntry =
                serde_json::from_str(&json).unwrap_or_else(|_| WordEntry::new(&word));
            out.push(WordRow { word, lang, entry, added_at });
        }
        Ok(out)
    }

    /// 某词库中待学习的词（尚未建立学习状态），用于开始新一轮背诵。
    pub fn unscheduled_words_in_book(
        &self,
        book_id: &str,
        lang: &str,
        limit: i64,
    ) -> Result<Vec<WordRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"SELECT w.word, w.lang, w.entry_json, w.added_at
               FROM wordbook_words bw
               JOIN words w ON w.word = bw.word AND w.lang = bw.lang
               LEFT JOIN study_state s ON s.word = bw.word AND s.lang = bw.lang
               WHERE bw.book_id = ?1 AND bw.lang = ?2 AND s.word IS NULL
               ORDER BY bw.ord
               LIMIT ?3"#,
        )?;
        let rows = stmt.query_map(rusqlite::params![book_id, lang, limit], |r| {
            let word: String = r.get(0)?;
            let lang: String = r.get(1)?;
            let json: String = r.get(2)?;
            let added_at: i64 = r.get(3)?;
            Ok((word, lang, json, added_at))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (word, lang, json, added_at) = row?;
            let entry: WordEntry =
                serde_json::from_str(&json).unwrap_or_else(|_| WordEntry::new(&word));
            out.push(WordRow { word, lang, entry, added_at });
        }
        Ok(out)
    }

    /// 词库学习进度统计。
    pub fn book_progress(&self, book_id: &str, lang: &str, now: i64) -> Result<(i64, i64, i64)> {
        let conn = self.conn.lock();
        let total: i64 = conn.query_row(
            "SELECT COUNT(*) FROM wordbook_words WHERE book_id=?1 AND lang=?2",
            rusqlite::params![book_id, lang],
            |r| r.get(0),
        )?;
        let learned: i64 = conn.query_row(
            r#"SELECT COUNT(*) FROM wordbook_words bw
               JOIN study_state s ON s.word=bw.word AND s.lang=bw.lang
               WHERE bw.book_id=?1 AND bw.lang=?2"#,
            rusqlite::params![book_id, lang],
            |r| r.get(0),
        )?;
        let due: i64 = conn.query_row(
            r#"SELECT COUNT(*) FROM wordbook_words bw
               JOIN study_state s ON s.word=bw.word AND s.lang=bw.lang
               WHERE bw.book_id=?1 AND bw.lang=?2 AND s.due_at<=?3 AND s.is_mastered=0"#,
            rusqlite::params![book_id, lang, now],
            |r| r.get(0),
        )?;
        // 用 total 占位（mastered 由上层再查，避免多重 join 复杂化）
        Ok((total, learned, due))
    }

    /// 已学过的词（按最近复习时间倒序），用于「查看已背过的单词」（需求 4）。
    pub fn reviewed_words(
        &self,
        lang: &str,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<WordRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"SELECT w.word, w.lang, w.entry_json, w.added_at
               FROM study_state s
               JOIN words w ON w.word = s.word AND w.lang = s.lang
               WHERE s.lang = ?1 AND s.repetitions > 0
               ORDER BY s.last_review_at DESC
               LIMIT ?2 OFFSET ?3"#,
        )?;
        let rows = stmt.query_map(rusqlite::params![lang, limit, offset], |r| {
            let word: String = r.get(0)?;
            let lang: String = r.get(1)?;
            let json: String = r.get(2)?;
            let added_at: i64 = r.get(3)?;
            Ok((word, lang, json, added_at))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (word, lang, json, added_at) = row?;
            let entry: WordEntry =
                serde_json::from_str(&json).unwrap_or_else(|_| WordEntry::new(&word));
            out.push(WordRow { word, lang, entry, added_at });
        }
        Ok(out)
    }

    /// 记录一次导入。
    pub fn log_import(&self, r: &crate::models::ImportResult, source: &str, now: i64) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            r#"INSERT INTO import_log (book_id,source,total,imported,skipped,failed,message,at)
               VALUES (?1,?2,?3,?4,?5,?6,?7,?8)"#,
            rusqlite::params![
                r.book_id, source, r.total, r.imported, r.skipped, r.failed, r.message, now
            ],
        )?;
        Ok(())
    }
}

/// rusqlite row → StudyState
fn row_to_state(r: &rusqlite::Row) -> rusqlite::Result<StudyState> {    Ok(StudyState {
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

/// rusqlite row → Wordbook
fn row_to_wordbook(r: &rusqlite::Row) -> rusqlite::Result<Wordbook> {
    Ok(Wordbook {
        id: r.get(0)?,
        name: r.get(1)?,
        category: r.get(2)?,
        level: r.get(3)?,
        parent_id: r.get(4)?,
        lang: r.get(5)?,
        description: r.get(6)?,
        source_url: r.get(7)?,
        license: r.get(8)?,
        word_count: r.get(9)?,
        builtin: r.get::<_, i32>(10)? != 0,
        installed: r.get::<_, i32>(11)? != 0,
        ord: r.get(12)?,
        created_at: r.get(13)?,
    })
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Sense, WordEntry};

    fn tmp_db(tag: &str) -> Db {
        let p = std::env::temp_dir().join(format!("wordwise-test-{}-{}.db", tag, std::process::id()));
        let _ = std::fs::remove_file(&p);
        Db::open(&p).expect("打开测试库")
    }

    fn entry(lang: &str, word: &str, pos: &str, def: &str) -> WordEntry {
        let mut e = WordEntry::new(word);
        e.lang = lang.to_string();
        e.senses = vec![Sense {
            pos: pos.to_string(),
            definition: def.to_string(),
            examples: vec![],
        }];
        e
    }

    /// 搜索必须同时命中「单词」和「中文释义」。
    ///
    /// 这是「单词列表的搜索只能搜索英文，搜索中文搜索不了」的回归测试：
    /// 老实现只对 word 列做 LIKE，搜「苹果」一条都出不来。
    #[test]
    fn search_words_matches_chinese_gloss() {
        let db = tmp_db("search");
        let rows = vec![
            entry("en", "apple", "n.", "苹果；苹果树"),
            entry("en", "banana", "n.", "香蕉"),
            entry("en", "abandon", "v.", "放弃；抛弃；遗弃"),
        ];
        db.bulk_upsert_words(&rows, 1).expect("写入词库");

        // 英文单词照旧能搜到
        let by_word = db.search_words(Some("en"), "app", 50).unwrap();
        assert_eq!(by_word.len(), 1);
        assert_eq!(by_word[0].word, "apple");

        // 中文释义也要能搜到
        let by_gloss = db.search_words(Some("en"), "苹果", 50).unwrap();
        assert_eq!(by_gloss.len(), 1, "按中文释义搜索应当命中 apple");
        assert_eq!(by_gloss[0].word, "apple");

        // 多个词共享同一段释义文字时全部返回
        let many = db.search_words(Some("en"), "香蕉", 50).unwrap();
        assert_eq!(many.len(), 1);
        assert_eq!(many[0].word, "banana");

        // 子串匹配（释义中间）
        let mid = db.search_words(Some("en"), "遗弃", 50).unwrap();
        assert_eq!(mid.len(), 1);
        assert_eq!(mid[0].word, "abandon");

        // 大小写不敏感
        assert_eq!(db.search_words(Some("en"), "APPLE", 50).unwrap().len(), 1);
        // 无匹配
        assert!(db.search_words(Some("en"), "不存在的东西", 50).unwrap().is_empty());

        // 语言维度依然生效：日语库里不该出现英语词
        assert!(db.search_words(Some("ja"), "苹果", 50).unwrap().is_empty());
    }

    /// 不指定语言时要**跨语言**搜索。
    ///
    /// 这是「其他语言也搜不了」的回归测试：单词列表顶部搜索框不传 lang，
    /// 若按当前学习语言过滤，日语词条的中文释义就永远搜不出来。
    #[test]
    fn search_words_without_lang_spans_languages() {
        let db = tmp_db("search-all");
        let rows = vec![
            entry("en", "apple", "n.", "苹果"),
            entry("ja", "りんご", "n.", "苹果；林檎"),
        ];
        db.bulk_upsert_words(&rows, 1).expect("写入词库");

        let all = db.search_words(None, "苹果", 50).unwrap();
        assert_eq!(all.len(), 2, "跨语言搜索应同时命中英语与日语词条：{:?}",
            all.iter().map(|r| format!("{}:{}", r.lang, r.word)).collect::<Vec<_>>());
        assert!(all.iter().any(|r| r.lang == "en" && r.word == "apple"));
        assert!(all.iter().any(|r| r.lang == "ja" && r.word == "りんご"));

        // 指定语言时仍然只在该语言内搜
        let only_ja = db.search_words(Some("ja"), "苹果", 50).unwrap();
        assert_eq!(only_ja.len(), 1);
        assert_eq!(only_ja[0].lang, "ja");
    }

    /// JSON 键名不应该被当成释义命中（防止「搜 word 命中全库」）。
    #[test]
    fn search_words_ignores_json_keys() {
        let db = tmp_db("keys");
        db.bulk_upsert_words(&[entry("en", "apple", "n.", "苹果")], 1)
            .unwrap();
        assert!(
            db.search_words(Some("en"), "definition", 50).unwrap().is_empty(),
            "不应因 entry_json 里的键名而产生误匹配"
        );
        assert!(
            db.search_words(Some("en"), "senses", 50).unwrap().is_empty(),
            "不应因 entry_json 里的键名而产生误匹配"
        );
    }

    /// 即使库里存在一条坏 JSON，搜索也不能整体报错（要退回只搜单词）。
    #[test]
    fn search_words_survives_broken_json() {
        let db = tmp_db("broken");
        db.bulk_upsert_words(&[entry("en", "apple", "n.", "苹果")], 1)
            .unwrap();
        {
            let conn = db.conn.lock();
            conn.execute(
                "INSERT INTO words(word, lang, entry_json, added_at) VALUES('bad','en','{not json',1)",
                [],
            )
            .unwrap();
        }
        // 不能 panic、不能返回 Err
        let got = db.search_words(Some("en"), "app", 50).unwrap();
        assert!(got.iter().any(|r| r.word == "apple"));
    }
}
