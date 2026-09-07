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

NextAPI 是一个自托管的 LLM 网关（Rust + axum + PostgreSQL）：客户端用任一主流协议接入，网关按路由规则转发到多个上游供应商，自动完成协议互转、故障转移、限流与成本记账。管理后台（Vue 3）内嵌进单二进制，同源部署，浏览器打开即用。

```
客户端 (OpenAI / Anthropic / Gemini SDK)  ──任一协议──►  NextAPI 网关  ──透传或转换──►  多上游供应商
                                                              │
                              管理后台 (内嵌 SPA) + PostgreSQL（日志 / 计价 / 配额）
```

## 配置要求

1. 服务端（二选一）：
  - Docker：Docker 20.10+ 与 Compose v2（推荐，镜像含全部依赖，自动迁移数据库）；
  - 源码运行：Linux / macOS，Rust stable、Node.js ≥ 20（构建前端）、PostgreSQL 15+。
  - 资源：内存 ≥ 512 MiB 空闲，硬盘 ≥ 500 MiB 可用。

2. 客户端：
  - 管理后台：可运行 Chrome / Edge ≥ 111 或 Firefox ≥ 113 的设备（含手机和平板）；
  - 网关调用：任意 OpenAI / Anthropic / Gemini 兼容 SDK 或 HTTP 客户端。

## 主要功能

- 协议互转：OpenAI Chat Completions / OpenAI Responses / Anthropic Messages / Gemini 四协议互相转换，客户端与上游可任意组合；上游支持入口协议时透传优先（仅改写模型名与鉴权头）。
- 聚合路由：模型通配匹配、优先级分组 + 加权随机、自动重试与故障转移、熔断（连续失败自动禁用 + 冷却 + 半开探活，冷却状态重启不丢）。
- 渠道类型：标准 API Key 渠道 + Codex OAuth 渠道（粘贴 `~/.codex/auth.json`，临期自动刷新、轮换回写、失败联动熔断）；多个内置供应商预设一键接入；渠道模型列表自动同步托管路由。
- 鉴权限流：网关 Key（SHA-256 哈希存储）+ 每 Key RPM/TPM 滑窗与 token/成本配额上限；管理员登录防爆破 + 可选 TOTP 二步验证。
- 日志与成本：请求明细按 30 天声明式分区（不自动清理，手动整分区 DROP）+ 小时聚合；双币种（CNY/USD）分段计价统计；异步批量写库 + WAL 兜底，绝不阻塞主链路。
- 图片生成：统一 OpenAI 语义入口，适配 OpenAI / Gemini / Dashscope（同步与异步任务轮询计费闭环）四种上游形状。
- 可观测：Prometheus `/metrics`（管理员 JWT 鉴权）、审计日志（含来源 IP）、日志 CSV 导出、脱敏存储。
- 管理后台：登录 / 仪表盘 / 密钥 / 上游 / 路由 / 价格 / 日志 / 统计 / 系统设置，移动端抽屉侧栏适配。

## 快速开始

### Docker（推荐，x86_64 / arm64）

发布镜像托管在 GitHub Container Registry（`ghcr.io/snnh/nextapi`），compose 默认拉取发布镜像，PostgreSQL 16 一并启动：

```sh
# 1. 准备配置与密钥
cp config.example.yaml config.yaml
export NEXTAPI_SECRET_KEY=$(openssl rand -base64 48)        # 敏感字段加密密钥（env-only）
export NEXTAPI_ADMIN_JWT_SECRET=$(openssl rand -base64 48)  # 管理端 JWT 签名密钥

# 2. 启动
docker compose up -d

# 3. 查看初始管理员密码（未设 NEXTAPI_ADMIN_INITIAL_PASSWORD 时随机生成，仅打印一次）
docker compose logs nextapi | grep 初始密码
```

浏览器打开 `http://<主机IP>:3220` 登录管理后台。

不用 compose 时（需自备 PostgreSQL 15+）：

```sh
docker run -d --name nextapi --restart unless-stopped \
  -p 3220:3220 -v nextapi-data:/data \
  -e DATABASE_URL=postgres://user:pass@host:5432/nextapi \
  -e NEXTAPI_SECRET_KEY=$(openssl rand -base64 48) \
  -e NEXTAPI_ADMIN_JWT_SECRET=$(openssl rand -base64 48) \
  ghcr.io/snnh/nextapi:latest
```

- **数据**：日志/配额持久化在 PostgreSQL（卷 `pgdata`），WAL 与运行数据在卷 `nextapi-data`。
- **升级**：`docker compose pull && docker compose up -d`（迁移自动执行）。
- **从源码构建**：`docker build -t nextapi .`，或在 compose 里取消 `build:` 注释替换 `image:`。

### 首次使用

