/* ============================================================
   books.rs —— 词库管理命令（需求 3 / 5 / 4）
   多级词库：wordbooks（词库表）+ wordbook_words（关联表）
   支持：列出 / 详情 / 导入 / 下载 / 删除 / 已背回看 / 按词库出题
   ============================================================ */

use crate::db::WordRow;
use crate::dict::importer::{self, ImportMapping};
use crate::models::{ImportResult, RemoteBook, Wordbook, WordbookProgress};
use crate::state::AppState;
use crate::timeutil;
use std::sync::Arc;
use tauri::State;

fn err(e: impl std::fmt::Display) -> String {
    format!("{}", e)
}

/// 内置的根词库（虚拟节点，仅用于界面分组，不含单词）。
/// level=0 为根，level=1 为考试大类。
fn builtin_roots() -> Vec<Wordbook> {
    let mk = |id: &str, name: &str, cat: &str, level: i64, desc: &str, lang: &str| Wordbook {
        id: id.to_string(),
        name: name.to_string(),
        category: cat.to_string(),
        level,
        parent_id: if level == 0 { String::new() } else { "root".to_string() },
        lang: lang.to_string(),
        description: desc.to_string(),
        source_url: String::new(),
        license: String::new(),
        word_count: 0,
        builtin: true,
        installed: true,
        ord: level,
        created_at: 0,
    };
    vec![
        mk("root", "全部词库", "root", 0, "所有已安装词库的汇总视图", ""),
        mk("exam-en", "英语考试", "exam", 1, "四六级、考研、雅思、托福、GRE、高考", "en"),
        mk("exam-jp", "日语", "exam", 1, "JLPT N5~N1、MOJI 常用词", "ja"),
    ]
}

/// 列出全部词库（含内置根节点 + 用户导入的），并附带学习进度。
#[tauri::command(async)]
pub fn cmd_list_wordbooks(
    state: State<'_, Arc<AppState>>,
    lang: Option<String>,
) -> Result<Vec<WordbookProgress>, String> {
    let cfg = state.cfg();
    let now = timeutil::now_ts();

    // 前端不传 lang 时列出**全部语言**的词库。
    //
    // 「我的词库」是总览页，若默认按 target_lang 过滤，用户在学英语时下载的
    // 日语词库会在树里彻底看不见 —— 表现就是「明明提示下载成功，却哪儿都找不到」。
    let mut books = builtin_roots();
    let user_books = state.db.list_wordbooks(lang.as_deref()).map_err(err)?;
    books.extend(user_books);

    let mut out = Vec::with_capacity(books.len());
    for b in books {
        // 进度按**词库自身**的语言统计，而不是当前学习语言：
        // 否则日语词库的已学数永远是 0。
        let progress_lang = if b.lang.is_empty() {
            cfg.target_lang.clone()
        } else {
            b.lang.clone()
        };
        // 根节点不查进度，避免无谓查询
        let (learned, mastered, due_today) = if b.id == "root" || b.level < 2 {
            (0, 0, 0)
        } else {
            state
                .db
                .book_progress(&b.id, &progress_lang, now)
                .unwrap_or((0, 0, 0))
        };
        out.push(WordbookProgress {
            book: b,
            learned,
            mastered,
            due_today,
        });
    }
    Ok(out)
}

/// 取单个词库详情。
#[tauri::command(async)]
pub fn cmd_get_wordbook(
    state: State<'_, Arc<AppState>>,
    id: String,
) -> Result<Option<Wordbook>, String> {
    if let Some(b) = builtin_roots().into_iter().find(|b| b.id == id) {
        return Ok(Some(b));
    }
    state.db.get_wordbook(&id).map_err(err)
}

