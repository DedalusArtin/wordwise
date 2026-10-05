# 翻译 / 查词：互译方向与语言链路改造（v0.35）

> 目标：修掉「选了日语只拿到罗马音」「选日语却返回英语」「顶部语言控件
> 与下方三个搜索面板各说各话」，并新增独立的「翻译」栏目。

---

## 一、五条需求各自对应什么改动

| # | 需求 | 落点 |
|---|---|---|
| 1 | 顶部改成「源 → 目标」方向选择器，支持一键互换 | 新增 `src/js/dir.js` + `cmd_swap_direction`；查词页/翻译页共用同一份配置 |
| 2 | 必须返回目标语言实际文字，读音只能单独一行 | 新增 `src-tauri/src/translate/mod.rs`，把 `text` 与 `phonetic` 严格分开；前端 `.tr-phonetic` / `.lk-trans-phonetic` 独立成行 |
| 3 | 语言码全链路一致 | 统一语言码表 `translate::LANGS`，`youdao_code()` 单点转换；`auto` 在**进入接口前**就被解析成真实语言码 |
| 4 | 三面板联动 | `DirPicker.onChange()` 广播；`Lookup` 三个面板各自订阅并刷新占位提示 / 请求语言 / 译文 |
| 5 | 新增独立「翻译」栏目 | 新增 `#page-translate` + `src/js/translate.js`，左右分栏、实时翻译、复制/朗读/收藏/历史 + 4 项 AI 增强 |

---

## 二、需求 2 的根因：读音被当成了译文

老链路把有道返回的 `basic.phonetic` 当成「释义主体」渲染。而对中文词查询时，
`basic.phonetic` 是**拼音**；对日语目标，它常常是**假名注音 / 罗马音**。于是
「你好 → 日语」看到的是 `nǐ hǎo`，而不是 `こんにちは`。

修法是在数据结构层面把二者切开，`TranslateResult` 里：

```rust
pub struct TranslateResult {
    pub text: String,          // 目标语言的**实际文字**：こんにちは
    pub phonetic: Option<String>, // 附属读音，只能单独一行：konnichiwa
    pub alternatives: Vec<String>,
    pub from: String, pub to: String,
    pub tts_url: Option<String>,
    pub engine: String,        // youdao / llm / cache / none
    pub note: Option<String>,  // 降级原因
    pub record_id: Option<i64>,
    ...
}
```

前端渲染顺序固定为「大字正文 → 读音小字一行 → 其他译法 → 降级提示」，
读音**永远不会**出现在 `.tr-dst` / `.lk-trans-text` 里。冒烟测试里有一条
专门断言「译文主体里不得出现罗马音」。

---

## 三、需求 3：语言码的四个卡点

整条链路是 `UI 选择 → 查询参数 → 接口 → 渲染`，有四个地方会跑偏：

1. **中文必须写 `zh-CHS`。**
   有道用 `zh` 会被判成未知语言。`translate::youdao_code()` 是唯一的转换点，
   `zh / zh-CN / zh-Hans` 一律收敛到 `zh-CHS`。

2. **该接口不支持 `auto`。**
   直接传 `auto` 返回 `errorCode: 411`。所以源语言为 `auto` 时，先跑
   `dict::detect_lang()`（假名→ja、谚文→ko、汉字→zh、西里尔→ru…；
   拉丁字母无法区分英/法/德，返回 `None`）猜一个真实码，猜不出才回落 `en`。

3. **返回值里的方向要回填。**
   有道返回 `l` 形如 `zh-CHS2ja`。`from_youdao_code()` 取 `2` 左边并还原成
   `zh`，写回 `TranslateResult.from`。这样「自动检测」也能告诉用户
   「实际按日语翻的」。

4. **源语言 = 目标语言直接短路。**
   `cmd_translate` 里 `from == to` 时返回原文 + `note`，不再浪费一次网络请求。
   前端 `DirPicker.set()` 还会主动把撞车的目标语言挪开（`zh→en`，否则 `en→zh`）。

---

## 四、翻译引擎：为什么是三级

```
1. 本地缓存（SQLite trans_cache，TTL 30 天）  —— 零网络，命中即返回
2. 有道在线 aidemo.youdao.com/trans          —— 快且准，但**限频很严**
3. 本地大模型                                 —— 限频/离线时兜底
```

有道这个公开接口实测**连打几次就返回 411**，约 12 秒后才恢复。所以：

- **缓存**：翻译页「实时翻译」会反复请求同一句话的前缀，必须挡住；
- **节流**：进程内全局 `THROTTLE`，最小调用间隔 1200ms，撞上限频后冷却 20s；
- **兜底**：冷却期内直接走本地大模型，用户看到的是
  「触发频率限制，已改用本地模型翻译」而不是「翻译失败」。

> 实现上的一条纪律：**只发一次有道请求**。失败原因要在这一次的结果里取，
> 绝不能「为了拿错误信息再请求一遍」——那只会更快撞上 411。

---

## 五、需求 5：翻译页的 AI 增强

`cmd_translate_ai(action, ...)` 支持四种动作：

| action | 作用 | 输出语言 |
|---|---|---|
| `explain` | AI 释义与语境说明，顺带评价译文是否地道 | `explain_lang`（默认中文） |
| `variants` | 直译 / 口语 / 书面三个版本 | 目标语言 |
| `polish` | 把译文改得更地道 | 目标语言 |
| `examples` | 用目标语言造句并附译文 | 目标语言 + 对照 |

