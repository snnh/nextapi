# Changelog

## [0.3.8] - 2026-09-24

### Fixed
- **安全/健壮性（代码评审修复）**：DevIn Connect 帧解析对恶意 `len` 头做溢出与上限防护
  （此前 `i + ln` 可回绕绕过守卫并 panic，且巨大 len 会撑爆缓冲）；单帧/缓冲双上限（16MB）。
- **额度采集覆盖限流场景**：codex 429/5xx 响应此前因「先判状态码再取头」而被丢弃，
  现改为响应头回调在状态校验前触发（`execute_stream_capture`），限流时也能拿到额度。
- Devin 流式：`created` 与 `completed` 的 `response.id` 此前可能不同（首帧无 id 时各生成一次），
  现首次生成即回写；EOF 无结束帧不再当成功（流式补 `response.failed`，非流式报错），
  避免把截断的半个响应记账为 200；非 JSON 错误体不再丢成 `": "`。
- Devin 探测/缓存：`GetUserStatus` 未返回额度块时不再落库（避免空快照覆盖上次有效额度）；
  探测结果 5 分钟内复用（省额度）；`compact` 候选排除 devin（Connect 协议无 `/responses/compact`）。
- 上游删除时清理额度节流表与 codex 刷新锁表条目（此前只增不减）。
- 预设接入的连通性探测 `latency_ms` 此前在请求前取值（恒 ~0），改为请求后取值。

### Changed
- **性能/占用优化（评审）**：`debug_payload` 改为惰性求值（debug 关闭时不再序列化整个响应体）；
  `LogSink.pending` 回收已完成句柄（此前长时间运行会无界增长）；SSE 解析改游标推进
  （原逐行 drain 在大 chunk 下退化 O(n²)）；流式记账收集文本改 `mem::take`（省一次 ≤256KB 拷贝）；
  已知事件类型时不再二次解析 JSON 取 `type`；日志列表/CSV 导出的 Key 名查找改一次建索引
  （原 O(行数 × Key 数)）；`ProxyRow` 不再持有密码密文（只用明文，减少密文常驻）。

### Removed
- **代码清理（评审）**：移除 `protocol` 模块级 `#![allow(dead_code)]/#![allow(unused_imports)]`
  全局抑制（M2 遗留）；删除无引用的 `codex::AUTHORIZE_URL`、`StreamState::stop_seen`（只写不读）、
  `ProxyRow.password_enc`、前端 `fmtDate`/`WEEKDAY_LABEL`；`ir::ToolCallAggregator` 降级为
  `#[cfg(test)]`（仅单测聚合校验用）。重复实现合一：新增 `text.rs`（4 份截断实现归一）、
  `upstream::probe_ok_json/probe_err_json`（4 处探测返回体）、`error::error_body`（错误体手工拼装）、
  `limit::window_allow`（auth 内另一份滑窗）、`execute_stream_capture`（codex 抓头与普通路径共用）。
  文档纠正：`docs/protocol-matrix.md` 去掉并不存在的 `passthrough_fallback`（转换失败不回退透传）、
  `logging` 的 `convert_mode` 注释与实际取值对齐、`AGENTS.md` 目录树补全（text.rs / auth/totp.rs /
  logging/headers.rs / media/{resolve,download}.rs）并标注 tests/ 目录当前不存在。

