# LLM 厂商接入调研（2026-07-15）

Solum 的云端网关（ARCHITECTURE.md §3.6，`solum-core/src/llm.rs`）只说一种协议：
**OpenAI 兼容 `/chat/completions`**。配置来自 `solum-llm.json` 或 `SOLUM_LLM_*` 环境变量：

```json
{
  "base_url": "…/v1",          // 必填，末尾不带 /chat/completions
  "api_key": "…",              // 必填
  "model": "…",                // 选填，缺省 mimo-v2.5
  "temperature": 0.3,          // 选填；写 null 表示"不发送该字段"（OpenAI gpt-5 系必须）
  "max_tokens": 1024,          // 选填；缺省不发送
  "timeout_secs": 30           // 选填；思考类模型建议 60+
}
```

结论先行：**下表所有厂商都有 OpenAI 兼容端点，现有"填 base_url + api_key + model"的
自定义机制全部覆盖**，不需要为任何一家写专用客户端。需要注意的只有各家参数怪癖
（见每节"坑"），其中 OpenAI gpt-5 系拒绝自定义 temperature 一条曾是代码硬伤，
已在本次改动中修复（temperature 可配置为 null）。

---

## 1. 小米 MiMo（Token Plan，当前默认）

| 项 | 值 |
|---|---|
| Token Plan base_url | `https://token-plan-cn.xiaomimimo.com/v1`（key 形如 `tp-…`） |
| 按量付费 base_url | `https://api.xiaomimimo.com/v1`（key 形如 `sk-…`，两种 key 不能混用） |
| 主力模型 | `mimo-v2.5`（1x 额度）、`mimo-v2.5-pro`（2x 额度） |
| 上下文 | 1M（1048576），最大输出 131072 |
| 认证 | `Authorization: Bearer <key>` |

坑：
- Token Plan 与按量的 base_url、key 相互独立，配错组合直接 401。
- TTS 系列模型（`mimo-v2.5-tts*`）不是 chat 模型，别填进 `model`。

