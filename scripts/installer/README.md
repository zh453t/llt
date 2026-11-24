# 桌面 GUI 安装包（Inno Setup）

把 `target\release\llt-gui.exe` 打成 Windows 安装程序，产物在 `out\installer\`（该目录被 `.gitignore` 忽略）。

## 用法

```powershell
# 先确保已编译 release，再打包（版本自动读 Cargo.toml）
pwsh -File scripts/installer/build.ps1

# 一步到位：先 cargo build --release 再打包
pwsh -File scripts/installer/build.ps1 -Build

# 连数据库一起打包（默认不带，应用内可下载/导入）
pwsh -File scripts/installer/build.ps1 -BundleDb

# 手动指定版本 / 编译器
pwsh -File scripts/installer/build.ps1 -Version 1.2.3 -Iscc 'C:\Program Files\Inno Setup 7\ISCC.exe'
```

也可直接调用编译器：`ISCC.exe /dMyAppVersion=1.2.3 scripts\installer\llt-gui.iss`。

## 设计说明

- **按用户安装**：默认装到 `{userpf}\llt`（即 `AppData\Local\Programs\llt`）。GUI 把 `llt-gui.json` 写在 exe 旁，
  装到 `Program Files` 会写不进去；按用户安装可保证配置可写，且不需要管理员权限（`PrivilegesRequired=lowest`）。
- **不带数据库**：与 Android 分发策略一致，首次启动在「设置 → 数据」下载/导入即可；`-BundleDb` 可覆盖。
- **图标**：安装程序用 Inno 默认图标（`llt-gui.exe` 太大，无法内嵌）；程序菜单、桌面快捷方式
  和卸载项一应俱全，快捷方式图标取自 exe 本身。
