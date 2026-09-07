# NextAPI

自托管 **LLM 网关**（Rust + axum）：统一接入多供应商模型，一套网关 Key 走遍四种主流协议。

- **协议互转**：OpenAI Chat Completions / OpenAI Responses / Anthropic Messages / Gemini 四协议互相转换，客户端与上游可用任意协议组合；
- **聚合路由**：模型通配匹配、优先级分组 + 加权随机、自动故障转移与重试、熔断（连续失败自动禁用 + 冷却 + 半开探活）；
- **鉴权限流**：网关 Key（哈希存储）+ 每 Key RPM/TPM/配额（token/成本）上限；
- **日志与成本**：请求明细（分区存储）+ 小时聚合 + 双币种（CNY/USD）计价，**只统计不扣费**；
- **图片生成**：统一 OpenAI 语义入口，适配 OpenAI / Gemini / Dashscope（同步与异步任务）四种上游形状；
- **管理后台**：内置 Vue 3 控制台（登录 / 仪表盘 / 密钥 / 上游 / 路由 / 价格 / 日志 / 统计 / 系统设置 9 页），产物内嵌进单二进制，同源部署。

## 快速开始（Docker Compose）

```bash
cp config.example.yaml config.yaml
# 生产必改（docker-compose.yml 中）：
#   NEXTAPI_SECRET_KEY        —— 敏感字段 AES-256-GCM 加密密钥（env-only），如 `openssl rand -base64 48`
#   NEXTAPI_ADMIN_JWT_SECRET  —— 管理端 JWT 签名密钥
docker compose up -d
```

启动后：

1. 打开 `http://<host>:8080`，使用管理员账号登录。
   - 用户名：`config.yaml` 的 `admin.username`（默认 `admin`）；
   - 初始密码优先级：`admin.password_hash` > `admin.initial_password` > 环境变量 `NEXTAPI_ADMIN_INITIAL_PASSWORD` > **自动生成的随机密码（仅打印一次到容器日志）**；
   - 登录后请立即在「系统设置 → 修改密码」中更换密码。
2. 在「供应商预设」页一键接入上游（仅需填 API Key），或手动创建上游与路由；
3. 创建网关 Key，即可以 OpenAI/Anthropic/Gemini 客户端直连网关。

> 单容器运行（无 Docker）：需要 PostgreSQL 15+，`cargo build --release` 后以 `DATABASE_URL` / `NEXTAPI_SECRET_KEY` / `NEXTAPI_ADMIN_JWT_SECRET` 环境变量启动；`server.admin_jwt_secret` 与 `admin_jwt_secret` 为空且非 `server.debug` 时拒绝启动。

## 网关调用示例

网关默认监听 `0.0.0.0:8080`，四协议端点：

| 协议 | 端点 |
|---|---|
| OpenAI Chat Completions | `POST /v1/chat/completions` |
| OpenAI Responses | `POST /v1/responses` |
| Anthropic Messages | `POST /v1/messages` |
| Gemini | `POST /v1beta/models/{model}:generateContent`（流式 `:streamGenerateContent?alt=sse`） |
| 模型列表 | `GET /v1/models`（需网关 Key） |
| 图片生成 | `POST /v1/images/generations`、`GET /v1/images/tasks/{id}` |
| 视频 | `POST /v1/videos/generations`（占位 501，未接入） |

```bash
# OpenAI 风格
curl http://localhost:8080/v1/chat/completions \
  -H "Authorization: Bearer sk-网关Key" \
  -H "Content-Type: application/json" \
  -d '{"model":"gpt-4o","messages":[{"role":"user","content":"你好"}]}'

# Anthropic 风格（同一网关 Key）
curl http://localhost:8080/v1/messages \
  -H "x-api-key: sk-网关Key" \
  -H "anthropic-version: 2023-06-01" \
  -H "Content-Type: application/json" \
  -d '{"model":"claude-3-5-sonnet","max_tokens":1024,"messages":[{"role":"user","content":"你好"}]}'

# Gemini 风格
curl http://localhost:8080/v1beta/models/gemini-1.5-pro:generateContent \
  -H "x-goog-api-key: sk-网关Key" \
  -H "Content-Type: application/json" \
  -d '{"contents":[{"parts":[{"text":"你好"}]}]}'
```

