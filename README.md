# WordWise

> 基于本地 LM Studio 大模型能力的**可视化背单词 + AI 讲解**一体化桌面软件

<p align="left">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-1.77%2B-orange?logo=rust">
  <img alt="Tauri" src="https://img.shields.io/badge/Tauri-v2-24C8DB?logo=tauri">
  <img alt="Platform" src="https://img.shields.io/badge/Platform-Windows%2010%2F11-blue">
  <img alt="License" src="https://img.shields.io/badge/License-MIT-green">
</p>

WordWise 把「背单词」和「AI 讲解」放进同一个窗口：左边是干净的词条，
右边是本地大模型逐字写出的讲解；答错的词会自己钻进错词本，
并按遗忘曲线准时回到你眼前。

整个前端仅用原生 HTML/CSS/JS（无 node_modules），后端 Rust + Tauri，
安装包不到 10 MB，启动即用。

---

## 目录

- [功能总览](#功能总览)
- [界面说明](#界面说明)
- [快速开始](#快速开始)
- [功能详解](#功能详解)
- [技术架构](#技术架构)
- [项目结构](#项目结构)
- [构建与打包](#构建与打包)
- [常见问题](#常见问题)

---

## 功能总览

| # | 需求 | 实现情况 |
|---|---|---|
| 1 | **双向背诵模式** | 看英文选中文 / 看中文选英文，一键切换，键盘 A-D 可选答 |
| 2 | **内置搜索引擎 + 联网查词** | 4 个公开词典源并行聚合 + 输入联想 + 维基百科知识；断网时由本地大模型兜底生成 |
| 3 | **记忆辅助内容可选开关** | 音标 / 变形 / 例句 / 相关词 / 词根记忆法 / 答错弹卡 / AI 讲解，共 7 个独立开关，实时联动 |
| 4 | **遵循遗忘规律动态排程** | SM-2 改良算法 + 可配置记忆周期表（8 档间隔）；读取系统时间计算下次复习点，逾期优先、常错优先 |
| 5 | **常错词自动强化 + 详情卡** | 错误率超阈值自动进入强化队列；答错弹出仿有道词典的完整详情卡（5 个标签页） |
| 6 | **小语种可扩展** | 词典源 = URL 模板 + JSON 字段路径映射，纯配置接入新语言，内置源本身即是范例 |
| 7 | **可长期挂起的侧边栏** | 独立置顶窗口，托盘一键唤出，支持查词与单题快测；关闭主窗口后仍常驻 |
| 8 | **打包与自动上传** | Inno Setup 中文安装程序（可选保留学习数据）；`scripts/upload_github.py` 读取 hermes 中已保存的 token 自动建仓推送 |

### 额外增强

- **AI 流式讲解**：打字机效果逐字输出，支持追问（「这个词和 rely 有什么区别？」）
- **词条详情卡**：释义 / 变形 / 例句 / 相关词 / AI 讲解 五个标签页，仿有道词典视觉
- **学习统计**：14 天答题柱状图、掌握度分布、连续学习天数、正确率
- **复习计划视图**：未来 14 天到期量可视化 + 记忆周期表图示
- **示例词库**：首次启动自动载入 20 个高频核心词（含音标、释义、变形、例句、记忆法），开箱即用
- **数据导入导出**：一键备份为 JSON，支持跨设备迁移
- **系统托盘**：打开主界面 / 显示侧边栏 / 开始复习 / 退出
- **浏览器 Mock 模式**：前端可脱离 Tauri 单独调试
- **词典源自测**：配置面板内置「测试连接」，逐源定位问题
- **快捷键**：`Ctrl+1~7` 切换页面，`A~D`/`1~4` 选答，`Esc` 关弹层

---

## 界面说明

### 主窗口

```
┌──────────┬──────────────────────────────────────────────┐
│  导航栏   │  背诵 · 查词 · 词库 · 错词本 · 复习计划 ...    │
│          │                                              │
│  WordWise │   ┌────────┐ ┌────────┐ ┌────────┐          │
│  背诵     │   │ 待复习  │ │ 强化记忆│ │ 已掌握  │  ...      │
│  查词     │   └────────┘ └────────┘ └────────┘          │
│  词库     │                                              │
│  错词本 ③ │   ┌──────────────────────────────────┐      │
│  复习计划 │   │  题目 + 选项 + 即时反馈           │      │
│  学习统计 │   └──────────────────────────────────┘      │
│  设置     │                                              │
│          │                                              │
│ ● 模型就绪│                                              │
└──────────┴──────────────────────────────────────────────┘
```

### 侧边栏

独立的窄条窗口（默认 380×620），置顶、无边框、不占任务栏：

```
┌──────────────────────┐
│ 查词侧边栏    ▣   ✕  │
├──────────────────────┤
│ [输入单词…]   [查词]  │
│ (联想候选 chips)      │
├──────────────────────┤
│ abandon              │
│ 英 /əˈbæn.dən/       │
│ ── 释义 │ 变形 │ AI ─│
│ v. 放弃；抛弃         │
│ He abandoned his car.│
│ 他把车丢弃在雪地里。   │
├──────────────────────┤
│ [快速背词] [AI 讲解] │
└──────────────────────┘
```

---

## 快速开始

### 1. 安装

下载 `WordWise-Setup-0.35.0.exe` 运行即可（安装向导支持中英文）。

若从源码构建，见 [构建与打包](#构建与打包)。

### 2. 直接开始背

首次启动会自动载入 **20 个示例单词**，点「开始背诵」就能用。

要导入自己的词库：**词库 → 批量导入**，粘贴单词列表（每行一个），
勾选「导入后立即联网补全释义」即可。

### 3. 配置 AI 讲解（可选但推荐）

WordWise 的 AI 讲解由**本机 LM Studio** 提供：

1. 打开 LM Studio → **Search** 标签 → 下载模型
   （推荐 `Qwen3-8B` 或 `DeepSeek-R1-Distill-Qwen-14B`，中文讲解质量好）
2. 切到 **Chat** 标签 → 加载该模型
3. 打开 **Developer** 标签 → 点 **Start Server**（默认 `http://127.0.0.1:1234`）
4. 回到 WordWise → 点左下角状态条检测连接 → 显示「本地模型已就绪」

> 未配置 LM Studio 时，在线查词完全可用，只是没有 AI 讲解和离线兜底。

### 4. 唤出侧边栏

点左下角「打开侧边栏」，或**左键单击托盘图标**即可显示/隐藏。

关闭主窗口不会退出程序，而是退到托盘并显示侧边栏 —— 方便随时查词。

---

## 功能详解

### 双向背诵

点「看英选中」或「看中选英」切换模式。

- **看英选中**：显示英文单词 + 音标，四选一选中文释义
- **看中选英**：显示中文释义，四选一选英文单词

干扰项从当前语言词库中随机抽取，因此词库越大，题目质量越好。

答完即时反馈：答对显示绿色对勾；答错标红并显示正确答案，
同时提示「该词已加入强化记忆」。

### 记忆调度（需求 4）

WordWise 使用 **SM-2 改良算法**，核心是两条：

**① 记忆周期表**（可在设置中查看可视化）

```
第1次    第2次    第3次    第4次    第5次    第6次    第7次    第8次
 1天   →  2天   →  4天   →  7天   →  15天  →  30天  →  90天  → 180天
```

连续答对就沿表前进一格；答错则回到起点，并按 `lapse` 处理（半天后重现）。

**② 难度系数 EF（Ease Factor）**

- 初始 2.5，范围 [1.3, 3.2]
- 答「很简单」+0.1，答「有点难」-0.1，答错 -0.2
- 实际间隔 = 周期表基础值 × (EF / 2.5)

所以同一个词对「记得牢的人」和「总记不住的人」会走出不同的复习节奏。

**出题顺序**：

1. 强化记忆词（常错词）优先
2. 已逾期的词，逾期越久越优先
3. 数量不够时补「熟练度最低」的词
4. 还不够则引入词库中未学过的新词

**记忆强度**用指数遗忘曲线计算：`R = e^(-t/S)`，其中 S 为稳定性
（间隔 × 难度系数）。详情卡底部会显示这个值。

### 常错词强化（需求 5）

满足以下条件自动标记为「强化记忆」：

- 作答次数 ≥ 3
- 错误率 ≥ 50%
- 尚未掌握

强化词的复习间隔会被压缩到最多 1 天，并在出题队列中**排最前面**。

答错时（可在设置中关闭）自动弹出**完整详情卡**，含 5 个标签页：

| 标签页 | 内容 |
|---|---|
| 释义 | 分义项列出词性 + 中文释义 + 例句译文 |
| 变形 | 过去式 / 复数 / 比较级等，卡片式展示 |
| 例句 | 汇总所有义项下的例句 |
| 相关词 | 同义词 / 近义词，点击可直接跳转查询 |
| AI 讲解 | 本地大模型逐字流式讲解 |

卡片底部显示该词的学习状态：熟练度、正确率、下次复习时间。

### 联网查词（需求 2）

查询链路的优先级：

```
本地词库  →  词典缓存（7天）  →  在线多源聚合  →  本地大模型兜底
```

**在线多源聚合**：按优先级依次请求，一旦拿到「释义 + 音标 + 例句」就提前返回；
否则把各源结果**合并**（一个源给释义，另一个补变形和例句）。

内置 4 个源：

| 源 | 提供 | 需要 Key |
|---|---|---|
| Free Dictionary (dictionaryapi.dev) | 英英释义、音标、变形、同义词、例句 | 否 |
| Wiktionary | 多语言释义与例句 | 否 |
| 有道词典（公开建议接口） | 中文释义、词性 | 否 |
| LibreTranslate | 纯翻译（默认关闭） | 视实例 |

**输入联想**同时请求有道与必应两个建议接口，谁先返回谁上榜。

**维基百科知识**：查词时可附带该词的百科摘要（`cmd_search` 接口，前端已接好）。

### 小语种扩展（需求 6）

这是设计上最花心思的部分：**词典源 = 配置，不是代码**。

每个源由三部分组成：

```jsonc
{
  "id": "custom-1",
  "name": "我的法语词典",
  "enabled": true,
  "langs": ["fr"],                                    // 支持哪些语言
  "url_template": "https://api.x.com/dict?q={word}",  // {word} {lang} {key}
  "method": "GET",
  "api_key": "",
  "mapping": {                                        // JSON 字段路径映射
    "word": "result.headword",
    "phonetic_uk": "result.pronunciation.ipa",
    "senses": "result.entries",
    "sense_pos": "category",
    "sense_def": "meaningZh",
    "sense_examples": "samples[*]",
    "example_text": "fr",
    "example_translation": "zh"
  }
}
```

路径语法支持 `a.b.c`（逐层）、`a[0].b`（下标）、`a[*].b`（遍历收集）、`$`（根）。

因此**新增一门语言 = 加一条配置**。内置源（`dict/builtin.rs`）本身就用
同一套机制实现，是现成的模板。

完整指南见 **[docs/DICT_SOURCES.md](docs/DICT_SOURCES.md)**。

### 侧边栏（需求 7）

- 独立 Tauri 窗口，`always_on_top`，`decorations: false`，`skip_taskbar: true`
- 关闭时拦截 `CloseRequested` 改为 `hide()`，保证常驻
- 首次显示自动贴屏幕右侧垂直居中
- 打开时若词库非空，自动出一道快测题
- 主窗口关闭时自动显示侧边栏，服务不中断

---

## 技术架构

### 技术选型

| 层 | 技术 | 理由 |
|---|---|---|
| 界面 | 原生 HTML/CSS/JS | 零构建步骤，无 node_modules，改完即生效 |
| 外壳 | Tauri v2 | 复用系统 WebView2，安装包小（~8 MB） |
| 后端 | Rust 1.77+ | 性能与内存安全，单二进制分发 |
| 存储 | SQLite (rusqlite bundled) | 无需外部依赖，单文件，随包分发 |
| 网络 | reqwest + rustls | 不依赖系统 schannel，规避证书吊销列表离线问题 |
| 打包 | Inno Setup 7 | 中文安装向导、自定义任务、卸载保留数据 |

### 分层设计

```
┌─────────────────────────────────────────────────┐
│  前端  src/                                      │
│  app.js(路由) study.js(背诵) lookup.js(查词+AI)  │
│  library.js  settings.js  sidebar.js  ui.js      │
│                      ↓ api.js (invoke/listen)    │
├─────────────────────────────────────────────────┤
│  命令层  commands/mod.rs  （40+ 个 #[tauri::command]）│
├─────────────────────────────────────────────────┤
│  业务层                                          │
│   srs/     记忆调度引擎 (SM-2 + 遗忘曲线)         │
│   dict/    多源聚合 + JSON路径映射 + 归一化        │
│   llm/     LM Studio 客户端 (含 SSE 流式)         │
│   search/  联想 + 维基知识                        │
├─────────────────────────────────────────────────┤
│  基础层                                          │
│   db/ (SQLite)   net/ (HTTP)   state/ (共享状态)  │
│   timeutil/ (系统时间)   models/ (数据模型)        │
└─────────────────────────────────────────────────┘
```

### 数据库表

| 表 | 用途 |
|---|---|
| `words` | 词库，存归一化后的 `WordEntry` JSON |
| `study_state` | 每词的学习状态（EF、间隔、到期时间、对错次数、熟练度） |
| `review_log` | 每次作答的日志（用于统计与遗忘曲线校准） |
| `config` | 应用配置（JSON） |
| `dict_sources` | 自定义词典源 |
| `dict_cache` | 词条缓存（避免重复联网） |
| `search_log` | 搜索历史 |

### 关键实现细节

- **时间**：所有时间戳统一走 `timeutil::now_ts()`（系统本地时钟），
  自然日边界按本地时区计算，符合用户对「今天到期」的直觉。
- **并发**：查词时多个 HTTP 请求并行（`tokio::join!` / `buffer_unordered`）；
  数据库用 `parking_lot::Mutex` 包裹 `Connection`，短临界区，不跨 await 持锁。
- **流式**：LM Studio 的 SSE 响应逐行解析 `data:` 负载，
  通过 Tauri 事件 `explain://delta` 推给前端，实现打字机效果。
- **容错**：模型输出的 JSON 会自动剥离 ```json 代码块包裹；
  义项释义为数组时会用「；」拼接；推理模型的 `reasoning_content` 也会被接收。

---

## 项目结构

```
wordwise/
├── README.md                    本文档
├── BUILD.md                     构建与运行指南
├── LICENSE                      MIT
├── build.ps1 / 构建.ps1          一键构建脚本（旧版 build.sh 等已移入 archive/）
│
├── src/                         前端（无构建步骤）
│   ├── index.html               主界面 + 侧边栏共用入口（按 ?view=sidebar 分流）
│   ├── css/app.css              全部样式（浅色主题，仿有道词典）
│   └── js/
│       ├── api.js               命令封装 + 事件订阅 + 浏览器 Mock
│       ├── ui.js                词条渲染、轻量 Markdown、图表、提示
│       ├── study.js             背诵引擎（双向、判分、进度）
│       ├── lookup.js            查词页 + 详情卡弹层 + AI 流式讲解
│       ├── library.js           词库 / 错词本 / 复习计划 / 统计
│       ├── settings.js          设置面板 + 词典源配置 UI
│       ├── sidebar.js           侧边栏窗口逻辑
│       └── app.js               页面路由与初始化
│
├── src-tauri/                   Rust 后端
│   ├── Cargo.toml
│   ├── build.rs
│   ├── tauri.conf.json          窗口/打包配置
│   ├── capabilities/default.json 权限声明
│   ├── icons/                   图标（含多尺寸 .ico）
│   └── src/
│       ├── main.rs              入口
│       ├── lib.rs               模块声明
│       ├── windows.rs           主窗/侧边栏/托盘管理
│       ├── state.rs             全局共享状态
│       ├── models.rs            数据模型
│       ├── timeutil.rs          系统时间（含单元测试）
│       ├── net.rs               HTTP 基础设施（重试、中文错误）
│       ├── seed.rs              内置示例词库
│       ├── srs/mod.rs           记忆调度引擎（含单元测试）
│       ├── db/mod.rs            SQLite 存储层
│       ├── dict/
│       │   ├── mod.rs           多源聚合、归一化、缓存
│       │   ├── builtin.rs       4 个内置词典源
│       │   └── jsonpath.rs      通用 JSON 路径映射（含单元测试）
│       ├── llm/mod.rs           LM Studio 客户端（流式 + 词条生成）
│       ├── search/mod.rs        联想 + 维基知识
│       └── commands/mod.rs      全部 Tauri 命令
│
├── installer/
│   └── wordwise.iss             Inno Setup 打包脚本
│
├── scripts/
│   └── upload_github.py         自动上传到 GitHub
│
└── docs/
    ├── DICT_SOURCES.md          词典源扩展指南
    └── ARCHITECTURE.md          架构说明
```

---

## 构建与打包

### 环境要求

Rust 1.77+ · MSVC 2019/2022/2026 · Windows SDK · WebView2（Win10 1803+ 内置）

### 构建

```bash
# 开发运行
cd src-tauri && cargo run

# 发布构建
cd src-tauri && cargo build --release
# 产物：src-tauri/target/release/wordwise.exe
```

### 打包安装程序

```bash
# 一键（PowerShell）
.\build.ps1

# 交互式菜单（双击运行即可）
.\构建.ps1

# 或直接用 Inno Setup 编译（ISCC.exe 不在 PATH，必须写完整路径）
"G:\Programming\07-utils\Inno Setup 7\ISCC.exe" installer\wordwise.iss
# 产物：installer\output\WordWise-Setup-0.35.0.exe
```

> ISCC.exe 未加入系统 PATH。装在其他位置时：
> `.\build.ps1 -IsccPath "<你的路径>\ISCC.exe"`，或改 `build.ps1` 的 `$DefaultIscc`。

安装程序特性：中英文双语向导 · 自定义安装目录 · 桌面/快速启动图标 ·
开机自启（侧边栏）· 安装后引导配置 LM Studio · **卸载时可选保留学习数据**。

### 便携版（免安装）

前端资源已内嵌进 exe，`wordwise.exe` 是**单文件绿色版**，拷到任何地方双击即可运行，
不写注册表、不往系统目录拷文件。

```
dist\wordwise.exe            直接双击运行
installer\output\WordWise-0.35.0-portable.zip    便携版压缩包（解压即用）
```

运行一次 `.\build.ps1` 会同时产出**便携版**和**安装程序**两种形式。

- 系统要求：Windows 10/11（x64）+ WebView2 运行时（多数系统已自带）
- 学习数据默认放 `%APPDATA%\WordWise`；设 `WORDWISE_DATA_DIR` 可改到别处
  （例如放 U 盘随身携带）
- 删除目录即卸载；学习数据不在目录内，会保留

### 上传到 GitHub

```bash
python scripts/upload_github.py --public
```

脚本会按以下顺序自动寻找 token：

1. 环境变量 `GITHUB_TOKEN` / `GH_TOKEN`
2. **hermes 配置**：`%LOCALAPPDATA%\hermes\.env` 中的 `GITHUB_TOKEN=`
3. **hermes 凭据池**：`%LOCALAPPDATA%\hermes\auth.json` 的 `credential_pool`
4. `~/.config/wordwise/github_token`

然后校验身份 → 创建仓库 → `git init` 并提交 → 推送，全程无需手工操作。

详见 [BUILD.md](BUILD.md)。

---

## 常见问题

**Q：提示「本地模型未连接」怎么办？**  
A：在线查词不受影响。需要 AI 讲解时，按[快速开始第 3 步](#3-配置-ai-讲解可选但推荐)启动 LM Studio 服务。

**Q：查词一直失败？**  
A：进入「设置 → 词典与翻译源」，逐个点「测试」定位问题源。
若同时本地模型也没启动，则会完全无法查词。若是网络问题，
注意本机代理设置（`HTTPS_PROXY`）。

**Q：音标或变形不显示？**  
A：到「设置 → 记忆辅助内容」检查对应开关 —— 这是需求 3 的可选开关，
默认「相关词」是关闭的。

**Q：想加一门新语言？**  
A：见 [docs/DICT_SOURCES.md](docs/DICT_SOURCES.md)。
在「设置 → 词典与翻译源 → 新增源」里填 URL 模板和字段映射即可，无需改代码。

**Q：学习数据在哪？重装会丢吗？**  
A：`%APPDATA%\WordWise\wordwise.db`。卸载时会询问是否删除；
选「否」则保留，重装后继续原有进度。也可用「导出备份」生成 JSON。

**Q：词库太小，题目干扰项不够？**  
A：干扰项从词库随机抽取，建议至少导入 30 个词。
可先点「载入示例词库」凑够数量。

**Q：能同时背多门语言吗？**  
A：可以。在「设置 → 学习语言」切换，不同语言的词库与学习进度互相独立
（数据库按 `(word, lang)` 联合主键区分）。

---

## 许可

MIT License · Copyright © 2026 DedalusArtin

## 致谢

- [Tauri](https://tauri.app/) —— 轻量桌面应用框架
- [LM Studio](https://lmstudio.ai/) —— 本地大模型运行环境
- [dictionaryapi.dev](https://dictionaryapi.dev/) · [Wiktionary](https://www.wiktionary.org/) —— 免费词典数据
- 界面视觉参考有道词典桌面版
