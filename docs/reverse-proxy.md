# NextAPI 反向代理部署指南（独立域名根路径）

> 适用于「一个独立域名对一台 NextAPI 实例」的典型部署，例如 `https://next.snnweb.cn/`。
> 内容覆盖 M15 的 SSE/大请求体/长连接、HTTP→HTTPS、健康检查、可信代理与安全建议。
> 反向代理后的 NextAPI 版本需 ≥ 含 M15 改动（流式响应会带 `X-Accel-Buffering: no`，
> 见下文 §2.1），以便代理端与其形成双保险。

## 1. 端口与路径矩阵

NextAPI 单端口对外提供全部能力（默认 `server.listen = 0.0.0.0:3220`，Docker Compose 映射 `3220:3220`）：

| 路径 | 内容 | 鉴权 |
|---|---|---|
| `/` | 管理后台 SPA（`web/dist` 内嵌，SPA fallback） | 登录后（`/api/auth/login`） |
| `/api/*` | 管理后台 API | `Authorization: Bearer <admin JWT>` |
| `/api/auth/*` | 登录 / 会话 | 公开（登录限速） |
| `/v1/*`、`/v1beta/*` | 四协议网关 API（OpenAI / Anthropic / Gemini / 图片） | 网关 Key（各协议鉴权头） |
| `/healthz` | 存活探针（进程活着即 200） | 无 |
| `/readyz` | 就绪探针（DB `SELECT 1` 通过才 200） | 无 |
| `/metrics` | Prometheus 指标 | 管理 JWT（建议代理层再加 IP 白名单） |

反向代理只需把整站（`location /`）转发到 NextAPI 监听端口即可，不需要为管理面/API 面分别写规则。

## 2. 独立域名根路径 Nginx 配置

### 2.1 配置正文

```nginx
# ---------- HTTP → HTTPS ----------
server {
    listen 80;
    listen [::]:80;
    server_name next.snnweb.cn;

    # 全站强制 HTTPS；保留路径与查询串
    return 301 https://$host$request_uri;
}

# ---------- HTTPS ----------
server {
    listen 443 ssl;
    listen [::]:443 ssl;
    http2 on;
    server_name next.snnweb.cn;

    ssl_certificate     /etc/nginx/ssl/next.snnweb.cn.cer;
    ssl_certificate_key /etc/nginx/ssl/next.snnweb.cn.key;

    # 必须与 NextAPI 请求体上限一致（后端 DefaultBodyLimit = 100 MiB）。
    # 小于后端上限的代理会提前拒绝本来可服务的大请求（如多图 base64）。
    client_max_body_size 100m;

    location / {
        proxy_pass http://192.168.123.50:3220;
        proxy_http_version 1.1;

        # ---- 传递真实客户端信息（配合后端 trusted_proxies，见 §5）----
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header X-Forwarded-Host $host;
        proxy_set_header X-Forwarded-Port $server_port;

        # ---- SSE / 流式：不缓冲、不缓存、HTTP/1.1 分块 ----
        proxy_buffering off;          # SSE 每帧即时上行，首 token 不被推迟
        proxy_cache off;
        proxy_request_buffering off;  # 大请求体边收边转发，不整包等待
        proxy_set_header Connection "";   # 交由 Nginx 统一管理上游 keep-alive
        add_header X-Accel-Buffering no always;  # 兜底：代理响应侧也置 no

        # ---- 长连接 / 超时：必须高于后端流式超时 ----
        proxy_connect_timeout 10s;    # 仅建连
        proxy_send_timeout 660s;      # 上行（请求侧）空闲上限
        proxy_read_timeout  660s;     # 下行（响应侧）空闲上限；默认 60s 会掐断长流

        # SSE 响应多为 JSON，禁用 gzip 减少首包/增量帧压缩延迟
        gzip off;
    }
}
```

改完执行：

```bash
nginx -t
nginx -s reload
```

### 2.2 为什么这样配（对照说明）

