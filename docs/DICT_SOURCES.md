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
| 有道（中文释义） | 30 | 中文释义质量高 |
| Free Dictionary | 10 | 英英释义、音标、变形、同义词 |
| Wiktionary | 20 | 多语言兜底 |
| LibreTranslate | 40 | 纯翻译，需自行配置实例 |

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

| id | 名称 | 语言 | 是否需要 Key | 提供内容 |
|---|---|---|---|---|
| `free-dictionary` | Free Dictionary (dictionaryapi.dev) | en | 否 | 英英释义、音标、变形、同义词、例句 |
| `wiktionary` | Wiktionary | en/ja/fr/de/es/ru/ko/it | 否 | 多语言释义与例句 |
| `youdao-suggest` | 有道词典 | en/ja/ko/fr/de/es | 否 | 中文释义、词性 |
| `libre-translate` | LibreTranslate | 12 种 | 视实例而定 | 纯翻译（默认关闭） |

所有内置源的定义都在 `src-tauri/src/dict/builtin.rs`，
它们本身就走与自定义源完全相同的配置路径——因此它们是**扩展的模板**。
