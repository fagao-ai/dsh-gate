# dsh-rs-gateway

Rust authentication gateway in front of a **host-resident** DeepSeek Harness web (`dsh web`). The gateway can run anywhere — a Docker container, a VPS, the same machine — while `dsh web` stays on the host, so the agent keeps full access to the host's toolchain and real project directories.

```
公网/局域网 → dsh-rs-gateway (认证 + 反代) → dsh web (127.0.0.1:3080, 宿主机)
```

## Why

The official `dsh web` has no authentication layer and its `/api` trust fence only trusts loopback. This gateway provides: **argon2 用户名+密码登录、会话 cookie、CSRF、按 IP 失败限速（5 次锁 5 分钟）**, and it rewrites `Host`/`Origin` to the loopback backend, which makes dsh treat the gateway as local — the trust fence passes and loopback-pinned privileged RPC (directory browsing etc.) works remotely without touching official code.

Unlike a containerized dsh, the agent runs on the host: full toolchain, real cwd, your ssh keys stay where they are.

## Features

- 登录页 `/login`（内嵌 HTML，中文），`/logout`
- argon2 密码（启动时哈希，不落盘）
- 内存会话（12h TTL，HttpOnly + SameSite=Strict）
- CSRF 一次性 token
- 失败 5 次锁 5 分钟（按来源 IP，识别 `cf-connecting-ip`）
- HTTP 反代 + WebSocket 双向泵（dsh GUI 的下行流依赖 WS）
- `Host`/`Origin` 改写为后端（解锁信任栅栏与特权方法）

## Run locally

```sh
AUTH_USER=hezz AUTH_PASSWORD='your-password' \
LISTEN=127.0.0.1:8080 BACKEND=http://127.0.0.1:3080 \
cargo run --release
```

Env vars: `AUTH_USER` / `AUTH_PASSWORD` (required), `LISTEN` (default `127.0.0.1:8080`), `BACKEND` (default `http://127.0.0.1:3080`).

## Docker (gateway only — dsh stays on the host)

```sh
docker build -t dsh-rs-gateway .

# macOS / Windows (Docker Desktop / OrbStack): reach the host dsh via host.docker.internal
docker run -d --name dsh-gw --restart unless-stopped -p 8080:8080 \
  -e AUTH_USER=hezz -e AUTH_PASSWORD='your-password' \
  -e BACKEND=http://host.docker.internal:3080 \
  dsh-rs-gateway

# Linux: use host networking so 127.0.0.1:3080 resolves to the host directly
docker run -d --name dsh-gw --network host \
  -e AUTH_USER=hezz -e AUTH_PASSWORD='your-password' \
  -e BACKEND=http://127.0.0.1:3080 \
  dsh-rs-gateway
```

Verified on macOS (OrbStack) with `BACKEND=http://host.docker.internal:3080`: login, SPA, `/api`, loopback-pinned privileged RPC, and WebSocket all work through the container. The `Host`/`Origin` the gateway sends upstream is always rewritten to the loopback form (`127.0.0.1:<port>`) regardless of the connect target, so dsh treats the gateway as local no matter where the container runs.
```

## Public / LAN access

Front it with TLS (Cloudflare Tunnel or a reverse proxy) so the login page runs in a secure context — required by the browser, and it keeps the password off the wire.

## Layout

- `src/main.rs` — entry, router, session gate middleware
- `src/auth.rs` — login page, argon2 verify, sessions, CSRF, rate limiter
- `src/proxy.rs` — HTTP reverse proxy + WebSocket pump, Host/Origin rewrite
- `src/state.rs` — in-memory session/CSRF/rate-limit stores

## License

MIT.
