<div align="center">
  <h1>NextAPI</h1>
  <p><strong>自托管 LLM 网关：四协议互转 · 多上游聚合 · 成本统计</strong></p>
  <p>
    <a href="https://github.com/snnh/nextapi/releases"><img src="https://img.shields.io/github/v/release/snnh/nextapi" alt="Release"></a>
    <a href="./LICENSE"><img src="https://img.shields.io/badge/license-Apache%202.0-blue" alt="License"></a>
    <img src="https://img.shields.io/badge/platform-Docker%20%7C%20Linux-informational" alt="Platform">
  </p>
  <p>简体中文</p>
</div>

NextAPI 是一个自托管的 LLM 网关，Rust + axum + PostgreSQL 写就，管理后台（Vue 3）编译进同一个二进制里。客户端用 OpenAI Chat Completions、OpenAI Responses、Anthropic Messages 或 Gemini 里任意一种协议接入，网关按你配置的路由把请求转发给多家上游供应商，协议转换、重试与故障转移、限流、成本记账都在中间做掉。装好后浏览器打开 `http://<主机IP>:3220` 就能管理，不用另外部署前端。

```
客户端（OpenAI / Anthropic / Gemini SDK）──任一协议──► NextAPI ──透传或转换──► 多上游供应商
                                                        │
                                        管理后台（内嵌 SPA）+ PostgreSQL（日志 / 计价 / 配额）
```

## 配置要求

服务端二选一：

- Docker：Docker 20.10+ 与 Compose v2。镜像里依赖齐全，启动时自动执行数据库迁移，推荐这种方式；
- 源码运行：Linux / macOS，Rust stable、Node.js ≥ 20（只用于构建前端）、PostgreSQL 15+。

资源：空载通常几十 MiB；管理后台登录后 glibc 可能保留堆内存，v0.4.0 起会在登录后与周期任务中主动 `malloc_trim`，但仍建议给容器 512 MiB 配额；磁盘预留 500 MiB 以上，之后主要增长的是 PostgreSQL 里的日志。

客户端：

- 管理后台：Chrome / Edge ≥ 111 或 Firefox ≥ 113，手机、平板也能开；
- 网关调用：任何 OpenAI / Anthropic / Gemini 兼容的 SDK 或 HTTP 客户端。

## 主要功能

- 四协议互转：OpenAI Chat Completions / OpenAI Responses / Anthropic Messages / Gemini 两两互转。上游本身就支持入口协议时原样透传，只改模型名和鉴权头，不掺和请求体。
- 多上游聚合：模型名支持 `*` 通配，同优先级组内加权随机，配合重试与故障转移；渠道自动禁用（熔断）默认关闭，需要时到系统设置里打开，支持全局与单渠道两级开关。
- 渠道接入：内置常见供应商预设，填个 API Key 就能接（百炼、千帆、火山、智谱、Kimi、OpenRouter 等）；Codex 用 OAuth，粘贴 `~/.codex/auth.json` 即可，token 临期自动刷新并轮换回写；渠道的模型列表可以拉取后自动同步成托管路由。
- 上游余额监控：只读查询 DeepSeek、Kimi、OpenRouter、Zenmux、OpenAI billing 兼容、智谱 GLM 套餐额度，也支持自定义余额端点；只展示上游供应商侧余额/额度，不涉及用户余额或扣费。
- 鉴权与限流：网关 Key 以 SHA-256 哈希落库，完整 Key 只在创建时显示一次；每个 Key 可单独配 RPM、TPM 和 token / 成本配额。管理员登录有防爆破限速，可以开启 TOTP 二次验证。
- 日志与成本：请求明细按 30 天声明式分区存放，不做自动清理（要腾空间就在后台手动 DROP 分区）；小时聚合表供长区间统计。计价支持分时段、分上下文长度分段与 CNY/USD 双币种，价格表可用 XML / JSON 导入导出。写库是异步批量的，队列满了先落 WAL 再丢内存条目，不阻塞转发。
- 图片生成：`/v1/images/generations` 统一入口，适配 OpenAI / Gemini / Dashscope 同步 / Dashscope 异步任务四种上游形状，异步任务自动轮询并在完成时计费。
- 可观测：Prometheus `/metrics`（管理员 JWT 鉴权）、含来源 IP 的审计日志、日志 CSV 导出、按 Key 开关的 debug 抓包（自动脱敏、有 TTL）。
- 管理后台：仪表盘、密钥、上游、路由、价格、日志、统计、系统设置，移动端能正常使用。

### 上游余额/额度监控

