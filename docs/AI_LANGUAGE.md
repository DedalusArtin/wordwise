# AI 讲解语言：实现方案

> 目标：修掉「选了输入语言 / 输出语言，AI 讲解却还是全英文」。
> 附带解决界面上重复的关闭/收起控件。

---

## 一、根因

有三层原因叠加，只修任何一层都不够：

1. **默认人设里写死了输出语言。**
   `DEFAULT_TUTOR_PROMPT`（`src-tauri/src/models.rs`）原来是
   「使用**简体中文**讲解」。用户把讲解语言改成日语后，模型看到的是两条
   互相冲突的指令，而「只用简体中文」这条更硬、位置更靠前，于是直接忽略
   后面的语言要求。

2. **`system_prompt` 是持久化的。**
   它会被写进用户的 `config.json`。所以**只改代码里的默认值追不到老用户** ——
   老用户磁盘上那份仍然是写死中文的旧 prompt。必须在配置迁移里订正。

3. **后端只返回一个字符串。**
   `cmd_ai_explain` 返回 `String`。一旦译文替换掉原文，原文就再也拿不回来，
   「查看英文原文」这个需求在数据结构层面就不可能实现。

---

## 二、整体方案：两层都要

### 结论：翻译模板**两层都插，以提示词层为主、输出后处理层兜底**。

| | 提示词层（主） | 输出后处理层（兜底） |
|---|---|---|
| 落点 | `llm::with_lang_constraint()` 追加到 system 末尾 | `llm::translate_markdown()` 在拿到完整输出后跑 |
| 成本 | **0**（不额外请求模型） | 多一次模型调用（约 1～2 秒） |
| 流式打字机 | 保住 | 不保（结束后整体替换） |
| Markdown 结构 | 保住 | 依赖模型，靠模板里的「铁律」约束 |
| 遵守度 | 本地小模型不稳定（60～90%） | 接近 100% |
| 失败后果 | 无（只是没生效） | 降级为保留原文 + 提示 |

**为什么不能只做提示词层**：本地小模型（1.5B～7B）对 system 里「用 X 语言
回答」的遵循度不稳定，尤其当 prompt 里还有更强的语言指令时。用户报的就是
这个现象——选了却没生效。**没有兜底 = 需求 1「选择后生效」不成立**。

**为什么不能只做后处理层**：
- 每次讲解都要多一次完整生成，延迟翻倍；
- 流式输出会被打断（用户先看到英文，几秒后整段跳变），体验割裂；
- 翻译本身有信息损耗，能不翻就不翻。

所以：**先让模型直接说对（提示词层），说不对再兜底翻（后处理层）**。
后处理层只在 `llm::text_matches_lang()` 判定「模型没听话」时才触发。

```
                    ┌─────────────────────────────────────┐
  用户选了「日本語」 │  1. 提示词层                        │
  ───────────────► │  system += EXPLAIN_LANG_CONSTRAINT   │  零成本
                    │  （{LANG} → 日本語）                 │
                    └──────────────┬──────────────────────┘
                                   ▼
                            模型流式输出
                                   ▼
                    ┌─────────────────────────────────────┐
                    │  2. 语言检测 text_matches_lang()     │
                    └──────┬────────────────────┬─────────┘
                       已符合│                    │不符合
                           ▼                    ▼
                      直接展示          ┌────────────────────┐
                                       │ 3. 后处理层兜底     │
                                       │  protect → 翻译模板 │
                                       │  → restore          │
                                       └─────────┬──────────┘
                                                 ▼
                                      整体替换原文并展示
```

---

## 三、涉及的配置项

在 `AppConfig`（`src-tauri/src/models.rs`）新增三个字段：

| 字段 | 类型 | 默认 | 说明 |
|---|---|---|---|
| `explain_lang` | `String` | `"zh"` | AI 讲解与回复使用的语言（ISO 639-1）。**对所有讲解与追问生效**，与「查询语言」互相独立 |
| `explain_auto_translate` | `bool` | `true` | 模型没按 `explain_lang` 输出时，是否自动套翻译模板重译并替换原文。关掉就只靠提示词层 |
| `explain_translate_template` | `String` | `""` | 自定义翻译模板。留空用内置 `DEFAULT_TRANSLATE_TEMPLATE`。可用占位符 `{LANG}`（目标语言）、`{CONTENT}`（待翻译正文） |

### 配置迁移（v4）