/// 新建一个空词库（用户自建，如「我的生词本」）。
#[tauri::command(async)]
pub fn cmd_create_wordbook(
    state: State<'_, Arc<AppState>>,
    name: String,
    category: Option<String>,
    parent_id: Option<String>,
    description: Option<String>,
    source_url: Option<String>,
    license: Option<String>,
    lang: Option<String>,
) -> Result<Wordbook, String> {
    let cfg = state.cfg();
    let lang = lang.unwrap_or(cfg.target_lang.clone());
    let parent = parent_id.unwrap_or_else(|| "root".to_string());
    let level = if parent == "root" { 1 } else { 2 };
    let id = format!("user-{}", timeutil::now_ts());
    let b = Wordbook {
        id: id.clone(),
        name,
        category: category.unwrap_or_else(|| "other".to_string()),
        level,
        parent_id: parent,
        lang,
        description: description.unwrap_or_default(),
        source_url: source_url.unwrap_or_default(),
        license: license.unwrap_or_default(),
        word_count: 0,
        builtin: false,
        installed: true,
        ord: level,
        created_at: timeutil::now_ts(),
    };
    state.db.upsert_wordbook(&b).map_err(err)?;
    Ok(b)
}

/// 删除词库（内置根节点不可删）。
#[tauri::command(async)]
pub fn cmd_delete_wordbook(state: State<'_, Arc<AppState>>, id: String) -> Result<(), String> {
    if builtin_roots().iter().any(|b| b.id == id) {
        return Err("内置词库不可删除".into());
    }
    state.db.delete_wordbook(&id).map_err(err)
}

/// 导入文本内容到指定词库（需求 3）。
///
/// - `content` 原始文本（JSON / CSV / TSV / 纯文本皆可，自动识别）
/// - `book_id` 目标词库；不存在会自动创建
/// - `auto_create` 为 true 且 book_id 为空时，按内容推断词库名
#[tauri::command(async)]
pub fn cmd_import_words_to_book(
    state: State<'_, Arc<AppState>>,
    content: String,
    book_id: Option<String>,
    book_name: Option<String>,
    format_hint: Option<String>,
    source: Option<String>,
    lang: Option<String>,
) -> Result<ImportResult, String> {
    let cfg = state.cfg();
    let now = timeutil::now_ts();
    let hint = format_hint.unwrap_or_default();

    // 目标词库：给了 id 用之；否则新建。
    //
    // ★ 先定词库、再定语言 —— 顺序很关键。
    //   原来的写法是 `let lang = lang.unwrap_or(target_lang)` 排在最前面，
    //   于是「往已有词库里导入」也一律按**当前在学的语言**给词条打标：
    //   在学日语时往「考研核心词汇」里补几百个词，那批词就被写成 ja，
    //   而词库本身是 en —— `words_in_book` 按 `w.lang = bw.lang` 取词，
    //   这批词立刻变成「看得到条数、点进去没内容」。
    //   现在改成三级决议：
    //     ① 目录里有的内置词库 → 以目录标注为准（最权威，且不可被绕过）
    //     ② 库里已存在的词库   → 跟着**这本词库自己的**语言走
    //     ③ 其余（新建词库）   → 才用调用方传的 / 当前目标语言
    let target_book = match book_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => state.db.get_wordbook(id).map_err(err)?,
        None => None,
    };

    let builtin_lang = importer::builtin_book_langs()
        .into_iter()
        .find(|(id, _)| {
            target_book
                .as_ref()
                .map(|b| b.id.as_str() == id)
                .unwrap_or(false)
        })
        .map(|(_, l)| l);

    let lang = if let Some(cl) = builtin_lang.filter(|l| !l.trim().is_empty()) {
        if let Some(l) = lang.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
            if !l.eq_ignore_ascii_case(&cl) {
                log::warn!(
                    "导入到内置词库 {} 时语言 {} 与目录标注 {} 不一致，以目录为准",
                    target_book.as_ref().map(|b| b.id.as_str()).unwrap_or(""),
                    l,
                    cl
                );
            }
        }
        cl
    } else if let Some(bl) = target_book
        .as_ref()
        .map(|b| b.lang.trim().to_string())
        .filter(|l| !l.is_empty())
    {
        bl
    } else {
        lang.as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .unwrap_or(cfg.target_lang.as_str())
            .to_string()
    };

    let mapping = ImportMapping::default();
    let entries = importer::parse_auto(&content, &mapping, &lang, &hint).map_err(err)?;
    let total = entries.len() as i64;

    // 目标词库：给了 id 用之；否则新建
    let book = match book_id {
        Some(id) if !id.trim().is_empty() => state
            .db
            .get_wordbook(&id)
            .map_err(err)?
            .unwrap_or_else(|| Wordbook {
                id: id.clone(),
                name: book_name.clone().unwrap_or_else(|| id.clone()),
                category: "other".into(),
                level: 2,
                parent_id: "root".into(),
                lang: lang.clone(),
                ..Default::default()
            }),
        _ => Wordbook {
            id: format!("user-{}", now),
            name: book_name.unwrap_or_else(|| "导入词库".to_string()),
            category: "other".into(),
            level: 2,
            parent_id: "root".into(),
            lang: lang.clone(),
            description: format!("由 {} 导入", source.clone().unwrap_or_else(|| "本地文件".into())),
            source_url: String::new(),
            license: String::new(),
            word_count: 0,
            builtin: false,
            installed: true,
            ord: 2,
            created_at: now,
        },
    };

    // 建库 + 落词
    state.db.upsert_wordbook(&book).map_err(err)?;

    // 单词 -> WordEntry 全量入库（保留释义，便于离线背诵）。
    //
    // ★ 走**带语种闸门**的批量写：词表里混进来的日语 / 韩语行会被拦下，
    //   且不计进 imported —— 否则界面显示「导入 5000 词」而实际少了几百，
    //   用户只会以为过滤没生效、甚至以为程序丢了词。
    let write = state
        .db
        .bulk_upsert_words_gated(&entries, now)
        .map_err(err)?;
    let filtered = write.rejected.len() as i64;
    let words: Vec<String> = entries
        .iter()
        .filter(|e| crate::lang::lang_compatible(&e.word, &e.lang))
        .map(|e| e.word.clone())
        .collect();
    let (imported, skipped) = state
        .db
        .add_words_to_book(&book.id, &words, &lang)
        .map_err(err)?;

    let result = ImportResult {
        book_id: book.id.clone(),
        total,
        imported,
        skipped,
        failed: filtered,
        message: format!(
            "已导入 {} 个单词到「{}」（去重跳过 {}{}）",
            imported,
            book.name,
            skipped,
            if filtered > 0 {
                format!("，语种不符拦下 {filtered} 个")
            } else {
                String::new()
            }
        ),
    };
    let _ = state
        .db
        .log_import(&result, &source.unwrap_or_else(|| "local".into()), now);

    Ok(result)
}

