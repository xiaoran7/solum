# Solum Cloud

Solum 的统一中心后端。桌面、Android、HarmonyOS 和以后新增的客户端都只连接这一套 HTTPS API；客户端仍以本地数据库为权威，中心库不会保存可查询的行为日志、人格、健康采样或聊天明文。

生产模式由一个 `solum-cloud` 容器和一个 PostgreSQL 容器组成，提供：

- `POST /v1/auth/register|login|refresh|logout`
- `POST /v1/ai/chat/completions`
- `GET /v1/ai/models|usage`（默认/可选模型目录与账号月度用量）
- `POST /v1/push`、`GET /v1/pull|stats`（端到端加密同步）
- `POST|GET /v1/alerts`（固定形状的状态告警）
- `POST /v1/devices/register`、`GET /v1/devices`（设备公钥目录）
- `POST|GET /v1/keys/envelopes`（加密后的同步主密钥信封）
- `GET|PUT /v1/keys/recovery`（账号固定范围、create-only 的登录恢复信封）
- `GET /v1/health`

账号、AI 代理和同步使用同一个 origin。同步接口只存储客户端产生的 XChaCha20-Poly1305 密文，服务端不持有解密主密钥。

## 部署

1. 复制 `.env.example` 为 `.env`，不要提交。
2. 分别生成：
   - 至少 32 字符的随机 `SOLUM_AUTH_SECRET`；
   - 两个不同的数据库强密码 `SOLUM_DB_OWNER_PASSWORD` / `SOLUM_DB_APP_PASSWORD`；
   - 初始管理员账号与至少 12 字符的强密码。
3. 将 MiMo Token Plan 密钥填入 `MIMO_API_KEY`。用 `SOLUM_DEFAULT_MODEL` 指定所有登录用户可直接使用且始终保留的默认模型，用逗号分隔的 `SOLUM_ALLOWED_MODELS` 指定其他可选模型；默认模型即使未写进列表也会自动加入。`SOLUM_STANDARD_MONTHLY_REQUEST_LIMIT` 是新账号默认月度请求额度。
4. 启动：

```sh
docker compose up -d --build
docker compose ps
```

5. 使用 Caddy、Nginx 或云厂商网关为宿主机 `127.0.0.1:8787` 提供 HTTPS。客户端的“账号服务器”和“同步服务器”都填写同一个 HTTPS 地址。

PostgreSQL 不发布宿主端口，只存在于 Compose 私有网络。迁移容器每次启动都会幂等执行 schema；API 使用非 owner、非 superuser、无 `BYPASSRLS` 的 `solum_runtime` 账号和有限连接池。所有租户数据表都启用并强制 RLS，API 必须在事务内设置由 access token `sub` 得到的 `app.current_tenant_id`。

公开中心部署使用开放注册：

```env
SOLUM_REGISTRATION_MODE=open
```

自托管部署如只供单人使用可改为 `closed`；初始管理员账号始终保留。
桌面、Android 与 HarmonyOS 客户端的“注册”按钮都调用这一中心接口；账号只写入本 PostgreSQL，不会在其他容器各建一份。

## 数据与密钥边界

- `auth.*`：用户 UUID、用户名、密码哈希和刷新令牌哈希。
- `ai.entitlements`：账号当前权益名称、月度请求上限和自定义模型开关；这是以后支付系统更新权益的入口，当前不包含支付处理。
- `ai.usage_monthly` / `ai.usage_events`：服务端权威的月度原子计数与逐请求结果，只记录模型、状态、延迟和 token 数，不保存提示词或回复正文。
- `sync.blobs`：端侧加密后的增量操作，不含明文业务字段。
- `sync.devices`：设备标识、公钥、最后在线时间和撤销状态；撤销后该设备的注册、push、pull 均返回 403，不能靠再次上线自我恢复。
- `sync.preferences`：未来账号级设置的加密文档；现有规则、主动性和画像指针仍随加密 oplog 同步。
- `vault.key_envelopes`：只保存被恢复密钥或设备公钥包装后的主密钥。

登录恢复使用固定 recipient `account-recovery-v1`、key version 1 和
`recovery-xchacha20poly1305`。`PUT` 只在记录不存在时创建并始终返回当前权威信封，
不会覆盖既有主密钥；通用 `/v1/keys/envelopes` 仅接受设备公钥信封，不能读写该保留
recipient。客户端用账号密码与不可变 user UUID 在本地导出包装密钥，中心服务只看到
随机密文，拿到数据库或 access token 都不能解密同步内容。
- 大图片、文档和语音以后放对象存储，PostgreSQL 只留密文索引。

登录只证明“这是哪个租户”，不自动授予解密能力。客户端用本次登录密码在本机解封账号恢复信封；密码和同步主密钥都不上传。

AI 请求与响应都经过 Solum Center。中心在联系上游前原子预留一次额度；非默认模型在响应开始前失败时只重试一次默认模型，并通过 `X-Solum-*` 响应头说明 requested/effective model、回退原因和剩余额度。流式响应一旦开始写出就不会切换模型，避免拼接两条不同回复。一次用户调用即使内部发生回退也只扣一次请求额度；成功、失败、回退与可获得的 token 数分别统计。

## SQLite 兼容与迁移

`npm run start:sqlite` 仍可启动原账号 SQLite 实现，Rust `solum-sync-server` 也继续兼容既有 relay；它们只用于本地测试和迁移期，不应与 PostgreSQL 生产服务同时接受新写入。

迁移生产数据时遵循“停写 → 备份 SQLite/WAL/SHM → 导入并核对账号 UUID和每租户 blob 数 → 切换域名 → 保留旧库只读”的顺序。当前提交不会自动认领 `legacy` 静态 token 租户，也不会删除任何旧数据库；在为 legacy 数据明确指定归属账号前，不做不可逆迁移。

一次性迁移工具为 `scripts/import-legacy-sqlite.js`。它只读两个 SQLite 文件，在单个 PostgreSQL 事务中保留账号密码哈希、有效刷新会话、租户归属以及 blob/告警原始 `seq`；目标中心非空、租户无法唯一映射或旧 schema 不兼容时会整体回滚。运行前必须停止旧服务写入并完成 WAL checkpoint 与文件备份：

```sh
PGUSER=postgres PGPASSWORD=... node scripts/import-legacy-sqlite.js /backup/solum-cloud.db /backup/solum-sync.sqlite
```

## 本地验证

```sh
npm install
npm test
node --check src/postgres-server.js
```

完整 PostgreSQL/RLS 集成测试需要 Docker。仅运行 Node 测试不会创建或修改生产数据库。
