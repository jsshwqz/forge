# aioncore team CLI 401 交接说明（给接手 agent）

## 一、症状（硬证据）
`aioncore.exe team *` 所有子命令全部 401：
- `task list/create`、`send-message`、`spawn-agent`、`members`、`context` →
  `TEAM_CLI_HTTP_STATUS_ERROR status=401 Unauthorized: runtime bridge returned non-success status` / `runtime_auth_failed`

## 二、根因（已排查确认）
1. **aioncore 连的是 `AIONCORE_URL=http://127.0.0.1:12831`，而 12831 端口没在监听**（netstat 查无）。
2. **AionUi 主程序在 25808 端口监听**（PID 532），但那个端口服务的是 **AionUi Web UI**（curl 回 `<!doctype html>...AionUi`），**不是**接受 token 的 runtime bridge。
3. 所以 aioncore 打到一个"没人听"的 12831 → 401。

## 三、已排除（不用再试）
- **不是 token 过期**：`AIONUI_RUNTIME_TOKEN=efab102a...` 是新的（每次 AionUI 重启都重新生成，我重启后取的），用新 token 手动试 401 依旧 401 → token 本身有效，问题在"桥不在线"。
- **不是 bootstrap 秘密缺失**：`AIONCORE_BOOTSTRAP_SECRET=ccad2b33...` 在 env 里存在。
- **25808 不是答案**：那是 Web UI 前端，不是 token bridge，直接打 401。

## 四、关键机制（需要你判断/动手的点）
**12831 runtime bridge 是 AionUi 主程序"按需拉起"的**：
- 我观察到：团队在活跃干活时 → 某些 `team` 子命令（list/members/context）一度通；团队停/无活跃 task/conversation → bridge 没在线 → 全 401。
- 即：**bridge 的可用性取决于 AionUI 侧是否有活跃的 session/task**。我单方面从 CLI 这边无法让它上线。

## 五、建议接手方向（按优先级）
1. **确认 12831 bridge 怎么被拉起**：读 AionUi/aioncore 源码，看 aioncore 是怎么选择 bridge URL/port 的、在什么条件下 12831 起来。很可能需要 AionUI 处于某活跃状态。
2. **核对 aioncore 该不该打 12831**：也许它应该指向另一个在跑的 endpoint，或 12831 应由某服务绑定却没绑。
3. **触发 token/bridge 重新同步**：AionUI 重启或新开会话后，让 aioncore 重新注册/握手。
4. **最小可复现验证**：起一个确定的活跃 task，再跑 `team task list` / `send-message`，看 12831 是否起来、401 是否消失。

## 六、环境事实（重要，避免接手 agent 走弯路）
- **`aioncore.exe` 不在磁盘上**（全盘递归搜不到，不在 skills/、bin/、AionUi 运行时目录）。它是 AionUI 主程序**运行时生成**的，配合每次重启重新生成的 `AIONUI_RUNTIME_TOKEN`。
- 所以"找一个 aioncore 可执行文件去跑"这条路不通，**401 是 bridge 在线性问题，不是可执行文件缺失**。
- `AIONUI_RUNTIME_TOKEN` 和 `AIONCORE_BOOTSTRAP_SECRET` 都是 env 变量，每次 AionUI 重启刷新。

## 七、对 B 任务的影响（重要）
- **B 阶段团队能手动开**：用户已手动建团队（id 01a0999a）并把 `ready_to_kickoff.json` 里的任务贴给团队，团队能跑（之前 fs+search 10 工具就是团队跑出来的）。
- **我（主审）这边的 401 只挡"我直接通过 aioncore 团队通道下发/监控"**，不挡团队本身执行。
- 所以 B 不卡死，只是我用自动通道盯团队的能力受 401 影响。你修好 401，我就能恢复"直接对话+下发+监控"。