/// 内置可下载词库目录（需求 5）。
/// 会把「已安装」状态回填，方便界面区分。
#[tauri::command(async)]
pub fn cmd_remote_catalog(
    state: State<'_, Arc<AppState>>,
    category: Option<String>,
) -> Result<Vec<RemoteBook>, String> {
    let installed: Vec<Wordbook> = state.db.list_wordbooks(None).map_err(err)?;
    let ids: std::collections::BTreeSet<String> = installed.into_iter().map(|b| b.id).collect();

    let mut list = importer::remote_catalog();
    for r in &mut list {
        r.installed = ids.contains(&r.id);
    }
    if let Some(c) = category {
        if !c.trim().is_empty() && c != "all" {
            list.retain(|r| r.category == c);
        }
    }
    Ok(list)
}

/// 下载词库包并导入（需求 5）。
///
/// 关键改动：**按镜像链依次回退**。
/// 词库都在 GitHub 上，而 `raw.githubusercontent.com` 在国内直连必失败，
/// 所以目录里给每个词库都配了 jsDelivr / gh-proxy 等镜像。
/// 这里逐个尝试，并把每个镜像的失败原因收集起来一起返回，
/// 而不是只丢一句「网络错误」让用户猜。
#[tauri::command]
pub async fn cmd_download_book(
    state: State<'_, Arc<AppState>>,
    id: String,
    lang: Option<String>,
) -> Result<ImportResult, String> {
    let catalog = importer::remote_catalog();
    let rb = catalog
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| format!("词库 {} 不在可下载目录中", id))?;

    let client = state.http();
    // 主地址 + 备用镜像，按顺序尝试
    let mut candidates: Vec<String> = vec![rb.url.clone()];
    candidates.extend(rb.mirrors.iter().cloned());

    // 两轮扫描：第一轮逐个镜像快速试一遍（失败立刻换下一个），
    // 第二轮再补一遍。
    //
    // 为什么要在「镜像」之上再加一层轮次：镜像的失败绝大多数是**瞬时**的
    // （实测 cdn.jsdelivr.net 约每 6 次会有 1 次 TCP 连接被静默丢弃；
    // gh-proxy.com 可能整体不可用）。如果只扫一轮、恰好三个镜像同时踩上
    // 瞬时故障，用户看到的就是「点了下载就报错」，而再点一次其实就能成功
    // ——这正是用户最初报的「开了代理还是网络问题不能下载」。
    //
    // 轮次放在**外层**而不是让每个镜像各重试两次，是为了更快地换到别的
    // 镜像：一个黑洞地址要等满连接超时，先试别人才更划算。
    const PASSES: usize = 2;
    // 总预算兜底，避免「镜像数 × 轮次 × 连接超时」叠成分钟级干等
    const TOTAL_BUDGET_SECS: u64 = 90;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(TOTAL_BUDGET_SECS);
    let mut errors: Vec<String> = Vec::new();
    // 地址本身有问题（404/403）的镜像，第二轮不必再试
    let mut dead: Vec<String> = Vec::new();
    let mut text: Option<(String, String)> = None; // (内容, 命中的地址)

    'outer: for pass in 0..PASSES {
        if pass > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        }
        for url in candidates.iter() {
            if dead.contains(url) {
                continue;
            }
            if std::time::Instant::now() >= deadline {
                errors.push(format!("已超过 {} 秒总时限，停止尝试", TOTAL_BUDGET_SECS));
                break 'outer;
            }
            match fetch_book_text_once(&client, url).await {
                Ok(t) => {
                    text = Some((t, url.clone()));
                    break 'outer;
                }
                Err(e) => {
                    let host = url
                        .split('/')
                        .nth(2)
                        .unwrap_or(url.as_str())
                        .to_string();
                    if e.is_permanent() {
                        dead.push(url.clone());
                        errors.push(format!("{}：{}（地址无效，不再重试）", host, e.message()));
                    } else {
                        errors.push(format!(
                            "{}（第 {} 轮）：{}",
                            host,
                            pass + 1,
                            e.message()
                        ));
                    }
                }
            }
        }
    }

    let Some((text, used_url)) = text else {
        return Err(format!(
            "下载失败：{} 个镜像都不可用。\n{}\n\n\
             提示：词库通过 jsDelivr 等国内可直连的镜像分发，正常情况下**不需要代理**。\
             这类镜像偶发连接超时（多试一次通常就好），请先**再点一次下载**；\
             若持续失败，去「设置 → 网络与代理」点「检测网络」看是哪条链路不通。",
            candidates.len(),
            errors.join("\n")
        ));
    };

    // 词条语言以**目录里标注的语言**为准，而不是「用户当前在学的语言」：
    // 在学英语时下载日语词库，若按 target_lang 入库，几千个日语词会被标成 en，
    // 之后既搜不到、也背不到（背的时候按 ja 去查，库里却是 en）。
    //
    // ★ 目录里标注的语言是**权威**，调用方传进来的 lang 只作为兜底。
    //
    //   原来这里是反过来的：`Some(l) if !l.is_empty() => l` 排在第一位，于是
    //   前端只要传了值，就完全绕过目录。而前端传的是卡片上的 `data-lang`
    //   （来自 `cmd_remote_catalog`），一旦那一层出现任何偏差，几千个词就会
    //   被整批写成错的语种 —— 实机上的「考研核心词汇（5057 · 日语）」就是这么来的：
    //   wordbooks.lang / wordbook_words.lang / words.lang 三张表全被写成 ja，
    //   而这份词表本身是**英语**大纲词汇（April / Bible / Catholic …）。
    //   错标的代价不只是界面标签难看：`words_in_book` 是按
    //   `wordbook_words JOIN words ON w.lang = bw.lang` 取词的，语种一旦错位，
    //   背诵选词、搜索、复习计划全部跟着错。
    //
    //   所以这里改成：目录有值就用目录的；两边不一致时把差异记进日志（便于排查
    //   「为什么我下的这本是别的语种」），但以目录为准。
    let lang = if !rb.lang.trim().is_empty() {
        let catalog_lang = rb.lang.trim().to_string();
        if let Some(l) = lang.as_deref().map(str::trim).filter(|l| !l.is_empty()) {
            if !l.eq_ignore_ascii_case(&catalog_lang) {
                log::warn!(
                    "词库 {} 的调用方语言 {} 与目录标注 {} 不一致，以目录为准",
                    rb.id,
                    l,
                    catalog_lang
                );
            }
        }
        catalog_lang
    } else {
        lang.as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .unwrap_or("en")
            .to_string()
    };
    let now = timeutil::now_ts();

    let mapping = ImportMapping::default();
    let entries = importer::parse_auto(&text, &mapping, &lang, &rb.format).map_err(err)?;
    let total = entries.len() as i64;
    if total == 0 {
        return Err(format!(
            "下载成功（{}）但内容为空或格式无法识别，无法导入。",
            used_url
        ));
    }

    // 建词库（含来源与许可标注）
    let book = Wordbook {
        id: rb.id.clone(),
        name: rb.name.clone(),
        category: rb.category.clone(),
        level: 2,
        // 挂到对应语言的考试大类下（日语 → 日语节点），分类型的挂到根
        parent_id: match (rb.lang.as_str(), rb.category.as_str()) {
            ("ja", _) | (_, "jlpt") => "exam-jp",
            (_, "other") => "root",
            _ => "exam-en",
        }
        .to_string(),
        lang: lang.clone(),
        description: rb.description.clone(),
        source_url: rb.source_url.clone(),
        license: rb.license.clone(),
        word_count: 0,
        builtin: false,
        installed: true,
        ord: 2,
        created_at: now,
    };
    state.db.upsert_wordbook(&book).map_err(err)?;
    // 同样过语种闸门：下载的词表由第三方维护，里面混几行别的语言很常见
    let write = state
        .db
        .bulk_upsert_words_gated(&entries, now)
        .map_err(err)?;
    let filtered = write.rejected.len() as i64;
    let words: Vec<String> = entries
        .iter()
        .filter(|e| crate::lang::lang_compatible(&e.word, &e.lang))
        .map(|e| e.word.clone())
        .collect();
    let (imported, skipped) = state
        .db
        .add_words_to_book(&book.id, &words, &lang)
        .map_err(err)?;

    let result = ImportResult {
        book_id: book.id.clone(),
        total,
        imported,
        skipped,
        failed: filtered,
        message: format!(
            "已下载「{}」，导入 {} 词{}",
            book.name,
            imported,
            if filtered > 0 {
                format!("（语种不符拦下 {filtered} 个）")
            } else {
                String::new()
            }
        ),
    };
    let _ = state.db.log_import(&result, &rb.source_url, now);
    Ok(result)
}

