# WordWise · 可扩展词典源配置指南

> 本文说明如何通过**纯配置**接入新的词典 / 翻译 API，从而支持更多语言，
> 无需修改任何 Rust 或 JavaScript 代码。

---

## 一、核心思路

WordWise 把所有词典源的返回统一归一化成同一个结构：

```rust
WordEntry {
    word,          // 词条
    lang,          // 语言代码
    phonetic: { uk, us, audio },
    senses: [ { pos, definition, examples: [ { text, translation } ] } ],
    inflections:  [ { label, form } ],
    related:      [ "..." ],
    mnemonic,      // 记忆法
    source,
}
```

任何 API 只要能用**一条 URL 模板**请求、并用**一组字段路径**描述其 JSON 结构，
就能接入。转换工作在 `src-tauri/src/dict/mod.rs` 的 `normalize()` 中自动完成。

---

## 二、字段路径语法

`src-tauri/src/dict/jsonpath.rs` 实现了轻量路径查询：

| 语法 | 含义 | 示例 |
|---|---|---|
| `a.b.c` | 逐层向下取值 | `data.entry.word` |
| `a[0]` | 数组下标 | `meanings[0].partOfSpeech` |
| `a[*].b` | 遍历数组并收集字段 | `definitions[*].example` |
| `a[*]` | 展开数组元素本身 | `synonyms[*]` |
| `$` | 根对象 | `$` |
| 空字符串 | 不使用该字段 | `""` |

**取值行为说明**

- 用于**单值字段**（word、phonetic_uk 等）：命中数组时取第一个非空元素。
- 用于**列表字段**（senses、inflections、related）：命中数组时全部收集；
  命中单个标量时包装成单元素列表。
- 路径不存在返回空值，不会报错 —— 因此映射写错只会导致该字段缺失，
  不会让整次查询失败。

---

## 三、示例一：接入一个标准 REST 词典

假设某小语种 API 形如：

```
GET https://dict.example.com/api/v2/lookup?q=bonjour&key=YOUR_KEY
```

返回：

```json
{
  "status": "ok",
  "result": {
    "headword": "bonjour",
    "pronunciation": { "ipa": "bɔ̃.ʒuʁ" },
    "entries": [
      {
        "category": "interj.",
        "meaningZh": "你好；早上好",
        "samples": [
          { "fr": "Bonjour, comment allez-vous ?", "zh": "你好，你好吗？" }
        ]
      }
    ],
    "forms": [ { "kind": "复数", "text": "bonjours" } ],
    "synonyms": ["salut", "coucou"]
  }
}
```

配置如下：

| 配置项 | 填写内容 |
|---|---|
| 名称 | 示例法语词典 |
| 请求地址模板 | `https://dict.example.com/api/v2/lookup?q={word}&key={key}` |
| 请求方法 | GET |
| API Key | `YOUR_KEY` |
| 支持语言 | 勾选「法语」 |

字段映射：

| 映射项 | 路径 |
|---|---|
| 词条 | `result.headword` |
| 英式音标 | `result.pronunciation.ipa` |
| 义项数组 | `result.entries` |
| 义项·词性 | `category` |
| 义项·释义 | `meaningZh` |
| 义项·例句 | `samples[*]` |
| 例句·原文 | `fr` |
| 例句·译文 | `zh` |
| 变形数组 | `result.forms` |
| 变形·类型 | `kind` |
| 变形·形式 | `text` |
| 相关词 | `result.synonyms` |

> 注意 `{key}` 会被替换成 API Key 字段的值。
> 若接口要求把 Key 放在请求头而非 URL，可在自定义源里通过
> `Authorization: Bearer xxx` 形式配置（见第六节）。

---

## 四、示例二：接入需要 POST 的翻译接口

LibreTranslate 风格：

```
POST https://your-libretranslate.example/translate
Content-Type: application/json

{ "q": "hello", "source": "en", "target": "zh", "format": "text" }
```

