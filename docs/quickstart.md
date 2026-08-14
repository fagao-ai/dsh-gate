# dsh-gate 快速开始

在 `dsh web` 前面加一道带登录认证的反代闸门：argon2 用户名密码登录、会话 cookie、CSRF、失败限速，HTTP 与 WebSocket 都转发给 `dsh web`，无需改动 dsh 本身。

```
浏览器 ──> dsh-gate（认证 + 反代）──> dsh web（默认 127.0.0.1:3080）
```

## 前置条件

- 一台跑着 `dsh web` 的宿主机（默认监听 `127.0.0.1:3080`，保持 loopback 即可）
- Docker

## 1. 构建并启动

```sh
docker build -t dsh-gate .

docker run -d --name dsh-gate --restart unless-stopped -p 3081:8080 \
  -e AUTH_USER=hezz \
  -e AUTH_PASSWORD='换成你的密码' \
  -e BACKEND=http://host.docker.internal:3080 \
  dsh-gate
```

- **macOS / Windows**（Docker Desktop / OrbStack）：`BACKEND` 用 `http://host.docker.internal:3080` 访问宿主机
- **Linux**：加 `--network host`，`BACKEND` 改成 `http://127.0.0.1:3080`
- 不想用 Docker：`AUTH_USER=hezz AUTH_PASSWORD='你的密码' cargo run --release`（默认监听 `127.0.0.1:8080`）

## 2. 验证

```sh
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:3081/login   # 期望 200
```

浏览器打开 `http://127.0.0.1:3081`，用上面的用户名密码登录，进入 dsh 界面即成功。

## 3. 配置项

| 环境变量 | 必填 | 默认 | 说明 |
|---|---|---|---|
| `AUTH_USER` | ✅ | — | 登录用户名 |
| `AUTH_PASSWORD` | ✅ | — | 登录密码（启动时 argon2 哈希，不落盘） |
| `LISTEN` | | `127.0.0.1:8080` | 网关监听地址（容器里为 `0.0.0.0:8080`） |
| `BACKEND` | | `http://127.0.0.1:3080` | 上游 `dsh web` 地址 |

网关自带行为：登录失败按来源 IP 限速（5 次锁 5 分钟，识别 `cf-connecting-ip`）；会话 12h 过期，HttpOnly + SameSite=Strict；反代时把 `Host`/`Origin` 改写为 loopback 形式，让 dsh 的 `/api` 信任栅栏把它当成本地访问，无需改 dsh 配置。

## 4. 局域网 / 公网访问

对外暴露必须走 TLS（登录页需要在 secure context 下运行）。Cloudflare Tunnel 示例（`~/.cloudflared/dsh-tunnel.yml`）：

```yaml
ingress:
  - hostname: dsh.example.com
    service: http://127.0.0.1:3081   # 指向网关端口，不是 dsh web 的 3080
  - service: http_status:404
```

## 5. 修改登录密码

```sh
./set-password.sh
```

## 6. 更多

环境变量细节、维护脚本说明等见 [README](../README.md)。
