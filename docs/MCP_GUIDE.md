# Forge MCP 接入指南

> Forge MCP server（`forge-mcp-server`）把 Forge 的编排、记账、进度能力暴露为
> 标准 MCP 协议（2024-11-05, stdio/json-rpc），**任何支持 MCP 的 agent 均可接入**，
> 不限任务、不限客户端。本指南面向"其他 AI / 外部 agent 接入"。

## 一、能力总览（缺省全注册，无需白名单）

启动 forge-mcp-server 即有 **14 个工具**：

| 类别 | 工具 |
|---|---|
| 基础文件 | `echo` `read_file` `write_file` `list_dir` `edit_patch` |
| 编排 | `forge_task_create` `forge_task_get` `forge_task_list` `forge_orchestrate` |
| 台账 | `forge_worklog_add` `forge_worklog_show` `forge_progress_add` `forge_progress_update` `forge_export` |

`FORGE_TOOLS_BUILTIN` 是**扩展**（csv_parse/json_parse/markdown_render 等），不是必需开关。
`FORGE_MCP_ALLOWLIST` 是服务端调用闸（可选收紧）。

## 二、启动方式

```bash
# 项目根内启动（自动探测项目根）
cargo run -p forge-mcp --features server-bin --bin forge-mcp-server

# 显式指定项目根（台账所在，必须有 AI_WORKFLOW.md/progress.json）
FORGE_PROJECT_ROOT=/path/to/forge \
FORGE_WORKSPACE=/path/to/task-workspace \
cargo run -p forge-mcp --features server-bin --bin forge-mcp-server

# 可选：扩展解析工具 + LLM 规划
FORGE_TOOLS_BUILTIN=csv_parse,json_parse,markdown_render \
FORGE_LLM_BASE_URL=https://token.sensenova.cn/v1 \
FORGE_LLM_API_KEY=<key 存 .env> \
FORGE_TIER_HIGH_MODEL=sensenova-6.8-flash-lite \
...
```

## 三、作为独立 MCP 客户端注册

Claude Desktop / Cursor / 其他 MCP host 的 config：

```json
{
  "mcpServers": {
    "forge": {
      "command": "/path/to/forge/target/debug/forge-mcp-server",
      "args": [],
      "env": {
        "FORGE_PROJECT_ROOT": "/path/to/forge",
        "FORGE_WORKSPACE": "/path/to/workspace"
      }
    }
  }
}
```

## 四、Forge 自身 / 其他 AI 的接入（FORGE_MCP_SERVERS）

Forge 服务端（server）本身也是 MCP client：设 `FORGE_MCP_SERVERS` 即可把
forge-mcp-server 注册为 server 侧工具源（供 Forge 编排器调用）：

```bash
FORGE_MCP_SERVERS='[{"name":"forge","command":"/path/to/forge/target/debug/forge-mcp-server","args":[],"env":{"FORGE_PROJECT_ROOT":"/path/to/forge"}}]'
```

任何 AI 若运行在支持 MCP 的 agent 环境，同样按其配置方式注册上述 command。

## 五、任务间共享状态说明

- 编排任务/会话：默认内存栈（进程内），配 `FORGE_PG_URL` 才跨进程持久。
- 台账（worklog/progress/handoff）：落 JSON 事实源文件，跨进程共享（跨 agent 可见）。
- 工作区：`FORGE_WORKSPACE` 下按任务 `create_for` 隔离，编排工具 root 与验收同目录。

## 六、常见问题

- 工具列表看不到 forge_*：确认启动的是 `--features server-bin --bin forge-mcp-server`。
- worklog 写入失败："未找到项目根" → 设 `FORGE_PROJECT_ROOT` 到有 AI_WORKFLOW.md 的目录。
- 想要 LLM 规划：配 `FORGE_LLM_BASE_URL`+`FORGE_LLM_API_KEY`（模型自动探测：6.8→6.7→glm→chat）。
