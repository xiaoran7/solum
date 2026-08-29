# Solum Changelog

本页保留版本级摘要。逐提交实现、测试计数和会话过程由 Git 历史保存。

## [Unreleased]

- 2026-08-29：Solum Center 生产完成 Tg-oauth 统一身份接入发版（`solum-release-0a497cd`）；Postgres 迁移 `auth.oauth_identities` 表；支持 `/v1/auth/oauth/config` 与 `/v1/auth/oauth/exchange` 自动开户与 E2EE 恢复码解耦。
- 2026-08-28：全仓文档按当前 `0.2.8/1018`、local-first 边界和真实待办重构；删除重复会话流水与过期生产断言。

## [0.2.8]

- Tg-oauth 2.0 集成与端到端恢复信封解耦。
- Agent 工具调用适配、通知默认策略与密码规则统一。
- 中心面板更新为纸墨视觉；Solum 品牌与服务入口收敛。

## [0.2.7]

- 完善 Android 本机核心闭环、精确提醒、通知/健康桥接和生产服务联调。
- 增加加密同步、账号隔离、福利告警与设备 QA 证据。

## [0.2.0–0.2.6]

- 建立 Tauri Android 应用、SQLite 本机数据、Guard、提醒、记忆、计划、搜索、账号和加密同步。
- 逐步加入通知监听、Health Connect、后台 alarm、服务端 AI 与恢复能力。

## [0.1.x]

- 建立 Rust workspace、CLI、核心 schema/ingest、行为日志、建议和初版服务端。

历史细节请查仓库 tag/提交；设备与生产状态不能由旧 Changelog 推断。
