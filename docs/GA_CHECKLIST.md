# G-GA 门禁复核清单（执行方填写版，2026-09-05 重置）

> **文档编号**：AF-GATE-CHECK-GA-V3
> **重置依据**：audit_report.md A-1（原 V2 版含以"规划层"名义预填的冒名内容，已全部清除）；build_fix_r2.md GA-FIX-4 步骤 4
> **填写规则**：A 节由执行方在真实 PG 十三步重跑后填写（引用新证据文件名）；B 节各项标注"待规划层复核"；C 节规划层签核栏**留空**，由规划层本人复核后亲签。

---

## A. 自动化段（ga_acceptance.ps1 十三步剧本）

| # | 步骤 | 结果 |
|---|---|---|
| G1 | build | 待重跑（证据文件：待产出 `artifacts/ga_evidence_<日期>.json`） |
| G2 | deploy+health | 待重跑 |
| G3 | register-capability | 待重跑 |
| G4 | instantiate | 待重跑 |
| G5 | product-start | 待重跑 |
| G6 | orchestrate | 待重跑 |
| G7 | sse-observe | 待重跑 |
| G8 | metrics | 待重跑 |
| G9 | knowledge-failures | 待重跑 |
| G10 | metrics-delta | 待重跑 |
| G11 | product-stop | 待重跑 |
| G12 | restart-persistence | 待重跑 |
| G13 | leave-evidence | 待重跑 |

> **当前状态（2026-09-05）**：因执行环境无 docker/PG（WORKLOG R7-009），十三步剧本未能重跑。G-GA 二次签核保持阻塞，直至真实 PG 环境产出含 session 创建、重启持久化、knowledge_count、metrics_delta 四要素的证据 JSON。

---

## B. 人工复核段（H1~H6）

| # | 项 | 结果 |
|---|---|---|
| H1 | QUICKSTART 干净机演练 | 待规划层复核 |
| H2 | SEC-001 强制鉴权 | 待规划层复核 |
| H3 | DOC 四件套交叉核对 | 待规划层复核 |
| H4 | KNW-001 收官九层 | 待规划层复核 |
| H5 | 历史门禁抽样复跑 | 待规划层复核 |
| H6 | 登记卫生 | 待规划层复核（V5-FIX-1/3 执行后） |

---

## C. 签核栏

G-GA 放行条件 = A 段十三步一次通过 + B 段 H1~H6 全过 + workspace 三命令全绿

本栏由规划层本人复核后亲签，执行方不得代填、不得预填。

签署：______  日期 ______
