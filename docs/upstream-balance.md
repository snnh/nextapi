# 上游余额/额度监控

NextAPI 支持查询上游供应商的账户余额或套餐额度，并在管理后台展示最近一次快照。该功能是**只读监控**：不引入用户余额、充值、扣费、账单或套餐售卖语义。

## 支持范围

| 适配器 | 取数接口 | 凭证 | 说明 |
|---|---|---|---|
| `deepseek` | `/user/balance` | 推理 Key 或独立 Key | 支持多币种余额、赠送/充值明细 |
| `kimi` | `/v1/users/me/balance` | 推理 Key 或独立 Key | CNY 可用、代金券、现金余额 |
| `openrouter` | `/api/v1/credits` | Management Key | 普通推理 Key 会 403 |
| `zenmux` | `/api/v1/management/payg/balance` | Management Key | 普通推理 Key 会 403 |
| `openai_billing` | `/dashboard/billing/subscription` + `/usage` | 推理 Key 或独立 Key | 面向 OneAPI/NewAPI 等兼容中转站；官方 OpenAI billing 已废弃 |
| `zhipu` | `/api/monitor/usage/quota/limit` | 推理 Key（裸 `Authorization`） | 实验性契约，面向 GLM Coding Plan；失败只提示，不影响渠道 |
| `custom` | 自定义 GET URL | 独立 Key / 推理 Key / 匿名 | 金额 + 币种必填，可选总额/已用/重置 |

`auto`（默认）只识别 `kind` 为 `deepseek` / `kimi` / `moonshot` / `openrouter` / `zenmux` / `zhipu` 的渠道。OpenAI billing 与自定义端点需要在渠道表单中显式选择，避免对所有 OpenAI 兼容渠道误探测。

## 配置

非敏感配置保存在 `upstreams.extra.balance`。独立查询凭证保存在加密列 `upstreams.balance_key_enc`，留空时回退渠道推理 Key。

```json
{
  "adapter": "custom",
  "url": "/api/user/self",
  "header": "Authorization",
  "prefix": "Bearer ",
  "amount_path": "data.balance",
  "currency": "CNY",
  "currency_path": "data.currency",
  "total_path": "data.total",
  "used_path": "data.used",
  "reset_at_path": "data.reset_at"
}
```

自定义端点字段：

| 字段 | 必填 | 说明 |
|---|---|---|
| `adapter` | 是 | `auto` / `off` / 具体适配器 / `openai_billing` / `custom` |
| `url` | custom 必填 | 以 `/` 开头拼到渠道 origin；否则拼到 `base_url`；绝对地址仅允许公网 HTTPS |
| `header` | 否 | 鉴权 Header 名；留空表示匿名请求 |
| `prefix` | 否 | Header 值前缀，默认 `Bearer `；留空表示直接使用凭证 |
| `amount_path` | custom 必填 | 余额字段点路径，如 `data.balance`、`data.items.0.balance` |
| `currency` | 二选一 | 固定币种，如 `CNY` |
| `currency_path` | 二选一 | 币种字段点路径 |
| `total_path` / `used_path` | 否 | 总额/已用字段路径 |
| `reset_at_path` | 否 | 重置时间路径，支持 epoch 秒/毫秒或 RFC 3339 字符串 |

配置在创建/更新渠道时校验；非法配置返回 400。`adapter: "off"` 可显式关闭某个渠道的余额查询。

### 凭证

- 独立凭证通过渠道表单的「查询凭证」填写，AES-256-GCM 加密后落库，接口只返回 `has_balance_key`，不回传明文或密文。
- 留空时回退推理 Key；OpenRouter / Zenmux 必须单独填写 Management Key。
- 编辑时留空保持原值，显式「清除凭证」才会删除。
- 余额查询鉴权失败只返回错误，不写 `auth_state`、不触发熔断、不影响推理流量。

## API

```http
GET  /api/upstreams/{id}/quota
POST /api/upstreams/{id}/quota
POST /api/upstreams/{id}/quota?force=true
```

- `GET`：读取最近一次快照。
- `POST`：默认在 5 分钟缓存内复用；`force=true` 跳过缓存实时查询（管理后台「立即探测」使用该模式）。
- 失败统一返回 HTTP 200 + `ok:false` + `error`，与现有连通性测试风格一致。
- `supported` / `adapter` / `reason` 用于前端入口与诊断。

```json
{
  "ok": true,
  "probed": true,
  "cached": false,
  "status": 200,
  "latency_ms": 120,
  "adapter": "deepseek",
  "quota": {
    "kind": "balance",
    "adapter": "deepseek",
    "source": "probe",
    "captured_at": 1760000000,
    "balances": [
      {
        "currency": "CNY",
        "available": "110.00",
        "total": "110.00",
        "used": "0.00"
      }
    ],
    "details": [
      { "label": "赠送余额", "value": "10.00", "currency": "CNY" }
    ]
  }
}
```

`kind:"balance"` 是通用适配器快照结构：

- `balances[]`：币种、可用/总额/已用（十进制字符串）
- `windows[]`：窗口名称、已用/剩余百分比、重置时间（智谱等套餐额度）
- `details[]`：赠送、充值、软限额等补充明细
- `plan`：套餐等级（如智谱返回）

codex / devin 的旧快照结构保持不变；渠道列表与弹窗按 `kind` 分支渲染。

## 前端

- 渠道表单新增「余额/额度」分组：适配器、自定义端点、字段映射、查询凭证。
- 「额度」弹窗扩展为「余额/额度」，展示金额卡片、进度窗口、明细与快照时间。
- 渠道列表显示最近快照摘要：金额优先、多币种并列；无金额时显示窗口百分比；超过 24 小时灰显。

## 安全与限制

- 自定义绝对 URL 仅允许 HTTPS，探测时经 netguard 做内网/元数据拦截与 DNS 钉住；禁止重定向。
- 响应体限制 256 KB，请求超时 10 秒；凭证不进入日志或 API 响应。
- 自定义端点只支持 GET，不支持任意 Header 或请求体；复杂场景请使用兼容的查询网关。
- 智谱接口为实验性契约，可能随上游变更失效。
- 本功能不包含 Token Plan 云 API（腾讯 SecretId/SecretKey、阿里 AccessKey、百度 IAM）以及 Anthropic；这些需要单独的凭证模型，后续再评估。