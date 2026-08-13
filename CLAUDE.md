# 息壤 Solum（Personal Agent）项目须知

> 完整的智能体工作规范（开工前必须做什么 / 工作中红线 / 完工后必须做什么）见
> [AGENTS.md](AGENTS.md)——本文件只保留最核心的留痕规则，两边如有出入以 AGENTS.md 为准。

## 强制文档留痕规则

**每一次改动（无论是否由 Claude 完成）都必须在以下三份文档之一留下记录，不可省略：**

1. [docs/PITFALLS.md](docs/PITFALLS.md) — 踩坑集。任何调试超过几分钟、走了弯路、被坑过的问题，必须记录：现象、根因、解决方式、如何避免再犯。**不管问题最后有没有解决，踩过就要记。**
2. [docs/CHANGELOG.md](docs/CHANGELOG.md) — 变更日志。每次新增/完善/修改功能，按 [Keep a Changelog](https://keepachangelog.com/) 风格写一条，放在 `Unreleased` 下。
3. [docs/MISC.md](docs/MISC.md) — 杂项记录。不适合写进 changelog（比如：讨论过但否决的方案、临时的架构调整原因、非功能性的观察和决定）但有留存价值的内容。

判断放哪一份的简单标准：**改了行为 → CHANGELOG；踩了坑 → PITFALLS；两者都不是但值得记 → MISC。** 拿不准就宁可多记一条到 MISC，不要漏记。

- 开工前先读一遍 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)，确认当前设计跟文档是否一致，不一致要先在文档里说明再改代码。
- 这条规则本身也适用于对这三份文档、对本 CLAUDE.md 的修改。