1. 登录后台（用户名默认 `admin`，初始密码见上；登录后立即在「系统设置 → 修改密码」更换）。
2. 在「上游」页用供应商预设一键接入（仅填 API Key），或粘贴 Codex `auth.json` 建 OAuth 渠道。
3. 在「路由」页确认网关模型 → 上游模型的映射（预设与模型同步会自动托管）。
4. 在「密钥」页创建网关 Key（完整 Key 仅展示一次），即可用客户端直连网关。

## 网关调用示例

网关默认监听 `0.0.0.0:3220`，四协议端点共用网关 Key 鉴权：

| 协议 | 端点 | 鉴权头 |
|---|---|---|
| OpenAI Chat Completions | `POST /v1/chat/completions` | `Authorization: Bearer sk-…` |
| OpenAI Responses | `POST /v1/responses` | `Authorization: Bearer sk-…` |
| Anthropic Messages | `POST /v1/messages` | `x-api-key: sk-…` |
| Gemini | `POST /v1beta/models/{model}:generateContent` | `x-goog-api-key: sk-…` |
| 模型列表 | `GET /v1/models` | 任一上述鉴权头 |
| 图片生成 | `POST /v1/images/generations`、`GET /v1/images/tasks/{id}` | 任一上述鉴权头 |

```bash
# OpenAI 风格
curl http://localhost:3220/v1/chat/completions \
  -H "Authorization: Bearer sk-网关Key" \
  -H "Content-Type: application/json" \
  -d '{"model":"gpt-4o","messages":[{"role":"user","content":"你好"}]}'

# Anthropic 风格（同一网关 Key）
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

## 配置

配置文件默认 `./config.yaml`（`NEXTAPI_CONFIG` 可覆盖），字段与默认值见 [`config.example.yaml`](./config.example.yaml)。

| 分类 | 生效方式 |
|---|---|
| `hot`（运行参数） | 文件热加载；**UI 保存值优先**（不被文件重载覆盖） |
| 启动类（`server.*` / `database.url`） | 需重启；优先级 **env > UI > YAML** |
| `seed`（admin/上游/汇率种子） | 仅首次初始化写入 DB，之后以管理后台为准 |
| `NEXTAPI_SECRET_KEY` | env-only 例外：不落 DB、不进 YAML、不可 UI 设置 |

| 环境变量 | 用途 |
|---|---|
| `NEXTAPI_SECRET_KEY` | 敏感字段（上游 Key、OAuth token、代理密码）AES-256-GCM 加密密钥，**生产必配** |
| `NEXTAPI_ADMIN_JWT_SECRET` / `NEXTAPI_LISTEN` / `DATABASE_URL` | 启动类参数 env 覆盖 |
| `NEXTAPI_ADMIN_INITIAL_PASSWORD` | 首次启动管理员初始密码（留空则随机生成打印一次） |

## 核心设计

```
网关 Key 鉴权 → RPM/TPM 限流 + 配额预检 → 协议解析（内部 IR）
→ 模型路由（通配匹配 / 加权 / 熔断过滤）→ 上游调用
   ├─ 透传优先：支持入口协议 → 原样转发（SSE 边转发边改写 model/id）
   └─ 否则按 IR 转换协议（失败可回退透传）
→ （旁路）usage 提取 → 计价 → 异步批量写库（绝不阻塞主链路）
```

- **异步记账**：有界队列 + 批量写者（同事务写明细/聚合/配额）；队列满先落 WAL（append-only JSONL + CRC）再丢弃内存条目，后台幂等重放。
- **计价**：`cost = matched_price × quantity`，分段（时间窗/星期/上下文长度）有序第一命中、无命中回落 base_price；汇率手动维护或自动拉取（USD↔CNY 内置兜底 6.73）；价格表 XML/JSON 导入导出。**只统计、不扣费**。
- **明确不做**：充值/余额/兑换码/套餐/支付等运营售卖功能、终端用户账号体系、倍率体系、Embeddings/Rerank/Audio/Moderation/Fine-tuning 端点（v1）。

## 从源码构建

需要 Rust stable、Node.js ≥ 20、PostgreSQL 15+：

```sh
cd web && npm install && npm run build   # 前端（产物经 rust-embed 内嵌进二进制）
cargo build --release                    # 后端
cargo test                               # 单元测试（集成测试 tests/ 需 docker compose 起 PG + mock 上游）
```

> **注意**：`cargo build`/`cargo test` 编译期读取 `web/dist`，目录缺失会编译失败——改完前端先 `npm run build` 再执行 cargo 命令；Docker 多阶段构建已自动保证顺序。

## 文档

- [`AGENTS.md`](./AGENTS.md) — 面向 AI 编码代理的工程约定：请求链路、配置单一事实源、透传优先、安全考虑、测试约定
- [`config.example.yaml`](./config.example.yaml) — 全部配置项与默认值
- [`CHANGELOG.md`](./CHANGELOG.md) — 版本更新日志

## License

[Apache-2.0](./LICENSE)
