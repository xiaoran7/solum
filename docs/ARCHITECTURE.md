# Solum 架构

## 1. 系统边界

```text
Tauri UI / CLI / Android bridges
             │ typed commands
             ▼
          solum-core ── SQLite (per profile, local plaintext)
             │
             ├─ E2EE sync/backup blobs ── Solum server
             └─ minimal AI context ────── server LLM proxy
```

`solum-core` 是领域规则和跨平台协议权威；UI、CLI、Android 桥接不得复制或绕过其不变量。服务端不拥有个人明文数据库，也不能解密同步/备份正文。

## 2. Workspace 模块

| Crate | 职责 |
| --- | --- |
| `solum-core` | schema、ingest、日程/提醒、记忆、行为日志、建议、Guard、同步和备份 |
| `solum-cli` | 本地开发、调试和自动化入口 |
| `solum-app` | Tauri 命令、静态 UI、桌面/Android 打包 |
| `solum-sync-server` | 账号、会话、模型代理、密文同步/备份与福利告警 |
| `solum-notif-access` | Android 通知监听桥接与白名单 |
| `solum-health-connect` | Android Health Connect 桥接 |
| `solum-alarm` | Android alarm/receiver/service 桥接 |

## 3. 本机数据与 profile

访客和每个账号使用独立 profile/UUID 与 SQLite 空间。输入先解析和预览，再写入事件、提醒、记忆或行为日志。第三方通知标记 `local_only`，仅支持本机功能；健康数据按用户授权和最小化范围采集。

Core 不读取系统时间，调用方传入 `now`。写入尽量单事务；文件状态采用临时文件、同步和原子 rename。恢复只允许写空目标，并在任何写入前完成版本、摘要、密钥、租户、容量和附件清单预检。

## 4. Guard

危险动作遵循：生成预览 → 用户确认 → Guard 签发绑定主体/动作/参数的一次性 token → 执行 → append-only 审计。`Grant` 只能由 Guard 创建，模型、UI、IPC 和导入文件不能伪造。

## 5. 同步与备份

- 同步基于本机 oplog，端到端加密后上传；合并采用协议定义的 HLC/LWW 与删除规则。
- 未知未来表可隔离泊车；损坏 JSON、鉴权失败和摘要不符必须显式失败。
- `solum-export` v3、`solum-backup` v1、`solum-attachment` v1 通过固定夹具维持 Android/HarmonyOS 字节级互操作。
- 备份支持目录、分片/续传、代际接管和受保护删除；协议兼容不等于真机互通已验收。

## 6. 云端

服务端只保存账号/会话/设备/用量、福利告警和不可解密密文。云端 AI 请求由客户端选择最小上下文，服务端鉴权、限额和代理。云端故障不能阻断本机 ingest、搜索或提醒。

## 7. 前端与平台

UI 是无框架静态页面，通过 Tauri command 调用 Rust。Android 系统能力通过专用 crate/插件接入；权限授予、manifest 注册和后台运行分别验证。HarmonyOS 由独立仓 ArkTS 重写，只共享协议/产品语义。

前端导航由 `showView` 管线统一维护视图状态、主/子导航、滚动恢复、动画与数据刷新。浏览历史栈以初始视图为锚：真实视图切换压入视图条目，确认框、说明/资料弹窗、抽屉、移动工具面板和可关闭账户门复用单个栈顶浮层条目，使 Android 系统返回键始终先关闭瞬态界面，再回退到上一视图。

启动只刷新壳层状态、提醒角标与当前可见视图；隐藏视图在进入时按需刷新。IPC 读取按命令与参数做短时合并，同一视图/全量刷新使用 single-flight，页面隐藏时暂停周期刷新。对话写入后只更新当前可见的今日摘要和全局提醒角标，不预加载邮箱、备份、台账等隐藏页面。

## 8. 变更检查

- schema 变化：迁移、触发器、同步表、apply、导出/备份映射、固定夹具和两平台测试同步。
- 新工具/危险动作：输入 schema、租户、幂等、Guard 和审计同步。
- 新云端字段：确认数据分类、最小化、加密和向后兼容。
- 平台权限：更新隐私清单，并用目标平台 Release 真机验证。