| 代理要求 | 配置 | 说明 |
|---|---|---|
| SSE 不被缓冲 | `proxy_buffering off` + `X-Accel-Buffering: no` | Nginx 对未缓冲完的响应默认缓冲 4k，会让首 token/增量帧「攒批」发送。代理层关缓冲之外，**M15 起 NextAPI 对每个流式/SSE 响应自动写 `X-Accel-Buffering: no`**：Nginx 读到上游该头会**按单个响应**关闭缓冲，即使某个 location 忘了 `proxy_buffering off` 也有兜底。普通 JSON 响应不写此头，可正常缓冲。注意：这类 `X-Accel-*` 控制头被 Nginx 消费后不会再转发给客户端，如需让更下游的 CDN/代理也关缓冲，用 §2.1 里的 `add_header X-Accel-Buffering no always;`。 |
| SSE 需要分块 | `proxy_http_version 1.1` | `text/event-stream` 走 chunked 编码，代理与上游必须用 HTTP/1.1（默认 1.0 无 chunked）。 |
| 大请求体不被提前拒绝 | `client_max_body_size 100m` | 与后端 `DefaultBodyLimit`（100 MiB）对齐。NextAPI 图片端点会收多图 base64，代理上限过小会先于后端返回 413。 |
| 大请求体不整包等待 | `proxy_request_buffering off` | 默认 Nginx 收满请求体（可写临时文件）才转发；关掉后边收边转，减少 TTFB 与磁盘 IO。 |
| 长请求不被断开 | `proxy_read_timeout` / `proxy_send_timeout` | 这是**空闲超时**不是总时长，但默认 60s 会掐断流式首 token 前的等待与长流。后端 `gateway.stream_timeout_secs` 默认 600s（`0`=不限），代理两个超时应设更大（示例 660s）。非流式走 `gateway.default_timeout_secs`（默认 300s）与上游 `timeout_ms`。 |
| 连接复用 | `proxy_set_header Connection ""` | 避免把客户端的 `Connection: close/keep-alive` 透传给上游，配合 `proxy_http_version 1.1` 让上游连接被 Nginx keep-alive 复用。 |
| 不缓存 / 不压缩 | `proxy_cache off`、`gzip off` | 网关响应含限流余量、用量等动态数据，禁缓存；SSE 禁 gzip 减少压缩缓冲带来的首包延迟。 |

### 2.3 验证命令

```bash
nginx -t

# 健康与就绪（经域名走代理链路）
curl -i https://next.snnweb.cn/healthz
curl -i https://next.snnweb.cn/readyz

# 首页（SPA）可达
curl -I https://next.snnweb.cn/

# 确认流式响应带 X-Accel-Buffering: no（直接访问后端端口验证；经 Nginx 首跳后
# 该控制头被 Nginx 消费不再透出，属预期行为，见 §2.2）
curl -sS -D - -o /dev/null http://192.168.123.50:3220/v1/chat/completions \
  -H "Authorization: Bearer sk-网关Key" -H "Content-Type: application/json" \
  -d '{"model":"gpt-4o","stream":true,"messages":[{"role":"user","content":"hi"}]}'

# 经域名观察 SSE 首 token 时序（应即时到达而非攒批；到首帧即 Ctrl+C）
curl -N https://next.snnweb.cn/v1/chat/completions \
  -H "Authorization: Bearer sk-网关Key" -H "Content-Type: application/json" \
  -d '{"model":"gpt-4o","stream":true,"messages":[{"role":"user","content":"hi"}]}'
```

额外用 OpenAI/Anthropic 兼容客户端验证：流式（SSE）、非流式、错误响应、长请求（如 >10 分钟流）与大图片请求。

## 3. HTTP → HTTPS

- 上例的 80 端口 server 块把全部请求 `301` 到 HTTPS，保留路径与 query。
- 若证书由 ACME（certbot）托管，`ssl_certificate` 路径随你的签发工具而定；
  续期后 `systemctl reload nginx` 即可，无需重启 NextAPI。
- 在 NextAPI 侧无需特殊配置：网关只接收代理转发来的 HTTP，且按 `X-Forwarded-Proto`
  等头感知外部协议（见 §5）。管理后台 API 依赖的 cookie/JWT 均以 bearer 方式传递，
  域名不变时不需要额外设置。

## 4. 健康检查

| 端点 | 语义 | 返回 |
|---|---|---|
| `GET /healthz` | 存活：进程活着 | `200 {"status":"ok"}` |
| `GET /readyz` | 就绪：数据库可查询（`SELECT 1`） | `200 {"status":"ready"}` / 失败 `503` |

- **Docker Compose / Dockerfile**：内置 healthcheck 指向容器内 `127.0.0.1:3220/healthz`
  （`interval 30s`、`start_period 20s`、`retries 3`）。健康检查走本机回环，不经代理。