注意 `explain` 走的是 `explain_lang` 而不是目标语言：**学日语时，解释该用
母语说**。这一层复用了第十三轮的 `llm::with_lang_constraint()`。

---

## 六、配置项与迁移

`AppConfig`（`src-tauri/src/models.rs`）新增/复用的字段：

```rust
pub source_lang: String,   // 默认 "auto"，见 default_source_lang()
pub target_lang: String,   // 默认 "en"
pub search_engine: String, // 在线搜索方式，与方向同属一个命令维护
```

`CONFIG_VERSION` 从 4 升到 **5**。v5 迁移（`dict::builtin::migrate_config`）
只做一件事：老配置里 `source_lang` 是被清空的空串（当时语言下拉已被合并），
补成 `translate::AUTO`。迁移只在版本落后时跑一次，不会覆盖用户自己选的方向。

数据库新增两张表（`db::migrate`）：

```sql
trans_history(id, source_lang, target_lang, src_text, dst_text,
              engine, favorite, created_at,
              UNIQUE(src_text, source_lang, target_lang))
trans_cache(key PRIMARY KEY, json, updated_at)
```

同方向同原文只占一行（`upsert_translation` 用 `ON CONFLICT` 更新译文与时间），
「清空历史」默认保留收藏。

---

## 七、改动清单

**后端**

| 文件 | 改动 |
|---|---|
| `src-tauri/src/translate/mod.rs` | **新增**：语言码表、`youdao_code`/`from_youdao_code`、限频保护、`youdao_translate`、`llm_translate`、5 条测试 |
| `src-tauri/src/commands/translate.rs` | **新增**：9 个命令（翻译 / AI 增强 / 历史 / 收藏 / 删除 / 清空 / 互换 / 语言表 / 状态） |
| `src-tauri/src/db/mod.rs` | 两张表 + 7 个方法 + 5 条测试 |
| `src-tauri/src/models.rs` | `TransRecord`、`default_source_lang()`、`CONFIG_VERSION = 5` |
| `src-tauri/src/dict/builtin.rs` | v5 迁移分支 |
| `src-tauri/src/dict/mod.rs` | `LookupResult.lang_note`（检测语言与所选冲突时的提示） |
| `src-tauri/src/commands/mod.rs` | `cmd_lookup` 的语言判定重写：显式 lang > 配置 `source_lang` > `detect_lang`；冲突时**以检测为准**并生成 `lang_note` |
| `src-tauri/src/llm/mod.rs` | 修「把 `reasoning_content` 当正文」的 bug；`strip_thinking_tags`；`TRANSLATE_SYSTEM`；`looks_like_rule_echo` 闸门 |
| `src-tauri/src/windows.rs` | 注册 9 个新命令 |

**前端**

| 文件 | 改动 |
|---|---|
| `src/js/dir.js` | **新增**：方向选择器组件（`[data-dir-picker]` 约定、`set`/`swap`/`onChange`） |
| `src/js/translate.js` | **新增**：翻译页逻辑（防抖 600ms + 请求序号、渲染、朗读/复制/收藏、AI 增强、历史） |
| `src/js/lookup.js` | 去掉旧单下拉；`syncLookupPlaceholder()`；`loadWordTranslation()` 词级译文；订阅方向变化 |
| `src/js/api.js` | 9 个新 API + 调试模式 mock（含「你好→こんにちは」词典与历史落库） |
| `src/js/ui.js` | `renderPlainText()`；音标标签按 `entry.lang` 分派（中文=拼音、日文=读音、韩文=罗马音） |
| `src/js/app.js` | `loaders.translate`；启动前 `DirPicker.loadLangs() + bind()` |
| `src/index.html` | 导航加「🌐 翻译」；查词页头部换方向选择器；新增 `#page-translate` |
| `src/css/app.css` | `.dir-picker` / `.lk-trans*` / `.tr-*` 三组样式 + 窄屏堆叠 |

---

## 八、为什么前端组件要写成 `[data-*]` 约定而不是 id

同一个页面里可能有多个方向选择器（查词页一个、翻译页一个），用 id 就只能
有一个。`DirPicker` 用 `document.querySelectorAll('[data-dir-picker]')` 找所有
实例，统一 `render()` 成同一个方向，并用 `_dirBound` 打标防重复绑定。
任意一处改动 → `persist()` 落盘 → `emit()` 广播 → 所有订阅方刷新。

---

## 九、验证

```bash
cd src-tauri && cargo test --lib        # 127 passed / 0 failed
node scripts/smoke_frontend.cjs         # 57 项，含本轮新增的 16 项语言链路用例
node scripts/verify_embedded_assets.cjs # 构建后校验内嵌前端与源码一致
```

冒烟测试新增的关键用例：

- 方向选择器：源=目标自动挪开、目标不能是 auto、⇄ 互换落盘、广播可达；
- 翻译：选日语必须得到 `こんにちは` 且罗马音不得混进主体；
- 语言链路：UI → 参数 → 返回 → 渲染全程同一个语言码；`auto` 要回报真实检测结果；
- 查词页：占位提示跟方向走；词级译文独立渲染；同语言时不浪费请求；
- 音标标签：中文词条标「拼音」而不是「英」。

> 构建提醒：前端是 brotli 内嵌进 exe 的（`frontendDist: "../src"`），
> **改完前端必须重跑 `build.ps1`**，否则 `verify_embedded_assets` 会报不一致。