在「上游」页的渠道表单中启用「余额/额度」，选中适配器即可。DeepSeek/Kimi 可直接复用推理 Key；OpenRouter/Zenmux 需单独 Management Key；自定义端点支持同源相对路径或公网 HTTPS、点路径映射和可选总额/已用/重置字段。凭证加密保存，接口只返回是否已配置。

额度弹窗的「立即探测」会强制实时查询；日常列表直接展示最近快照，超过 24 小时灰显。详见 [`docs/upstream-balance.md`](./docs/upstream-balance.md)。

## 快速开始

### Docker（推荐，x86_64 / arm64）

发布镜像在 GitHub Container Registry（`ghcr.io/snnh/nextapi`），compose 会一并拉起 PostgreSQL 16：

```sh
# 1. 准备配置与密钥
cp config.example.yaml config.yaml
export NEXTAPI_SECRET_KEY=$(openssl rand -base64 48)        # 敏感字段加密密钥（env-only）
export NEXTAPI_ADMIN_JWT_SECRET=$(openssl rand -base64 48)  # 管理端 JWT 签名密钥

# 2. 启动
docker compose up -d

# 3. 取初始管理员密码（没设 NEXTAPI_ADMIN_INITIAL_PASSWORD 时会随机生成，只打印这一次）
docker compose logs nextapi | grep 管理员密码
```

然后浏览器打开 `http://<主机IP>:3220` 登录。不想用 compose，或者复用已有的 PostgreSQL，可以直接 run（需要 PostgreSQL 15+）：

```sh
docker run -d --name nextapi --restart unless-stopped \
  -p 3220:3220 -v nextapi-data:/data \
  -e DATABASE_URL=postgres://user:pass@host:5432/nextapi \
  -e NEXTAPI_SECRET_KEY=$(openssl rand -base64 48) \
  -e NEXTAPI_ADMIN_JWT_SECRET=$(openssl rand -base64 48) \
  ghcr.io/snnh/nextapi:latest
```

数据落在两处：日志、配额、配置在 PostgreSQL（compose 的 `pgdata` 卷），WAL 和运行期文件在 `nextapi-data` 卷。升级就是 `docker compose pull && docker compose up -d`，迁移会自动跑。想用本地代码构建，执行 `docker build -t nextapi .`，或者在 compose 里注释掉 `image:`、放开 `build:`。

### 首次使用

1. 用 `admin` 和上面的初始密码登录，顺手在「系统设置 → 修改密码」里换掉。
2. 到「上游」页接入供应商：标准渠道填 API Key 即可，Codex 渠道粘贴 `auth.json`。
3. 到「路由」页确认对外模型名和上游的对应关系（预设和模型同步会自动生成一部分）。
4. 到「密钥」页建一个网关 Key，然后就能用下面的方式调用了。

## 网关调用

网关默认监听 `0.0.0.0:3220`，几个协议共用同一个网关 Key：

| 协议 | 端点 | 鉴权头 |
|---|---|---|
| OpenAI Chat Completions | `POST /v1/chat/completions` | `Authorization: Bearer sk-…` |
| OpenAI Responses | `POST /v1/responses` | `Authorization: Bearer sk-…` |
| Anthropic Messages | `POST /v1/messages` | `x-api-key: sk-…` |
| Gemini | `POST /v1beta/models/{model}:generateContent` | `x-goog-api-key: sk-…` |
| 模型列表 | `GET /v1/models` | 上述任一种 |
| 图片生成 | `POST /v1/images/generations`、`GET /v1/images/tasks/{id}` | 上述任一种 |

```bash
# OpenAI 风格
curl http://localhost:3220/v1/chat/completions \
  -H "Authorization: Bearer sk-网关Key" \
  -H "Content-Type: application/json" \
  -d '{"model":"gpt-4o","messages":[{"role":"user","content":"你好"}]}'

# Anthropic 风格（同一个网关 Key）
curl http://localhost:3220/v1/messages \
  -H "x-api-key: sk-网关Key" -H "anthropic-version: 2023-06-01" \
  -H "Content-Type: application/json" \
  -d '{"model":"claude-3-5-sonnet","max_tokens":1024,"messages":[{"role":"user","content":"你好"}]}'

# Gemini 风格
curl http://localhost:3220/v1beta/models/gemini-1.5-pro:generateContent \
  -H "x-goog-api-key: sk-网关Key" \
  -H "Content-Type: application/json" \
  -d '{"contents":[{"parts":[{"text":"你好"}]}]}'
```

## 性能与资源占用

