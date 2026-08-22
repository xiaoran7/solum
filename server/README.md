# Solum Cloud

Solum 的统一中心后端。2026-08-22 起产品目标只有 Android 客户端；桌面、HarmonyOS 与多设备双向同步不再作为并行产品方向。Android 本地数据库仍是业务权威，中心目标职责为账号、AI、必要元数据与端到端加密版本化备份/换机恢复，不保存可查询的行为日志、人格、健康采样或聊天明文。当前 `/v1/push|pull`、SQLite 兼容入口和旧 relay 仍为迁移期历史实现，只有在新备份恢复闭环验收并保护完现有数据后才能退役。

生产模式由一个 `solum-cloud` 容器和一个 PostgreSQL 容器组成，提供：

- `POST /v1/auth/register|login|refresh|logout`
- `POST /v1/ai/chat/completions`
- `GET /v1/ai/models|usage`（默认/可选模型目录与账号月度用量）
- `POST /v1/push`、`GET /v1/pull|stats`（端到端加密同步）
- `POST|GET /v1/alerts`（固定形状的状态告警）
- `GET /v1/devices`（当前账号实际参与同步的设备状态）
- `GET|PUT /v1/keys/recovery`（账号固定范围、create-only 的登录恢复信封）
- `GET /v1/health|meta`（健康、版本、构建提交和能力发现）

账号、AI 代理和同步使用同一个 origin。同步接口只存储客户端产生的 XChaCha20-Poly1305 密文，服务端不持有解密主密钥。

## 部署

1. 复制 `.env.example` 为 `.env`，不要提交。
2. 分别生成：
   - 至少 32 字符的随机 `SOLUM_AUTH_SECRET`；
   - 两个不同的数据库强密码 `SOLUM_DB_OWNER_PASSWORD` / `SOLUM_DB_APP_PASSWORD`；
   - 初始管理员账号与至少 12 字符的强密码。
3. 将 MiMo Token Plan 密钥填入 `MIMO_API_KEY`。用 `SOLUM_DEFAULT_MODEL` 指定所有登录用户可直接使用且始终保留的默认模型，用逗号分隔的 `SOLUM_ALLOWED_MODELS` 指定其他可选模型；默认模型即使未写进列表也会自动加入。`SOLUM_STANDARD_MONTHLY_REQUEST_LIMIT` 是新账号默认月度请求额度。`SOLUM_AI_MAX_RESPONSE_BYTES` 限制非流式上游响应体（默认 8 MiB），`SOLUM_AI_PENDING_TIMEOUT_SECONDS` 决定进程异常退出后待处理调用多久按中断结算（默认 600 秒）。`SOLUM_TENANT_MAX_BLOB_BYTES` / `SOLUM_TENANT_MAX_BLOBS` / `SOLUM_TENANT_MAX_DEVICES` 分别限制单租户的密文总字节、blob 条数和设备数。
4. 启动：

```sh
docker compose up -d --build
docker compose ps
```

5. 使用 Caddy、Nginx 或云厂商网关为宿主机 `127.0.0.1:8787` 提供 HTTPS。客户端的“账号服务器”和“同步服务器”都填写同一个 HTTPS 地址。

`SOLUM_TRUST_PROXY` 默认关闭。只有在反向代理会覆盖客户端传入的
`CF-Connecting-IP` 或 `X-Real-IP` 时才能显式设为 `true`，否则攻击者可伪造来源绕过
进程内认证限流。发布时把当前 Git
提交写入 `SOLUM_BUILD_REVISION`；部署后以 `/v1/health` 或 `/v1/meta` 返回的
`version` / `revision` 核对实际运行版本。上线前先做 `pg_dump` 或 1Panel 卷快照，保留
上一版服务归档和 `.env`，不要把“容器能重建”误当成数据库可回滚。

在 Windows 上从 Git 提取服务端发布包时，从仓库根归档 `server` 路径，让根级
`.gitattributes` 保持 shell 脚本为 LF：

```sh
git archive --format=tar.gz --output=solum-server.tar.gz HEAD server
```

如果必须用 `HEAD:server` 把子树直接作为归档根，则显式加
`-c core.autocrlf=false`，并在部署前确认 `db/init/00-roles.sh` 不含 CR 字节。

PostgreSQL 不发布宿主端口，只存在于 Compose 私有网络。迁移容器每次启动都会幂等执行 schema；API 使用非 owner、非 superuser、无 `BYPASSRLS` 的 `solum_runtime` 账号和有限连接池。所有租户数据表都启用并强制 RLS，API 必须在事务内设置由 access token `sub` 得到的 `app.current_tenant_id`。

公开中心部署使用开放注册：

```env
SOLUM_REGISTRATION_MODE=open
```

自托管部署如只供单人使用可改为 `closed`；初始管理员账号始终保留。
本仓桌面与 Android 客户端的“注册”按钮都调用这一中心接口；兼容版本的 HarmonyOS 客户端同样调用该接口。账号只写入本 PostgreSQL，不会在其他容器各建一份。

## 数据与密钥边界