/// 一次下载尝试的失败原因，区分「值不值得再试」。
///
/// 为什么需要区分：实测词库镜像的失败绝大多数是**瞬时**的
/// （TCP 连接被静默丢弃 / TLS 握手卡死，几秒后同样的地址又能正常返回），
/// 而 404 / 403 这类是地址本身的问题，重试只是白等。
/// 不区分就只能在「不重试（容易失败）」和「无脑重试（白等很久）」之间二选一。
#[derive(Debug)]
enum FetchErr {
    /// 瞬时失败：超时、连接被重置、握手失败等，值得再试一次
    Transient(String),
    /// 明确性失败：HTTP 4xx 等，重试没有意义
    Permanent(String),
}

impl FetchErr {
    fn message(&self) -> &str {
        match self {
            FetchErr::Transient(m) | FetchErr::Permanent(m) => m,
        }
    }
    fn is_permanent(&self) -> bool {
        matches!(self, FetchErr::Permanent(_))
    }
}

/// 抓取单个下载地址的文本内容（单次尝试，不做重试）。
async fn fetch_book_text_once(client: &reqwest::Client, url: &str) -> Result<String, FetchErr> {
    let resp = client.get(url).send().await.map_err(|e| {
        FetchErr::Transient(format!("{}", crate::net::friendly_reqwest_error(&e)))
    })?;
    let status = resp.status();
    if !status.is_success() {
        let msg = format!("HTTP {}", status.as_u16());
        // 4xx 是地址/权限问题；5xx 可能是上游临时故障，值得重试
        return Err(if status.as_u16() >= 400 && status.as_u16() < 500 {
            FetchErr::Permanent(msg)
        } else {
            FetchErr::Transient(msg)
        });
    }
    resp.text()
        .await
        .map_err(|e| FetchErr::Transient(format!("读取内容失败：{}", e)))
}

