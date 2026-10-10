; ============================================================
;  WordWise 安装程序脚本（Inno Setup 7）
;
;  构建方式（ISCC.exe 不在系统 PATH 里，必须写完整路径）：
;    "G:\Programming\07-utils\Inno Setup 7\ISCC.exe" installer\wordwise.iss
;
;  或用构建脚本（自动定位并校验版本）：
;    .\build.ps1
;    .\build.ps1 -IsccPath "<你的路径>\ISCC.exe"      ; 装在别处时
;
;  产物：
;    installer\output\WordWise-Setup-0.42.0.exe
;
;  前置条件：
;    先执行 cargo build --release，确保 wordwise.exe 已生成
;
;  ★ 关于安装位置与数据位置
;    运行时数据目录按 `state::resolve_data_dir_with` 的六级规则解析，
;    其中「新装默认」这一级是 **exe 同级 data\** —— 也就是：装到哪个盘，
;    词库和模型就落哪个盘，不会再去写 C 盘。
;    所以下面 InitializeWizard 会把默认安装目录从 {localappdata}（必在 C 盘）
;    改成第一个可写的非 C 固定盘；找不到就退回原默认，不会报错。
; ============================================================

#define MyAppName "WordWise"
#define MyAppPublisher "DedalusArtin"
#define MyAppURL "https://github.com/DedalusArtin/wordwise"
#define MyAppExeName "wordwise.exe"
#define MyAppId "{{8F3C2A41-7B5E-4D9A-A1C6-2E8D4F6B9C03}"

; 版本号：由 build.ps1 读 src-tauri\tauri.conf.json 后用 /DMyAppVersion=... 传进来
; （CI 里则来自 git tag）。保留一个兜底默认值，方便直接用 ISCC 编译本脚本。
; ★ 要改版本号请改 tauri.conf.json —— 两处各写一个版本号必然会漂移，
;   表现是「安装包文件名 / 界面显示 / 程序属性」三个版本号对不上。
#ifndef MyAppVersion
  #define MyAppVersion "0.49.3"
#endif

; 相对本 .iss 文件定位构建产物；若使用自定义 target 目录，
; 可通过 /DMySourceDir=... 覆盖
#ifndef MySourceDir
  #define MySourceDir "..\src-tauri\target\release"
#endif

; ============================================================
;  离线 / 在线 两种装法，由本机有没有 vendor 决定
; ============================================================
;  打包机上如果已经准备好了 vendor\llama / vendor\piper / vendor\tts-voices，
;  就按老办法**随包带走**（离线安装，装完就能用）；
;  没有的话，那些模块改成**安装时联网下载**（[Code] 里的下载页负责抓）。
;
;  ★ 两种装法落到的是同一个目录（{app}\vendor\...），所以同一份 [Files]
;    不能同时出现「随包」和「下载」两条 —— 否则两条都往同一个 DestDir 写，
;    会出现「文件被谁覆盖」这种说不清的问题。这里用预处理开关二选一。
#ifndef OfflineLlama
  #ifexist "..\vendor\llama\llama-server.exe"
    #define OfflineLlama
  #endif
#endif
#ifndef OfflinePiper
  #ifexist "..\vendor\piper\piper.exe"
    #define OfflinePiper
  #endif
#endif
#ifndef OfflineVoice
  #ifexist "..\vendor\tts-voices\en_US-amy-medium\en_US-amy-medium.onnx"
    #define OfflineVoice
  #endif
#endif

; ============================================================
;  可选模块的「版本标记」——**必须与 Rust 侧完全一致**
; ============================================================
;  这几处是第三、第四个版本号，最容易漂：
;    LlamaTag  ← src-tauri\src\localllm\mod.rs 的 ENGINE_TAG
;    PiperTag  ← src-tauri\src\tts\mod.rs      的 ENGINE_TAG
;    VoiceTag  ← src-tauri\src\tts\mod.rs      的 VOICE_TAG
;    ModelFile ← src-tauri\src\localllm\mod.rs 里 models() 的 file 字段
;  漂了的表现是「安装到一半下载 404」，而界面上显示的版本号还是对的，
;  排查时极容易误判成网络问题。scripts/smoke_frontend.cjs 有一道用例
;  直接比对双方，改错会当场失败。
#ifndef LlamaTag
  #define LlamaTag "b11414"
#endif
#ifndef PiperTag
  #define PiperTag "tts-engine-v1"
#endif
#ifndef VoiceTag
  #define VoiceTag "tts-voices-v1"
#endif
#ifndef ModelFile
  #define ModelFile "Qwen3-0.6B-Q4_K_M.gguf"
#endif

