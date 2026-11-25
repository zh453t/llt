# 本机 GUI 调试脚本

界面验证用的 PowerShell 脚本（`scripts/debug/`），与安装包脚本一起放在 `scripts/` 下。
目标进程默认是 `llt-gui`（`shot.ps1` 全屏模式除外），都可用 `-ProcessName` 覆盖。

从仓库根运行：`pwsh -File scripts\debug\<脚本名>.ps1 <参数>`。
截图等产物默认写仓库根的 `out\`（被 `.gitignore` 忽略），不污染版本库。

| 脚本 | 用途 | 常用参数 |
|---|---|---|
| `_common.ps1` | 公共 Win32 封装 / 工具函数，由其它脚本 dot-source，**不要直接运行** | — |
| `focus_click.ps1` | 置前 + 点击 / 双击 / 拖拽（远程会话最稳） | `-X -Y [-X2 -Y2] [-Action click\|dblclick\|drag]` |
| `mouse.ps1` | 通用输入：click / dblclick / drag / type（中文）/ key（按键） | `-Action ... -X -Y [-X2 -Y2] [-Text]` `[-NoFocus]` |
| `win.ps1` | 打印可见矩形；默认只打印，可置前 + 点击 / 双击 / 拖拽 | `[-Action info\|click\|dblclick\|drag] [-TopMost] -X -Y` |
| `slowdrag.ps1` | 按住 600ms 再分 150 步拖动，模拟触摸屏长按换序 / 拖列宽 | `-X -Y -X2 -Y2` |
| `hover.ps1` | 移动鼠标并停留，触发 hover 提示 | `-X -Y [-DurationMs 700]` `[-NoFocus]` |
| `move.ps1` | 移动 / 缩放窗口，输出缩放后的可见矩形 | `-Width -Height [-Left -Top]` |
| `paste.ps1` | 写剪贴板 + Ctrl+V 粘贴；`-Text` 必填，结束尽量恢复原剪贴板文本 | `-Text "北京"` `[-NoFocus]` |
| `shot.ps1` | 截图。默认全虚拟屏；`-Window` 用 PrintWindow + DWM 可见框抓整窗（比屏幕宽也抓得全、无黑边） | `[-Window] [-Path] [-ProcessName]` |

## egui-mcp（协议驱动，不走鼠标）

除了上面这些 Win32 脚本，`llt-gui` 还可以开一条**进程内检查通道**，让 `egui-mcp` 直接读
界面树、注入点击 / 输入、截图、改窗口尺寸（不依赖窗口置前，远程会话也能用）：

```powershell
# 只有带 inspection feature 的构建才有这条通道（缺省关，发布包没有它）
$env:EGUI_INSPECTION = '1'
cargo run -p llt-gui --features inspection
```

跑起来后向 `127.0.0.1:5719` 连上即可（pi 的 `egui` MCP 已配好）。注意两点：
- 点表格**行**要落在单元格的空白处（文字标签会把点击吃掉），这是测试工具的限制，不是应用 bug。
- 看列宽 / 布局别靠截图像素推算：需要精确值临时加 `eprintln!`，验完删掉（见 AGENTS.md §4）。

## 行为约定与注意事项

- **统一公共逻辑**（`_common.ps1`）：DPI 感知、找带窗口的进程（多实例不再选错）、置前
  （`AttachThreadInput` + ALT 脉冲，失败自动重试 3 次）、点击 / 拖拽（按下前自动纠正光标、
  拖拽含 ≥400ms 长按门槛，途中被碰动只告警）、中文走剪贴板粘贴。
- `win.ps1` 无参时**只打印矩形不点击**（以前默认点 0,0 会点到任务栏）；`-TopMost` 置顶只在
  本次动作期间生效，结束（含异常）后必定还原为 `HWND_NOTOPMOST`。
- `mouse.ps1` 的 `type` 用剪贴板粘贴，中文可靠且不用转义；`key` 才是原始 `SendKeys`，
  特殊字符 `+ ^ % ~ ( ) { } [ ]` 需转义，不要用它输中文。
- 遇到「置前失败 / 光标被移动」告警，按 AGENTS.md 的说法重试即可；脚本已自动重试并在
  拖拽途中给出告警，需要时重新执行一次。
- 坐标是**物理像素**（统一 `SetProcessDPIAware`），不是 egui 逻辑点；不同机器缩放不同，
  不要照抄数字。需要精确 Rect 用 `win.ps1` 或在代码里临时 `eprintln!`。
- `shot.ps1` 默认 `CopyFromScreen` 只截可见区域；布局验证一律用 `-Window`（已验证 wgpu
  窗口可抓到完整中文内容，非黑图，尺寸与 DWM 可见框一致）。
- `paste.ps1` 会临时覆盖剪贴板并在结束时尽量还原**文本**；剪贴板里若是图片等非文本内容，
  则无法完整还原。默认会先置前，避免粘错窗口。
- 除 `shot.ps1` 全屏模式外都要求 `llt-gui` 正在运行；找不到带窗口的进程会给出明确报错。
- 用 `pwsh -File` 起独立进程；公共封装已做类型存在性判断，同一会话内重复 dot-source 也不会
  冲突。

## 可用性测试记录（2026-11-24，修复后）

- 9 个 `.ps1` 全部通过 PowerShell 语法解析（0 错误）；启动 release GUI 后逐个冒烟，
  除受用户操作干扰的个例（按告警重试即可）外均退出码 0、无异常。
- `win.ps1` 无参只打印矩形；`-TopMost` 执行后实测 `WS_EX_TOPMOST = False`（已还原）。
- `move.ps1` 输出的可见矩形与实测一致；`focus_click.ps1` 报 `foreground is app: True`。
- `shot.ps1 -Window` 输出 1174×787 PNG，与 `win.ps1` 打印的 DWM 可见框完全一致（无黑边）。
- `paste.ps1` 执行后剪贴板内容恢复为执行前文本；`mouse.ps1` 的 click/type/key 均可用。
- 修复过程中发现并消除了两个移植错误（`MoveWindow` 参数重名、`GetCurrentThreadId` 所在的
  DLL 写错），均已修正并复测通过。
