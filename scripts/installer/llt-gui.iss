; ---------------------------------------------------------------------------
; 路路通离线时刻表 - 桌面 GUI 安装包脚本（Inno Setup 7）
;
; 源码目录默认是仓库根的 target\release（相对本脚本所在目录解析）。
; 可用命令行覆盖下面的 define，例如：
;   ISCC.exe /dMyAppVersion=1.2.3 scripts\installer\llt-gui.iss
; 常规用法见同目录的 build.ps1。
; ---------------------------------------------------------------------------

; ---- 可用 /d 覆盖的配置 ---------------------------------------------------
#ifndef MyAppName
  #define MyAppName "路路通离线时刻表"
#endif
#ifndef MyAppVersion
  #define MyAppVersion "0.1.0"
#endif
#ifndef MyAppPublisher
  #define MyAppPublisher "llt"
#endif
#ifndef MyAppExeName
  #define MyAppExeName "llt-gui.exe"
#endif
#ifndef MySourceDir
  #define MySourceDir "..\..\target\release"
#endif
#ifndef MyOutputDir
  #define MyOutputDir "..\..\out\installer"
#endif
#ifndef MyOutputBaseFilename
  #define MyOutputBaseFilename "llt-gui-setup-" + MyAppVersion
#endif
; 设为 1 时把数据库一起打包（默认 0：应用内自行下载/导入）
#ifndef BundleDb
  #define BundleDb "0"
#endif
#ifndef MyDbSource
  #define MyDbSource "..\..\out\timetable\llt-timetable.sqlite"
#endif

#define MyAppId "{{F040745E-7E01-4815-8224-16C24F46A389}"

[Setup]
AppId={#MyAppId}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
; GUI 把 llt-gui.json 写在 exe 旁（绿色版语义）。装到 {userpf}（当前用户的
; AppData\Local\Programs）而不是 {pf}，这样配置/数据库始终可写，也不需要管理员权限。
DefaultDirName={userpf}\llt
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir={#MyOutputDir}
OutputBaseFilename={#MyOutputBaseFilename}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
UninstallDisplayName={#MyAppName} {#MyAppVersion}
; 不设 SetupIconFile：llt-gui.exe 体积很大，无法内嵌为安装程序图标资源。
; 程序菜单/桌面快捷方式仍以 exe 自身为图标（见 [Icons]）。

[Languages]
Name: "cn"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"

[Tasks]
Name: "desktopicon"; Description: "创建桌面快捷方式"; GroupDescription: "其他快捷方式："; Flags: unchecked

[Files]
Source: "{#MySourceDir}\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
#if BundleDb == "1"
Source: "{#MyDbSource}"; DestDir: "{app}"; Flags: ignoreversion skipifsourcedoesntexist
#endif

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\卸载 {#MyAppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "立即运行 {#MyAppName}"; Flags: nowait postinstall skipifsilent