### Added
- **Devin OAuth 渠道（kind='devin'）**：以 devin CLI 形式接入上游 Devin Connect 协议
  （`GetChatMessage`，protobuf over HTTP + 5 字节信封帧），对外转发为标准接口
  （OpenAI Responses 优先；其余三协议经 IR 互通）。协议字段号全部一手逆向 + 真实流量实证
  （含官方 CLI exec 工具循环双向捕获），端到端实测：流式/非流式/工具调用/工具循环多轮/
  Chat 入口转换均通过。
  - 凭证为 devin CLI 的 session token（`devin-session-token$…`，存 api_key 加密 + 掩码）；
    管理后台内置 **CLI PKCE 无头授权**（`POST /api/upstreams/devin/pkce/start` → 浏览器授权 →
    `…/pkce/exchange` 兑换 token 自动填表）。token 过期本期不自动刷新（过期报错 + 提示）。
  - 转换：Responses `instructions/input/tools` → CLI 形态请求（Metadata 指纹 / 头提示词 /
    消息三形态：user·assistant(f6 工具调用+f11 文本)·tool 结果(f3+f7) / 采样配置 / 工具定义 /
    轨迹与会话代号）；响应帧 → 标准 Responses SSE 事件或非流式 JSON。
  - usage 口径实测映射：Devin 未缓存输入/缓存命中/输出 → 标准 `input_tokens(含缓存)/
    cached_tokens/output_tokens`（计价/计量口径兼容既有 P0 归一）。
  - `POST /api/upstreams/{id}/test`：GetUserStatus 真实探测（账号/邮箱/套餐/可用模型数）；
    模型同步走模型目录并按**计划门控**过滤（Free 计划仅个别模型可用，锁定模型调用返回
    `failed_precondition/permission_denied` 并附套餐提示）。
- **上游额度探测（Codex / Devin）**：管理端新增 `GET/POST /api/upstreams/{id}/quota` +
  上游列表「额度」弹窗，展示各渠道额度窗口与重置时间。
  - **Codex**：ChatGPT 订阅额度（官方 CLI `/status` 同源）——读取响应头
    `x-codex-plan-type` / `x-codex-active-limit` / `x-codex-{primary,secondary}-used-percent` /
    `-window-minutes` / `-reset-at` / `-reset-after-seconds` / `-primary-over-secondary-limit-percent` /
    `x-codex-credits-{has-credits,balance,unlimited}`。**真实流量旁路自动抓头**
    （每次请求后台落库，内容未变且 60s 内不重复写），也可点「立即探测」发一次极小请求即时取
    （5 分钟内已有快照则直接复用，不消耗额度）；429 等错误响应的额度头同样被采集。
  - **Devin**：`GetUserStatus` plan 块的额度字段（实测）：日额度 `f9` / 周额度 `f8` /
    百分数对 `f14`-`f15` / 每日重置 `f17` / 每周重置 `f18`（08:00 UTC ≈ 太平洋日历日零点），
    附账号、套餐与可用模型数；探测不消耗额度。
  - 快照存 `upstreams.extra.quota`（含 `source=live|probe` 与采集时间），前端按渠道渲染窗口、
    百分比、重置时间与积分状态。
- **Devin 预设一键接入**：供应商预设新增 `devin`（第 13 个，`auth_kind='oauth'`）——
  免 API Key 接入 Devin CLI OAuth 渠道：基础地址/协议（Responses 优先）预填，
  `POST /api/presets/{name}/provision` 对该预设允许空凭证（先建渠道、凭证由授权流程后置写入，
  存储 NULL），连通性探测复用 GetUserStatus（未授权返回「尚未完成 Devin 授权」提示而非报错）。
  接入向导识别 OAuth 预设：以「Devin 授权」（PKCE 开始授权 → 粘贴授权码 → 兑换凭证）替代 API Key
  输入框；未授权的渠道跳过模型发现并提示先授权。
- **管理端登录有效期可配置**：新增热参数 `gateway.admin_login_expire_minutes`
  （分钟，1–43200，默认 1440 = 24 小时，原固定 1 小时），WebUI「设置 → 运行参数 →
  登录有效期」可改、即时生效（仅对新签发 token 生效）；登录响应新增 `expires_in`（秒）。
  启用 TOTP 时无需频繁重新验证。

## [0.3.7] - 2026-09-21