/// 查看「已背过的单词」（需求 4）。
#[tauri::command(async)]
pub fn cmd_reviewed_words(
    state: State<'_, Arc<AppState>>,
    lang: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<WordRow>, String> {
    let lang = lang.unwrap_or_else(|| state.cfg().target_lang);
    let limit = limit.unwrap_or(200).clamp(1, 2000);
    let offset = offset.unwrap_or(0).max(0);
    state.db.reviewed_words(&lang, limit, offset).map_err(err)
}

/// 一条「词 → 词库」归属（已背列表分容器渲染用）。
#[derive(Debug, serde::Serialize)]
pub struct BookRef {
    pub id: String,
    pub name: String,
}

/// 批量查一批词的词库归属 → `{ "word": [{id, name}, …], … }`。
///
/// 已背列表（本轮 / 最近背过）此前把不同词库的词平铺在一个容器里，
/// 前端要按词库分容器就必须知道每个词属于哪本书 —— 而 WordRow 不带
/// 归属字段，纯前端做不到，所以开这个批量反查口。
/// 同属多本书时按书名排序全部返回，由前端按「当前所选词库优先」定组。
#[tauri::command(async)]
pub fn cmd_word_book_refs(
    state: State<'_, Arc<AppState>>,
    lang: Option<String>,
    words: Vec<String>,
) -> Result<std::collections::HashMap<String, Vec<BookRef>>, String> {
    let lang = lang.unwrap_or_else(|| state.cfg().target_lang);
    let rows = state.db.word_book_refs(&lang, &words).map_err(err)?;
    let mut out: std::collections::HashMap<String, Vec<BookRef>> = std::collections::HashMap::new();
    for (word, book_id, book_name) in rows {
        out.entry(word)
            .or_default()
            .push(BookRef { id: book_id, name: book_name });
    }
    Ok(out)
}

/// 列出某个词库里的单词（需求 5：进词库看内容）。
#[tauri::command(async)]
pub fn cmd_words_in_book(
    state: State<'_, Arc<AppState>>,
    book_id: String,
    lang: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
) -> Result<Vec<WordRow>, String> {
    // 未指定语言时用**词库自身**的语言，而不是当前学习语言：
    // 点开一本日语词库却按 en 去查，结果永远是空的。
    let lang = match lang {
        Some(l) if !l.trim().is_empty() => l.trim().to_string(),
        _ => state
            .db
            .get_wordbook(&book_id)
            .ok()
            .flatten()
            .map(|b| b.lang)
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| state.cfg().target_lang),
    };
    let limit = limit.unwrap_or(200).clamp(1, 2000);
    let offset = offset.unwrap_or(0).max(0);
    state
        .db
        .words_in_book(&book_id, &lang, limit, offset)
        .map_err(err)
}
