# AGENTS.md — NextAPI

> 本文件供 AI 编码代理阅读。假设读者对本项目一无所知。

## 项目概述

**NextAPI** 是一个**自托管（开源）的 LLM 网关**：协议转换（OpenAI Chat Completions / OpenAI Responses / Anthropic Messages / Gemini 四协议互转）+ 多上游聚合路由 + 日志/成本统计 + 图片/视频生成透传 + 管理后台，定位为「面向团队/应用的工具型网关」。

**当前状态：M1 工程骨架、M2 协议层已完成（2026-08）。** 仓库为 Rust + axum 工程，设计文档 `PLAN.md`（当前 v1.15，约 950 行中文）仍是**唯一事实源**，里程碑验收以此为准。做任何开发前先读它。

M1 已交付：Cargo 工程、配置加载 + hot/seed/derived 分类热加载（`src/config.rs`）、`system_settings` UI 持久化与优先级（hot：UI > YAML；启动类：env > UI > YAML，`src/settings.rs`）、`proxy_configs` 基础表 + CRUD API、单管理员 JWT 登录（防爆破）、axum 服务、`/healthz`、`/metrics`、Dockerfile、docker-compose（含 PG）、`config.example.yaml`。

M2 已交付：协议层 `src/protocol/`——IR（OpenAI Chat 为枢轴 + ext 扩展字段）+ 四协议适配器（chat/responses/anthropic/gemini，非流式 + SSE 流式双向）、错误体翻译、SSE 行解析器、降级收集（`ConvCtx` → `X-NextAPI-Degraded`）、转换矩阵文档（`docs/protocol-matrix.md`）。转换逻辑为纯函数，M3 网关核心负责接线。

其他文件说明：

- `.owc/memory.md` — AI 代理的会话记忆（非项目文档）。
- `.owc/venv/` — AI 代理工具链的 Python venv，与项目无关，勿动。
- 根目录有一个 0 字节的乱码文件名（`:\x1a@=...`），疑似误建，与本项目无关。
- 尚未 git init。

### 明确不做（硬约束）

- 不做任何运营售卖功能：充值、余额、积分、兑换码、邀请、套餐、支付、账单、发票；
- 不做终端用户账号体系：仅单管理员登录，无用户/角色/分组；
- 不做倍率体系：后端只存直接价格（倍率仅为纯前端试算工具）；
- v1 不做 Embeddings / Rerank / Audio / Moderation / Fine-tuning 端点；
- 价格只按「供应商 + 模型 ID」绑定存储，未设价格即不计价（记 NULL + 提示）。

## 技术栈（规划）

| 层 | 选型 |
|----|------|
| 后端语言 | Rust (stable) + tokio (multi-thread) |
| Web 框架 | axum 0.8 |
| HTTP 客户端 | reqwest (rustls)，按代理/直连分池 |
| 数据库 | PostgreSQL 15+，sqlx（编译期校验，rustls）|
| 配置 | YAML（serde_yaml）+ ArcSwap 热加载 |
| 精度 | 计价全程 `rust_decimal`（DB `NUMERIC(20,10)`）|
| 观测 | tracing；Prometheus `/metrics` |
| 前端 | Vue 3 + TypeScript + Vite + Element Plus + ECharts，rust-embed 嵌入二进制 |
| 部署 | 单 Docker 镜像（后端 + 内嵌前端）+ docker-compose（含 PG）|

实现要点：异步记账用 `tokio::sync::mpsc` 有界队列 + 批量写 worker；WAL 兜底为 append-only JSONL（CRC 校验，单文件 128MB）；价格导入导出用 `serde_json` / `quick-xml`。

## 构建与测试命令

- 构建：`cargo build`；检查：`cargo check`；运行：`cargo run`（需要 PostgreSQL：本机 PG 或 `docker-compose up postgres`）
- 测试：`cargo test`（单元测试在各模块内 `#[cfg(test)]`；集成测试规划在 `tests/`，需 docker-compose 起 PG + mock 上游）
- 前端（M8 才存在）：`cd web && npm install && npm run build`（产物经 rust-embed 嵌入）
- 部署：`docker-compose up -d`（`nextapi` + `postgres:16`，自动迁移；首次先 `cp config.example.yaml config.yaml`）
- M9 加入 `cargo audit` / `cargo deny` 依赖审计

**注意**：所有 SQL 用 sqlx **运行时校验**（`sqlx::query`，禁用 `query!` 宏），保证无数据库环境也能 `cargo check`。沙盒无 docker 守护进程时，DB 相关逻辑只能编译验证，无法集成测试。

## 代码组织

M1 实际结构（PLAN.md §14 的后续模块将在对应里程碑补充）：

```
src/
├── main.rs          # 启动流程：配置→加密→DB→迁移→种子→设置引擎→路由→热加载监听→serve
├── config.rs        # YAML 加载 + hot/seed/derived 分类 + hot_flat/apply_overrides/掩码/文件监听
├── settings.rs      # system_settings 持久化 + 优先级引擎（hot: UI>YAML；启动类: env>UI>YAML）
├── db.rs            # sqlx 连接池 + 迁移（sqlx::migrate!）
├── seed.rs          # app_meta.seeded_at 标记 + admin/proxies/fx_rates 种子（argon2）
├── crypto.rs        # AES-256-GCM（NEXTAPI_SECRET_KEY，env-only）
├── error.rs         # ApiError 统一错误
├── state.rs         # AppState（PgPool / ArcSwap<HotConfig> / SettingsEngine / Crypto / Jwt / Metrics）
├── metrics.rs       # Prometheus /metrics
├── protocol/        # IR + 4 协议适配器（chat/responses/anthropic/gemini）+ sse + errors + 降级收集
├── auth/mod.rs      # 单管理员 JWT + 登录防爆破（内存滑窗）+ require_admin 中间件 + 审计写入
└── admin/
    ├── mod.rs       # /api 路由聚合
    ├── proxies.rs   # /api/proxies CRUD（密码加密、掩码回传=保持原值、test 桩 M3 启用）
    ├── settings_api.rs # /api/settings + /api/config(+reload)
    └── audit.rs     # /api/audit 分页查询
migrations/0001_init.sql  # app_meta/system_settings/admin_users/admin_audit_logs/proxy_configs/fx_rates
tests/                    # 集成测试（后续里程碑）
web/                      # Vue3 管理后台（M8）
```