[Setup]
AppId={#MyAppId}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}

; 默认装到用户目录，避免 UAC 提权，符合 Tauri 的 currentUser 安装模式。
; 注意：这个值会被 InitializeWizard 在「本机有可用非 C 盘」时改写 ——
; 理由见文件头「关于安装位置与数据位置」。
DefaultDirName={localappdata}\Programs\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
DisableDirPage=no
AllowNoIcons=yes

; 输出
OutputDir=output
OutputBaseFilename={#MyAppName}-Setup-{#MyAppVersion}
Compression=lzma2/max
SolidCompression=yes
LZMANumBlockThreads=4

; 图标与版权
SetupIconFile=..\src-tauri\icons\icon.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
UninstallDisplayName={#MyAppName}

;  界面：支持简体中文与英文
;  ★ modern dynamic：在线安装要用 Inno 7 自带的下载进度页（CreateDownloadPage），
;    它只在 modern 系列风格下可用；dynamic 让向导在下载阶段能正确换文案。
WizardStyle=modern dynamic
ShowLanguageDialog=auto
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog

; 在线安装下载的 zip（llama.cpp 引擎 / Piper 语音）允许 [Files] 直接自动解压，
; 省掉「下载完再手工解一层」的步骤。见下面 [Files] 里的 extractarchive。
ArchiveExtraction=auto

; 架构限制
ArchitecturesInstallIn64BitMode=x64compatible
ArchitecturesAllowed=x64compatible

; 许可证与说明（可选，存在才显示）
; LicenseFile=..\LICENSE

[Languages]
Name: "chinesesimplified"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

; ============================================================
;  多语言消息表
; ============================================================
;  ★ 为什么要有这一段：**直接写死的中文在英文系统上照样显示中文**，
;    安装向导于是变成「一半英文按钮、一半中文描述」的夹生界面 ——
;    这就是用户报的「安装包的语言没有跟随系统提示」。
;    [Languages] 里的 Name 就是这里的前缀；不带前缀的那份是所有语言的兜底
;    （某语言没定义时用兜底，而不是退化成空白）。
;
;  ★ 新增面向用户的文字时，一律加到这里，不要写死在 [Tasks] / [Components]
;    / [Icons] 或 Code 段里。
;  ★ 多行文本用 %n 表示换行（Inno 的转义），%% 表示百分号本身。
[CustomMessages]
english.WWGroupStartup=Startup options:
chinesesimplified.WWGroupStartup=启动选项:
WWGroupStartup=Startup options:

english.WWTaskStartup=Start the WordWise sidebar at sign-in, so you can look words up anytime
chinesesimplified.WWTaskStartup=开机时自动启动 WordWise 侧边栏（随时查词）
WWTaskStartup=Start the WordWise sidebar at sign-in

english.WWCompCore=Main program and documentation (required)
chinesesimplified.WWCompCore=主程序与说明文档（必需）
WWCompCore=Main program and documentation (required)

english.WWCompOpt=Optional modules: downloaded during setup when checked (you can also add them later inside the app)
chinesesimplified.WWCompOpt=可选模块 —— 勾选后在安装过程中联网下载（也可之后在软件内下载）
WWCompOpt=Optional modules: downloaded during setup when checked

english.WWCompEngine=Local LLM engine llama.cpp (~118 MB; needed for offline AI explanations)
chinesesimplified.WWCompEngine=本地大模型推理引擎 llama.cpp（约 118 MB，离线 AI 讲解要用）
WWCompEngine=Local LLM engine llama.cpp (~118 MB)

english.WWCompVoice=Local neural TTS engine Piper + English voice (~85 MB; needed for offline read-aloud)
chinesesimplified.WWCompVoice=本地神经语音引擎 Piper + 英文语音（约 85 MB，离线朗读要用）
WWCompVoice=Local neural TTS engine Piper + English voice (~85 MB)

english.WWCompModel=Qwen3-0.6B model weights (~378 MB; enables offline AI explanations)
chinesesimplified.WWCompModel=Qwen3-0.6B 模型权重（约 378 MB，下载后即可离线 AI 讲解）
WWCompModel=Qwen3-0.6B model weights (~378 MB)

english.WWShortcutComment=Vocabulary learning with AI explanations
chinesesimplified.WWShortcutComment=背单词与 AI 讲解
WWShortcutComment=Vocabulary learning with AI explanations

; —— Code 段对话框 ——
; 注：%% 表示百分号本身（Inno 把 % 当转义引导符，写成单个 % 会被吃掉）。

english.WWMUpgradeL1=This is an upgrade: your existing study data may not be in the new folder.
chinesesimplified.WWMUpgradeL1=这是升级安装：你当前的学习数据可能不在新目录里。
WWMUpgradeL1=This is an upgrade: your existing study data may not be in the new folder.

english.WWMUpgradeL2=The model will be downloaded to
chinesesimplified.WWMUpgradeL2=模型将下载到
WWMUpgradeL2=The model will be downloaded to

english.WWMUpgradeL3=If the old version kept its data in %%APPDATA%%\WordWise, download the model from inside the app instead - it will place it in the folder actually in use.
chinesesimplified.WWMUpgradeL3=（如果旧版本的数据目录是 %%APPDATA%%\WordWise，模型应改在软件内下载，软件会放到它真正在用的那一份目录。）
WWMUpgradeL3=If the old version kept its data in %%APPDATA%%\WordWise, download the model from inside the app instead.

english.WWMUpgradeQ=Download to the new folder now anyway?
chinesesimplified.WWMUpgradeQ=仍然现在下载到新目录吗？
WWMUpgradeQ=Download to the new folder now anyway?

english.WWPrevFound=WordWise is already installed at:
chinesesimplified.WWPrevFound=已安装的 WordWise 在：
WWPrevFound=WordWise is already installed at:

english.WWPrevChosen=You have chosen:
chinesesimplified.WWPrevChosen=你现在选的是：
WWPrevChosen=You have chosen:

english.WWPrevWarn=Installing to a different folder creates two independent copies; your existing wordbooks and study progress will not follow.
chinesesimplified.WWPrevWarn=装到别的目录会变成两个独立副本，原有的词库与背诵进度不会跟过去。
WWPrevWarn=Installing to a different folder creates two independent copies; your existing wordbooks and study progress will not follow.

english.WWPrevYes=[Yes] = upgrade in place at the original folder and continue
chinesesimplified.WWPrevYes=「是」 = 覆盖升级到原目录并继续
WWPrevYes=[Yes] = upgrade in place at the original folder and continue

english.WWPrevNo=[No] = install to the new folder and continue (two independent copies; progress will not follow)
chinesesimplified.WWPrevNo=「否」 = 装到新目录并继续（会变成两个独立副本，原有词库与背诵进度不会跟过去）
WWPrevNo=[No] = install to the new folder and continue (two independent copies)

english.WWPrevCancel=[Cancel] = stay on this page and think about it
chinesesimplified.WWPrevCancel=「取消」 = 留在本页，我再想想
WWPrevCancel=[Cancel] = stay on this page and think about it

english.WWDLFailHead=Download failed:
chinesesimplified.WWDLFailHead=下载失败：
WWDLFailHead=Download failed:

english.WWDLFailBody=A failed optional-module download does not affect the main installation.
chinesesimplified.WWDLFailBody=可选模块下载失败不影响主程序安装。
WWDLFailBody=A failed optional-module download does not affect the main installation.

english.WWDLFailAsk=Continue installing? (you can download these modules later inside the app)
chinesesimplified.WWDLFailAsk=是否继续安装（之后可在软件内再下载这些模块）？
WWDLFailAsk=Continue installing? (you can download these modules later inside the app)

english.WWDoneTitle=Installation complete!
chinesesimplified.WWDoneTitle=安装完成！
WWDoneTitle=Installation complete!

english.WWDoneBody1=WordWise's "AI word explanations" need a model service. Pick any one of three options:
chinesesimplified.WWDoneBody1=WordWise 的「AI 单词讲解」需要一个模型服务，三种办法任选：
WWDoneBody1=WordWise's "AI word explanations" need a model service. Pick any one of three options:

english.WWDoneA1=  A. One-click deploy (recommended, no extra software)
chinesesimplified.WWDoneA1=  A. 一键部署（推荐，无需额外软件）
WWDoneA1=  A. One-click deploy (recommended, no extra software)

english.WWDoneA2=     Click the "AI service" status bar at the bottom left, then Download model, then Start
chinesesimplified.WWDoneA2=     点窗口左下角「AI 服务」状态条 → 下载模型 → 启动
WWDoneA2=     Click the "AI service" status bar at the bottom left, then Download model, then Start

english.WWDoneA3=     The engine ships with the installer; only the model needs downloading (~400 MB to 1.1 GB)
chinesesimplified.WWDoneA3=     引擎已随安装包附带，只需下载模型（约 400MB～1.1GB）
WWDoneA3=     The engine ships with the installer; only the model needs downloading (~400 MB to 1.1 GB)

english.WWDoneA4=     To launch it automatically each time, turn on "launch on startup" in that panel
chinesesimplified.WWDoneA4=     想让每次开程序自动起，打开面板里的「启动时自动拉起」
WWDoneA4=     To launch it automatically each time, turn on "launch on startup" in that panel

english.WWDoneB1=  B. Connect to an existing LM Studio
chinesesimplified.WWDoneB1=  B. 连接已有的 LM Studio
WWDoneB1=  B. Connect to an existing LM Studio

english.WWDoneB2=     Start the server inside LM Studio, then click "Test connection" back in WordWise
chinesesimplified.WWDoneB2=     LM Studio 里 Start Server，回 WordWise 点「测试连接」
WWDoneB2=     Start the server inside LM Studio, then click "Test connection" back in WordWise

english.WWDoneC1=  C. Use an online API (DeepSeek / Qwen / Kimi / Zhipu GLM, etc.)
chinesesimplified.WWDoneC1=  C. 用在线 API（DeepSeek / 通义千问 / Kimi / 智谱 GLM 等）
WWDoneC1=  C. Use an online API (DeepSeek / Qwen / Kimi / Zhipu GLM, etc.)

english.WWDoneC2=     Settings, AI service, pick a provider and paste your API key
chinesesimplified.WWDoneC2=     设置 → AI 服务 → 服务来源里选一家，填上 API Key 即可
WWDoneC2=     Settings, AI service, pick a provider and paste your API key

english.WWDoneTail=Without a configured model, lookup, translation, study and the knowledge graph all keep working normally.
chinesesimplified.WWDoneTail=不配置模型时，查词、翻译、背诵、知识图谱全部照常可用。
WWDoneTail=Without a configured model, lookup, translation, study and the knowledge graph all keep working normally.

english.WWDoneAsk=Open the LM Studio download page now?
chinesesimplified.WWDoneAsk=是否现在打开 LM Studio 官网下载页？
WWDoneAsk=Open the LM Studio download page now?

english.WWUninstAsk=Also delete your study data (wordbooks and study progress)?
chinesesimplified.WWUninstAsk=是否同时删除学习数据（词库与背诵记录）？
WWUninstAsk=Also delete your study data (wordbooks and study progress)?

english.WWUninstAt=Data location:
chinesesimplified.WWUninstAt=数据位置：
WWUninstAt=Data location:

english.WWUninstKeep=Choosing "No" keeps the data, so a later reinstall can pick up where you left off.
chinesesimplified.WWUninstKeep=选择「否」将保留数据，日后重新安装可继续使用原有进度。
WWUninstKeep=Choosing "No" keeps the data, so a later reinstall can pick up where you left off.

english.WWUninstNote=(If you changed the data folder in settings, that copy is not listed above and must be cleaned up manually.)
chinesesimplified.WWUninstNote=（如果你在设置里换过数据目录，那份数据不在上面，需要自己清理。）
WWUninstNote=(If you changed the data folder in settings, that copy is not listed above and must be cleaned up manually.)

english.WWWhereUninst=uninstall entry
chinesesimplified.WWWhereUninst=卸载项
WWWhereUninst=uninstall entry

english.WWWhereCommon=common install location
chinesesimplified.WWWhereCommon=常见安装位置
WWWhereCommon=common install location

english.WWFound=Found an existing WordWise
chinesesimplified.WWFound=检测到已安装的 WordWise
WWFound=Found an existing WordWise

english.WWWillUpgrade=Will upgrade to
chinesesimplified.WWWillUpgrade=将升级到
WWWillUpgrade=Will upgrade to

english.WWKeepProgress=Your existing wordbooks and study progress will be kept.
chinesesimplified.WWKeepProgress=原有词库与背诵进度会保留。
WWKeepProgress=Your existing wordbooks and study progress will be kept.

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: checkedonce
Name: "quicklaunchicon"; Description: "{cm:CreateQuickLaunchIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "startup"; Description: "{cm:WWTaskStartup}"; GroupDescription: "{cm:WWGroupStartup}"

; ============================================================
;  组件选择
; ============================================================
;  主程序之外的东西都不进安装包本体（它们动辄上百 MB，全塞进去会让
;  安装包从 18 MB 变成 600 MB+，而大多数人其实并不需要全部）。
;  所以这里给一个组件页，让用户自己挑；**勾了什么就装时联网下载什么**。
;
;  ★ 全部默认不勾：不下载任何一项，程序照样能用（查词 / 翻译 / 背诵 / 图谱
;    都不依赖本地模型或本地语音）。用户后面想加，软件内的「一键部署」和
;    「语音包管理」随时能补，走的是同一批镜像源。
[Components]
Name: "core"; Description: "{cm:WWCompCore}"; Flags: fixed
Name: "opt"; Description: "{cm:WWCompOpt}"; Flags: checkablealone
Name: "opt\engine"; Description: "{cm:WWCompEngine}"
Name: "opt\voice"; Description: "{cm:WWCompVoice}"
Name: "opt\model"; Description: "{cm:WWCompModel}"

[Files]
; 主程序
Source: "{#MySourceDir}\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
; 前端资源随 exe 内嵌，无需单独复制

; 若采用「未内嵌前端」的构建方式，可放开下面这行
; Source: "{#MySourceDir}\..\..\src\*"; DestDir: "{app}\src"; Flags: ignoreversion recursesubdirs createallsubdirs

; ---- 可选模块一：本地大模型推理引擎（llama.cpp） ----
;   llama.cpp 的发行包是**平铺**的（zip 里直接就是 dll/exe，没有外层目录），
;   Inno 的 extractarchive 解到 {app}\vendor\llama 正好是程序要的层级。
#ifdef OfflineLlama
Source: "..\vendor\llama\*.exe"; DestDir: "{app}\vendor\llama"; Components: opt\engine; Flags: ignoreversion skipifsourcedoesntexist
Source: "..\vendor\llama\*.dll"; DestDir: "{app}\vendor\llama"; Components: opt\engine; Flags: ignoreversion skipifsourcedoesntexist
#else
Source: "{tmp}\llama-engine.zip"; DestDir: "{app}\vendor\llama"; Components: opt\engine; \
  Flags: external extractarchive recursesubdirs ignoreversion skipifsourcedoesntexist
#endif

; ---- 可选模块二：本地神经语音（Piper 引擎 + 英文语音） ----
;   ★ piper_windows_amd64.zip 的顶层就是 piper\ 这一层，所以解压目标必须是
;     {app}\vendor（解出来变成 vendor\piper）；写成 vendor\piper 会多套一层。
;     这个目录层级是踩过坑的，别凭直觉改。
#ifdef OfflinePiper
Source: "..\vendor\piper\*"; DestDir: "{app}\vendor\piper"; Components: opt\voice; \
  Flags: ignoreversion recursesubdirs createallsubdirs skipifsourcedoesntexist
#else
Source: "{tmp}\piper_windows_amd64.zip"; DestDir: "{app}\vendor"; Components: opt\voice; \
  Flags: external extractarchive recursesubdirs ignoreversion skipifsourcedoesntexist
#endif

#ifdef OfflineVoice
Source: "..\vendor\tts-voices\*"; DestDir: "{app}\vendor\tts-voices"; Components: opt\voice; \
  Flags: ignoreversion recursesubdirs createallsubdirs skipifsourcedoesntexist
#else
Source: "{tmp}\en_US-amy-medium.onnx"; DestDir: "{app}\vendor\tts-voices\en_US-amy-medium"; Components: opt\voice; \
  Flags: external ignoreversion skipifsourcedoesntexist
Source: "{tmp}\en_US-amy-medium.onnx.json"; DestDir: "{app}\vendor\tts-voices\en_US-amy-medium"; Components: opt\voice; \
  Flags: external ignoreversion skipifsourcedoesntexist
#endif

; ---- 可选模块三：模型权重 ----
;   新装时数据目录默认就是 {app}\data，所以放在 {app}\data\models 是程序
;   认得的位置；**升级安装**时数据目录可能在别处（老版本在 AppData），
;   那种情况 [Code] 里会先问过用户再决定要不要下。
Source: "{tmp}\Qwen3-0.6B-Q4_K_M.gguf"; DestDir: "{app}\data\models"; Components: opt\model; \
  Flags: external ignoreversion skipifsourcedoesntexist

; 附带文档
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist; DestName: "README.md"
Source: "..\BUILD.md"; DestDir: "{app}\docs"; Flags: ignoreversion skipifsourcedoesntexist
Source: "..\docs\*"; DestDir: "{app}\docs"; Flags: ignoreversion recursesubdirs createallsubdirs skipifsourcedoesntexist

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Comment: "{cm:WWShortcutComment}"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon; Comment: "{cm:WWShortcutComment}"
Name: "{userappdata}\Microsoft\Internet Explorer\Quick Launch\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: quicklaunchicon

[Registry]
; 开机自启（写入 HKCU，无需管理员权限）
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; \
  ValueType: string; ValueName: "{#MyAppName}"; ValueData: """{app}\{#MyAppExeName}"""; \
  Flags: uninsdeletevalue; Tasks: startup

[Run]
; 安装完成后询问是否立即运行
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(MyAppName, '&', '&&')}}"; \
  Flags: nowait postinstall skipifsilent

[UninstallDelete]
; 清理运行期可能产生的日志（学习数据默认保留，见下方代码段）
Type: filesandordirs; Name: "{app}\logs"

[Code]
// ============================================================
//  常量：可选模块的下载地址
// ============================================================
//  ★ 这些地址必须与 Rust 侧保持一致：
//    - llama.cpp 的 tag 来自 src-tauri\src\localllm\mod.rs 的 ENGINE_TAG
//    - Piper / 语音来自 src-tauri\src\tts\mod.rs 的 ENGINE_TAG / VOICE_TAG
//    - 模型来自同文件 models() 里的 urls
//    这里是第二处，改了那边忘了这边会直接 404，用户看到的是「装到一半报错」。
//    scripts/smoke_frontend.cjs 里有一道用例专门比对这几处，会当场失败。
const
  LlamaTag = '{#LlamaTag}';
  LlamaUrl = 'https://gh-proxy.com/https://github.com/ggml-org/llama.cpp/releases/download/{#LlamaTag}/llama-{#LlamaTag}-bin-win-vulkan-x64.zip';
  PiperUrl = 'https://github.com/DedalusArtin/wordwise/releases/download/{#PiperTag}/piper_windows_amd64.zip';
  VoiceUrlBase = 'https://github.com/DedalusArtin/wordwise/releases/download/{#VoiceTag}';
  ModelUrl = 'https://www.modelscope.cn/models/unsloth/Qwen3-0.6B-GGUF/resolve/master/{#ModelFile}';

// 检测到的旧安装信息
var
  PrevDir: String;      // 旧安装目录（空 = 没检测到）
  PrevVer: String;      // 旧版本号
  PrevWhere: String;    // 从哪类来源检测到的，提示给用户看
  IsUpgrade: Boolean;   // 是否属于「原地升级」
  DownloadPage: TDownloadWizardPage;
  WarnedOffPrevDir: Boolean;

// ---------- 版本号比较 ----------
//
//  不用系统提供的比较函数：Inno 各版本提供的实现不一样，而这里只需要
//  「谁大谁小」这一个结论。自己拆段比，行为可控、在任何版本上都一样。
//  返回：1 = A 新，-1 = B 新，0 = 一样。非数字段按 0 处理。
function CompareVer(A, B: String): Integer;
var
  I: Integer;
  Va, Vb: Longint;
begin
  Result := 0;
  // 最多比 4 段（x.y.z.w）。逐段削掉已比过的部分 —— Pascal Script 没有 Split。
  for I := 1 to 4 do
  begin
    Va := StrToIntDef(Copy(A, 1, Pos('.', A + '.') - 1), 0);
    Vb := StrToIntDef(Copy(B, 1, Pos('.', B + '.') - 1), 0);
    if Va > Vb then begin Result := 1; Exit; end;
    if Va < Vb then begin Result := -1; Exit; end;
    // 削掉已比的一段，继续下一段
    A := Copy(A, Pos('.', A + '.') + 1, MaxInt);
    B := Copy(B, Pos('.', B + '.') + 1, MaxInt);
  end;
end;

// ---------- 从卸载命令里反推安装目录 ----------
//  形如 "D:\WordWise\unins000.exe" → "D:\WordWise"
function DirFromUninstallString(S: String): String;
var
  I, Last: Integer;
  T: String;
begin
  Result := '';
  T := Trim(S);
  if T = '' then Exit;
  // 去掉可能存在的引号与参数
  if (Copy(T, 1, 1) = '"') then
  begin
    Delete(T, 1, 1);
    I := Pos('"', T);
    if I > 0 then T := Copy(T, 1, I - 1);
  end else
  begin
    I := Pos('.exe', Lowercase(T));
    if I > 0 then T := Copy(T, 1, I + 3);
  end;
  // 去尾部的反斜杠，再砍掉最后一级（unins000.exe / uninstall.exe）
  Last := 0;
  for I := 1 to Length(T) do
    if T[I] = '\' then Last := I;
  if Last > 0 then Result := Copy(T, 1, Last - 1);
end;

// ---------- 扫描「卸载」注册表项 ----------
//
//  为什么要自己扫：Inno 只认**自己那套 AppId** 的卸载项。而历史上这份
//  软件还被 Tauri 的 NSIS / MSI 打包器产出过，AppId 与卸载项命名都不一样；
//  用户从那种包升到本包时，Inno 会当成全新安装，把程序装到另一个目录 ——
//  结果就是「装完桌面上出现两个 WordWise，进度还在老的那个里」。
//  所以这里按 DisplayName 含 WordWise 全量扫一遍。
function ScanUninstallKeys(Root: Integer): Boolean;
var
  Names: TArrayOfString;
  I: Integer;
  Sub, Disp, Loc, Ver, Uninst: String;
  Dir: String;
begin
  Result := False;
  if not RegGetSubkeyNames(Root, 'SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall', Names) then Exit;
  for I := 0 to GetArrayLength(Names) - 1 do
  begin
    Sub := 'SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\' + Names[I];
    Disp := '';
    if not RegQueryStringValue(Root, Sub, 'DisplayName', Disp) then Disp := '';
    // 只认 WordWise，避免把名字里带这个词的别家软件当成自己
    if Pos('WordWise', Disp) > 0 then
    begin
      Loc := ''; Ver := ''; Uninst := '';
      RegQueryStringValue(Root, Sub, 'InstallLocation', Loc);
      RegQueryStringValue(Root, Sub, 'DisplayVersion', Ver);
      RegQueryStringValue(Root, Sub, 'UninstallString', Uninst);

      Dir := Trim(Loc);
      // InstallLocation 经常是空的（MSI 包尤其如此），那就从卸载程序路径反推
      if (Dir = '') and (Uninst <> '') then Dir := DirFromUninstallString(Uninst);
      if (Dir <> '') and not DirExists(Dir) then Dir := '';

      // 版本空着也要认：有些卸载项没写 DisplayVersion，只写目录
      if (PrevDir = '') and (Dir <> '') then PrevDir := Dir;
      if (PrevVer = '') and (Ver <> '') then PrevVer := Ver;
      if (Dir <> '') or (Ver <> '') then
      begin
        PrevWhere := ExpandConstant('{cm:WWWhereUninst}');
        Result := True;
      end;
    end;
  end;
end;

// ---------- 兜底：App Paths + 常见安装位置 ----------
function ScanFallbackLocations(): Boolean;
var
  S: String;
  Candidates: array[0..3] of String;
  I: Integer;
begin
  Result := False;
  // App Paths 里的路径值（有些安装器只写这里）
  if RegQueryStringValue(HKLM, 'SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{#MyAppExeName}', '', S) then
  begin
    if (S <> '') and DirExists(S) then
    begin
      PrevDir := S;
      PrevWhere := 'App Paths';
      Result := True;
      Exit;
    end;
  end;
  // 常见的几个落点（含 Tauri NSIS 默认目录与非 C 盘习惯）
  Candidates[0] := ExpandConstant('{localappdata}\Programs\{#MyAppName}');
  Candidates[1] := ExpandConstant('{autopf}\{#MyAppName}');
  Candidates[2] := 'D:\{#MyAppName}';
  Candidates[3] := 'D:\Program Files\{#MyAppName}';
  for I := 0 to 3 do
  begin
    if DirExists(Candidates[I]) and FileExists(Candidates[I] + '\{#MyAppExeName}') then
    begin
      PrevDir := Candidates[I];
      PrevWhere := ExpandConstant('{cm:WWWhereCommon}');
      Result := True;
      Exit;
    end;
  end;
end;

// ---------- 汇总检测 ----------
procedure DetectPreviousInstall();
begin
  PrevDir := ''; PrevVer := ''; PrevWhere := ''; IsUpgrade := False;
  ScanUninstallKeys(HKCU);
  if PrevDir = '' then ScanUninstallKeys(HKLM64);
  if PrevDir = '' then ScanUninstallKeys(HKLM32);
  if PrevDir = '' then ScanFallbackLocations();
  IsUpgrade := PrevDir <> '';
end;

//
//  用户的要求是「默认不要写 C 盘」。而 PrivilegesRequired=lowest 时
//  localappdata 一定在 C 盘；所以这里在向导初始化时把它改成第一个
//  **真的能建、真的能写**的非 C 盘目录。
//
//  ★ 必须先探测可写性再改默认值：
//    只看「盘符存在」是不够的 —— 光盘、插着碟的读卡器、只读的移动硬盘
//    都能通过 DirExists，但装进去会在写文件时才失败，而那时用户已经
//    点了好几次「下一步」。探不动就老实退回原默认，不制造新的坑。
//
//  注意：这里一律用 // 行注释，不要用 { } ——
//  Pascal 的 { } 注释**不嵌套**，注释里写 localappdata 这类带花括号的
//  常量名会让注释在第一个 } 处提前结束，后面全变成代码。

function TryUseDriveAsDefault(Root: String): Boolean;
var
  Candidate, Probe: String;
begin
  Result := False;
  { 目录名沿用程序名，用户一眼就知道这是什么 }
  Candidate := Root + '{#MyAppName}';
  if not ForceDirectories(Candidate) then Exit;
  { 真写一个探针文件再删掉：只判「目录建出来了」挡不住只读盘 }
  Probe := Candidate + '\.write-probe';
  if not SaveStringToFile(Probe, 'ok', False) then Exit;
  if not FileExists(Probe) then Exit;
  DeleteFile(Probe);
  WizardForm.DirEdit.Text := Candidate;
  Result := True;
end;

procedure InitializeWizard();
var
  I: Integer;
  Roots: array[0..2] of String;
  Note: String;
begin
  // ---- 1) 先查本机有没有装过（决定是「全新安装」还是「原地升级」）----
  DetectPreviousInstall();

  if IsUpgrade then
  begin
    // ★ 自动合并升级：目录沿用旧的那份，文件覆盖上去，数据一概不动。
    //   这一步是新装/升级两套路径唯一的分叉点 —— 选错目录就会出现
    //   「两个 WordWise 并存，进度还在老的那个里」。
    WizardForm.DirEdit.Text := PrevDir;

    if PrevVer <> '' then
      Note := ExpandConstant('{cm:WWFound}') + ' v' + PrevVer + ' (' + PrevWhere + ': ' + PrevDir + ')'
    else
      Note := ExpandConstant('{cm:WWFound}') + ' (' + PrevWhere + ': ' + PrevDir + ')';
    Note := Note + #13#10 + ExpandConstant('{cm:WWWillUpgrade}') + ' v{#MyAppVersion}' + '. '
          + ExpandConstant('{cm:WWKeepProgress}');
  end
  else
  begin
    // ---- 2) 全新安装：默认目录优先非 C 盘 ----
    // 只看 D / E / F：这三者覆盖了绝大多数国产整机的「数据盘」，
    // 再往后找就容易碰到临时插上的 U 盘了
    Roots[0] := 'D:\';
    Roots[1] := 'E:\';
    Roots[2] := 'F:\';
    for I := 0 to 2 do
    begin
      if DirExists(Roots[I]) and TryUseDriveAsDefault(Roots[I]) then Break;
    end;
    // 一个都没成功 → 保持 localappdata 原默认，安装照常进行
  end;

  // ---- 3) 在线安装用的下载页（Inno 7 内置，带进度与校验）----
  DownloadPage := CreateDownloadPage(SetupMessage(msgWizardPreparing), SetupMessage(msgPreparingDesc), nil);
  // 显示文件名而不是一长串 URL，用户才知道「现在在下什么」
  DownloadPage.ShowBaseNameInsteadOfUrl := True;

  // 把升级提示放到欢迎页第二行，用户一进来就能看见
  if IsUpgrade then
    WizardForm.WelcomeLabel2.Caption := Note;
end;

// ---------- 按用户勾选组织下载清单 ----------
//
//  只把「勾了」且「本机没随包版本」的模块加进下载页。随包版本由预处理器
//  开关（OfflineLlama / OfflinePiper / OfflineVoice）决定，所以离线装法
//  下这段代码里对应的分支根本不会被编译进去。
function BuildDownloadList(): Boolean;
var
  DoModel: Boolean;
begin
  Result := False;
  DownloadPage.Clear;

#ifndef OfflineLlama
  if WizardIsComponentSelected('opt\engine') then
  begin
    DownloadPage.Add(LlamaUrl, 'llama-engine.zip', '');
    Result := True;
  end;
#endif

#ifndef OfflinePiper
  if WizardIsComponentSelected('opt\voice') then
  begin
    DownloadPage.Add(PiperUrl, 'piper_windows_amd64.zip', '');
    Result := True;
  end;
#endif

#ifndef OfflineVoice
  if WizardIsComponentSelected('opt\voice') then
  begin
    DownloadPage.Add(VoiceUrlBase + '/en_US-amy-medium.onnx', 'en_US-amy-medium.onnx', '');
    DownloadPage.Add(VoiceUrlBase + '/en_US-amy-medium.onnx.json', 'en_US-amy-medium.onnx.json', '');
    Result := True;
  end;
#endif

  if WizardIsComponentSelected('opt\model') then
  begin
    DoModel := True;
    // ★ 升级安装时 {app}\data\models 未必是用户正用着的那个数据目录：
    //   老版本把数据放在 %APPDATA%\WordWise，而解析顺序里「已有 AppData 数据」
    //   优先于「exe 同级 data」。所以先问一句再下 —— 白等 400 MB 之后
    //   才发现放错位置，是不可接受的体验。
    if IsUpgrade then
    begin
      DoModel := MsgBox(
        ExpandConstant('{cm:WWMUpgradeL1}') + #13#10#13#10 +
        ExpandConstant('{cm:WWMUpgradeL2}') + ' ' + ExpandConstant('{app}') + '\data\models' + #13#10 +
        ExpandConstant('{cm:WWMUpgradeL3}') + #13#10#13#10 +
        ExpandConstant('{cm:WWMUpgradeQ}'), mbConfirmation, MB_YESNO) = IDYES;
    end;
    if DoModel then
    begin
      DownloadPage.Add(ModelUrl, '{#ModelFile}', '');
      Result := True;
    end;
  end;
end;

// ---------- 页面推进：目录纠偏 + 触发下载 ----------
function NextButtonClick(CurPageID: Integer): Boolean;
var
  Error: String;
  Has: Boolean;
  Answer: Integer;
begin
  Result := True;

  // 升级场景：用户把目录改到了别处 → 提醒一次。
  // 目的不是拦他，是让他知道「装到别处 = 两个独立副本，进度不会跟过去」。
  //
  // 用 MB_YESNOCANCEL 三选一，三个分支都能明确收场：
  //   「是」  → 改回原目录并继续（覆盖升级）
  //   「否」  → 尊重所选目录并继续（变成两个独立副本）
  //   「取消」→ 留在本页，不改任何东西，下次再点还能问
  // 关键：这里是「提醒」，不是「拦截」——绝不能出现「只有点否才能继续」。
  if (CurPageID = wpSelectDir) and IsUpgrade and not WarnedOffPrevDir then
  begin
    if CompareText(WizardForm.DirEdit.Text, PrevDir) <> 0 then
    begin
      Answer := MsgBox(ExpandConstant('{cm:WWPrevFound}') + #13#10 + PrevDir + #13#10#13#10 +
                ExpandConstant('{cm:WWPrevChosen}') + #13#10 + WizardForm.DirEdit.Text + #13#10#13#10 +
                ExpandConstant('{cm:WWPrevWarn}') + #13#10#13#10 +
                ExpandConstant('{cm:WWPrevYes}') + #13#10 +
                ExpandConstant('{cm:WWPrevNo}') + #13#10 +
                ExpandConstant('{cm:WWPrevCancel}'),
                mbConfirmation, MB_YESNOCANCEL);
      if Answer = IDYES then
      begin
        WizardForm.DirEdit.Text := PrevDir;
        WarnedOffPrevDir := True;
        Result := True;    // 目录已改回原目录，直接继续推进
      end
      else if Answer = IDNO then
      begin
        WarnedOffPrevDir := True;   // 用户执意装别处，不再追问
        Result := True;             // 尊重所选目录，继续推进
      end
      else
        Result := False;   // IDCANCEL：留在本页，下次还能再问
    end;
  end;

  // 点「安装」之后、真正写文件之前：先把勾了的可选模块下载到 {tmp}
  if CurPageID = wpReady then
  begin
    Has := BuildDownloadList();
    if Has then
    begin
      DownloadPage.Show;
      try
        try
          DownloadPage.Download;   // 下载到 {tmp}
        except
          if DownloadPage.AbortedByUser then
          begin
            Log('用户取消了下载');
            Result := False;
          end
          else
          begin
            Error := Format('%s: %s', [DownloadPage.LastBaseNameOrUrl, GetExceptionMessage]);
            // ★ 下载失败**绝不阻断主程序安装**：没有这些可选模块，
            //   查词 / 翻译 / 背诵 / 图谱全都照常可用，之后在软件内的
            //   「一键部署」「语音包管理」随时能补，走的是同一批镜像源。
            Result := MsgBox(ExpandConstant('{cm:WWDLFailHead}') + AddPeriod(Error) + #13#10#13#10 +
              ExpandConstant('{cm:WWDLFailBody}') + #13#10#13#10 +
              ExpandConstant('{cm:WWDLFailAsk}'),
              mbError, MB_YESNO) = IDYES;
          end;
        end;
      finally
        DownloadPage.Hide;
      end;
    end;
  end;
end;

{ ---------- 安装前：结束正在运行的实例 ---------- }
function InitializeSetup(): Boolean;
begin
  { 具体结束动作放在 PrepareToInstall 中执行 }
  Result := True;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ResultCode: Integer;
begin
  Result := '';
  { 尝试静默结束正在运行的实例，避免文件占用导致安装失败 }
  Exec('taskkill', '/F /IM {#MyAppExeName}', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Sleep(600);
end;

{ ---------- 安装后：引导配置本地大模型（两种办法任选） ---------- }
procedure CurStepChanged(CurStep: TSetupStep);
var
  ErrorCode: Integer;
begin
  if CurStep = ssPostInstall then
  begin
    // 首次安装时给出 AI 服务配置提示。
    // 标记文件放在「安装目录\data」里 —— 那正是运行时的默认数据目录，
    // 不再往「用户 AppData」写东西（用户要的就是「别写 C 盘」）。
    if not FileExists(ExpandConstant('{app}\data\.llm_hinted')) then
    begin
      if MsgBox(ExpandConstant('{cm:WWDoneTitle}') + #13#10 + #13#10 +
                ExpandConstant('{cm:WWDoneBody1}') + #13#10 +
                ExpandConstant('{cm:WWDoneA1}') + #13#10 +
                ExpandConstant('{cm:WWDoneA2}') + #13#10 +
                ExpandConstant('{cm:WWDoneA3}') + #13#10 +
                ExpandConstant('{cm:WWDoneA4}') + #13#10 +
                ExpandConstant('{cm:WWDoneB1}') + #13#10 +
                ExpandConstant('{cm:WWDoneB2}') + #13#10 +
                ExpandConstant('{cm:WWDoneC1}') + #13#10 +
                ExpandConstant('{cm:WWDoneC2}') + #13#10 + #13#10 +
                ExpandConstant('{cm:WWDoneTail}') + #13#10 + #13#10 +
                ExpandConstant('{cm:WWDoneAsk}'),
                mbConfirmation, MB_YESNO) = IDYES then
      begin
        ShellExec('open', 'https://lmstudio.ai/', '', '', SW_SHOWNORMAL, ewNoWait, ErrorCode);
      end;

      ForceDirectories(ExpandConstant('{app}\data'));
      SaveStringToFile(ExpandConstant('{app}\data\.llm_hinted'), '1', False);
    end;
  end;
end;

// ---------- 卸载前：询问是否保留学习数据 ----------
//
//  数据可能落在两处：
//    - app\data                   新装默认（跟着软件目录走）
//    - userappdata\WordWise       老版本的位置，升级用户还在用
//  两处都要问，否则会出现「用户以为数据删了、其实还在 AppData 里躺着」。
//
//  同样用 // 注释：下面会用到带花括号的常量，写进 { } 注释里会提前收尾。

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  NewDir, OldDir, Found, Msg: String;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    NewDir := ExpandConstant('{app}\data');
    OldDir := ExpandConstant('{userappdata}\WordWise');

    Found := '';
    if DirExists(NewDir) then Found := NewDir;
    if DirExists(OldDir) then
    begin
      if Found <> '' then Found := Found + #13#10 + OldDir
      else Found := OldDir;
    end;

    if Found = '' then Exit;

    { 用户可能在设置页把数据目录改到过别的地方；那种情况这里管不到，
      所以顺带提醒一句「指针文件」的存在。 }
    Msg := ExpandConstant('{cm:WWUninstAsk}') + #13#10 + #13#10 +
           ExpandConstant('{cm:WWUninstAt}') + #13#10 + Found + #13#10 + #13#10 +
           ExpandConstant('{cm:WWUninstKeep}') + #13#10 +
           ExpandConstant('{cm:WWUninstNote}');

    if MsgBox(Msg, mbConfirmation, MB_YESNO) = IDYES then
    begin
      if DirExists(NewDir) then DelTree(NewDir, True, True, True);
      if DirExists(OldDir) then DelTree(OldDir, True, True, True);
    end;
  end;
end;