### Added
- **阿里云百炼接入域名适配（共享域名 → 统一域名 / 业务空间专属域名）**：百炼共享域名
  （`dashscope.aliyuncs.com`）自 2026-09-30 起不再迭代新特性。
  - 预设 `dashscope` 新增两个接入域名选项：**千问AI平台统一域名**
    `https://maas.qianwenaiapi.com/compatible-mode/v1`（推荐，无需 Workspace ID，百炼 API Key
    通用，Anthropic 根 `/apps/anthropic/v1`、图像原生根同域名）与**业务空间专属域名**
    `{WorkspaceId}.{region}.maas.aliyuncs.com`（6 个地域：北京/新加坡/中国香港/东京/法兰克福/
    弗吉尼亚；媒体地址与 Anthropic 根按模板派生）。
  - `POST /api/presets/{name}/provision` 新增 `domain` 参数（`shared` 缺省 / `unified` /
    `workspace`）+ `workspace_id` / `region`：后端按预设声明的域名选项生成 base_url /
    媒体地址 / 分协议地址并校验（Workspace ID 仅字母数字连字符、首尾非连字符；region 必须
    在预设支持列表内；选项不匹配 → 400）。接入向导新增「接入域名」选择与实时 URL 预览。
  - 上游编辑对话框新增「百炼域名」迁移助手：对仍在使用共享域名的阿里云百炼上游，可一键
    改用统一域名，或填入 Workspace ID + 地域生成专属域名（含 Base URL / 媒体地址 /
    Anthropic 分协议地址；提交前仍可手动调整）。
  - `dashscope` 预设补充 `openai_responses` 与 `anthropic` 协议：文本三协议（Chat/Responses
    同根 + Anthropic 独立根 `https://dashscope.aliyuncs.com/apps/anthropic/v1`）。
- `/api/presets` 输出 `workspace_domain` 与 `unified_domain`（模板 / 地域 / 提示），供前端渲染
  域名选择与 URL 预览。

### 迁移指引（存量上游）
1. 上游列表 → 编辑阿里云百炼上游；
2. 「百炼域名」处二选一：
   - **改用统一域名**：一键把 Base URL / 媒体地址 / Anthropic 分协议地址切到
     `maas.qianwenaiapi.com`（无需 Workspace ID）；
   - **业务空间专属域名**：填入 Workspace ID（控制台「业务空间详情」或 API Key 弹窗的
     API Host，形如 `llm-xxxxxx`）与地域后点「填入专属域名」；
3. 如需 Anthropic 协议调用，在「协议」中勾选 `anthropic`；
4. 保存后确认连通性（专属域名仅接受同业务空间所属 API Key；统一域名按平台要求使用对应 Key）。

> 说明：API Key 与地域、业务空间绑定；统一域名（maas.qianwenaiapi.com）与共享域名同一套百炼
> API Key，专属域名仅接受同业务空间所属 API Key。两种域名均不改变调用方式；`dashscope_tokenplan`
> （Token Plan）与其它预设地址不受本次公告影响。

## [0.3.6] - 2026-09-17

### Fixed
- **移动端列表页固定操作列挤掉内容列**：上游列表等页面把操作列设为 `fixed="right"` 且宽度
  较大（上游 330px / Key 260px / 代理 200px），手机上固定列独占视口，把名称列压成 0 宽
  （表现为「接入商名称被截断/看不到」）。现统一按 `≤768px` 窄屏断点处理：隐藏次要列
  （类型/Base URL/协议/凭证/代理/请求头/时间等）、把状态等关键信息折到主行下方、
  操作列取消固定并收成「操作 ▾」下拉菜单，桌面端布局不变。覆盖上游、Key、模型路由、
  计价、模型别名、代理设置与日志页（日志页额外收起 request_id 列）。

## [0.3.5] - 2026-09-14

### Added
- **Codex 渠道 WebSocket 传输（预设 auto：WS 优先，失败回落 HTTP）**：对齐官方 CLI 的
  Responses-over-WebSocket 链路——`{ws|wss}://…/responses` 握手（`openai-beta:
  responses_websockets=2026-02-06` 类 beta 头 + 鉴权 + 指纹头）、单帧
  `{"type":"response.create", …}`；服务端事件帧重编码为 SSE 复用既有透传/转换/打点链路，
  非流式入口读尽事件取 `response.completed`；握手/首个事件前失败计入可重试/故障转移，
  auto 模式回落 HTTP。上游 `extra.codex_transport` = `auto`（默认）/`ws`/`http`；
  代理下的上游自动只用 HTTP（v1 限制）。
