//! SQLite 存储层：词库、学习状态、错词本、复习日志、配置。
//!
//! 所有数据落在用户 AppData 目录下的 `wordwise.db`，随安装包分发、卸载可选保留。

use crate::models::{AppConfig, DictSourceConfig, StudyState, TransRecord, WordEntry, Wordbook};
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

/// 一条 AI 讲解存档。
///
/// 讲解是花算力换来的，必须留下来：再点开同一个词应当秒回本地内容，
/// 而且它本身也是一份可以搜索的词条资料。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExplainRow {
    pub word: String,
    pub lang: String,
    /// 讲解所用语言（zh / en / ja…），同一词可有多种语言的存档
    pub explain_lang: String,
    /// 展示文本（已按讲解语言处理过）
    pub text: String,
    /// 模型原文，用于「查看原文」
    pub original: String,
    pub translated: bool,
    /// 结构化词条（「保存到词库」时生成），空表示还没生成
    #[serde(default)]
    pub entry_json: String,
    /// 是否已并入 `words` 表
    pub saved: bool,
    pub updated_at: i64,
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

            -- ============ 翻译（需求 5：独立翻译栏目） ============
            -- 翻译历史：翻译页左下的历史列表与收藏夹都读这张表。
            -- 同一对「源文+方向」只保留一行（重复翻译只更新时间），
            -- 否则实时翻译每敲一个字就留一条记录，列表会被瞬间冲爆。
            CREATE TABLE IF NOT EXISTS trans_history (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                source_lang TEXT NOT NULL DEFAULT '',
                target_lang TEXT NOT NULL DEFAULT '',
                src_text    TEXT NOT NULL,
                dst_text    TEXT NOT NULL,
                engine      TEXT NOT NULL DEFAULT '',
                favorite    INTEGER NOT NULL DEFAULT 0,
                created_at  INTEGER NOT NULL,
                UNIQUE(src_text, source_lang, target_lang)
            );

            -- 翻译缓存：在线接口限频很严，缓存是刚需（实时翻译会反复
            -- 请求同一句话的前缀）。key = 源语言|目标语言|原文。
            CREATE TABLE IF NOT EXISTS trans_cache (
                key         TEXT PRIMARY KEY,
                result_json TEXT NOT NULL,
                cached_at   INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_trans_at  ON trans_history(created_at DESC);
            CREATE INDEX IF NOT EXISTS idx_trans_fav ON trans_history(favorite);

            -- ============ 知识图谱 ============
            -- 词与词的关系网络。
            --
            -- 为什么不像 study_state 那样现算，而要落一张表：
            --   · 本地关系（词典里写着的）可以从词条现算，但 AI 发散的边
            --     是**花算力换来的**，不能每次打开页面都重新问一遍模型；
            --   · 图查询需要按 src/dst 双向检索，现算要扫全表反序列化 JSON。
            --
            -- src/dst 都是「词」而不是外键 id：词库允许存在只有关系、
            -- 没有完整词条的节点（AI 发散出来的新词），用词本身做主键才不会
            -- 因为缺词条而丢掉边。
            CREATE TABLE IF NOT EXISTS word_edges (
                src        TEXT NOT NULL,
                dst        TEXT NOT NULL,
                rel        TEXT NOT NULL,
                lang       TEXT NOT NULL DEFAULT 'en',
                weight     REAL NOT NULL DEFAULT 1.0,
                source     TEXT NOT NULL DEFAULT 'local',
                created_at INTEGER NOT NULL,
                PRIMARY KEY (src, dst, rel, lang)
            );

            CREATE INDEX IF NOT EXISTS idx_edge_src ON word_edges(lang, src);
            CREATE INDEX IF NOT EXISTS idx_edge_dst ON word_edges(lang, dst);

            -- ============ AI 讲解存档 ============
            -- 为什么要落库里，而不是每次重新问模型：
            --   · 讲解是**花算力换来的**，同一个词第二次点开不该再等一遍模型；
            --   · 讲解内容本身就是一份「词条资料」，应当能在词库页被搜到
            --     —— 这正是「词库本地化」的一环：先本地命中，再考虑联网。
            --
            -- 主键带上 explain_lang：同一个人可能今天看中文讲解、明天看英文讲解，
            -- 两份都要留。entry_json 是「保存到词库」时生成的结构化词条；
            -- saved 标记它有没有并进 words 表。
            CREATE TABLE IF NOT EXISTS explain_store (
                word         TEXT NOT NULL,
                lang         TEXT NOT NULL DEFAULT 'en',
                explain_lang TEXT NOT NULL DEFAULT 'zh',
                text         TEXT NOT NULL,              -- 展示文本（已按讲解语言处理）
                original     TEXT NOT NULL DEFAULT '',   -- 模型原文（「查看原文」要用）
                translated   INTEGER NOT NULL DEFAULT 0,
                entry_json   TEXT NOT NULL DEFAULT '',   -- 结构化词条（保存到词库用）
                saved        INTEGER NOT NULL DEFAULT 0, -- 是否已并入 words
                created_at   INTEGER NOT NULL,
                updated_at   INTEGER NOT NULL,
                PRIMARY KEY (word, lang, explain_lang)
            );

            CREATE INDEX IF NOT EXISTS idx_explain_word ON explain_store(lang, word);
            CREATE INDEX IF NOT EXISTS idx_explain_saved ON explain_store(saved);
            "#,
        )?;
        Ok(())
    }

    // ---------- 知识图谱（需求：独立的知识图谱栏目） ----------

    /// 批量写入关系边（幂等）。返回真正新插入的条数。
    ///
    /// 用 `ON CONFLICT DO NOTHING` 而不是 UPDATE：边的 weight/source 由
    /// 「谁先抽到」决定即可，重复入库不该把 AI 边覆盖成本地边，反之亦然。
    pub fn upsert_edges(&self, edges: &[crate::graph::GraphEdge], lang: &str, now: i64) -> Result<usize> {
        let conn = self.conn.lock();
        let tx = conn.unchecked_transaction()?;
        let mut n = 0usize;
        {
            let mut stmt = tx.prepare(
                r#"INSERT INTO word_edges(src,dst,rel,lang,weight,source,created_at)
                   VALUES(?1,?2,?3,?4,?5,?6,?7)
                   ON CONFLICT(src,dst,rel,lang) DO NOTHING"#,
            )?;
            for e in edges {
                let changed = stmt.execute(params![
                    e.src, e.dst, e.rel, lang, e.weight, e.source, now
                ])?;
                n += changed;
            }
        }
        tx.commit()?;
        Ok(n)
    }

    /// 取某词的**出边**（以它为中心的邻居）。
    pub fn edges_from(&self, word: &str, lang: &str) -> Result<Vec<crate::graph::GraphEdge>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"SELECT src,dst,rel,weight,source FROM word_edges
               WHERE lang=?1 AND src=?2 ORDER BY rel, dst"#,
        )?;
        let rows = stmt.query_map(params![lang, word], |r| {
            Ok(crate::graph::GraphEdge {
                src: r.get(0)?,
                dst: r.get(1)?,
                rel: r.get(2)?,
                weight: r.get(3)?,
                source: r.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 取某词的**入边**。
    ///
    /// 「派生」是有方向的（原形 → 变形），只看出边会漏掉「变形 → 原形」
    /// 这条回程，用户点进变形词就看不到它的原形了。
    pub fn edges_into(&self, word: &str, lang: &str) -> Result<Vec<crate::graph::GraphEdge>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"SELECT src,dst,rel,weight,source FROM word_edges
               WHERE lang=?1 AND dst=?2 ORDER BY rel, src"#,
        )?;
        let rows = stmt.query_map(params![lang, word], |r| {
            Ok(crate::graph::GraphEdge {
                src: r.get(0)?,
                dst: r.get(1)?,
                rel: r.get(2)?,
                weight: r.get(3)?,
                source: r.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 全量边（限制条数，防止图太大卡住渲染）。
    pub fn all_edges(&self, lang: &str, limit: i64) -> Result<Vec<crate::graph::GraphEdge>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"SELECT src,dst,rel,weight,source FROM word_edges
               WHERE lang=?1 ORDER BY weight DESC, src LIMIT ?2"#,
        )?;
        let rows = stmt.query_map(params![lang, limit], |r| {
            Ok(crate::graph::GraphEdge {
                src: r.get(0)?,
                dst: r.get(1)?,
                rel: r.get(2)?,
                weight: r.get(3)?,
                source: r.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 边的总数与去重后的节点数。
    pub fn graph_counts(&self, lang: &str) -> Result<(usize, usize)> {
        let conn = self.conn.lock();
        let edges: i64 =
            conn.query_row("SELECT COUNT(*) FROM word_edges WHERE lang=?1", params![lang], |r| r.get(0))?;
        let nodes: i64 = conn.query_row(
            r#"SELECT COUNT(*) FROM (
                   SELECT src AS w FROM word_edges WHERE lang=?1
                   UNION
                   SELECT dst AS w FROM word_edges WHERE lang=?1
               )"#,
            params![lang],
            |r| r.get(0),
        )?;
        Ok((nodes as usize, edges as usize))
    }

    /// 度数最高的词（图谱默认视图要从最「核心」的词开始画）。
    pub fn top_degree_words(&self, lang: &str, limit: i64) -> Result<Vec<(String, i64)>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"SELECT w, COUNT(*) AS deg FROM (
                   SELECT src AS w FROM word_edges WHERE lang=?1
                   UNION ALL
                   SELECT dst AS w FROM word_edges WHERE lang=?1
               ) GROUP BY w ORDER BY deg DESC, w ASC LIMIT ?2"#,
        )?;
        let rows = stmt.query_map(params![lang, limit], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 按关键词搜词（图谱页搜索框用）。
    pub fn graph_search(&self, lang: &str, q: &str, limit: i64) -> Result<Vec<String>> {
        let conn = self.conn.lock();
        let like = format!("%{}%", q.replace('%', "\\%").replace('_', "\\_"));
        let mut stmt = conn.prepare(
            r#"SELECT DISTINCT w FROM (
                   SELECT src AS w FROM word_edges WHERE lang=:l
                   UNION
                   SELECT dst AS w FROM word_edges WHERE lang=:l
               ) WHERE w LIKE :q ESCAPE '\' ORDER BY LENGTH(w), w LIMIT :n"#,
        )?;
        let rows = stmt.query_map(
            rusqlite::named_params! { ":l": lang, ":q": like, ":n": limit },
            |r| r.get::<_, String>(0),
        )?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 清空某语言的图谱。
    pub fn clear_edges(&self, lang: &str) -> Result<usize> {
        let conn = self.conn.lock();
        Ok(conn.execute("DELETE FROM word_edges WHERE lang=?1", params![lang])?)
    }

    /// 某语言下已有的全部节点名（AI 发散时用来排除已记录的词）。
    pub fn edge_neighbors(&self, lang: &str, word: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            r#"SELECT DISTINCT w FROM (
                   SELECT dst AS w FROM word_edges WHERE lang=?1 AND src=?2
                   UNION
                   SELECT src AS w FROM word_edges WHERE lang=?1 AND dst=?2
               )"#,
        )?;
        let rows = stmt.query_map(params![lang, word], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
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

    // ---------- AI 讲解存档 ----------

    /// 写入（或覆盖）一条讲解存档。
    ///
    /// 已有记录时：`entry_json` 只在本次给了值时才覆盖（避免「重新生成讲解」
    /// 把之前生成好的结构化词条冲掉），`saved` 只增不减。
    pub fn save_explain(&self, r: &ExplainRow, now: i64) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            r#"INSERT INTO explain_store
                 (word, lang, explain_lang, text, original, translated, entry_json, saved, created_at, updated_at)
               VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?9)
               ON CONFLICT(word, lang, explain_lang) DO UPDATE SET
                 text       = excluded.text,
                 original   = excluded.original,
                 translated = excluded.translated,
                 entry_json = CASE WHEN excluded.entry_json <> '' THEN excluded.entry_json
                                   ELSE explain_store.entry_json END,
                 saved      = MAX(explain_store.saved, excluded.saved),
                 updated_at = excluded.updated_at"#,
            params![
                r.word,
                r.lang,
                r.explain_lang,
                r.text,
                r.original,
                r.translated as i64,
                r.entry_json,
                r.saved as i64,
                now
            ],
        )?;
        Ok(())
    }

    /// 取一条讲解存档。
    pub fn get_explain(
        &self,
        word: &str,
        lang: &str,
        explain_lang: &str,
    ) -> Result<Option<ExplainRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT word, lang, explain_lang, text, original, translated, entry_json, saved, updated_at
             FROM explain_store WHERE word=?1 AND lang=?2 AND explain_lang=?3",
        )?;
        let mut rows = stmt.query_map(params![word, lang, explain_lang], map_explain_row)?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    /// 某词的所有讲解存档（不限讲解语言），最近的在前。
    pub fn explains_of(&self, word: &str, lang: &str) -> Result<Vec<ExplainRow>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT word, lang, explain_lang, text, original, translated, entry_json, saved, updated_at
             FROM explain_store WHERE word=?1 AND lang=?2 ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map(params![word, lang], map_explain_row)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 搜索讲解存档：同时匹配**词**与**讲解正文**。
    ///
    /// 词库页的「单词列表」搜索会把它并进来，这样「在词库中搜索」
    /// 就能搜到 AI 讲解过、但还没导入成词条的内容。
    pub fn search_explains(
        &self,
        lang: Option<&str>,
        q: &str,
        limit: i64,
    ) -> Result<Vec<ExplainRow>> {
        let conn = self.conn.lock();
        let escaped = escape_like(&q.to_lowercase());
        let pattern = format!("%{}%", escaped);
        let lang_clause = if lang.is_some() { "lang = :l AND" } else { "" };
        let sql = format!(
            "SELECT word, lang, explain_lang, text, original, translated, entry_json, saved, updated_at
             FROM explain_store
             WHERE {lang_clause} (lower(word) LIKE :p ESCAPE '\\' OR lower(text) LIKE :p ESCAPE '\\')
             ORDER BY (lower(word) LIKE :p ESCAPE '\\') DESC, updated_at DESC
             LIMIT :n"
        );
        let mut stmt = conn.prepare(&sql)?;
        // 先把 lang 变成自有 String 再借用：`named` 里的引用必须活到
        // `query_map` 结束，而 `lang: Option<&str>` 的临时值撑不了那么久（E0597）。
        let lang_owned: Option<String> = lang.map(|s| s.to_string());
        let mut named: Vec<(&str, &dyn rusqlite::ToSql)> =
            vec![(":p", &pattern), (":n", &limit)];
        if let Some(l) = &lang_owned {
            named.push((":l", l));
        }
        let rows = stmt.query_map(named.as_slice(), map_explain_row)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 讲解存档总数（数据库面板展示用）。
    pub fn explain_count(&self) -> Result<i64> {
        let conn = self.conn.lock();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM explain_store", [], |r| r.get(0))?;
        Ok(n)
    }

    /// 标记某条讲解已并入词库，并记下生成的词条。
    pub fn set_explain_saved(
        &self,
        word: &str,
        lang: &str,
        explain_lang: &str,
        entry_json: &str,
        now: i64,
    ) -> Result<usize> {
        let conn = self.conn.lock();
        let n = conn.execute(
            "UPDATE explain_store SET saved=1, entry_json=?4, updated_at=?5
             WHERE word=?1 AND lang=?2 AND explain_lang=?3",
            params![word, lang, explain_lang, entry_json, now],
        )?;
        Ok(n)
    }

    /// 删掉一条讲解存档。
    pub fn delete_explain(&self, word: &str, lang: &str, explain_lang: &str) -> Result<usize> {
        let conn = self.conn.lock();
        let n = conn.execute(
            "DELETE FROM explain_store WHERE word=?1 AND lang=?2 AND explain_lang=?3",
            params![word, lang, explain_lang],
        )?;
        Ok(n)
    }

    /// 清空讲解存档。`keep_saved=true` 时保留已经并入词库的那些。
    pub fn clear_explains(&self, keep_saved: bool) -> Result<usize> {
        let conn = self.conn.lock();
        let n = if keep_saved {
            conn.execute("DELETE FROM explain_store WHERE saved=0", [])?
        } else {
            conn.execute("DELETE FROM explain_store", [])?
        };
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
    ///
    /// 保留旧签名：内部转调 [`Db::leech_states_filtered`] 的「无筛选」配置，
    /// 这样老调用方（侧边栏角标等）不用改。
    pub fn leech_states(&self, lang: &str, limit: i64) -> Result<Vec<StudyState>> {
        self.leech_states_filtered(lang, 0, 100, 0.0, "wrong", limit)
    }

    /// 带筛选的常错词查询（错题本增强）。
    ///
    /// 四个条件都是「可选」语义：`min_wrong=0` 表示不限错误次数，
    /// `max_mastery=100` 表示不限掌握度，`min_error_rate=0.0` 表示不限错误率。
    ///
    /// 排序字段走白名单映射而不是字符串拼接 —— 这是拼接进 SQL 的片段，
    /// 直接透传用户参数就是注入。
    pub fn leech_states_filtered(
        &self,
        lang: &str,
        min_wrong: i64,
        max_mastery: i64,
        min_error_rate: f64,
        order: &str,
        limit: i64,
    ) -> Result<Vec<StudyState>> {
        let conn = self.conn.lock();
        let order_by = match order {
            // 错得最多的排前面
            "wrong" => "wrong_count DESC, mastery ASC",
            // 最不熟的排前面
            "mastery" => "mastery ASC, wrong_count DESC",
            // 最近还在错的排前面
            "recent" => "last_review_at DESC, wrong_count DESC",
            // 错误率最高的排前面（用乘法避免除法，同时天然跳过 0 次作答）
            "rate" => "(CAST(wrong_count AS REAL) / MAX(correct_count + wrong_count, 1)) DESC, wrong_count DESC",
            "word" => "word ASC",
            _ => "wrong_count DESC, mastery ASC",
        };
        let sql = format!(
            r#"SELECT word,lang,ease_factor,interval_days,repetitions,due_at,last_review_at,
                      correct_count,wrong_count,is_leech,mastery,is_mastered
               FROM study_state
               WHERE lang=?1 AND is_leech=1
                 AND wrong_count >= ?2
                 AND mastery <= ?3
                 AND (CAST(wrong_count AS REAL) / MAX(correct_count + wrong_count, 1)) >= ?4
               ORDER BY {order_by} LIMIT ?5"#
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            params![lang, min_wrong, max_mastery, min_error_rate, limit],
            row_to_state,
        )?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 批量把词移出强化队列。
    pub fn clear_leech_many(&self, words: &[String], lang: &str) -> Result<usize> {
        let conn = self.conn.lock();
        let tx = conn.unchecked_transaction()?;
        let mut n = 0usize;
        {
            let mut stmt =
                tx.prepare("UPDATE study_state SET is_leech=0 WHERE word=?1 AND lang=?2")?;
            for w in words {
                n += stmt.execute(params![w, lang])?;
            }
        }
        tx.commit()?;
        Ok(n)
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

    // ---------- 翻译历史与缓存（需求 5） ----------

    /// 写入一条翻译历史，返回记录 id。
    ///
    /// 同一 `(原文, 源语言, 目标语言)` 只保留一行 —— 翻译页是**实时翻译**，
    /// 每敲一个字都会产生一次结果，若每次追加，历史列表会被瞬间冲爆。
    /// 重复时更新译文与时间即可，收藏状态保持不变。
    pub fn upsert_translation(
        &self,
        source_lang: &str,
        target_lang: &str,
        src_text: &str,
        dst_text: &str,
        engine: &str,
        now: i64,
    ) -> Result<i64> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO trans_history
                (source_lang, target_lang, src_text, dst_text, engine, favorite, created_at)
             VALUES(?1,?2,?3,?4,?5,0,?6)
             ON CONFLICT(src_text, source_lang, target_lang) DO UPDATE SET
                dst_text   = excluded.dst_text,
                engine     = excluded.engine,
                created_at = excluded.created_at",
            params![source_lang, target_lang, src_text, dst_text, engine, now],
        )?;
        let id: i64 = conn.query_row(
            "SELECT id FROM trans_history
              WHERE src_text=?1 AND source_lang=?2 AND target_lang=?3",
            params![src_text, source_lang, target_lang],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    /// 列出翻译历史。`only_favorite` 为真时只返回收藏项。
    pub fn list_translations(&self, limit: i64, only_favorite: bool) -> Result<Vec<TransRecord>> {
        let conn = self.conn.lock();
        let sql = if only_favorite {
            "SELECT id, source_lang, target_lang, src_text, dst_text, engine, favorite, created_at
               FROM trans_history WHERE favorite=1
              ORDER BY created_at DESC LIMIT ?1"
        } else {
            "SELECT id, source_lang, target_lang, src_text, dst_text, engine, favorite, created_at
               FROM trans_history
              ORDER BY favorite DESC, created_at DESC LIMIT ?1"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params![limit], |r| {
            Ok(TransRecord {
                id: r.get(0)?,
                source_lang: r.get(1)?,
                target_lang: r.get(2)?,
                src_text: r.get(3)?,
                dst_text: r.get(4)?,
                engine: r.get(5)?,
                favorite: r.get::<_, i64>(6)? != 0,
                created_at: r.get(7)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 收藏 / 取消收藏。
    pub fn set_translation_favorite(&self, id: i64, on: bool) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE trans_history SET favorite=?1 WHERE id=?2",
            params![if on { 1 } else { 0 }, id],
        )?;
        Ok(())
    }

    pub fn delete_translation(&self, id: i64) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM trans_history WHERE id=?1", params![id])?;
        Ok(())
    }

    /// 清空历史，返回删除条数。`keep_favorite` 为真时保留收藏项。
    pub fn clear_translations(&self, keep_favorite: bool) -> Result<usize> {
        let conn = self.conn.lock();
        let n = if keep_favorite {
            conn.execute("DELETE FROM trans_history WHERE favorite=0", [])?
        } else {
            conn.execute("DELETE FROM trans_history", [])?
        };
        Ok(n)
    }

    /// 读翻译缓存。超过 `max_age` 秒即视为失效。
    pub fn get_trans_cache(&self, key: &str, max_age: i64, now: i64) -> Result<Option<String>> {
        let conn = self.conn.lock();
        let row: Option<(String, i64)> = conn
            .query_row(
                "SELECT result_json, cached_at FROM trans_cache WHERE key=?1",
                params![key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(match row {
            Some((json, at)) if max_age <= 0 || now - at <= max_age => Some(json),
            _ => None,
        })
    }

    pub fn put_trans_cache(&self, key: &str, json: &str, now: i64) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO trans_cache(key, result_json, cached_at) VALUES(?1,?2,?3)
             ON CONFLICT(key) DO UPDATE SET result_json=excluded.result_json,
                                            cached_at=excluded.cached_at",
            params![key, json, now],
        )?;
        Ok(())
    }

    pub fn clear_trans_cache(&self) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute("DELETE FROM trans_cache", [])?;
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
    // ---------- 数据库维护（需求：把「本地小型数据库」变得可见可控） ----------

    /// 各表的行数，按行数从多到少。
    ///
    /// 表名写死在代码里而不是查 `sqlite_master`：维护面板要展示的是
    /// **业务表**，把 `sqlite_sequence` 之类内部表也列出来只会让人困惑。
    pub fn table_counts(&self) -> Result<Vec<(String, i64)>> {
        const TABLES: [&str; 14] = [
            "words",
            "study_state",
            "review_log",
            "config",
            "dict_sources",
            "dict_cache",
            "search_log",
            "wordbooks",
            "wordbook_words",
            "import_log",
            "trans_history",
            "trans_cache",
            "word_edges",
            "explain_store",
        ];
        let conn = self.conn.lock();
        let mut out = Vec::new();
        for t in TABLES {
            let n: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0))
                .unwrap_or(0);
            out.push((t.to_string(), n));
        }
        out.sort_by(|a, b| b.1.cmp(&a.1));
        Ok(out)
    }

    /// 完整性检查。返回 `("ok", 详情)` 或 `("error", 详情)`。
    pub fn integrity_check(&self) -> (String, String) {
        let conn = self.conn.lock();
        let mut stmt = match conn.prepare("PRAGMA integrity_check") {
            Ok(s) => s,
            Err(e) => return ("error".into(), e.to_string()),
        };
        let rows: Vec<String> = match stmt.query_map([], |r| r.get::<_, String>(0)) {
            Ok(rs) => rs.filter_map(|r| r.ok()).collect(),
            Err(e) => return ("error".into(), e.to_string()),
        };
        // integrity_check 正常时只返回一行 "ok"，异常时会把每处问题列成一行
        if rows.len() == 1 && rows[0].trim().eq_ignore_ascii_case("ok") {
            ("ok".into(), "数据库结构完整，没有发现问题".into())
        } else {
            (
                "error".into(),
                format!("发现 {} 处问题：\n{}", rows.len(), rows.join("\n")),
            )
        }
    }

    /// 整理数据库：`VACUUM` 重建文件、回收删除留下的空洞。
    ///
    /// 为什么值得做：`words` 表把整条词条存成 JSON，删词/换词库之后
    /// 文件里会留下大量空洞，体积虚高。VACUUM 之后通常能小一大截。
    /// 代价是它需要临时空间、且会锁库，所以只在用户点按钮时才跑。
    pub fn vacuum(&self) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute_batch("VACUUM;")?;
        Ok(())
    }

    /// 用 `VACUUM INTO` 导出数据库副本（一致性快照）。
    ///
    /// 比直接拷 .db 文件可靠：拷文件可能拿到「写入进行到一半」的状态，
    /// 或者漏掉还在 WAL 里的已提交数据。
    pub fn backup_to(&self, path: &std::path::Path) -> Result<()> {
        let conn = self.conn.lock();
        // VACUUM INTO 的目标文件必须不存在
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        let p = path.to_string_lossy().replace('\'', "''");
        conn.execute_batch(&format!("VACUUM INTO '{p}';"))?;
        Ok(())
    }

    /// 数据库文件与 WAL 的实际磁盘占用（字节）。
    pub fn disk_usage(&self) -> (u64, u64) {
        let p = self.path();
        let main = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        let mut wal = 0u64;
        for suffix in ["-wal", "-shm"] {
            let q = std::path::PathBuf::from(format!("{}{}", p.display(), suffix));
            if let Ok(m) = std::fs::metadata(q) {
                wal += m.len();
            }
        }
        (main, wal)
    }

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


/// 转义 LIKE 的通配符，避免用户输入的 `%` 或 `_` 被当成模式匹配。
fn escape_like(q: &str) -> String {
    q.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// 把 `explain_store` 的一行读成 [`ExplainRow`]。
fn map_explain_row(r: &rusqlite::Row) -> rusqlite::Result<ExplainRow> {
    Ok(ExplainRow {
        word: r.get(0)?,
        lang: r.get(1)?,
        explain_lang: r.get(2)?,
        text: r.get(3)?,
        original: r.get(4)?,
        translated: r.get::<_, i32>(5)? != 0,
        entry_json: r.get(6)?,
        saved: r.get::<_, i32>(7)? != 0,
        updated_at: r.get(8)?,
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

    /* ---- 翻译历史与收藏（需求 5） ---- */

    /// 重复翻译同一句只保留一行。
    ///
    /// 这条守的是「翻译页实时翻译会把历史冲爆」：边打字边翻，
    /// 每次都是新记录的话，历史列表几秒钟就被同一句话的不同前缀淹没了。
    #[test]
    fn repeated_translation_keeps_one_row() {
        let db = tmp_db("trans-dedup");
        let id1 = db
            .upsert_translation("zh", "ja", "你好", "こんにちは", "youdao", 100)
            .unwrap();
        let id2 = db
            .upsert_translation("zh", "ja", "你好", "こんにちは。", "youdao", 200)
            .unwrap();
        assert_eq!(id1, id2, "同一句应命中同一行");

        let list = db.list_translations(50, false).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].dst_text, "こんにちは。", "译文应被更新为最新");
        assert_eq!(list[0].created_at, 200);
    }

    /// 换个方向就是另一条记录（中→日 与 日→中 不能互相覆盖）。
    #[test]
    fn different_direction_is_a_new_row() {
        let db = tmp_db("trans-dir");
        db.upsert_translation("zh", "ja", "你好", "こんにちは", "youdao", 1)
            .unwrap();
        db.upsert_translation("ja", "zh", "你好", "nǐ hǎo", "youdao", 2)
            .unwrap();
        assert_eq!(db.list_translations(50, false).unwrap().len(), 2);
    }

    /// 收藏置位后：收藏列表能查到，且清空历史时能被保留。
    #[test]
    fn favorite_survives_clear() {
        let db = tmp_db("trans-fav");
        let keep = db
            .upsert_translation("zh", "ja", "你好", "こんにちは", "youdao", 1)
            .unwrap();
        db.upsert_translation("zh", "ja", "谢谢", "ありがとう", "youdao", 2)
            .unwrap();
        db.set_translation_favorite(keep, true).unwrap();

        let favs = db.list_translations(50, true).unwrap();
        assert_eq!(favs.len(), 1);
        assert_eq!(favs[0].src_text, "你好");
        assert!(favs[0].favorite);

        // 保留收藏地清空 → 只掉 1 条
        assert_eq!(db.clear_translations(true).unwrap(), 1);
        let rest = db.list_translations(50, false).unwrap();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].src_text, "你好", "收藏项必须留下");

        // 全量清空
        db.clear_translations(false).unwrap();
        assert!(db.list_translations(50, false).unwrap().is_empty());
    }

    /// 删除单条。
    #[test]
    fn delete_single_translation() {
        let db = tmp_db("trans-del");
        let id = db
            .upsert_translation("zh", "en", "你好", "Hello", "youdao", 1)
            .unwrap();
        db.delete_translation(id).unwrap();
        assert!(db.list_translations(50, false).unwrap().is_empty());
    }

    /// 翻译缓存要能读回，并遵守 TTL。
    #[test]
    fn trans_cache_roundtrip_and_ttl() {
        let db = tmp_db("trans-cache");
        let key = "zh|ja|你好";
        assert!(db.get_trans_cache(key, 600, 100).unwrap().is_none());

        db.put_trans_cache(key, r#"{"text":"こんにちは"}"#, 100)
            .unwrap();
        assert!(db.get_trans_cache(key, 600, 200).unwrap().is_some(), "未过期应命中");
        assert!(
            db.get_trans_cache(key, 600, 100 + 601).unwrap().is_none(),
            "超过 TTL 应失效"
        );
        // 覆盖写入
        db.put_trans_cache(key, r#"{"text":"新しい"}"#, 900)
            .unwrap();
        assert_eq!(
            db.get_trans_cache(key, 0, 1000).unwrap().unwrap(),
            r#"{"text":"新しい"}"#
        );
    }

    /* ---- 错词本多维筛选（需求 4） ---- */

    /// 造几条「错得多但答得也多」和「错得少」的混合状态。
    ///
    /// 关键在于**错误率 ≠ 错误次数**：一个词错了 8 次但也对了 8 次（50%），
    /// 另一个词错了 3 次、对了 0 次（100%）。只按次数筛会把后者漏掉，
    /// 而它恰恰是最该优先背的。
    fn seed_leeches(db: &Db) {
        let now = 1_000_000;
        let cases = [
            ("abandon", 8, 8, 20),   // 错误率 50%
            ("reluctant", 3, 0, 30), // 错误率 100%
            ("benefit", 1, 9, 70),   // 错误率 10%
        ];
        for (w, wrong, correct, mastery) in cases {
            let mut s = StudyState::new(w, "en", now);
            s.is_leech = true;
            s.wrong_count = wrong;
            s.correct_count = correct;
            s.mastery = mastery;
            db.upsert_state(&s).unwrap();
        }
    }

    #[test]
    fn leech_filter_by_min_wrong() {
        let db = tmp_db("leech-wrong");
        seed_leeches(&db);
        // 不限 → 3 条
        assert_eq!(db.leech_states_filtered("en", 0, 100, 0.0, "wrong", 50).unwrap().len(), 3);
        // ≥3 次 → abandon(8) 和 reluctant(3)
        let got = db.leech_states_filtered("en", 3, 100, 0.0, "wrong", 50).unwrap();
        let words: Vec<_> = got.iter().map(|s| s.word.as_str()).collect();
        assert_eq!(words, vec!["abandon", "reluctant"]);
    }

    #[test]
    fn leech_filter_by_error_rate_finds_all_wrong_words() {
        let db = tmp_db("leech-rate");
        seed_leeches(&db);
        // 错误率 ≥ 60% 只能命中 reluctant（3 错 0 对）；
        // abandon 虽然错了 8 次，但错误率只有 50%，不该出现
        let got = db.leech_states_filtered("en", 0, 100, 0.6, "rate", 50).unwrap();
        let words: Vec<_> = got.iter().map(|s| s.word.as_str()).collect();
        assert_eq!(words, vec!["reluctant"], "错误率筛选不能用错误次数代替");
    }

    #[test]
    fn leech_filter_by_max_mastery() {
        let db = tmp_db("leech-mastery");
        seed_leeches(&db);
        let got = db.leech_states_filtered("en", 0, 30, 0.0, "mastery", 50).unwrap();
        let words: Vec<_> = got.iter().map(|s| s.word.as_str()).collect();
        // 掌握度 ≤30 且按掌握度升序
        assert_eq!(words, vec!["abandon", "reluctant"]);
    }

    /// 排序字段必须走白名单：传一个不存在的 key 不能拼进 SQL。
    #[test]
    fn leech_filter_ignores_unknown_order() {
        let db = tmp_db("leech-order");
        seed_leeches(&db);
        let got = db
            .leech_states_filtered("en", 0, 100, 0.0, "wrong; DROP TABLE study_state", 50)
            .unwrap();
        assert_eq!(got.len(), 3, "未知排序键应回退到默认排序而不是报错");
        // 表还在
        assert_eq!(db.leech_states_filtered("en", 0, 100, 0.0, "wrong", 50).unwrap().len(), 3);
    }

    #[test]
    fn clear_leech_many_only_touches_given_words() {
        let db = tmp_db("leech-clear");
        seed_leeches(&db);
        let n = db.clear_leech_many(&["abandon".into()], "en").unwrap();
        assert_eq!(n, 1);
        let left = db.leech_states_filtered("en", 0, 100, 0.0, "wrong", 50).unwrap();
        let words: Vec<_> = left.iter().map(|s| s.word.as_str()).collect();
        assert_eq!(words, vec!["reluctant", "benefit"]);
    }

    /* ---- 知识图谱关系表 ---- */

    /// 派生关系有方向，取边时必须双向都算，否则图谱会缺一半连线。
    #[test]
    fn graph_edges_are_bidirectional() {
        let db = tmp_db("graph-dir");
        let edges = vec![crate::graph::GraphEdge {
            src: "happy".into(),
            dst: "happiness".into(),
            rel: "derived".into(),
            weight: 0.6,
            source: "local".into(),
        }];
        assert_eq!(db.upsert_edges(&edges, "en", 1).unwrap(), 1);

        assert_eq!(db.edges_from("happy", "en").unwrap().len(), 1, "出边要能查到");
        assert_eq!(db.edges_into("happiness", "en").unwrap().len(), 1, "入边要能查到");
        // 反向没有边
        assert!(db.edges_from("happiness", "en").unwrap().is_empty());
    }

    /// 同一条边重复写入不能产生重复行（构建图谱会被反复调用）。
    #[test]
    fn graph_edges_are_idempotent() {
        let db = tmp_db("graph-dup");
        let edges = vec![crate::graph::GraphEdge {
            src: "big".into(),
            dst: "large".into(),
            rel: "synonym".into(),
            weight: 1.0,
            source: "local".into(),
        }];
        assert_eq!(db.upsert_edges(&edges, "en", 1).unwrap(), 1);
        assert_eq!(db.upsert_edges(&edges, "en", 2).unwrap(), 0, "重复写入应被忽略");
        let (n, e) = db.graph_counts("en").unwrap();
        assert_eq!(e, 1);
        assert!(n >= 2);
    }

    // ---------------- AI 讲解存档 ----------------

    fn explain_row(word: &str, el: &str, text: &str) -> ExplainRow {
        ExplainRow {
            word: word.into(),
            lang: "en".into(),
            explain_lang: el.into(),
            text: text.into(),
            original: format!("raw {text}"),
            translated: false,
            entry_json: String::new(),
            saved: false,
            updated_at: 0,
        }
    }

    /// 同一个词的不同讲解语言互不覆盖（中文讲解和英文讲解要能并存）。
    #[test]
    fn explain_store_roundtrip_and_multilang() {
        let db = tmp_db("explain-roundtrip");
        db.save_explain(&explain_row("apple", "zh", "苹果的讲解"), 100)
            .unwrap();
        db.save_explain(&explain_row("apple", "en", "explanation in english"), 101)
            .unwrap();

        let zh = db
            .get_explain("apple", "en", "zh")
            .unwrap()
            .expect("中文讲解应当在");
        assert_eq!(zh.text, "苹果的讲解");
        assert_eq!(zh.original, "raw 苹果的讲解");
        assert!(!zh.saved);

        let all = db.explains_of("apple", "en").unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].explain_lang, "en", "应当按 updated_at 倒序");

        assert_eq!(db.explain_count().unwrap(), 2);
    }

    /// 重新生成讲解不能把已生成的词条冲掉，`saved` 也不该回落。
    #[test]
    fn explain_save_keeps_entry_and_saved_flag() {
        let db = tmp_db("explain-keep");
        db.save_explain(&explain_row("apple", "zh", "第一版"), 100)
            .unwrap();
        db.set_explain_saved("apple", "en", "zh", r#"{"word":"apple"}"#, 200)
            .unwrap();

        let r = db.get_explain("apple", "en", "zh").unwrap().unwrap();
        assert!(r.saved);
        assert_eq!(r.entry_json, r#"{"word":"apple"}"#);

        // 再来一次：text 更新，但 entry_json 为空 → 不该覆盖，saved 只增不减
        db.save_explain(&explain_row("apple", "zh", "第二版"), 300)
            .unwrap();
        let r2 = db.get_explain("apple", "en", "zh").unwrap().unwrap();
        assert_eq!(r2.text, "第二版");
        assert!(r2.saved, "saved 只增不减");
        assert_eq!(r2.entry_json, r#"{"word":"apple"}"#, "空值不该覆盖已有词条");
    }

    /// 搜索必须**同时匹配词与讲解正文**——这是「在词库中搜索能搜到讲解」的关键。
    #[test]
    fn explain_search_matches_word_and_body() {
        let db = tmp_db("explain-search");
        db.save_explain(&explain_row("apple", "zh", "苹果；也表示苹果公司"), 100)
            .unwrap();
        db.save_explain(&explain_row("banana", "zh", "香蕉"), 101)
            .unwrap();

        let by_word = db.search_explains(None, "appl", 50).unwrap();
        assert_eq!(by_word.len(), 1);
        assert_eq!(by_word[0].word, "apple");

        // ★ 按**讲解正文**搜（正文里出现「香蕉」，但词是 banana）
        let by_body = db.search_explains(None, "香蕉", 50).unwrap();
        assert_eq!(by_body.len(), 1);
        assert_eq!(by_body[0].word, "banana");

        // 指定语言过滤
        assert_eq!(db.search_explains(Some("ja"), "appl", 50).unwrap().len(), 0);
        assert_eq!(db.search_explains(Some("en"), "appl", 50).unwrap().len(), 1);

        // LIKE 通配符必须被转义：输入 % 不该匹配到所有记录
        assert_eq!(db.search_explains(None, "%", 50).unwrap().len(), 0);
        assert_eq!(db.search_explains(None, "_", 50).unwrap().len(), 0);
    }

    #[test]
    fn clear_explains_can_keep_saved_ones() {
        let db = tmp_db("explain-clear");
        db.save_explain(&explain_row("apple", "zh", "a"), 1).unwrap();
        db.save_explain(&explain_row("banana", "zh", "b"), 2).unwrap();
        db.set_explain_saved("banana", "en", "zh", "{}", 3).unwrap();

        assert_eq!(db.clear_explains(true).unwrap(), 1);
        assert!(db.get_explain("apple", "en", "zh").unwrap().is_none());
        assert!(db.get_explain("banana", "en", "zh").unwrap().is_some());

        assert_eq!(db.clear_explains(false).unwrap(), 1);
        assert_eq!(db.explain_count().unwrap(), 0);
    }

    #[test]
    fn delete_single_explain() {
        let db = tmp_db("explain-del");
        db.save_explain(&explain_row("apple", "zh", "a"), 1).unwrap();
        assert_eq!(db.delete_explain("apple", "en", "zh").unwrap(), 1);
        assert_eq!(db.delete_explain("apple", "en", "zh").unwrap(), 0);
    }
}
