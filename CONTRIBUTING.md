# 提交规范

采用 [Conventional Commits](https://www.conventionalcommits.org/)，一句话说明改了什么、为什么。

```
<type>(<scope>): <描述>

[可选的正文]
```

## type

| type | 用途 |
|---|---|
| `feat` | 新功能 |
| `fix` | 修复缺陷 |
| `refactor` | 重构，不改变行为 |
| `perf` | 性能优化 |
| `test` | 增删改测试 |
| `docs` | 文档、注释 |
| `build` | Cargo、依赖、构建配置 |
| `chore` | 杂项（`.gitignore`、脚本等） |

## scope

可省略；跨多个包时省略或写多个，如 `cli,gui`。

- `cli` — `cli/`
- `gui` — `gui/`、`android/`（同一套界面，Android 只是入口与打包）
- `lltdata` — `crates/lltdata/`（下载 / 解包 / 解码 / 写库，三端共用）
- `tools` — `tools/` 下的解码脚本
- `data` — `out/` 产物与数据版本说明
- `docs` — `docs/` 逆向报告

## 规则

- 描述用中文，动词开头，不加句号，不超过 50 字。
- 一个提交只做一件事；纯格式化、重排与逻辑修改分开提交。
- 破坏性变更在 type 后加 `!`，正文写明迁移方式。
- 关联 issue 放在正文末尾，如 `Closes #12`。

## 示例

```
feat(cli): 支持 query od 按中转次数过滤
fix(lltdb): 跨零点车次日期计算错误
refactor(gui)!: 车次详情改为只读视图
docs: 补充 an.db v2026.10 表结构说明
build: 升级 rusqlite 到 0.41
```
