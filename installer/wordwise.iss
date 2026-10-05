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
;    installer\output\WordWise-Setup-0.41.0.exe
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
  #define MyAppVersion "0.41.0"
#endif

; 相对本 .iss 文件定位构建产物；若使用自定义 target 目录，
; 可通过 /DMySourceDir=... 覆盖
#ifndef MySourceDir
  #define MySourceDir "..\src-tauri\target\release"
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

; 界面：支持简体中文与英文
WizardStyle=modern
ShowLanguageDialog=auto
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog

; 架构限制
ArchitecturesInstallIn64BitMode=x64compatible
ArchitecturesAllowed=x64compatible

; 许可证与说明（可选，存在才显示）
; LicenseFile=..\LICENSE

[Languages]
Name: "chinesesimplified"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: checkedonce
Name: "quicklaunchicon"; Description: "{cm:CreateQuickLaunchIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "startup"; Description: "开机时自动启动 WordWise 侧边栏（随时查词）"; GroupDescription: "启动选项:"

[Files]
; 主程序
Source: "{#MySourceDir}\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
; 前端资源随 exe 内嵌，无需单独复制

; 若采用「未内嵌前端」的构建方式，可放开下面这行
; Source: "{#MySourceDir}\..\..\src\*"; DestDir: "{app}\src"; Flags: ignoreversion recursesubdirs createallsubdirs

; 随包 llama.cpp 引擎（本地大模型一键部署）
;  运行时从 GitHub Release 下引擎要 14 分钟（镜像限速），所以直接随包。
;  缺失时只是回退到联网下载，不影响主程序 → 用 skipifsourcedoesntexist。
Source: "..\vendor\llama\*.exe"; DestDir: "{app}\vendor\llama"; Flags: ignoreversion skipifsourcedoesntexist
Source: "..\vendor\llama\*.dll"; DestDir: "{app}\vendor\llama"; Flags: ignoreversion skipifsourcedoesntexist

; 附带文档
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist; DestName: "README.md"
Source: "..\BUILD.md"; DestDir: "{app}\docs"; Flags: ignoreversion skipifsourcedoesntexist
Source: "..\docs\*"; DestDir: "{app}\docs"; Flags: ignoreversion recursesubdirs createallsubdirs skipifsourcedoesntexist

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Comment: "背单词与 AI 讲解"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon; Comment: "背单词与 AI 讲解"
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
// ---------- 默认安装目录：优先非 C 盘 ----------
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
begin
  // 只看 D / E / F：这三者覆盖了绝大多数国产整机的「数据盘」，
  // 再往后找就容易碰到临时插上的 U 盘了
  Roots[0] := 'D:\';
  Roots[1] := 'E:\';
  Roots[2] := 'F:\';
  for I := 0 to 2 do
  begin
    if not DirExists(Roots[I]) then Continue;
    if TryUseDriveAsDefault(Roots[I]) then Break;
  end;
  // 一个都没成功 → 保持 localappdata 原默认，安装照常进行
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
      if MsgBox('安装完成！' + #13#10 + #13#10 +
                'WordWise 的「AI 单词讲解」需要一个模型服务，三种办法任选：' + #13#10 +
                '  A. 一键部署（推荐，无需额外软件）' + #13#10 +
                '     点窗口左下角「AI 服务」状态条 → 下载模型 → 启动' + #13#10 +
                '     引擎已随安装包附带，只需下载模型（约 400MB～1.1GB）' + #13#10 +
                '     想让每次开程序自动起，打开面板里的「启动时自动拉起」' + #13#10 +
                '  B. 连接已有的 LM Studio' + #13#10 +
                '     LM Studio 里 Start Server，回 WordWise 点「测试连接」' + #13#10 +
                '  C. 用在线 API（DeepSeek / 通义千问 / Kimi / 智谱 GLM 等）' + #13#10 +
                '     设置 → AI 服务 → 服务来源里选一家，填上 API Key 即可' + #13#10 + #13#10 +
                '不配置模型时，查词、翻译、背诵、知识图谱全部照常可用。' + #13#10 + #13#10 +
                '是否现在打开 LM Studio 官网下载页？',
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
    Msg := '是否同时删除学习数据（词库与背诵记录）？' + #13#10 + #13#10 +
           '数据位置：' + #13#10 + Found + #13#10 + #13#10 +
           '选择「否」将保留数据，日后重新安装可继续使用原有进度。' + #13#10 +
           '（如果你在设置里换过数据目录，那份数据不在上面，需要自己清理。）';

    if MsgBox(Msg, mbConfirmation, MB_YESNO) = IDYES then
    begin
      if DirExists(NewDir) then DelTree(NewDir, True, True, True);
      if DirExists(OldDir) then DelTree(OldDir, True, True, True);
    end;
  end;
end;