M3 及以后规划的模块（protocol/、routing/、upstream/、billing/、limit/、stats.rs、cache.rs）见 PLAN.md §14。

## 核心架构约定（开发时必须遵守）

- **请求链路**：鉴权 → 限流/用量上限 → 协议解析 → 路由决策 → 透传/转换 → 上游调用 → （旁路）usage 提取 → 计价 → 异步批量写库（只统计，绝不阻塞主链路）。
- **透传优先**：上游支持入口协议则原样转发（仅改写模型名、替换鉴权头、应用用户配置的请求头/体 add/set 覆盖；SSE 边转发边改写 `model/id`）；不支持则走内部 IR 换协议（转换失败可回退透传）。
- **配置单一事实源**：DB 为运行态事实，YAML 仅首次初始化种子（`app_meta.seeded_at` 标记，不靠"表空才写入"）。热参数优先级 **UI > YAML**；启动类参数 **env > UI > YAML**；`NEXTAPI_SECRET_KEY` 为 env-only 例外（不落 DB/YAML）。
- **日志存储**：`usage_logs` 按 `ts` 声明式分区（每 30 天一个分区），**不自动清理**，管理员手动整分区 DROP（支持 dry_run）；`usage_hourly` 滚动聚合与明细并存；配额用独立 `quota_usage` 计数器（与日志清理解耦，request_id 幂等）。
- **计价**：`cost = matched_price × quantity`；分段有序第一命中（时间 ∧ 上下文长度条件），无命中回落 `base_price`；CNY/USD 双币种快照 + `price_used`/`fx_snapshot` 入账；展示统一舍入 6 位小数；视频秒数向上取整至少 1 秒。
- **路由容灾**：重试参数只在 `model_routes` 维护；熔断连续失败自动禁用 + 冷却 + 半开探活；手动禁用不被自动恢复；SSE 首字节后不可换上游。

## 开发流程与里程碑

M0 计划确认 → M1 工程骨架 → M2 协议层 → M3 网关核心 → M4 日志统计 → M5 计价引擎 → M6 图片/视频 → M7 供应商预设 → M8 管理后台 → M9 测试与发布。每阶段交付物/验收标准见 PLAN.md §9。

**工作约定**：

- 文档与注释使用**中文**（PLAN.md 全部为中文）。
- PLAN.md 采用版本化变更说明（v1.1 … v1.15），修改设计决策时按此惯例追加版本变更段落。
- 风险表（§13）中 License 默认 Apache-2.0 但仍标注"待最终确认"，发布前需关闭。
- 所有设计细节（字段、API 路径、SQL schema、默认值）以 PLAN.md 为准；实现与文档冲突时先确认是改文档还是改实现。

## 测试策略（PLAN.md §10 摘要）

- **单元测试**：协议转换（每对协议 × 文本/图片/工具/流式）、透传/转换决策、计价引擎（分段边界、双币种换算、维度回落）、限流/熔断状态机、quota_usage 幂等。
- **集成测试**：docker-compose 起 PG，mock 上游（wiremock 风格）验证透传/转换/记账；日志分区/手动清理/WAL 重放；配置热加载与 UI/YAML/env 优先级；代理矩阵。
- **客户端兼容测试**：OpenAI / Anthropic / Gemini 官方 SDK 直接指向网关作为验收用例。
- **前端/安全**：Vitest + Playwright E2E；登录防爆破、越权、掩码测试。
- **性能冒烟**：透传附加开销 < 5ms 中位数；SSE 首字节延迟。
- Live 测试（真实供应商 Key）可选，CI 不跑。

## 安全考虑

- 网关 Key：DB 只存 SHA-256 哈希 + 前 8 位前缀，完整 Key 仅创建时展示一次。
- 管理员密码 argon2；JWT 短期 access token；登录接口防爆破限速（默认 10 次/分钟）。
- 敏感字段（上游 api_key、JWT secret、代理密码）落库前 AES-256-GCM 加密，密钥来自 env `NEXTAPI_SECRET_KEY`；导出/展示一律掩码，掩码值回传语义 = 保持原值。
- 脱敏：永不记录 `Authorization` / `x-api-key` / `x-goog-api-key` / `Cookie` / `Proxy-*`；debug 模式完整记录限 64KB 且强制脱敏，最长 1 小时自动过期。
- SSRF 防护：URL 导入/媒体下载默认仅 https、限制大小与超时、禁内网地址；代理连通性测试只允许固定探测地址白名单。
- 任务查询校验归属 Key（防跨 Key 遍历）；`/v1/upstream/*` 透传需 Key 显式授权。
- 生产环境 `admin_jwt_secret` 缺失且非 debug 模式拒绝启动（无 change-me 默认值）。