**请求的是「网关模型名」而非上游模型名**：`路由（model_routes）` 负责把网关模型映射到上游模型的协议端点；透传模式下仅改写模型名与鉴权头，上游请求体原样转发（支持的协议），上游不支持入口协议时网关按内部 IR 自动转换。

## 配置

配置文件默认 `./config.yaml`（环境变量 `NEXTAPI_CONFIG` 可覆盖），字段与默认值见 `config.example.yaml`。

**配置分类**（详细语义见文件头部注释）：

| 分类 | 生效方式 |
|---|---|
| `hot`（网关运行参数） | 文件热加载；**UI 保存值优先**（`system_settings.source=ui` 不被文件重载覆盖） |
| 启动类（`server.*` / `database.url`） | 需重启；优先级 **env > UI > YAML** |
| `seed`（admin/上游/汇率种子） | 仅首次初始化写入 DB（`app_meta.seeded_at` 标记），之后以管理后台为准 |
| `NEXTAPI_SECRET_KEY` | env-only 例外：不落 DB、不进 YAML、不可 UI 设置 |

**环境变量**：

| 变量 | 用途 |
|---|---|
| `NEXTAPI_SECRET_KEY` | 敏感字段（上游 api_key、JWT secret、代理密码）落库 AES-256-GCM 加密密钥，**生产必配** |
| `NEXTAPI_ADMIN_JWT_SECRET` / `NEXTAPI_LISTEN` / `DATABASE_URL` | 启动类参数 env 覆盖 |
| `NEXTAPI_ADMIN_INITIAL_PASSWORD` | 首次启动管理员初始密码 |
| `NEXTAPI_CONFIG` | 配置文件路径（默认 `./config.yaml`） |

> `log_partition_days` 首次建分区时固化到 `app_meta`，此后以固化值为准（修改仅对尚未建过分区的空库生效）。

## 核心设计

### 请求链路

```
网关 Key 鉴权 → RPM/TPM 限流 + 配额预检 → 协议解析（内部 IR）
→ 模型路由（通配匹配/加权/熔断过滤）→ 上游调用
   ├─ 透传优先：支持入口协议 → 原样转发（改写模型/鉴权头/覆盖规则；SSE 边转发边改写 model/id）
   └─ 否则按 IR 转换协议（失败可回退透传）
→ （旁路）usage 提取 → 计价 → 异步批量写库（绝不阻塞主链路）
```

- **透传优先**：上游支持入口协议则原样转发，仅改写模型名、替换鉴权头、应用用户配置的 add/set 覆盖；SSE 流式边转发边改写 `model`/`id`，并保留 `event:` 事件行（Anthropic / Responses 原生流式兼容）。
- **路由容灾**：重试参数在 `model_routes` 维护；熔断状态回写 DB（`disabled_by='auto' + cooldown_until`），进程重启后冷却仍生效；手动禁用不被自动恢复；SSE 首字节发出后不可换上游。
- **异步记账**：有界 mpsc 队列 + 批量写者（同事务：`usage_logs` UNNEST 多行 → `usage_hourly` 聚合 → `quota_usage` 累加 → `last_used_at`）；队列满则先写 WAL（append-only JSONL + CRC，单文件 128MB 滚动）再丢弃内存条目，后台定时重放；`insert_batch` 以 `RETURNING request_id` 保证重复重放只聚落实际新插入的行（幂等）。

### 日志与统计

- `usage_logs` 按时间声明式分区（每 30 天，epoch 对齐），**不自动清理**，管理员手动整分区 DROP（支持 dry_run 预览）；
- `usage_hourly` 滚动小时聚合（明细按天/小时查询自动回落选择数据源）；
- `quota_usage` 独立计数器，与日志清理解耦、request_id 幂等；
- 管理后台「日志」页支持 CSV 导出；审计（登录/管理操作，含来源 IP）独立 `admin_audit_logs` 表；
- Prometheus `/metrics` 需管理员 JWT（`Authorization: Bearer <token>`；Prometheus 抓取配置 `bearer_token`）。
- 管理员登录支持可选 TOTP 二步验证（RFC 6238，SHA1/30s/6 位 ±1 步）：设置 → 系统设置 → 安全；设置与禁用均需当前密码，启用/禁用即注销全部会话；同一验证码不可重放。

### 计价

