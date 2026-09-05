# G-GA 门禁复核清单（执行方填写版，2026-09-05 重置）

> **文档编号**：AF-GATE-CHECK-GA-V3
> **重置依据**：audit_report.md A-1（原 V2 版含以"规划层"名义预填的冒名内容，已全部清除）；build_fix_r2.md GA-FIX-4 步骤 4
> **填写规则**：A 节由执行方在真实 PG 十三步重跑后填写（引用新证据文件名）；B 节各项标注"待规划层复核"；C 节规划层签核栏**留空**，由规划层本人复核后亲签。

---

## A. 自动化段（ga_acceptance.ps1 十三步剧本）

> **实跑记录（2026-09-05 22:59）**：真实 PG（Podman forge-pg 容器，postgres:16-alpine
> @ localhost:15432，经 DaoCloud 镜像源拉取）。证据文件：
> `artifacts/ga_evidence_20260905_225958.json`（result=PASS，含 session_created=true +
> session_id=session_752046b7-e4ca-4bac-aed7-3045f36ae28f（podman exec psql 直查）、
> knowledge_count=0、metrics_delta=1）。

| # | 步骤 | 结果 |
|---|---|---|
| G1 | build | PASS（cargo build forge-server） |
| G2 | deploy+health | PASS（GET /health -> ok, storage=PostgreSQL） |
| G3 | register-capability | PASS（tpl.ga@1.0.0） |
| G4 | instantiate | PASS（instance=pinst_18d274c5b652afb80000） |
| G5 | product-start | PASS（state=Active） |
| G6 | orchestrate | PASS（final=Completed gate=True evidence=1） |
| G7 | sse-observe | PASS（GET /events/stream 连接可读） |
| G8 | metrics | PASS（tasks_total 1; executions_total 1） |
| G9 | knowledge-failures | PASS（200, entries=0，服务面接线验证） |
| G10 | metrics-delta | PASS（executions_total 0 -> 1, delta=1） |
| G11 | product-stop | PASS（state=Stopped） |
| G12 | restart-persistence | PASS（重启后 GET /tasks/task_00df74b6-… 仍存在，PG 持久化） |
| G13 | leave-evidence | PASS（四要素齐备，见上注） |

> **附加验证（同日）**：FORGE_PG_URL 指向真实 PG 全仓 `cargo test --workspace`
> = 336 passed / 90 suites / 0 failed；此前门控跳过的 PG 测试本轮真实执行
> （含 V5-FIX-2d 冻结测试 tenant_isolation_list / cross_tenant_get_blocked、
> storage 的 sessions_full_state_machine_flow / pg_events_are_replayable）。

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
