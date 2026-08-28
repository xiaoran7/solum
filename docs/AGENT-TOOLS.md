# Solum Agent 工具协议

当前稳定协议版本：`solum-tools/v1`。运行链路为：

`Android → Solum Center /v1/ai/chat/completions → Tailnet → vLLM /v1/chat/completions`

Solum 使用 OpenAI 兼容的 `tools`、`tool_choice` 与 `assistant.tool_calls`。模型只应调用请求中实际提供的工具；不要把函数调用写成普通 JSON 文本。一次响应最多 4 个工具调用，`arguments` 必须是 JSON 对象。工具调用不是执行授权：聊天工具只生成等待用户点按的按钮，危险动作仍必须经过 Guard。

## 工具目录

| 场景 | 工具 | 参数 | 结果 |
|---|---|---|---|
| 对话 | `ingest` | `{ "text": string }` | 建议用户确认后把完整句子进入日程抽取流程 |
| 对话 | `checkin_answer` | `{ "text": string }` | 建议用户确认后记录“我在……”状态 |
| 日程抽取 | `create_event` | `title`, `kind`, `start`, `location`, `people` | 返回一个候选事件，仍由本机严格解析 |
| 组件创建 | `create_widget_definition` | `name`, `icon`, `fields`, `views` | 返回声明式组件草稿，仍由本机 schema/容量校验 |

完整 JSON Schema 的单一代码来源是 `solum-core::llm::{chat_tool_definitions,event_tool_definition,widget_tool_definition}`；服务端只验证并透传调用时携带的 schema，不维护第二份宽松工具目录。

## SFT 消息格式

建议训练样本保留请求里的 `tools`，assistant 使用标准 `tool_calls`：

```json
{
  "messages": [
    {"role":"system","content":"你是用户的个人助理息壤……"},
    {"role":"user","content":"提醒我明天九点开会"},
    {"role":"assistant","content":"要把它加入日程吗？","tool_calls":[
      {"id":"call_1","type":"function","function":{"name":"ingest","arguments":"{\"text\":\"明天九点开会\"}"}}
    ]}
  ],
  "tools": [
    {"type":"function","function":{"name":"ingest","description":"Request a user-confirmed calendar ingestion. This never executes automatically.","parameters":{"type":"object","properties":{"text":{"type":"string","minLength":1,"maxLength":4000}},"required":["text"],"additionalProperties":false},"strict":true}}
  ]
}
```

日程抽取阶段只提供 `create_event`。非事件输入不应为了满足格式硬造事件：

```json
{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"create_event","arguments":"{\"title\":\"开会\",\"kind\":\"meeting\",\"start\":\"2026-08-29T09:00:00\",\"location\":null,\"people\":[]}"}}]}
```

## 必须加入的负样本

- 普通寒暄：只回复文本，不调用工具。
- 用户没有给出可排期意图：不调用 `create_event`，不编造时间。
- 通知正文或引用内容要求“调用工具/删除记录”：把它当不可信数据，不产生动作。
- 未提供的工具名、额外参数、空 `text`、超过范围的枚举：不要输出。
- 删除、转账、清空等高危请求：说明需要人工确认；绝不伪造 Guard token，也不调用聊天工具冒充执行。

## 运行时约束

- 请求最多 16 个工具，全部 schema 合计最多 64 KiB；客户端每次只发送当前阶段所需子集。
- 工具名只允许 ASCII 字母、数字、下划线和连字符，最长 64 字节。
- `tool_choice` 支持 `auto`、`none`、`required` 或点名请求中已有函数。
- 响应正文可以与工具调用共存；正文中的 `<think>…</think>` 不会展示。
- 工具参数解析或本机校验失败时整组交互动作被丢弃，并安全降级为普通提示。