`cost = matched_price × quantity`，分段（时间窗/星期/上下文长度）有序第一命中、无命中回落 `base_price`；CNY/USD 双币种快照（`price_used` / `fx_snapshot` 入账）；汇率支持手动维护与自动拉取（Frankfurter / ECB / 自定义）；价格表支持 XML/JSON 导入导出。**只统计、不扣费**——不设余额/扣减语义，成本配额仅用于用量上限告警与阻断。

## 管理后台 API（前缀 `/api`）

- `POST /api/auth/login`（公开，防爆破限速）/ `GET /api/auth/me` / `PUT /api/auth/password`；
- `/api/keys`、`/api/upstreams`（含连通性测试）、`/api/proxies`、`/api/model-routes`；
- `/api/pricing`（规则/预览/建议/未定价/导入导出）、`/api/fx`、`/api/presets`（8 个内置预设 + 一键接入）；
- `/api/logs`（明细/详情/分区清理/CSV）、`/api/stats`（summary/series）、`/api/audit`；
- `/api/settings`（运行参数键值）、`/api/config`（YAML 查看/保存/重载）、`/api/system`（版本/更新检查）。

除登录外全部接口需 `Authorization: Bearer <管理端 JWT>`（1 小时短期令牌）；管理操作自动写审计。

## 开发

```bash
# 后端（需要 PostgreSQL 15+）
cargo run                     # 或 cargo run --release
# 前端（web/）
cd web && npm install && npm run dev   # vite :5173，/api 代理到 127.0.0.1:8080
```

> **注意**：`cargo build`/`cargo test` 编译期读取 `web/dist`（rust-embed 内嵌），目录缺失会编译失败——改动前端后先 `npm run build` 再执行 cargo 命令；Docker 多阶段构建已自动保证顺序。

- 检查：`cargo check` / `cargo fmt` / `cargo clippy`；测试：`cargo test`（单元测试随模块；集成测试在 `tests/`，需要 docker compose 起的 PG + mock 上游）；
- 前端：`cd web && npm run typecheck`（vue-tsc）、`npm run build`；
- 所有 SQL 用 sqlx 运行时校验（`sqlx::query`，禁用 `query!` 宏），无数据库环境也能 `cargo check`。

### 目录结构

```
src/
├── main.rs        # 启动流程：配置 → 加密 → DB → 迁移 → 种子 → 设置引擎 → 路由 → 热加载 → serve
├── config.rs      # YAML 加载 + hot/seed/derived 分类 + 掩码 + 文件监听
├── settings.rs    # system_settings 持久化 + 优先级引擎（UI > YAML / env > UI > YAML）
├── gateway.rs     # 网关入口与主链路（/v1/*、图片通道、视频占位）
├── protocol/      # IR + 4 协议适配器 + SSE + 错误映射
├── routing/       # 通配匹配、加权路由、熔断状态机
├── limit/         # RPM/TPM 滑窗、配额预检
├── upstream/      # reqwest 分池、透传改写、SSE 流、usage 提取
├── logging/       # 日志队列/批量写者/WAL/分区/脱敏
├── billing/       # 计价引擎 + 汇率（fx）+ 价格导入导出
├── media/         # 图片通道（4 上游形状适配 + 异步任务闭环）
├── auth/ admin/   # 管理后台：JWT/防爆破/审计 + /api CRUD
├── presets.rs     # 供应商预设 + 一键接入
└── embed.rs       # 前端静态资源内嵌 + SPA fallback
migrations/        # 0001~0006 SQL 迁移（sqlx::migrate! 自动执行）
web/               # Vue 3 管理后台（详见 web/README.md）
contracts/         # 里程碑实现契约（仅本地参考，不进版本库）
tests/             # 集成测试（需要 PG + mock 上游）
```

### 设计文档与约定

- `AGENTS.md`：面向 AI 编码代理的工程约定（请求链路、配置单一事实源、透传优先、安全考虑、测试约定）；
- `PLAN.md`：里程碑计划与版本记录（唯一事实源，仅本地）；
- 明确不做：充值/余额/兑换码/套餐/支付等运营售卖功能、终端用户账号体系、倍率体系、Embeddings/Rerank/Audio/Moderation/Fine-tuning 端点（v1）。

## License

[Apache-2.0](LICENSE)