- **Codex 官方客户端指纹模拟（预设开启）**：出站请求补齐官方 codex-tui 形态的请求头
  （`User-Agent`/`originator`/`session-id`/`thread-id`/`x-client-request-id`/
  `x-codex-window-id`/`x-codex-turn-metadata`）与请求体 Codex 专属字段
  （`client_metadata`/`include`/`prompt_cache_key`/`reasoning`/`parallel_tool_calls`/
  `tool_choice`）；入站真实 Codex CLI 的既有值、用户 overrides 一律优先（不覆盖）。
  会话/安装标识优先复用入站 `client_metadata`，否则按 (Key, 入口模型) 确定性派生
  （UUIDv7 形态，保证粘性路由与提示缓存亲和）。上游 `extra.codex_fingerprint` 可传
  `false` 关闭或对象覆盖 `user_agent`/`originator`/`installation_id`/`simulate_body`。
- 上游表单（codex 渠道）新增「传输方式」与「指纹模拟 / User-Agent」配置项。
- **Responses 服务端 compact 直接转发**：新增 `POST /v1/responses/compact`（兼收单数
  拼写 `/v1/response/compact`）——Codex CLI 的服务端上下文压缩端点。不进 IR 转换层，
  body 原样转发（仅 override_model 模型名改写 + 用户 overrides）；候选限 codex 渠道或
  声明支持 openai_responses 的上游；codex 渠道仅 HTTP 传输（compact 无 WS 形态），
  指纹头照旧注入、body 只补缺失的 client_metadata（不强制 stream/store）；SSE/JSON
  按客户端 stream 标志透传并照常 usage 记账/计价；上游 404（端点不支持）故障转移到
  下一候选，其余 4xx 原样回错由客户端自行回落本地压缩。日志 `protocol_in` 独立记为
  `openai_responses_compact`。

## [0.3.4] - 2026-09-13

### Changed
- **XML 价格表导出改为人类易读格式**：此前全部内容挤在单行，人工查看/比对困难；现按
  两空格缩进逐元素换行（`<tag>文本</tag>` 叶子节点保持单行），导入解析忽略元素间空白，
  导出文件可原样回传（既有导出→导入往返测试覆盖）。

### Fixed
- **价格导入「本地文件导入」误报「参数错误: 请输入 url」**：axios 实例全局设了
  `Content-Type: application/json`，axios v1 见到该头会把 FormData 请求体交给
  `formDataToJSON` 序列化，本地文件因此被发成 JSON `{"file":{}}`（File 序列化为 `{}`），
  后端按 JSON 分支解析后报「缺少 url」。现移除全局 Content-Type（对象请求体由 axios
  自动带 application/json），并在文件导入请求上显式置空 Content-Type，让浏览器补
  `multipart/form-data` + boundary；后端该错误文案同步回显 Content-Type 便于定位。

## [0.3.3] - 2026-09-12

### Fixed
- **override_model 改写的模型未计价（计价跟随上游实际模型名）**：路由配置 `override_model`
  后（如入口名 `glm-5.3-qf` → 上游名 `glm-5.3` 走千帆套餐），计价仍按**入口名**查价格，
  而价格为「供应商 + 上游模型 ID」绑定，导致一律未定价（cost 恒 NULL）；同时「未定价模型」
  列表按改写后名字判断，与计价口径不一致，问题长期不可见。现计价统一按上游实际模型名
  （`usage_logs.upstream_model`，迁移 0016 新增，NULL=与入口同名；异步媒体任务
  `media_tasks.upstream_model` 同步修正），日志列表/详情/CSV 增加「入口名 → 上游名」展示。
  历史数据不回填。

## [0.3.2] - 2026-09-11

