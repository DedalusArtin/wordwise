# WordWise · 构建与运行指南

本文件面向**从零开始**的开发者，说明如何在本机构建、运行、打包 WordWise。

---

## 一、环境要求

| 组件 | 版本要求 | 说明 |
|---|---|---|
| Windows | 10 / 11 (x64) | 其他平台需自行适配打包脚本 |
| Rust | 1.77+（实测 1.99） | 工具链 `stable-x86_64-pc-windows-msvc` |
| MSVC | VS 2019 / 2022 / 2026 | 需勾选「使用 C++ 的桌面开发」 |
| Windows SDK | 10.0.19041+ | 随 VS 安装 |
| WebView2 | 任意版本 | Win10 1803+ 一般已内置 |
| LM Studio | 0.3+ | 可选，用于 AI 讲解与离线兜底 |
| Inno Setup | 6.2+ | 可选，用于生成安装程序 |

检查 Rust 环境：

```bash
rustc --version
cargo --version
```

### 网络与镜像（国内环境重要）

`cargo` 首次拉取依赖需要访问 crates.io。若下载缓慢或**长时间无输出**，
通常是本机 `HTTP_PROXY` 指向了不可用的代理端口。解决办法二选一：

```bash
# 方式一：清空代理后构建（build.ps1 已自动处理）
unset HTTP_PROXY HTTPS_PROXY http_proxy https_proxy

# 方式二：使用国内镜像（写入 ~/.cargo/config.toml）
[source.crates-io]
replace-with = 'rsproxy-sparse'
[source.rsproxy-sparse]
registry = "sparse+https://rsproxy.cn/index/"

# rustup 工具链本身也可走镜像加速
export RUSTUP_DIST_SERVER=https://mirrors.tuna.tsinghua.edu.cn/rustup
```

> 脚本已内置 `[env]` 段强制清空代理变量，一般无需手动处理。
> 如需保留代理（公司内网），设置环境变量 `WORDWISE_KEEP_PROXY=1` 即可跳过。

### 关于 `--cfg has_std`（已在项目中自动配置）

项目根目录的 `.cargo/config.toml` 注入了 `--cfg has_std`。这不是可选项，
而是**在部分 rustup 环境下编译成功的前提**：

- `indexmap 1.9.x` 的 build script 用 `autocfg` 探测 sysroot 中是否有 `std`；
  探测失败时它会编译成 no_std 版本，`IndexMap` 从 `<K, V, S = RandomState>`
  变为 `<K, V, S>`（无默认参数）。
- `schemars 0.8.22` 中有 `pub type Map<K, V> = indexmap::IndexMap<K, V>;`，
  于是报 `error[E0107]: struct takes 3 generic arguments but 2 supplied`。
- `tauri-utils 2.x` 硬性锁定 `schemars = "0.8.22"`，无法通过升级规避。

注入 `has_std` 即可让 indexmap 走 std 分支。相关文件已随仓库提供，
**请勿删除 `.cargo/config.toml`**。

---

## 二、获取代码并运行

```bash
git clone https://github.com/DedalusArtin/wordwise.git
cd wordwise

# 开发模式运行（首次会编译依赖，约 3-8 分钟）
cargo tauri dev
# 或使用内置脚本
cd src-tauri && cargo run
```

> **注意**：本项目前端为静态文件（`src/`），无需 npm/构建步骤，
> Tauri 直接加载 `src/index.html`，因此没有 `node_modules` 依赖。

---

## 三、配置 LM Studio（启用 AI 讲解）

1. 打开 LM Studio，在左侧 **Search** 标签下载一个模型  
   （推荐 `Qwen3-8B` 或 `DeepSeek-R1-Distill-Qwen-14B`，中文讲解效果好）
2. 切到 **Chat** 标签加载该模型
3. 打开左侧 **Developer** 标签 → 点击 **Start Server**
4. 确认地址为 `http://127.0.0.1:1234`（默认）
5. 回到 WordWise，点击左下角状态条检测连接

连接成功后：
- 查词页会自动用本地模型讲解单词
- 断网时词典查询会自动降级为由本地模型生成词条

---

## 四、打包为安装程序

### 4.1 生成可执行文件

```bash
# 仅构建 exe（产物在 target/release/wordwise.exe）
cd src-tauri
cargo build --release
```

### 4.2 使用 Tauri 自带打包（生成 MSI / NSIS 安装包）

```bash
cargo tauri build
```

产物位置：
- `src-tauri/target/release/bundle/msi/*.msi`
- `src-tauri/target/release/bundle/nsis/*-setup.exe`

### 4.3 使用项目自带的 Inno Setup 脚本（推荐）

项目提供了功能更完整的中文安装程序脚本，支持：
- 简体中文 / 英文双语安装界面
- 自定义安装目录、桌面快捷方式、开机自启选项
- 卸载时可选保留学习数据
- 自动检测并提示安装 WebView2

```bash
# 1) 先构建 release 版本
cd src-tauri && cargo build --release && cd ..

# 2) 用 Inno Setup 编译脚本（必须用 ISCC.exe 的完整路径）
"G:\Programming\07-utils\Inno Setup 7\ISCC.exe" installer\wordwise.iss
```

产物：`installer\output\WordWise-Setup-0.36.0.exe`

### 4.3b 便携版（免安装）

前端资源已内嵌进 exe，`wordwise.exe` 本身就是**单文件绿色版**，无需任何打包步骤：

```
src-tauri\target\release\wordwise.exe    ← 直接双击即可运行
```

