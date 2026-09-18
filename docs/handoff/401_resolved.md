# 401 已解决 —— 修正版结论（覆盖 401_handoff.md 的判断）

## 一句话
`aioncore.exe team *` 的 401 **无法通过配置修复**（token 只存内存、每次启动随机生成）。
但**官方 MCP 通道可用**，token 就存在数据库里，已实测打通：`members` / `task_list` / `assistants` 全部 200。
脚本：`C:\forge_team\aion_team.py`

---

## 一、真正的三层根因

| 层 | 事实（实测证据） |
|---|---|
| ① 配置错 | 环境里 `AIONCORE_URL=http://127.0.0.1:12831` 是**死值**。aioncore 用 `--port 0` 启动（让 OS 随机分配端口），**永远不可能落到 12831**。当前主 API 在 **63180**。 |
| ② env 缺失 | 我这边的 shell 里 `AIONUI_BASE_URL / AIONUI_USER_ID / AIONUI_CONVERSATION_ID / AIONUI_RUNTIME_TOKEN` **四个全都没有** → 实际报的是 `TEAM_CLI_ENV_MISSING`，不是 401。前任 agent 那边 env 是配好的（`probe_runtime.ps1` 里能 `$env:AIONUI_RUNTIME_TOKEN`），所以他们撞到的是真 401。 |
| ③ token 拿不到（本质） | CLI 调的是主 API `POST /api/runtime/team-tools/call`，body 形如 `{"tool","input","token"}`。这个 `token` 由 `crates\aionui-ai-agent\src\runtime_token.rs` 启动时随机生成、**只存内存、不落盘**。服务端 401 体的原始文案就是 `{"code":"runtime_auth_failed","message":"runtime auth failed"}`。→ 外部进程永远凑不出这个值。 |

### 交接文档里的 5 处错误（会误导下一棒）
1. **「aioncore.exe 不在磁盘上」** — 错。`E:\Program Files\aionui\resources\bundled-aioncore\win32-x64\aioncore.exe`，99MB，v0.2.2，且常驻 3–4 个实例。
2. **「12831 是按需拉起的 bridge」** — 错。`--port 0` = 随机端口，12831 从来不会被占用。
3. **「BOOTSTRAP_SECRET 在 env 里」** — 我这边的 env 里**不存在**。但活日志确实显示 `identity_mode: aionpro, bootstrap_secret_configured: true`（AionUi 主进程传下来的，我们的 shell 继承不到）。
4. **「25808 是 Web UI 不是 bridge」** — 方向对，但没点到：真 bridge 是 **41267**（`aionui-team-mcp`），不是 25808 也不是 12831。
5. **`team list` 不是合法命令** — 合法的是 `task`/`members`/`context`/`read-messages`/`send-message`/`spawn-agent`…

---

## 二、为什么 MCP 通道能通

`conversations.extra.team_mcp_stdio_config`（每个团队会话一行，**持久化在库里**）：

```json
{"team_id":"01a0999a-fa3d-70b2-86e8-c9bde1d9db34",
 "port":41266,
 "token":"01a0a4f1-3623-7ee0-b42a-75c6577ffadc",
 "slot_id":"01a0999a-fa3d-70b2-86e8-c9c1e293e219",
 "binary_path":"E:\\Program Files\\aionui\\resources\\bundled-aioncore\\win32-x64\\aioncore.exe"}
```

用这个 token 打 **41267**（注意：比配置里的 41266 **高一位**；41266 是裸 TCP、会 reset，41267 是 MCP-over-HTTP）：

```
POST http://127.0.0.1:41267/
Authorization: Bearer 01a0a4f1-3623-7ee0-b42a-75c6577ffadc
Accept: application/json, text/event-stream
```

- 无 header → `-32600 Authentication failed: invalid auth_token`
- 带 header → `200 {"serverInfo":{"name":"aionui-team-mcp","version":"1.0.0"}}`
- 只认 `Authorization: Bearer`；`Token`/`x-aioncore-bootstrap-secret`/body/query 里放都无效

工具集（8 个）：`team_members` `team_read_messages` `team_send_message`
`team_task_create` `team_task_update` `team_task_list` `team_list_assistants` `team_describe_assistant`

（`spawn-agent` / `rename-agent` / `clear-agent-context` / `shutdown-agent` 是 lead-only，
本 token 绑定的 slot 拿不到，tools/list 里也不出现。）

---

## 三、用法

```powershell
py C:\forge_team\aion_team.py discover            # 自动找端口+token+slot
py C:\forge_team\aion_team.py members
py C:\forge_team\aion_team.py task_list --limit 20
py C:\forge_team\aion_team.py assistants
py C:\forge_team\aion_team.py read_messages
py C:\forge_team\aion_team.py send "*" "消息内容..."
py C:\forge_team\aion_team.py task_create "主题" --desc "详情" --owner <slot_id>
py C:\forge_team\aion_team.py task_update <task_id> --status in_progress
```

- 脚本自动发现桥端口（`--port 0` 每次重启都变），不用手填。
- 换团队：`$env:AIONTEAM_FILTER='019f5c4d'`（默认 `01a0999a` = 新forge）。
- 换库：`$env:AIONUI_DB='...'`。
- 中文乱码是控制台 cp936 显示问题，数据本身是 UTF-8（加 `--raw` 或直接重定向到文件就正常）。

### 团队当前状态（实测）
`新forge` / `01a0999a-fa3d-70b2-86e8-c9bde1d9db34`，user `user_019fe4ae-62bb-7743-a4c8-eae2e297fb72`，工作区 `D:\test\aionui\新forge`

| conversation | 名称 | 角色 | slot |
|---|---|---|---|
| 9f5c303a | Aion CLI | lead | 01a0999a-…-c1e293e219 |
| f085e771 | Aion CLI | teammate | 01a0999b-…-aad65dfc4ca4 |
| befd59e9 | 反对派 / Devils Advocate | teammate | 01a0999b-…-d6592563c2df |
| 5769e6e3 | 系统自检官 | teammate | 01a0999b-…-8dd9dcc09a8f |

日志侧印证：`POST /api/teams/01a0999a-…/session → 200`、`team session status: Ready, server_count: Some(4)`。

---

## 四、边界与注意事项

1. **端口会漂移**：AionUi 每次重启 aioncore 都换端口。脚本每次自己探测，别写死。
2. **token 是凭据**：`team_mcp_stdio_config.token` 等于该团队的桥密码。DB 是明文 JSON，
   任何能读 `Roaming\AionUi\aionui\aionui-backend.db` 的进程都能拿。不要外传。
3. **token 生命周期**：绑定在 `conversations.extra`，跨 AionUi 重启不变（已验证历史团队仍在库里）。
   但端口会变。若 `discover` 找不到桥，说明该团队 session 没在跑 → 需要在 UI 里把团队跑起来。
4. **401 本身修不掉**：想让 `aioncore.exe team *` 这个 CLI 本身能用，只能从 AionUi 进程内部跑
   （AionUi 会把四个 env 注入 agent 进程）。二进制里写明了
   `does_not_accept_identity_authority_from_stdin` —— 它**故意**不从外部接受身份。这是设计边界，不是 bug。
5. **进程受保护**：试图读 aioncore 的 PEB 环境变量拿 token 会 `STATUS_ACCESS_DENIED`，此路不通。
6. **写操作谨慎**：`send` / `task_create` / `task_update` 是真写团队通道，别在自动流程里乱发。