- `auth.*`：用户 UUID、用户名、密码哈希、密码语义版本和刷新令牌哈希。新账号用 v2 精确密码；旧 SQLite/PostgreSQL 行保持 v1，先校验精确输入，仅在输入含首尾空白且精确失败时回退历史裁剪校验，同时兼容旧注册账号与旧 bootstrap 管理员。登录不会自动把 v1 升为 v2；只有能同步重包恢复信封的显式改密流程才可升级。带空白的失败尝试对 v1、v2 和不存在账号执行相同次数的 scrypt。每个账号最多保留 32 个活跃刷新会话，签发新会话时淘汰最旧项；后继 refresh token 由服务端密钥确定性派生，上一代 token 在 5 秒窗口内可幂等取回同一个后继，跨代重放仍撤销整族，数据库只保存哈希。PostgreSQL 的每个受保护请求都会重新确认账号状态，disabled 后既有 access token 立即失效。
- `ai.entitlements`：账号当前权益名称、月度请求上限和自定义模型开关；这是以后支付系统更新权益的入口，当前不包含支付处理。
- `ai.usage_monthly` / `ai.usage_events`：服务端权威的月度原子计数与逐请求结果，只记录模型、状态、延迟和 token 数，不保存提示词或回复正文。
- `sync.blobs`：端侧加密后的增量操作，不含明文业务字段。
- `sync.devices`：由首次 push/pull 自动登记的设备标识、最后在线时间和管理员撤销状态；撤销后该设备的 push/pull 返回 403，不能靠再次上线自我恢复。当前没有面向客户端的设备公钥注册或撤销 API。
- `sync.preferences`：未来账号级设置的加密文档；现有规则、主动性和画像指针仍随加密 oplog 同步。
- `vault.key_envelopes`：当前只通过固定恢复接口保存账号密码派生密钥包装后的主密钥。

登录恢复使用固定 recipient `account-recovery-v1`、key version 1 和
`recovery-xchacha20poly1305`。`PUT` 只在记录不存在时创建并始终返回当前权威信封，
不会覆盖既有主密钥。客户端用账号密码与不可变 user UUID 在本地导出包装密钥，中心服务只看到
随机密文，拿到数据库或 access token 都不能解密同步内容。
- 大图片、文档和语音以后放对象存储，PostgreSQL 只留密文索引。

`sync.blobs` 是增量操作历史，不是可独立丢弃的缓存：在没有快照/压缩协议前，按天删除
会让长期离线或新设备永久漏数据。因此生产服务不对 blob 做时间清理；容量由单租户字节、
条数和设备硬上限控制，达到上限明确返回 `storage_quota_exceeded`，不能静默删历史。
结构化告警不是同步历史，仍按 `SOLUM_ALERT_RETENTION_DAYS`（默认 7 天）清理。

登录只证明“这是哪个租户”，不自动授予解密能力。客户端用本次登录密码在本机解封账号恢复信封；密码和同步主密钥都不上传。

AI 请求与响应都经过 Solum Center。中心在联系上游前原子预留一次额度；非默认模型在响应开始前失败时只重试一次默认模型，并通过 `X-Solum-*` 响应头说明 requested/effective model、回退原因和剩余额度。中心先取得首个流块再提交客户端响应，因此空流或首字节前断流仍可安全回退；首字节写出后不再切换模型，避免拼接两条不同回复。流式转发遵守客户端背压，客户端断开时取消上游请求；非流式响应有明确体积上限。一次用户调用即使内部发生回退也只扣一次请求额度；成功、失败、回退与可获得的 token 数分别统计。进程异常退出留下的 `pending` 用量会在该租户下次读取额度或发起调用时幂等结算为 `interrupted`，不会永久悬空、重复累计或在服务启动时扫描全部账号。

## SQLite 兼容与迁移

`npm run start:sqlite` 仍可启动原账号 SQLite 实现，Rust `solum-sync-server` 也继续兼容既有 relay；它们只用于本地测试和迁移期，不应与 PostgreSQL 生产服务同时接受新写入。

迁移生产数据时遵循“停写 → 备份 SQLite/WAL/SHM → 导入并核对账号 UUID和每租户 blob 数 → 切换域名 → 保留旧库只读”的顺序。当前提交不会自动认领 `legacy` 静态 token 租户，也不会删除任何旧数据库；在为 legacy 数据明确指定归属账号前，不做不可逆迁移。

一次性迁移工具为 `scripts/import-legacy-sqlite.js`。它只读两个 SQLite 文件，在单个 PostgreSQL 事务中保留账号密码哈希、有效刷新会话及其 `session_id`、租户归属以及 blob/告警原始 `seq`；NULL 必填字段、缺少 `source` 的告警 schema、无效 session UUID、目标中心非空或租户无法唯一映射时都会在写入前拒绝或整体回滚。运行前必须停止旧服务写入并完成 WAL checkpoint 与文件备份：

```sh
PGUSER=postgres PGPASSWORD=... node scripts/import-legacy-sqlite.js /backup/solum-cloud.db /backup/solum-sync.sqlite
```

## 本地验证

```sh
npm install
npm test
node --check src/postgres-server.js
```

默认 `npm test` 会跳过真实数据库用例。向全新隔离测试库提供运行角色连接串
`SOLUM_TEST_DATABASE_URL`、所有者连接串 `SOLUM_TEST_OWNER_DATABASE_URL`，并显式设置
`SOLUM_TEST_DATABASE_IS_DISPOSABLE=true` 后，
`test/postgres-integration.test.js` 会实际验证旧数据迁移、运行角色、RLS、
注册/登录/刷新/注销、租户隔离、配额、告警约束和 AI 用量类型。旧库导入回归要求目标中心
为空，因此每轮完整验证都应新建数据库并在结束后销毁；不得复用上轮测试库，更不得把生产
连接串用于该用例。