`.\build.ps1` 会额外把它连同文档复制到 `dist\`，并压成
`installer\output\WordWise-<版本>-portable.zip`。

- 不写注册表、不往系统目录拷文件，拷到 U 盘也能跑
- 学习数据默认在 `%APPDATA%\WordWise`；设 `WORDWISE_DATA_DIR` 可改位置
- 需要 WebView2 运行时（Windows 10/11 一般已自带）

> **ISCC.exe 默认不在 PATH 里**，命令行敲 `iscc` 是调不到的，必须写完整路径。
> 本机安装位置：`G:\Programming\07-utils\Inno Setup 7\ISCC.exe`（Inno Setup 7）。
> 若装在别处，任选一种：
> - 临时指定：`.\build.ps1 -IsccPath "<你的路径>\ISCC.exe"`
> - 永久生效：改 `build.ps1` 顶部的 `$DefaultIscc`
>
> 排查实际位置：`where.exe ISCC.exe` 或
> `Get-ChildItem G:\ -Recurse -Filter ISCC.exe -ErrorAction SilentlyContinue`
>
> 脚本会自动校验找到的文件确实是 Inno Setup 编译器；版本与预期的
> Inno Setup 7 不一致时会给出提示。

### 4.4 一键构建脚本

```powershell
# PowerShell
.\build.ps1              # 完整构建 + 打包
.\build.ps1 -SkipInstaller   # 只构建 exe，跳过 Inno Setup
.\build.ps1 -Clean           # 先清理再构建
```

```bash
# 交互式菜单（推荐，双击 构建.ps1 即可）
.\构建.ps1
```

> 旧版基于 WSL / Git Bash 的 `build.sh`、`build_fixed.sh`、`BUILD_AND_PACK.sh`、
> `编译.cmd` 已移入 `archive/legacy-build-scripts/`，不再维护。
> 当前唯一受支持的构建入口是 `build.ps1`（原生 Windows，不依赖 WSL）。

---

## 五、项目结构

```
wordwise/
├── src/                        前端（原生 HTML/CSS/JS，无构建步骤）
│   ├── index.html              主界面 + 侧边栏共用一个入口
│   ├── css/app.css             全部样式（浅色主题，仿有道词典）
│   └── js/
│       ├── api.js              前后端桥接 + 浏览器 Mock 模式
│       ├── ui.js               通用渲染工具（词条、Markdown、图表）
│       ├── study.js            背诵引擎（双向模式、判分）
│       ├── lookup.js           查词 + AI 流式讲解 + 详情卡
│       ├── library.js          词库、错词本、复习计划、统计
│       ├── settings.js         设置面板 + 词典源配置 UI
│       ├── sidebar.js          长挂侧边栏
│       └── app.js              页面路由与初始化
│
├── src-tauri/                  Rust 后端
│   ├── src/
│   │   ├── main.rs             入口
│   │   ├── lib.rs              模块声明
│   │   ├── windows.rs          窗口/托盘/侧边栏管理
│   │   ├── state.rs            全局共享状态
│   │   ├── models.rs           数据模型
│   │   ├── timeutil.rs         系统时间工具
│   │   ├── net.rs              HTTP 基础设施
│   │   ├── seed.rs             内置示例词库
│   │   ├── srs/mod.rs          记忆调度引擎（遗忘曲线）
│   │   ├── db/mod.rs           SQLite 存储层
│   │   ├── dict/
│   │   │   ├── mod.rs          多源聚合与归一化
│   │   │   ├── builtin.rs      内置词典源定义
│   │   │   └── jsonpath.rs     通用 JSON 路径映射
│   │   ├── llm/mod.rs          LM Studio 客户端（含流式）
│   │   ├── search/mod.rs       在线搜索与维基知识
│   │   └── commands/mod.rs     全部 Tauri 命令
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   └── capabilities/default.json
│
├── installer/wordwise.iss      Inno Setup 打包脚本
├── scripts/upload_github.py    自动上传到 GitHub
├── build.ps1 / 构建.ps1         一键构建
└── docs/                       文档
```

---

## 六、常见问题

**Q: 编译报错「link.exe not found」**  
A: 未安装 MSVC 构建工具。安装 Visual Studio，勾选「使用 C++ 的桌面开发」。

**Q: 启动后提示「本地模型未连接」**  
A: 属正常情况，查词功能仍可用（走在线词典）。需要 AI 讲解时按第三节配置 LM Studio。

**Q: 查词提示「所有在线词典源均失败」**  
A: 检查网络/代理。可在「设置 → 词典源」里逐个点「测试」定位问题源。
若同时本地模型也没启动，则会完全无法查词。

**Q: 音标/变形不显示**  
A: 到「设置 → 记忆辅助内容」检查对应开关是否被关掉（需求 3 的可选开关）。

**Q: 想加一门新语言**  
A: 到「设置 → 词典与翻译源」新增一个源，填写 URL 模板与字段映射即可，
无需修改任何代码。详见 `docs/DICT_SOURCES.md`。

**Q: 学习数据存在哪**  
A: `%APPDATA%\WordWise\wordwise.db`（SQLite）。
可在「设置」或词库页用「导出备份」生成 JSON。

---

## 七、运行测试

```bash
cd src-tauri
cargo test            # 运行全部单元测试
cargo test -- --nocapture   # 显示 println 输出
cargo clippy          # 静态检查
```

测试覆盖：
- `srs`：间隔推进、答错重置、常错词识别、记忆衰减曲线、复习计划生成
- `dict::jsonpath`：路径取值、通配符、数组下标
- `dict`：多源归一化（真实 API 响应结构）、词性标准化
- `llm`：端点拼接、JSON 提取容错、模型输出解析
- `db`：建表、增删改查、统计聚合（需文件系统）