资源占用与登录次数、请求体大小、日志队列和 glibc malloc arena 的保留策略有关。历史版本在全新实例上的基线是「空载数 MiB、登录后数十 MiB」；长时间运行、多次登录或处理过大请求体后，glibc 可能保留多个 arena，容器占用可能显著高于空载基线，进程 RSS 通常也高于容器口径。两个口径都正常：`docker stats` 不计宿主机 page cache 中的共享库文件页，RSS 则把私有驻留和共享页都算进来。

v0.4.0 起新增：

- 管理员登录成功后主动 `malloc_trim` 归还空闲堆；
- 周期性 `malloc_trim`，降低长期运行后的 arena 保留；
- 请求体读完后提前释放，减少大请求峰值占用。

`malloc_trim` 只归还 glibc 认为空闲的堆页，不保证 RSS 立即下降，也不替代合理的内存配额。生产建议给 nextapi 容器 512 MiB 配额，并同时观察 cgroup `memory.current` 与进程 RSS；PostgreSQL 的空库通常几十 MiB，之后主要随日志增长。

## 配置

配置文件默认是 `./config.yaml`（可用 `NEXTAPI_CONFIG` 指向别处），所有字段和默认值见 [`config.example.yaml`](./config.example.yaml)。配置分四类：

| 分类 | 生效方式 |
|---|---|
| `hot`（运行参数） | 文件热加载；UI 里保存过的键以 UI 为准，不被文件重载覆盖 |
| 启动类（`server.*`、`database.url`） | 需要重启；优先级 env > UI > YAML |
| `seed`（admin、上游、汇率种子） | 只在首次初始化时写入数据库，之后以管理后台为准 |
| `NEXTAPI_SECRET_KEY` | 只认环境变量：不落库、不写 YAML、界面上改不了 |

常用的环境变量：

| 环境变量 | 用途 |
|---|---|
| `NEXTAPI_SECRET_KEY` | 上游 Key、OAuth token、代理密码等敏感字段的 AES-256-GCM 加密密钥，生产必配 |
| `NEXTAPI_ADMIN_JWT_SECRET` / `NEXTAPI_LISTEN` / `DATABASE_URL` | 启动类参数的 env 覆盖 |
| `NEXTAPI_ADMIN_INITIAL_PASSWORD` | 首次启动的管理员初始密码，留空则随机生成并打印一次 |

## 核心链路

```
网关 Key 鉴权 → RPM/TPM 限流 + 配额预检 → 协议解析（内部 IR）
→ 模型路由（通配匹配 / 加权 / 熔断过滤）→ 上游调用
   ├─ 透传：上游支持入口协议 → 原样转发（SSE 边转发边改写 model/id）
   └─ 否则：按 IR 转换协议（失败可转移到下一个候选）
→ （旁路）usage 提取 → 计价 → 异步批量写库
```

- 记账走旁路：有界队列 + 批量写者，一个事务里写明细、聚合和配额；队列满先落 WAL（JSONL + CRC 校验），后台幂等重放，主链路不受影响。
- 计价公式是 `cost = matched_price × quantity`，分段规则按数组顺序取第一个命中，都没命中就用 base_price；汇率可以手工维护，也可以定时从 ECB / Frankfurter 拉取。网关只统计成本，不碰余额和扣费。
- 明确不做：用户侧充值、用户余额、兑换码、套餐、支付一类运营功能；终端用户账号体系；倍率体系；Embeddings / Rerank / Audio / Moderation / Fine-tuning 端点。上游供应商余额只读监控不属于用户侧余额售卖语义。

## 从源码构建

需要 Rust stable、Node.js ≥ 20 和 PostgreSQL 15+：

```sh
cd web && npm install && npm run build   # 前端，产物经 rust-embed 内嵌进二进制
cargo build --release                    # 后端
cargo test                               # 单元测试
```

两点注意：`cargo build` / `cargo test` 会在编译期读取 `web/dist`，目录不存在会直接编译失败，所以改完前端要先 `npm run build`（Docker 多阶段构建已经保证了这个顺序）；Node.js 只在编译时需要，无论跑编译出的二进制还是官方镜像，运行期都不需要它。

## 文档

- [`AGENTS.md`](./AGENTS.md) — 工程约定：请求链路、配置单一事实源、透传优先、测试约束等
- [`config.example.yaml`](./config.example.yaml) — 全部配置项与默认值
- [`CHANGELOG.md`](./CHANGELOG.md) — 版本更新日志
- [`docs/reverse-proxy.md`](./docs/reverse-proxy.md) — 反向代理与子路径部署
- [`docs/protocol-matrix.md`](./docs/protocol-matrix.md) — 四协议能力矩阵
- [`docs/upstream-balance.md`](./docs/upstream-balance.md) — 上游余额/额度监控

## License

[Apache-2.0](./LICENSE)