返回 `{ "translatedText": "你好" }`。

配置：

| 配置项 | 值 |
|---|---|
| 请求方法 | POST (JSON) |
| 请求地址模板 | `https://your-libretranslate.example/translate` |
| 支持的API Key | （若需要） |
| 义项·释义 | `translatedText` |

WordWise 对 POST 会自动构造 `{q, source, target, format, api_key}` 请求体，
`source` 取当前学习语言，`target` 为 `zh`（学习语言为 en 时）或 `en`。

---

## 五、示例三：多语言（小语种）扩展

Wiktionary 内置源就是范例——它通过 `{lang}` 占位符一份配置覆盖 8 种语言：

```
https://{lang}.wiktionary.org/api/rest_v1/page/definition/{word}
```

> 注意：该源标记了 `needs_proxy: true`（中国大陆直连不通），
> 因此在默认配置下是**关闭**的。要启用它，先在「设置 → 网络与代理」里
> 打开「启用代理」，再回到「词典与翻译源」把它打开。

要新增一门语言，有两条路：

### 路线 A：复用已有源，只加语言

1. 进入「设置 → 词典与翻译源」
2. 找到要复用的源（如 Wiktionary）
3. 在「支持的语言」里勾选新语言
4. 保存

前提是该源确实支持这门语言（Wiktionary 覆盖几乎所有语言）。

### 路线 B：新增专用源

1. 点「新增源」
2. 填写名称、URL 模板（用 `{lang}` 表示语言）、字段映射
3. 在「支持的语言」勾选目标语言
4. 点「测试」验证 → 保存

---

## 六、请求头与鉴权

- **API Key 字段**：会自动以 `Authorization: Bearer <key>` 发送，
  同时替换 URL 中的 `{key}` 占位符。
- **自定义请求头**：目前内置源与自定义源均预置了常规 `User-Agent`；
  如需额外请求头（如 `X-API-Key`），可直接编辑 `src-tauri/src/dict/builtin.rs`
  中对应源的 `headers` 字段，或参考该文件新增一个内置源。

```rust
fn headers() -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert("User-Agent".into(), "WordWise/1.0".into());
    m.insert("X-API-Key".into(), "your-key".into());   // ← 追加
    m
}
```

---

## 七、多源聚合与优先级

- 所有**已启用**且**支持当前语言**的源，按 `priority`（小的优先）依次请求。
- 一旦某个源给出了有效释义（`senses` 非空）**且**拿到了音标与例句，
  就提前结束，不再请求后续源。
- 否则结果会**合并**：先到的源提供释义，后续源补全音标、变形、例句、
  相关词等缺失字段（`WordEntry::merge_from`）。
- 全部在线源失败时，若已启用 AI 讲解，则降级为由 LM Studio 生成词条。

因此推荐配置：

| 源 | 优先级 | 作用 |
|---|---|---|
| Free Dictionary（英英释义） | 10 | 英英释义、音标、词形变化、同义词 |
| Wiktionary | 20 | 多语言兜底（**需代理**，默认关闭） |
| 有道（中文释义） | 30 | 中文释义质量高 |
| LibreTranslate | 40 | 纯翻译，需自行配置实例 |

> **默认直连的前提**：应用默认不启用代理，因此**默认启用的源必须国内可直连**。
> 上表里只有 Wiktionary 需要代理，它默认是关闭的 —— 见第九节的 `needs_proxy`。
>
> 这也意味着：内置源一旦失效（对方接口下线、改了返回结构），
> 用户拿到的就是「每次查词都白等一次」。所以新增内置源时必须实测直连可达。

---

## 八、调试技巧

1. **点「测试」按钮**：会用 `hello` 做一次真实请求，
   返回「连接成功，解析出 N 个义项」或具体错误原因。
2. **查看 trace**：查词接口返回的 `trace` 数组记录了每个源的耗时与错误，
   可在浏览器开发者工具的网络/控制台里看到。