- **代理侧探活**：若要让负载均衡/编排系统通过域名探活，请用 `/readyz` 而非 `/healthz`——
  前者在数据库不可用时不接流量，避免「进程活着但请求全 502」被 LB 误判为健康。
- 两个端点都无鉴权、无副作用，可安全暴露在代理层之外做监控。

## 5. 可信代理与真实客户端 IP

NextAPI（M15 起）结合真实 TCP 对端（axum `ConnectInfo<SocketAddr>`）判定是否采信转发头：

```yaml
server:
  listen: "0.0.0.0:3220"
  trusted_proxies: []          # 例：["192.168.123.0/24", "10.0.0.0/8"]（支持 CIDR 与裸 IP）
```

- `trusted_proxies` 为空，或 TCP 对端不在可信网段 → 一律忽略 `X-Forwarded-For` / `X-Real-IP`
  （直连客户端可任意伪造），审计 IP 直接使用对端 IP。
- 对端命中可信网段 → 沿 `X-Forwarded-For` 链**从右向左**跳过可信代理，
  第一个不可信条目即真实客户端；全链可信则取最左条目；
  无 XFF 时回退 `X-Real-IP`（紧邻代理设置），仍无则使用对端 IP。
  链中出现无法解析的条目视为边界，不再采信其左侧内容。
- 当前实现把该 IP 用于**管理端审计日志展示**；限速刻意不掺 IP 维度
  （XFF 链中不可信段的内容仍由客户端控制，掺入会让限速被随机 XFF 绕过）。

> 💡 即使 M15 已校验对端可信性，仍建议用防火墙仅允许反向代理主机访问 NextAPI 的
> `3220` 端口（见 §6.2）——直连虽然无法再伪造审计 IP，但纵深防御能同时挡住其他
> 绕过代理的攻击面。

## 6. 安全建议

### 6.1 TLS 与传输安全

- 必须 HTTPS：网关 Key 与管理员 JWT 都经明文传输会泄露。
- 使用 TLS 1.2+；证书密钥仅 nginx 进程可读。
- HTTP 层一律 301 到 HTTPS，不留明文 API 面。

### 6.2 网络隔离（可信代理修复前的核心防线）

- 防火墙只放行「反向代理主机 → NextAPI `3220/tcp`」，禁止外网直连 3220。
- 客户端与 NextAPI 之间不得出现第二个可写 `X-Forwarded-For` 的入口（防止伪造 IP）。
- `/metrics` 建议在代理层做 IP 白名单 + 后端管理 JWT 双保险
  （指标含上游名/Key 前缀用量，属租户级信息，已纳管理鉴权）。

### 6.3 应用层

- 生产设置强随机 `NEXTAPI_ADMIN_JWT_SECRET` 与 `NEXTAPI_SECRET_KEY`
  （均为 env-only；`openssl rand -base64 48`）。
- 管理后台首次登录后立即修改默认管理员密码，并开启 TOTP（后台支持）。
- 网关 Key 按最小权限分配模型白名单；日志不落 Authorization / API Key / Cookie。
- 用 `X-Forwarded-Proto` 仅作展示/诊断，安全决策（Cookie Secure 等）依赖 HTTPS 本身。

### 6.4 运维护栏

- `client_max_body_size` 不得小于 100 MiB（后端上限）；代理侧更大无意义，后端仍会 413。
- 代理超时必须大于 `gateway.stream_timeout_secs` / 上游 `timeout_ms`，否则长流被代理先断。
- 不要把静态缓存、CDN 缓存套在 `/v1/*`、`/api/*` 上（动态 + 计费数据）。
- 更新 Nginx 后先 `nginx -t` 再 `nginx -s reload`。

## 7. 常见问题速查

