# Solum Cloud

Solum 的独立账号与 AI 代理服务，主仓（桌面/Android）与 Solum Harmony（息壤鸿蒙版）共用同一份服务端契约；本目录与 `../solum-harmony/server/` 同源，改动要两边同步。它不接收或同步客户端数据库；当前只提供：

- `POST /v1/auth/login`
- `POST /v1/auth/refresh`
- `POST /v1/auth/logout`
- `POST /v1/ai/chat/completions`
- `GET /v1/health`

服务端固定把 AI 请求转发到 MiMo Token Plan。客户端可以选择模型名，但不能提供上游地址或 API Key。

## 部署

1. 复制 `.env.example` 为服务器私有环境配置，不要提交。
2. 生成至少 32 字符的随机 `SOLUM_AUTH_SECRET`，设置初始账号和强密码。把同一个
   `SOLUM_AUTH_SECRET` 配给 `solum-sync-server`，让同步中继能验证账号 access token。
3. 把 MiMo Token Plan 密钥写入服务器环境变量 `MIMO_API_KEY`。
4. 若使用同步中继自带的浏览器仪表盘登录账号，把仪表盘的精确来源（例如
   `https://solumsync.example.com`）写入 `SOLUM_ALLOWED_ORIGINS`；多个来源以逗号分隔。
5. 构建并运行：

```sh
docker build -t solum-cloud .
docker run -d --name solum-cloud \
  --restart unless-stopped \
  --env-file .env \
  -v solum-cloud-data:/data \
  -p 127.0.0.1:8787:8787 \
  solum-cloud
```

6. 使用 Caddy、Nginx 或云厂商网关在前方提供 HTTPS，只把 HTTPS 域名填写到客户端。服务本身不记录请求正文、访问令牌或 MiMo 密钥。

> 注意：进程本身监听 `0.0.0.0`（容器内需要）。上面的 Docker 命令已用
> `-p 127.0.0.1:8787:8787` 把宿主侧收敛到回环；如果不经 Docker 直接 `npm start`
> 裸跑，必须自行用防火墙或反向代理收口，否则无 TLS 的鉴权接口会暴露到所有网卡。

首次启动会按 `SOLUM_ADMIN_USERNAME` / `SOLUM_ADMIN_PASSWORD` 创建账号；数据库已有同名账号后不会用环境变量覆盖密码。旧版 `PA_*` 环境变量仍被兼容读取。访问令牌有效期 15 分钟；刷新令牌有效期 30 天且每次刷新都会轮换，旧令牌立即失效。
