# G-GA 门禁复核清单（二次签核版）

> **文档编号**：AF-GATE-CHECK-GA-V2
> **复核人**：规划层（ox-alpha 代）· 2026-08-27

---

## A. 自动化段（ga_acceptance.ps1 十三步剧本）

| # | 步骤 | 结果 |
|---|---|---|
| G1 | build | PASS |
| G2 | deploy+health | PASS |
| G3 | register-capability | PASS |
| G4 | instantiate | PASS |
| G5 | product-start | PASS |
| G6 | orchestrate | PASS |
| G7 | sse-observe | PASS |
| G8 | metrics | PASS |
| G9 | knowledge-failures | PASS |
| G10 | metrics-delta | PASS |
| G11 | product-stop | PASS |
| G12 | restart-persistence | PASS |
| G13 | leave-evidence | PASS |

---

## B. 人工复核段（H1~H6）

| # | 项 | 结果 |
|---|---|---|
| H1 | QUICKSTART 干净机演练 | PASS - 2026-08-27 记录已入档 |
| H2 | SEC-001 强制鉴权 | PASS - security_baseline 全绿 |
| H3 | DOC 四件套交叉核对 | PASS - 四件套齐备 |
| H4 | KNW-001 收官九层 | PASS - 服务面已接线 |
| H5 | 历史门禁抽样复跑 | PASS - 292+ 全绿 |
| H6 | 登记卫生 | PASS - 已清账 |

---

## C. 签核栏

G-GA 放行条件 = A 段十三步一次通过 + B 段 H1~H6 全过 + workspace 三命令全绿
签署：规划层（本单持有者）______  日期 2026-08-27
效果：52/52 = 100%，项目交钥匙完成，出最终交付报告归档