### Added
- **百度千帆 Token Plan 预设**：新增 `qianfan_tokenplan` 一键接入预设——OpenAI 兼容
  Chat/Responses（`https://qianfan.baidubce.com/v2/tokenplan/personal`）与 Anthropic 兼容
  （`https://qianfan.baidubce.com/anthropic/tokenplan/personal/v1`）三协议；模型列表在
  `/v1/models`（该服务不提供 `/models`）。上游新增 `extra.models_path` 通用覆盖（默认
  `/models`），连通性测试与模型拉取随之按覆盖路径请求。
- **腾讯云 / 阿里云 Token Plan 预设**：新增 `tencent_tokenplan`（腾讯云 Token Plan 个人版，
  OpenAI 兼容 `/plan/v3` + Anthropic 兼容 `/plan/anthropic`）与 `dashscope_tokenplan`
  （阿里云百炼 Token Plan，OpenAI 兼容 `/compatible-mode/v1` + Anthropic 兼容
  `/apps/anthropic`，仅华北2地域）一键接入预设。

### Fixed
- **腾讯混元 TokenHub 修正**：`hunyuan` 预设 base_url 由旧混元地址
  `https://api.hunyuan.cloud.tencent.com/v1` 更正为 TokenHub 官方地址
  `https://tokenhub.tencentmaas.com/v1`，并补充 Responses / Anthropic 兼容协议支持。
- **浏览器自动填充静默覆盖上游 API Key**：上游 API Key / 一键接入 / 代理密码 /
  设置项密码等 secret 输入框未声明防自动填充，浏览器将其误判为登录密码框并
  自动填充管理后台登录密码，导致保存其它字段时 Key 被静默替换（表现为"刚填好
  的 Key 又变错"）。现全部加 `autocomplete="new-password"` 与主流密码管理器忽略
  标记；`upstream.update` 审计新增 `api_key_changed` 字段，Key 变更可追溯。

## [0.3.1] - 2026-09-10

### ⚠️ Fixed（紧急）
- **管理后台全站 API 404（v0.3.0 回归）**：`apiBase()` 误返回 `/api`，与 `api/index.ts` 中
  已含 `/api` 前缀的请求路径被 axios 拼接为 `/api/api/...`，导致登录、列表、保存等全部前端
  请求 404「资源不存在」（网关 `/v1/*` 不受影响）。现改为仅返回部署前缀（根路径 `/`，
  子路径 `/nextapi/`），并新增构建前自检 `web/scripts/check-api-paths.mjs` 防同类回归。
- **静态资源缓存策略**：`index.html` 加 `Cache-Control: no-cache`（升级后立即获取新首页，
  避免旧首页引用已删除的旧哈希资源）；`assets/*` 哈希资源加一年 `immutable` 强缓存；
  请求不存在的哈希资源返回 404 而非回退 HTML。

## [0.3.0] - 2026-09-10

### ⚠️ Breaking
- **Token 口径归一（计价正确性修复）**：`usage_logs.prompt_tokens` 与日志/统计中的
  「输入 token」统一为**未命中缓存的输入 token**（此前 OpenAI Chat / Responses / Codex、
  Gemini 上游为「含缓存」的全量输入，Anthropic 为「不含缓存」，口径不一致导致缓存
  token 被重复计价、并重复计入 `quota_usage` 配额）。`token_in` 计价与配额累计现与
  Anthropic 对齐；上游原始数值仍完整保留在 `usage_raw`。**历史数据不回填**，仅新记录
  按新口径；如需对比历史请以 `usage_raw` 为准。

### Added
- **模型工作台**：「模型」页新增模型视图，按对外模型名聚合展示上游、优先级、路由
  启用数、价格配置状态、别名与健康度，并提供模型详情抽屉（基本信息 / 路由上游 /
  别名 / 价格 / 调用示例）；
- **上游接入向导**：「上游供应商」页主操作改为三步向导（选择供应商 → 填写凭证 →
  连通与模型发现），接入后可直接自动同步或一键为新模型创建路由；
- **API Key 调用向导**：创建/轮换成功后展示网关地址、curl 示例与「去配置模型」入口；
- **日志清理 dry-run 汇总**：预览返回实际生效截止时间（按分区边界对齐）、待删分区、
  明细行数、Token 与成本合计，并明确不影响 `quota_usage` 配额计数器；