`CONFIG_VERSION` 由 `3` → `4`，`dict::builtin::migrate_config()` 新增：

```rust
// 只替换「确认没被用户改过」的人设
if cfg.llm.system_prompt.trim().is_empty()
    || crate::models::is_builtin_prompt(&cfg.llm.system_prompt)
{
    cfg.llm.system_prompt = crate::models::DEFAULT_TUTOR_PROMPT.to_string();
}
// 老配置缺字段时补默认中文
if cfg.explain_lang.trim().is_empty() { cfg.explain_lang = "zh".to_string(); }
```

关键点：`is_builtin_prompt()` 拿 `LEGACY_TUTOR_PROMPT_V1`（v1.0.0 原版）和新默认
做比对（CRLF 归一 + trim）。**用户自己调过的人设一律不动** —— 那是他的内容，
我们只在运行时追加语言约束、靠后处理兜底，不做破坏性覆盖。

---

## 四、需要修改的代码位置

### 后端

| 文件 | 改动 |
|---|---|
| `src-tauri/src/models.rs` | `CONFIG_VERSION` 3→4；`DEFAULT_TUTOR_PROMPT` 改为语言中性；新增 `LEGACY_TUTOR_PROMPT_V1` / `is_builtin_prompt()` / `EXPLAIN_LANG_CONSTRAINT` / `DEFAULT_TRANSLATE_TEMPLATE`；`AppConfig` 三个新字段 |
| `src-tauri/src/dict/builtin.rs` | `migrate_config()` 的 v4 分支（订正人设 + 补 `explain_lang`） |
| `src-tauri/src/llm/mod.rs` | `chat()` 抽出 `chat_ex()`（翻译需要更大的 `max_tokens`）；新增 `explain_lang_name()`、`with_lang_constraint()`、`text_matches_lang()`、`protect_technical_spans()` / `restore_technical_spans()`、`strip_outer_fence()`、`translate_markdown()` |
| `src-tauri/src/commands/mod.rs` | 新增 `ExplainResult` / `TranslateResult` 结构体；`cmd_ai_explain` / `cmd_ai_explain_sync` 返回类型 `String` → `ExplainResult`；新增 `cmd_translate_text`（对已有原文重译，切换语言秒回）、`cmd_set_explain_lang`（即时落盘） |
| `src-tauri/src/windows.rs` | 注册 `cmd_translate_text` / `cmd_set_explain_lang`；主窗口 `.decorations(true)` → `false` |

### 前端

| 文件 | 改动 |
|---|---|
| `src/js/api.js` | 新增 `setExplainLang()` / `translateText()`；Mock 返回值改成 `ExplainResult` 结构；导出 `EXPLAIN_LANGS` / `explainLangOptions()` / `explainLangLabel()` |
| `src/js/lookup.js` | `Detail.explainInto()` 改为消费 `ExplainResult`；新增 `renderExplain()`（语言提示条 + 「查看原文」切换）、`explainCache`、`bindExplainLang()`、`initExplainLang()`、`retranslateCurrent()`；移除 `lk-ai-collapse` 绑定 |
| `src/js/sidebar.js` | `explain()` 同步消费 `ExplainResult` |
| `src/js/settings.js` | `load()` / `save()` 读写三个新配置项；保存时额外调一次 `setExplainLang` 保证「选了就记住」 |
| `src/js/app.js` | 补「最大化/还原」按钮 + 双击标题栏；新增 `syncMaxIcon()` |
| `src/index.html` | 标题栏加 `#btn-maximize`；AI 面板头部加 `#ai-lang` 下拉、**删除**重复的 `#lk-ai-collapse`；设置页新增「AI 讲解语言」面板 |
| `src/css/app.css` | `.ai-lang-bar` / `.ai-lang-meta`；`.ai-head-right .inline-select` |

---

## 五、各个需求点的落点

### 需求 1：独立的「AI 讲解语言」下拉（默认中文）

- UI：查词页 AI 面板头部 `#ai-lang` + 设置页 `#set-explain-lang`（两处共享同一份配置，改动互相同步）。
- 它和上方「语言」下拉是**两件事**：上方决定「查哪个语种的词」，这里决定
  「AI 用哪种语言回答」。
- 模型侧用 `explain_lang_name()` 把 `ja` 换成 **「日本語」** 而不是「日语」——
  实测小模型对目标语言**自称**的遵循度明显更高。