3. **先验证 API 本身**：用浏览器或 curl 直接访问 URL 模板（把 `{word}` 换成
   实际单词），确认返回结构，再据此填写路径。
4. **注意 404 语义**：多数词典对未收录的词返回 404，这会被识别为
   「未收录该词」而**不再重试**，是正常行为。
5. **缓存**：查询结果会缓存 7 天。调试时用「强制刷新」或在设置里「清空缓存」。

---

## 九、内置源清单

| id | 名称 | 语言 | 是否需要 Key | 国内可直连 | 默认启用 | 提供内容 |
|---|---|---|---|---|---|---|
| `free-dictionary` | 英英释义 (freedictionaryapi) | en | 否 | ✅ 是 | ✅ | 英英释义、音标、词形变化、同义词、例句 |
| `youdao-suggest` | 有道词典 | en/ja/ko/fr/de/es | 否 | ✅ 是 | ✅ | 中文释义、词性 |
| `wiktionary` | Wiktionary | en/ja/fr/de/es/ru/ko/it | 否 | ❌ 需代理 | ❌ | 多语言释义与例句 |
| `libre-translate` | LibreTranslate | 12 种 | 视实例而定 | 视实例 | ❌ | 纯翻译 |

所有内置源的定义都在 `src-tauri/src/dict/builtin.rs`，
它们本身就走与自定义源完全相同的配置路径——因此它们是**扩展的模板**。

> **已下线的源**：`api.dictionaryapi.dev` 是早期版本的英英释义源，
> 2026-10 实测对**所有**单词都返回 404（含 apple、hello 这类基础词），
> 已换成 `freedictionaryapi.com`。老用户升级后由配置迁移自动替换，
> 见下方「配置迁移」。

### 关于 `needs_proxy`

`DictSourceConfig` 上的 `needs_proxy` 字段：

- `true`：该源在中国大陆必须走代理才能访问（如 `wiktionary.org`）。
- **默认关闭**：应用默认直连（`NetworkConfig::enable_proxy = false`），
  所以这类源在内置定义里就是 `enabled: false`，需要用户显式启用代理后再打开。
- 即使被打开，在没有可用代理时也会被**直接跳过**，并在查询结果的
  `trace`（数据来源明细）里写明「已跳过：该源在国内需走代理」。
- 这么做是因为老实现会串行等待一个必然超时的源，让「查一个词」卡上几十秒。

设置页会在源上显示一个「需代理」标签，配置入口在「设置 → 网络与代理」。

### 配置迁移（`CONFIG_VERSION`）

词典源是**跟配置一起持久化**的，所以只改 `default_sources()` 只能影响新装用户。
为了让「内置源定义修复」能送达老用户，`models.rs` 里有一个 `CONFIG_VERSION`，
`dict::builtin::migrate_config()` 在版本落后时执行**一次性**迁移：

1. 用代码里的最新定义刷新内置源（URL 模板、字段映射、`needs_proxy`、语言、显示名）；
2. 保留用户自己的选择（`enabled` / `api_key` / `priority`）；
3. 保留用户新增的自定义源；
4. 未启用代理时，把 `needs_proxy` 的内置源关掉；
5. 写入新的版本号。

> 之所以要「一次性」而不是每次启动都刷新：设置页**允许用户修改内置源的 URL**，
> 每次都刷会把用户的手工调整冲掉。改动内置源定义时要记得把 `CONFIG_VERSION` 加一。

### 真实词表冒烟测试

`src-tauri/src/dict/importer.rs` 里的 `real_wordlists_smoke` 测试会直接解析
真实的公开词表文件，用于防止「单元测试通过、真实文件解析失败」这种偏差。