来源：[官方文档](https://mimo.mi.com/docs/zh-CN/quick-start/summary/welcome)、
[套餐说明](https://codingplan.link/zh/plans/xiaomimimo)、
[版本价格](https://www.foreignserver.com/mimo/tokenplan.html)

## 2. DeepSeek

| 项 | 值 |
|---|---|
| base_url | `https://api.deepseek.com` 或 `https://api.deepseek.com/v1`（等价） |
| 当前模型 | `deepseek-v4-flash`（默认带思考模式）、`deepseek-v4-pro` |
| 旧名 | `deepseek-chat` / `deepseek-reasoner` → **2026-07-24 停用**（过渡期分别指向 V4-Flash 的非思考/思考模式） |
| 上下文 | V4 系列 1M；支持工具调用、JSON output、上下文缓存 |
| 认证 | Bearer |

> 用户口中的 "DeepSeek 的 flash 和 pro" 即 `deepseek-v4-flash` / `deepseek-v4-pro`，
> 是 2026-04-24 发布的 V4 双型号，不是 Gemini 那套命名的误记。

坑：
- **7 月 24 日后配置里还写 `deepseek-chat`/`deepseek-reasoner` 会直接报错**，要换新名。
- 思考模式：`temperature` 等采样参数对思考段不生效（不会报错，静默忽略）；
  最终答案在 `choices[0].message.content`，思维链在 `reasoning_content`（Solum 只取 content，天然安全）。
- 思考模式响应慢，30s 超时容易断，建议 `timeout_secs: 60` 以上。

来源：[API 文档](https://api-docs.deepseek.com/)、[更新日志](https://api-docs.deepseek.com/updates/)

## 3. 智谱 GLM

| 项 | 值 |
|---|---|
| base_url | `https://open.bigmodel.cn/api/paas/v4` |
| 旗舰 | GLM-5.2 / 5.1 / 5 |
| 高性价比 | `glm-4.6`（文本）、`glm-4.6v`（图文） |
| 免费 | `glm-4.7-flash`、`glm-4-flash`（长期免费，适合当 Solum 的零成本兜底） |
| 认证 | Bearer（key 形如 `id.secret` 两段式，整串填入即可） |

坑：
- base_url 是 `/api/paas/v4` 不是 `/v1`，照抄别家习惯会 404。
- 思考类模型（GLM-5 系）同样存在 `reasoning_content` 字段与响应偏慢的问题。

来源：[快速开始](https://docs.bigmodel.cn/cn/guide/start/quick-start)、
[对话补全 API](https://docs.bigmodel.cn/api-reference/%E6%A8%A1%E5%9E%8B-api/%E5%AF%B9%E8%AF%9D%E8%A1%A5%E5%85%A8)、
[GLM-4.7-Flash（免费）](https://docs.bigmodel.cn/cn/guide/models/free/glm-4.7-flash)

## 4. Kimi / 月之暗面（两条通道，注意区分）

### 4a. 开放平台（按量付费）— 推荐给 Solum 用

| 项 | 值 |
|---|---|
| base_url | `https://api.moonshot.cn/v1`（国际站 `api.moonshot.ai`） |
| 模型 | `kimi-k2.7-code`、`kimi-k2.6`、`kimi-k2.5`（支持视觉、思考/非思考双模式） |
| 认证 | Bearer |

### 4b. Kimi 订阅（会员 Coding Plan）— **Solum 直连大概率不可用**

| 项 | 值 |
|---|---|
| base_url | `https://api.kimi.com/coding/v1`（OpenAI 协议）/ `https://api.kimi.com/coding/`（Anthropic 协议） |
| 模型 | 固定 `kimi-for-coding` |
| 认证 | 会员控制台创建的 API key（最多 5 个） |

坑（关键）：
- 订阅 API **服务端校验 HTTP 头部白名单**，只放行已识别的 Coding Agent
  （Kimi CLI、Claude Code、Roo Code 等）。Solum 以 ureq 直连不在白名单内，
  预期会被拒。**结论：想在 Solum 里用 Kimi，走 4a 开放平台，别用订阅 key。**
- 订阅额度与开放平台余额完全独立。

来源：[Kimi Code 文档](https://www.kimi.com/code/docs/)、
[第三方 Coding Agent 接入](https://www.kimi.com/code/docs/third-party-tools/other-coding-agents.html)、
[开放平台](https://platform.moonshot.ai/)、
[会员权益](https://www.kimi.com/zh-cn/help/kimi-code/membership-guide)

## 5. 通义千问 Qwen（阿里）

### 5a. 百炼 DashScope 按量 API — 推荐给 Solum 用

| 项 | 值 |
|---|---|
| base_url | `https://dashscope.aliyuncs.com/compatible-mode/v1` |
| 模型 | `qwen3.7-max`、`qwen3.7-plus`、`qwen-plus`、`qwen3.7-vl-max`（多模态）等 |
| 认证 | Bearer（DashScope API key） |

### 5b. Token Plan / Coding Plan（订阅）

| 项 | 值 |
|---|---|
| Token Plan（团队版）base_url | `https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1`（以 API Keys 页给出的专属 URL 为准） |
| Coding Plan / Portal base_url | `https://portal.qwen.ai/v1`，模型 `qwen3-coder-plus` |
| 认证 | 套餐专属 API key（与按量 key 不通用） |

坑：
- 套餐的 base_url 是**专属地址**，不是公共 DashScope 地址，必须从控制台 API Keys 页复制。
- Coding Plan 面向 coding 工具按周配额，日常对话型负载（Solum 的场景）更适合 5a 或 Token Plan。
- Qwen OAuth 免费通道 2026-04-15 已停，只剩付费 key / 套餐。

来源：[OpenAI 兼容说明](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope)、
[Token Plan 概览](https://docs.qwencloud.com/token-plan/overview)、
[Qwen Code 认证](https://qwenlm.github.io/qwen-code-docs/en/users/configuration/auth/)

## 6. OpenAI（ChatGPT API）

| 项 | 值 |
|---|---|
| base_url | `https://api.openai.com/v1` |
| 模型 | gpt-5.x 系列（推理型）；chat/completions 仍可用（新特性偏向 Responses API，但 Solum 用不到） |
| 认证 | Bearer |

坑（本次修复的硬伤）：
- **gpt-5 系列只接受默认 temperature(=1)**，请求里带 `temperature: 0.3` 直接
  400 `unsupported_value`。Solum 此前把 0.3 写死在代码里 → 配 OpenAI 必挂。
  现在配置写 `"temperature": null` 即可不发送该字段。
- 国内直连可达性需自备网络环境（本文档不展开）。

来源：[GPT-5 temperature 讨论](https://community.openai.com/t/temperature-in-gpt-5-models/1337133)、
[litellm issue](https://github.com/BerriAI/litellm/issues/13781)

## 7. Google Gemini

| 项 | 值 |
|---|---|
| base_url | `https://generativelanguage.googleapis.com/v1beta/openai`（官方 OpenAI 兼容层） |
| 模型 | `gemini-3.5-flash`、`gemini-2.5-flash` 等 |
| 认证 | Bearer（就用 Gemini API key，不需要 Google OAuth） |

坑：
- base_url 末段是 `/openai` 不是 `/v1`；Solum 拼接后为
  `…/v1beta/openai/chat/completions`，正确。
- 兼容层只覆盖 chat/completions 常用参数，Gemini 专属功能（原生工具等）不透出，Solum 不受影响。

来源：[官方 OpenAI 兼容文档](https://ai.google.dev/gemini-api/docs/openai)

---

## 横向对比：接入 Solum 时的参数差异

| 厂商 | base_url 形态 | 思考模型 | temperature | 建议 timeout |
|---|---|---|---|---|
| 小米 MiMo | `/v1` | 否（v2.5 常规） | 正常 | 30s |
| DeepSeek | 根路径或 `/v1` | v4 默认带思考 | 思考段忽略 | 60s+ |
| GLM | `/api/paas/v4` | GLM-5 系带思考 | 正常 | 60s+ |
| Kimi 开放平台 | `/v1` | k2.5+ 双模式 | 正常 | 30–60s |
| Qwen DashScope | `/compatible-mode/v1` | 部分型号 | 正常 | 30–60s |
| OpenAI | `/v1` | gpt-5 系是推理型 | **必须省略（null）** | 60s+ |
| Gemini | `/v1beta/openai` | 3.x 带思考 | 正常 | 30–60s |

## 现状评估：API 自定义功能是否完善

已具备：
- 任意 OpenAI 兼容端点 + key + model 三件套，覆盖上表全部厂商 ✅
- key 永不入库/入 git（`solum-llm.json` 已 gitignore），状态栏只显掩码 ✅

本次调研暴露并已修复：
- temperature 写死 0.3 → 改为可配置、可置 null 省略（OpenAI gpt-5 需要）
- 无 max_tokens 配置 → 新增选填
- 30s 超时写死 → 改为 `timeout_secs` 选填（思考类模型需要）
- 思考模型可能在 content 里内联 `<think>…</think>`（部分兼容层实现）→ 解析时剥离

2026-07-15 当日跟进：前端已补「设置 → 云端」视图（厂商预设下拉 + 测试连接 + 保存热切换，
见 CHANGELOG 当日条目），本表的预设数据即取自上文调研。

仍欠缺（记录，不在本次范围）：
- 不支持多配置切换 / 故障转移（一次只有一个端点）
- 不支持 Anthropic 协议端点（Kimi 订阅、Claude API 原生格式）
