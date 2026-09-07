# KNW 全环演练记录（KNW-101，G-V6C 门禁第 2 条补强）

> 模板风格仿 docs/DR_EXERCISE.md（R6 惯例）：日期/执行人/演练范围/步骤结果表/发现/证据路径/签核——缺项=门禁不通过。
> 演练口径：AF-BP-V60C-001（build_v60c.md）KNW-101 全环（沙箱验证 → 人工逐条 approve → ≤5 用例分支补丁 → **人工**合入），全程 forge.exe 真实执行，所有 git 实验在系统临时目录，主仓代码/文档/git 状态零改动。

| 日期 | 执行人 | 演练范围 | reproduced | 步骤结果 | 证据路径 | 签核 |
|---|---|---|---|---|---|---|
| 2026-09-07 | drill-user（执行方代理） | KNW-101 全环 + 人工合入模拟 + 红线复测（D3 口径） | 1/2 | S1~S8 全 PASS | knw_drill_20260907_080048.json | （留空待签） |

## 演练范围

- 临时 git 仓库（`git init -b main` + README 首提交，HEAD `c4c506586439…`）中走通 KNW-101 全环：建议构造 → `knowledge-verify` 沙箱复现 → 人工逐条 `knowledge-approve`（approver=drill-user）→ `knowledge-pr` 生成分支补丁 → **人工** `git clone` + `git am` 合入模拟 → 用例落位与主干 HEAD 不变核验。
- 红线复测：①未 approve 直接 pr 必须失败；②6 条已 approve 用例必须被拒（D3）。
- 限制确认：`knowledge-suggest` 依赖 InMemoryKnowledgeBase 空库。

## 步骤结果表

| 步骤 | 命令/动作 | 结果 | 关键输出 |
|---|---|---|---|
| S1 临时仓库 | git init + README 首提交 | PASS | HEAD=`c4c506586439d3410b8b06d835b0e9c84f753a18` |
| S2 建议构造 | 手工 suggestions.json（2 条） | PASS | ①`PermissionDenied:write_file`（input 带 path/content）；②`ToolError:write_file`（write_file 合法形态 green case） |
| S3 沙箱验证 | `knowledge-verify` | PASS | ①reproduced=true（status=PermissionDenied 类别匹配）；②reproduced=false（观测 PermissionDenied ≠ 建议 ToolError） |
| S4 人工批准 | `knowledge-approve`（仅①，approver=drill-user） | PASS | 账本 1 行，case_hash=`d6189f3b1ab2…9dfc` |
| S5 补丁生成 | `knowledge-pr` | PASS | branch=`knw_20260906_d6189f3b`，cases=1，patch=`0001-KNW-101-PermissionDenied-write_file.patch`，head_before=`c4c506586439`；worktree 用后即删 |
| S6 人工合入模拟 | `git clone` → `git am` | PASS | am 一次通过（exit=0，未降级 git apply）；clone 内出现 `tests/knowledge_cases/permissiondenied_write_file.json`；clone HEAD `c4c5065→294527f`，**主仓 HEAD 前后一致**、status 干净 |
| S7a 红线：无 approve | `knowledge-pr`（空账本） | PASS | exit=1「no approved cases in ledger — 先 knowledge-verify + knowledge-approve」，无补丁产出 |
| S7b 红线：6 用例 | 6 条 approve + `knowledge-pr` + 冻结测试 | PASS | 库级硬门禁：`forge_pr_case_cap_five` 实跑 ok（6 条 → InvalidState）；CLI 层实测为静默截断至 5（见发现 F3） |
| S8 限制确认 | `knowledge-suggest` | PASS | 「wrote 0 suggestions」（空 JSON 数组）——空库无注入口，演练以手工建议文件绕过 |
| 附加 冻结测试 | `cargo test -p forge-knowledge` | PASS | 8+4=12 全绿（含 HEAD 不变 / 白名单 / 账本门禁 / roundtrip） |

## 发现

1. **F1（限制确认，供规划层落 R7）**：CLI `knowledge-suggest` 依赖 `InMemoryKnowledgeBase::default()` 空库且无任何知识注入子命令——真实闭环缺“注入口”。本次演练以手工构造 suggestions.json 绕过，该限制已如实记录。
2. **F2**：ToolError 类失败在冻结只读沙箱内不可构造：`EchoTool.invoke` 恒 Ok（router.rs）；`write_file`（WorkspaceWrite）在策略层即被拒，工具体不执行。故②按预案降级为 write_file 合法形态 green case，实测 not reproduced——同时构成 R5（沙箱不可放宽）的实测证据。另：unknown-tool 路由未命中（NotFound→Failed→classify=ToolError）是沙箱内唯一 ToolError 向量，本次未采用，供规划层参考。
3. **F3**：CLI `knowledge-pr` 对已 approve 用例做 `.take(5)` 静默截断（帮助文本即“仅取已 approve 的前 5 条用例”）：6 条已 approve 时 CLI 层不拒绝而是取前 5 条出补丁；D3 硬门禁由库级 `forge_pr`（InvalidState）承担并经冻结测试实跑验证。建议规划层知悉该分层语义。
4. **F4**：pattern 消毒实测：`PermissionDenied:write_file` → `permissiondenied_write_file.json`；补丁逐行仅触碰 `tests/knowledge_cases/` 白名单。

## 证据路径

- 结构化证据：`artifacts/knw_drill_20260907_080048.json`（各步 PASS/FAIL + report.json 内容 + 补丁文件名 + HEAD 前后值）
- 演练现场（系统临时目录，OS 回收前可查）：`D:/Temp/knw_drill_vzCSFORK/`（suggestions.json / report.json / approvals.jsonl / patches/ / knw_repo / clone_repo）
- 冻结测试：`cargo test -p forge-knowledge`（knowledge/tests/forge_pr.rs 8 项 + suggest_test.rs 4 项）

## 签核

| 项 | 签核人 | 日期 | 结论 |
|---|---|---|---|
| 规划层 | | | |