### 需求 2：翻译模板自动套用、整体替换原文

`apply_explain_lang()`（`commands/mod.rs`）统一处理：

1. `explain_auto_translate == false` → 原样返回；
2. `text_matches_lang()` 判定已经是目标语言 → 原样返回（**不做无意义的重译**）；
3. 否则 `translate_markdown()`：
   - **保护**：`protect_technical_spans()` 把代码围栏、行内代码、URL、Windows
     路径换成 `⟦0⟧` `⟦1⟧` 占位符（用 U+27E6/U+27E7，避开 Markdown 的 `[]`）；
   - **翻译**：套模板走一次 `chat_ex()`，温度 0.2、`max_tokens` 放宽到 2048；
   - **还原**：`restore_technical_spans()` 把占位符换回原文（容错 `⟦ 0 ⟧`
     这种被模型加了空格的写法，编号越界则原样保留）。
4. 任何失败都**降级为保留原文 + note 提示**，绝不因为一次翻译失败让用户看不到讲解。

### 需求 3：技术内容保持原文

两层都做了：
- 提示词层：`EXPLAIN_LANG_CONSTRAINT` 第 2 条明列「代码标识符、命令行、日志、
  文件路径、URL、配置键名」不翻译；
- 后处理层：占位符机制是**结构化保证**，不依赖模型自觉。

### 需求 4：保留「查看原文」入口

这是把返回值从 `String` 改成 `ExplainResult` 的原因：

```rust
pub struct ExplainResult {
    pub text: String,       // 最终展示（已处理）
    pub original: String,   // 模型原始输出，未翻译
    pub lang: String,       // 本次生效的讲解语言
    pub translated: bool,   // 是否走了兜底翻译
    pub note: Option<String>, // 兜底失败时的提示
}
```

前端 `renderExplain()` 在顶部渲染一条提示条，译文与原文不一致时给出
「查看原文 / 返回译文」开关。

### 需求 5：选择后即时生效 + 记住上次语言

- `cmd_set_explain_lang` 走 `state.update_config()` **立即落盘**；
- 切换后 `retranslateCurrent()` 拿缓存的**模型原文**重译一次，
  **不重新生成**（内容不变、几乎秒回）；
- 下次讲解/追问自动用新语言（提示词层读的就是这份配置）。

### 附带：界面重复控件

1. **两排标题栏按钮** → 主窗口改 `.decorations(false)`，只用自绘标题栏，
   并补齐「最小化 / 最大化 / 关闭」三键 + 双击标题栏最大化。
   （`decorations(false)` 不会移除 `WS_THICKFRAME`，拖拽边缘缩放仍可用。）
2. **「收起右栏」与 AI 面板右上角「›」重复** → 删掉 `lk-ai-collapse`，
   只保留查词页头部那一个按钮。

---

## 六、验证

| 层面 | 命令 | 结果 |
|---|---|---|
| Rust 单测 | `cargo test --lib` | 113 passed / 0 failed |
| 前端冒烟 | `node scripts/smoke_frontend.cjs` | 全绿（含 2 条新增的讲解语言用例） |
| 内嵌一致性 | `node scripts/verify_embedded_assets.cjs` | 需重编译后跑 |
| 打包 | `.\build.ps1` | 出 NSIS / portable 产物 |

新增的关键测试：
- `llm::tests::lang_constraint_is_appended` / `lang_constraint_falls_back`
- `llm::tests::detect_explanation_language`（含中日靠假名区分）
- `llm::tests::technical_spans_roundtrip` / `restore_tolerates_malformed_placeholders`
- `llm::tests::outer_fence_is_stripped` / `translate_template_placeholders`
- `builtin::tests::migrate_v4_replaces_hardcoded_chinese_prompt`
- `builtin::tests::migrate_v4_keeps_custom_prompt`

---

## 七、已知取舍

- **拉丁语言之间不做兜底**：英/法/德/西/葡/意 都是拉丁字母，靠字符分布判不准，
  `text_matches_lang()` 对这些语言一律返回 `true`（不触发兜底）。宁可偶尔漏翻，
  也不能把一段正确的外语讲解反复丢给模型重译。
- **中日区分靠假名**：日语与中文共用汉字，判定日语要求假名占比 ≥ 5%；
  判定中文要求假名/谚文占比 < 5%。
- **兜底翻译会丢失流式效果**：只有模型没听话时才会发生，属可接受代价。
