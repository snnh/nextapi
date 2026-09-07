# Changelog

## [0.1.0] - 2026-09-07

首个公开发布版本。

### 网关核心
- 四协议互转：OpenAI Chat Completions / OpenAI Responses / Anthropic Messages / Gemini，透传优先（SSE 边转发边改写），上游不支持入口协议时按内部 IR 自动转换；
- 聚合路由：模型通配匹配、优先级分组 + 加权随机、重试与故障转移、熔断（自动禁用 + 冷却 + 半开探活，冷却状态重启保留）；
- 鉴权限流：网关 Key（SHA-256 哈希存储）、每 Key RPM/TPM 滑窗、token/成本配额上限（fail-closed）；
- Codex OAuth 渠道：粘贴 auth.json 创建，临期自动刷新 + 轮换回写，失败联动熔断；
- 渠道模型自动同步：协议适配拉取上游模型列表，托管路由对账；
- 图片生成统一入口：OpenAI / Gemini / Dashscope（同步 + 异步任务轮询计费闭环）。

### 日志与计价
- usage_logs 30 天声明式分区（不自动清理，手动整分区 DROP 支持 dry_run）+ usage_hourly 滚动聚合；
- 异步批量写库（有界队列 + WAL 兜底，幂等重放）；
- 双币种分段计价（CNY/USD，只统计不扣费），汇率手动/自动拉取 + 内置兜底，价格表 XML/JSON 导入导出。

### 管理后台
- 内嵌 Vue 3 控制台 9 页面（登录/仪表盘/密钥/上游/路由/价格/日志/统计/系统设置），rust-embed 单二进制同源部署，移动端抽屉侧栏；
- 供应商预设一键接入（8 个内置）；登录防爆破 + 可选 TOTP 二步验证；审计日志；
- Prometheus `/metrics`（管理员 JWT 鉴权）；系统设置热加载（UI > YAML）；GitHub 更新检查。

### 安全
- 敏感字段（上游 Key / OAuth token / 代理密码 / JWT secret）AES-256-GCM 落库，展示一律掩码；
- SSRF 防护（私网段拦截 + DNS 钉住）、请求体 100MiB 上限、受控下载流式限长。

[0.1.0]: https://github.com/snnh/nextapi/releases/tag/v0.1.0