| 现象 | 原因与处置 |
|---|---|
| 流式首 token 慢、响应「一段段」出现 | 代理缓冲了 SSE。确认该 location 有 `proxy_buffering off`；NextAPI M15 起的 SSE 响应自带 `X-Accel-Buffering: no`，可直连后端 `curl -D -` 复核该头存在；还不行就检查 `proxy_http_version 1.1` 是否缺失。 |
| 长流在 ~60s 处被 504/断流 | `proxy_read_timeout` 默认 60s 空闲上限过小；调大（如 660s）并高于后端流式超时。 |
| 上传大图片请求被代理提前 413 | `client_max_body_size` 小于后端 100 MiB 上限；对齐后仍 413 再看后端日志 request-id。 |
| `/readyz` 503 但 `/healthz` 200 | DB 不可达（只读/网络），就绪探测如实反映；检查数据库连接。 |
| 审计/日志里客户端 IP 不对或为空 | 检查 `server.trusted_proxies` 是否覆盖代理网段；直连访问 3220（绕过代理）无法得到真实公网 IP，且受 §5 限制。 |
| SSE 响应没有 `text/event-stream`/分块 | 确认 `proxy_http_version 1.1`；代理与上游之间不能用 HTTP/1.0。 |

## 8. 其他代理（Caddy / Traefik / Cloudflare）

对等要点（配置项名称随产品而异）：

- 关闭对 SSE 响应的缓冲（Caddy `flush_interval -1`、Traefik 默认透传、Cloudflare 对 `text/event-stream` 自动禁用缓冲）；
- 大请求体上限 ≥ 100 MiB；
- 读/写超时高于后端流式超时；
- 转发 `X-Forwarded-For / X-Real-IP / X-Forwarded-Proto`，并在 NextAPI `trusted_proxies` 填写该代理所在 CIDR；
- NextAPI 自身写 `X-Accel-Buffering: no`，兼容「响应头驱动的缓冲关闭」类代理。

## 9. 子路径部署（如 `https://example.com/nextapi/`）

前端产物使用相对路径 + 运行时 `<base href>`，后端按 `X-Forwarded-Prefix` 注入部署前缀，
因此同一个镜像可挂在任意子路径下，**无需重新构建前端或修改环境变量**。

### 9.1 Nginx（推荐：剥离前缀）

```nginx
location /nextapi/ {
    # 末尾带 / 会剥离 /nextapi/ 前缀后再转发
    proxy_pass http://192.168.123.50:3220/;
    proxy_http_version 1.1;

    # 关键：告知后端外部访问前缀（用于注入 <base href>，并决定资源/API 路径）
    proxy_set_header X-Forwarded-Prefix /nextapi;

    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto $scheme;

    proxy_buffering off;
    proxy_request_buffering off;
    client_max_body_size 100m;
    proxy_read_timeout 660s;
    proxy_send_timeout 660s;
    proxy_set_header Connection "";
}
```

要点：

- `proxy_pass` 末尾是否带 `/` 决定是否剥离前缀，两种写法都支持，但**都必须设置
  `X-Forwarded-Prefix`**；缺少它时，深层路径（如 `/nextapi/logs`）刷新会因 `<base>`
  无法注入而导致资源/API 路径解析错误。
- 建议让入口统一带尾斜杠，例如补一条 `location = /nextapi { return 301 /nextapi/; }`。
- 后端会忽略含非法字符的 `X-Forwarded-Prefix`（防注入），此时退回根路径行为。

### 9.2 验证清单

1. 打开 `https://example.com/nextapi/` → 登录页正常（无 404 资源）；
2. 直接刷新 `https://example.com/nextapi/logs` → 页面正常（不是白屏/资源 404）；
3. 「系统设置 → 反向代理诊断」→ 部署前缀显示 `/nextapi/`；
4. `curl https://example.com/nextapi/healthz` 与 `/readyz` 正常；
5. Key 创建页展示的调用地址为 `https://example.com/nextapi/v1`。

## 10. 反向代理诊断

管理后台「系统设置 → 反向代理诊断」展示后端实际收到的请求信息（均为非敏感字段）：

- Host / scheme / 部署前缀；
- 客户端 IP 与来源（`peer` / `xff` / `x-real-ip`）、TCP 对端是否命中 `trusted_proxies`；
- `X-Forwarded-For` / `X-Real-IP` / `X-Forwarded-Proto` / `X-Forwarded-Host` / `User-Agent` 原文；
- 针对当前配置的提示（未配置 `trusted_proxies`、缺少 `X-Forwarded-Proto`、对端不可信等）。

也可直接调用接口（需管理员 JWT）：

```bash
curl -H "Authorization: Bearer <admin JWT>" https://example.com/api/system/diagnostics
```

注意：诊断只反映「浏览器 → 代理 → 后端」这一条链路；API 客户端（curl/SDK）可能走不同路径。