```bash
# 1) 下载样本到任意目录
mkdir -p /tmp/ww_samples && cd /tmp/ww_samples
curl -o cet4.txt  https://cdn.jsdelivr.net/gh/mahavivo/english-wordlists@master/CET4_edited.txt
curl -o toefl.txt https://cdn.jsdelivr.net/gh/mahavivo/english-wordlists@master/TOEFL.txt
curl -o ky.txt    "https://cdn.jsdelivr.net/gh/KyleBing/english-vocabulary@master/5%20%E8%80%83%E7%A0%94-%E4%B9%B1%E5%BA%8F.txt"
curl -o coca.txt  https://cdn.jsdelivr.net/gh/mahavivo/english-wordlists@master/COCA_abridged.txt
curl -o oald.txt  https://cdn.jsdelivr.net/gh/mahavivo/english-wordlists@master/OALD8_abridged_edited.txt

# 2) 带上环境变量跑测试
cd src-tauri && WORDWISE_SAMPLES=/tmp/ww_samples cargo test --lib
```

未设置 `WORDWISE_SAMPLES` 时该测试会直接跳过，不影响日常 `cargo test`。

---

## 十、在线词库下载与镜像

「词库 → 在线词库」里的下载项全部托管在 GitHub。`raw.githubusercontent.com`
在国内直连必然失败，因此 `remote_catalog()` 给每个词库都配了**镜像链**：

| 顺序 | 镜像 | 说明 |
|---|---|---|
| 1 | `cdn.jsdelivr.net/gh/<repo>@master/<file>` | jsDelivr 主后端，国内可直连 |
| 2 | `fastly.jsdelivr.net/gh/…` | jsDelivr 的 **Fastly** 后端（独立 CDN） |
| 3 | `gcore.jsdelivr.net/gh/…` | jsDelivr 的 **Gcore** 后端（独立 CDN） |
| 4 | `gh-proxy.com/https://raw.githubusercontent.com/…` | 完整透传，无大小限制；**直连时基本不可用**，但对开了代理的用户有效 |
| 5 | `raw.githubusercontent.com/…` | 原始地址，需要代理才能访问 |

前三条是关键：jsDelivr 的这几个 CNAME 由**三家不同的 CDN 供应商**提供，
互为独立故障域，一条被干扰另外两条通常仍正常。

> **实测（2026-10，直连）**：`fastly.jsdelivr.net` 3/3 通过、`cdn.jsdelivr.net` 2/3、
> `gcore.jsdelivr.net` 2/3；而所有走 raw 透传的第三方代理
> （`gh-proxy.com` / `ghproxy.net` / `ghfast.top` / `hub.gitmirror.com`）
> 以及 `cdn.statically.io` / `raw.githack.com` **全部 0/3 失败** ——
> 它们自己也要去 fetch 被墙的 raw，于是把失败一起继承了过来。
> 所以「多加几个第三方代理」并不能提高可用性，**换到独立的分发网络才行**。

失败还被分为两类，处理方式不同：

- **瞬时失败**（连接超时、TLS 握手卡死）：镜像链会**整体再扫一轮**。
  实测 `cdn.jsdelivr.net` 约每 6 次会有 1 次 TCP 连接被静默丢弃，只扫一轮的话
  「恰好三个镜像同时踩上瞬时故障」的概率并不低，用户看到的就是
  「点了下载就报错」——而这正是最初报的「网络问题不能下载」。
- **明确性失败**（HTTP 4xx）：地址本身有问题，该镜像会被标记并跳过，不再重试。

两轮扫描外层有 **90 秒总预算**兜底，避免「镜像数 × 轮次 × 连接超时」叠成分钟级干等。

`cmd_download_book` 全部失败时会把**每个镜像各自的失败原因**一起返回给界面，
而不是只丢一句「网络错误」。

> 注意：源仓库里并不存在 `KAOYAN_edited.txt` / `IELTS_edited.txt` /
> `TOEFL_edited.txt` / `GRE_edited.txt` / `GAOKAO_edited.txt`（都是 404）。
> 目录里的文件名必须先在源仓库确认真实存在。