- **子路径部署支持**：前端改用运行时 `<base href>`（后端按 `X-Forwarded-Prefix` 注入）
  与相对资源路径，同一镜像可挂在 `/nextapi/` 等任意子路径而无需重新构建；Vue Router
  与 API 前缀随之推导，401 跳转保留部署前缀；
- **反向代理诊断**：新增 `GET /api/system/diagnostics` 与「系统设置 → 反向代理诊断」面板，
  展示 Host/scheme/部署前缀、客户端 IP 与来源、可信代理判定、转发头原文与配置提示；
- **顶栏系统状态**：主界面右上角新增状态徽标（后端可用 / 数据库就绪 / 异常上游），
  点击展开版本、运行时长、日志队列、WAL 兜底、媒体待回联与最近错误，60s 自动刷新
  （页面隐藏时跳过）；
- **列表失败态统一**：密钥、上游、模型路由、别名、价格规则在加载失败时展示失败信息与
  「重新加载」按钮，不再只弹一次性提示或显示空表格；
- **上游 API Key 安全查看**：上游列表新增「查看 Key」操作——点击后需管理员密码二次
  验证（已启用 TOTP 时还需动态码，与登录同款防重放/限速语义）通过才解密返回明文，
  明文仅弹窗内本次展示、每次查看独立验证并写审计（`upstream.key_reveal`）；
  未设置 API Key 的渠道（如 Codex OAuth）不可查看。

### Changed
- **日志详情按排障顺序分组**：请求结果 / 路由信息 / 用量与成本 / 原始记录；列表状态
  列可 hover 查看错误摘要并标记降级，窄屏自动隐藏 Key、协议链、流式、Tokens 等次要列；
- **日志列表提示**：显示最近更新时间与异步落库说明，并标注 Tokens 列口径；
- **日志游标浏览**：新增 `(ts,id)` keyset 分页（API `cursor` / `next_cursor`，配套
  `usage_logs(ts DESC, id DESC)` 索引），日志页可切换「游标浏览」模式按「加载更多」
  连续翻页，深页不再受 OFFSET 退化影响（该模式不统计总数）；
- **危险操作统一确认**：删除 Key / 上游 / 代理 / 路由规则 / 别名 / 价格规则、删除已有
  定价规则、正式导入价格、整包覆盖配置与日志分区清理统一为「标题=动作名、正文=影响范围
  与不可恢复说明、确认按钮=动作名（红色）」，并禁用点击遮罩关闭，降低误触风险；
- **清理边界归一**：`usage_hourly` 改为按被删分区的最大 `to_ts` 清理（此前按 `before`
  精确时间，可能与明细口径背离；无可删分区时不再清理聚合表）。

### Fixed
- **容器健康检查适配实际端口**：Dockerfile 与 `docker-compose.yml` 的 healthcheck 原先固定
  探测 `127.0.0.1:3220`，当 `config.yaml` 显式写 `0.0.0.0:8080`（旧默认）时容器会被持续标记
  unhealthy（可能触发编排层反复重启）。现改为「3220 → 8080」依次探测；镜像内置 HEALTHCHECK
  还会优先读取 `NEXTAPI_LISTEN` 的端口。

## [0.2.1] - 2026-09-07

### Fixed
- **协议转换（Responses 入口）容错**：省略 `type:"message"` 的输入项、纯字符串
  content、客户端回显 assistant 历史的 `output_text`/`refusal` part 不再被整条丢弃
  ——此前这些客户端常见形态会被全部丢掉，产出空 messages 被上游以
  `messages cannot be empty` 拒绝（表现为直通正常、仅转换失败）；
- **网关 fail-fast**：协议转换后消息列表为空时直接由网关返回 400（附转换降级线索），
  不再把必然失败的请求发往上游；请求解析类转换错误归 400 且不做无意义故障转移；
- **失败日志可定位**：失败终点日志回填实际尝试的 `upstream_id` 与请求侧降级标记
  （此前恒 NULL/false）；
