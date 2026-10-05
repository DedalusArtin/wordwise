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
;    installer\output\WordWise-Setup-0.35.0.exe
;
;  前置条件：
;    先执行 cargo build --release，确保 wordwise.exe 已生成
; ============================================================

#define MyAppName "WordWise"
#define MyAppVersion "0.35.0"
#define MyAppPublisher "DedalusArtin"
#define MyAppURL "https://github.com/DedalusArtin/wordwise"
#define MyAppExeName "wordwise.exe"
#define MyAppId "{{8F3C2A41-7B5E-4D9A-A1C6-2E8D4F6B9C03}"

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

; 默认装到用户目录，避免 UAC 提权，符合 Tauri 的 currentUser 安装模式
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

{ ---------- 安装后：引导配置 LM Studio ---------- }
procedure CurStepChanged(CurStep: TSetupStep);
var
  ErrorCode: Integer;
begin
  if CurStep = ssPostInstall then
  begin
    { 首次安装时给出 LM Studio 配置提示 }
    if not FileExists(ExpandConstant('{userappdata}\WordWise\.llm_hinted')) then
    begin
      if MsgBox('安装完成！' + #13#10 + #13#10 +
                'WordWise 的「AI 单词讲解」依赖本地 LM Studio 大模型：' + #13#10 +
                '  1. 打开 LM Studio，加载一个中文能力较好的模型' + #13#10 +
                '  2. 进入 Developer 标签，点击 Start Server' + #13#10 +
                '  3. 回到 WordWise，点击左下角状态条即可连接' + #13#10 + #13#10 +
                '未配置 LM Studio 时，在线查词功能仍可正常使用。' + #13#10 + #13#10 +
                '是否现在打开 LM Studio 官网下载页？',
                mbConfirmation, MB_YESNO) = IDYES then
      begin
        ShellExec('open', 'https://lmstudio.ai/', '', '', SW_SHOWNORMAL, ewNoWait, ErrorCode);
      end;

      ForceDirectories(ExpandConstant('{userappdata}\WordWise'));
      SaveStringToFile(ExpandConstant('{userappdata}\WordWise\.llm_hinted'), '1', False);
    end;
  end;
end;

{ ---------- 卸载前：询问是否保留学习数据 ---------- }
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  DataDir: String;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    DataDir := ExpandConstant('{userappdata}\WordWise');
    if DirExists(DataDir) then
    begin
      if MsgBox('是否同时删除学习数据（词库与背诵记录）？' + #13#10 + #13#10 +
                '数据位置：' + DataDir + #13#10 + #13#10 +
                '选择「否」将保留数据，日后重新安装可继续使用原有进度。',
                mbConfirmation, MB_YESNO) = IDYES then
      begin
        DelTree(DataDir, True, True, True);
      end;
    end;
  end;
end;