### 目录现状（2026-10 补录后）

| 语言 | 来源 | 分支 | 许可 | 收录内容 |
|---|---|---|---|---|
| 英语 | `mahavivo/english-wordlists` | master | MIT | 四六级、考研（NPEE）、托福、GRE（8000 / 精简 / 红宝书）、高考、牛津、COCA、专四八级、小学、台湾高中 |
| 英语 | `KyleBing/english-vocabulary` | master | 见源仓库 | 初中 / 高中 / 考研 / 四级 / 六级 / **雅思** / SAT（乱序带释义） |
| 日语 | `evanclan/OpenJLPT` | **main** | CC-BY-SA-4.0 | JLPT N5 / N4 / N3 / N2 / N1（`data/json/vocab/n*.json`） |

两个易踩的坑：

1. **分支写错**：OpenJLPT 的默认分支是 `main`，照抄 `@master` 会整片 404。
   目录里因此把分支写进仓库常量（`owner/repo@branch`），由
   `catalog_declares_language_and_has_japanese` 测试守住。
2. **语言串味**：下载时按目录里标注的 `lang` 入库，**不是**按用户当前在学的语言。
   否则在学英语时下载日语词库，几千个日语词会被标成 `en`，之后搜不到也背不到。

---

## 十一、网络与代理：默认直连

**一条硬前提：默认配置不启用代理，并且所有默认开启的功能都必须能在无代理环境下正常工作。**

### 规则

| 规则 | 位置 | 说明 |
|---|---|---|
| `enable_proxy` 默认 `false` | `models.rs` | 总开关。关闭时**不读环境变量、不读注册表**，一律直连 |
| 总开关最先判断 | `net.rs::resolve_proxy_inner` | 必须放在第一道，否则残留的 `HTTP_PROXY` 会让「默认直连」失效 |
| 直连也要 `no_proxy()` | `net.rs::build_client_with` | 启用了 `system-proxy` 特性，不显式关掉的话 reqwest 仍会自己探测 |
| 默认启用的源必须国内可直连 | `dict/builtin.rs` | 由 `default_sources_need_no_proxy` 测试守住 |
| 默认搜索引擎必须国内可直连 | `models.rs::AppConfig` | 默认 `bing`，走 `cn.bing.com` 的 RSS |
| 词库主镜像必须国内可直连 | `dict/importer.rs::remote_catalog` | jsDelivr 排第一，raw 只作最后兜底 |
| 本机地址永远直连 | `net.rs::build_no_proxy` | LM Studio 的 `127.0.0.1:1234` 绝不能进代理 |

### 验证当前环境是否真的能直连

```bash
cd src-tauri

# 单元测试：默认必须解析为直连，且忽略 HTTP_PROXY
cargo test --lib net::tests

# 真实网络端到端（会实际请求各条链路）
WORDWISE_LIVE_TEST=1 cargo test --lib live_network_smoke -- --nocapture
```

`live_network_smoke` 刻意用**默认配置**（= 直连）跑，覆盖：词库镜像、
必应 RSS、有道、英英释义源。它在有 `HTTP_PROXY` 环境下也必须通过 ——
这恰好验证了「环境里有代理变量，程序照样直连」这条要求。

### 曾经踩过的坑

- **`curl` 在 Windows 上默认会读系统代理**（`-v` 里会出现
  `HTTP/1.1 200 Connection Established`）。排查「到底能不能直连」时必须加
  `--noproxy '*'`，否则测出来的是代理的结果，结论会完全跑偏。
- **`default-features = false` 会连带关掉 reqwest 的 `system-proxy`**：
  启用了 `system-proxy` 就必须在直连分支显式 `.no_proxy()`。
- **不要写「需要代理的源」当默认开启**：`needs_proxy` 的源即使被启用，
  在无代理时也会被跳过，留在默认列表里只会产生噪声。