- **调试模式覆盖失败路径**：转换错误 / 上游拒绝（含 4xx）现在也记录 debug_payload，
  内容为入口请求 + 实际发往上游的转换后请求体 + 上游错误原文（此前失败日志无任何
  调试载荷，开启调试也抓不到）。

### CI
- CI 与 Release 工作流合并为单文件：分支 push 不再触发；PR 保留检查；**打 v\* 标签
  push 即触发发布构建**（GitHub Release 用 gh 另行创建，不触发构建）；
- 多架构镜像改原生 ARM runner 分布式构建（amd64/arm64 各自原生编译，去除 QEMU
  模拟），发布耗时约 49 分钟 → 11 分钟。

## [0.2.0] - 2026-09-07

### ⚠️ Breaking
- 默认监听端口 8080 → **3220**（`server.listen` 默认值、Dockerfile EXPOSE/healthcheck、
  compose 映射、vite 开发代理、文档同步更新）。**Docker 用户升级时须把端口映射改为
  `3220:3220`**；如需保持旧端口，设 `NEXTAPI_LISTEN=0.0.0.0:8080` 或在 config.yaml
  显式写 `server.listen`（启动类参数 YAML 优先于代码默认值，显式配置者不受影响）。

### Added
- 定价页改版：默认按「供应商+模型」分组显示；新增「按模型定价」对话框，一屏配置
  同一模型的输入/输出/缓存写/缓存读四种价格（各自可含分段），提交自动 diff
  创建/更新/删除；参考定价一键填充；
- Codex 渠道模型拉取与连通测试适配 OAuth（ensure_token 临期刷新 + 目录解析，
  过滤不可见/不支持 API 的模型）；
- **渠道编辑内支持手动输入模型 ID 生成路由**：上游无模型列表接口或拉取失败时可直接
  输入（逗号/换行批量），复用既有手动路由接口，无需等待上游目录；
- 渠道编辑弹窗重构：模型列表支持搜索/状态筛选/统计/一键复制，移动端 label 顶对齐 +
  长表单内部滚动，脏表单关闭前确认，敏感字段关闭兜底清理。

### Changed（管理后台易用性全面优化）
- 高风险防护：完整密钥展示态禁止 Esc/X 误关；「按模型定价」取消勾选删除规则前确认；
  价格导入按钮文案随 dry-run 开关切换且正式导入二次确认；路由「保存全部」展示
  新增/修改/删除 diff 摘要；分段价格空值不再静默落 0 元（提交前校验 + 死段/不完整
  时间窗校验）；整包导入 YAML 覆盖前确认；
- 表单与反馈：密钥表单迁移内联校验（配额单位/窗口联动）；全站弹窗统一 Enter 提交与
  脏表单防丢；删除类按钮全部防重入；复制操作统一降级（http/LAN 场景可用
  execCommand）；request_id/模型名/别名/审计对象等高频字段一键复制；
- 列表与筛选：日志筛选同步 URL（可分享/刷新保持）、默认时间范围可见化、详情抽屉
  移动端全屏；统计页补 Key/上游筛选 + 图表数值格式化 + 查询防抖；密钥/上游/路由/
  别名/价格/代理列表支持搜索与分页；
- 仪表盘与设置：加载失败改为页面内错误态 + 重试（不再伪装「无数据」）；仪表盘手动
  刷新 + 更新时间；运行参数面板支持搜索、分组折叠、吸底保存栏、逐字段范围校验与
  恢复默认、未保存提醒；设置页 Tab 深链同步 URL；
- 移动端与可访问性：全部弹窗响应式宽度（小屏 94vw 兜底）；详情 descriptions 移动端
  单列；纯图标按钮补齐 tooltip 与 aria-label；图表容器 aria 标注。

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

[0.2.1]: https://github.com/snnh/nextapi/releases/tag/v0.2.1
[0.2.0]: https://github.com/snnh/nextapi/releases/tag/v0.2.0
[0.1.0]: https://github.com/snnh/nextapi/releases/tag/v0.1.0
