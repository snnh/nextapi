// ============================================================================
// NextAPI 管理后端 API 类型（唯一事实源，来自后端 serde 结构实测，M8 契约冻结）
// 约定：
// - 字段一律 snake_case（wire 名），禁止臆测改名；
// - Decimal（价格/成本/汇率）序列化为 JSON 字符串，如 "3.0"；
// - 时间均为 RFC3339/ISO8601 UTC 字符串，如 "2024-01-01T00:00:00Z"；
// - 掩码语义：更新类接口未传或 "***" = 保持原值，"" = 清除。
// ============================================================================

// ---------------------------------------------------------------------------
// 通用
// ---------------------------------------------------------------------------

/** 统一错误响应体 */
export interface ApiErrorBody {
  error: { message: string; type: string; code?: string | null }
}

/** 服务端分页响应（logs/audit） */
export interface Paged<T> {
  items: T[]
  total: number
  page: number
  page_size: number
}

// ---------------------------------------------------------------------------
// auth（/api/auth）
// ---------------------------------------------------------------------------

export interface LoginReq {
  username: string
  password: string
  /** 已启用 TOTP 时必填（6 位数字） */
  totp_code?: string
}

export interface LoginResp {
  token: string
  username: string
}

export interface MeResp {
  username: string
}

/** TOTP 状态 */
export interface TotpStatusResp {
  enabled: boolean
  /** 已生成机密但未确认启用 */
  pending: boolean
}

/** TOTP setup 返回（机密仅本次展示） */
export interface TotpSetupResp {
  secret: string
  otpauth_url: string
}

export interface ChangePasswordReq {
  old_password: string
  new_password: string
}

// ---------------------------------------------------------------------------
// keys（/api/keys）
// ---------------------------------------------------------------------------

export type QuotaUnit = 'tokens' | 'cost_cny' | 'cost_usd'
export type QuotaWindow = 'daily' | 'monthly' | 'total'

export interface ApiKeyRow {
  id: string
  name: string
  /** sk-nx- + 若干位，仅前缀 */
  prefix: string
  enabled: boolean
  /** 模型白名单；null/空 = 全部；支持 * 通配 */
  models: string[] | null
  rpm: number | null
  tpm: number | null
  quota_limit: number | null
  quota_unit: QuotaUnit | null
  quota_window: QuotaWindow | null
  allow_upstream_passthrough: boolean
  passthrough_upstreams: string[] | null
  debug_enabled: boolean
  debug_expires_at: string | null
  expires_at: string | null
  created_at: string
  last_used_at: string | null
}

export interface KeyCreateReq {
  name?: string | null
  models?: string[] | null
  rpm?: number | null
  tpm?: number | null
  quota_limit?: number | null
  quota_unit?: QuotaUnit | null
  quota_window?: QuotaWindow | null
  allow_upstream_passthrough?: boolean | null
  passthrough_upstreams?: string[] | null
  debug_enabled?: boolean | null
  debug_expires_at?: string | null
  expires_at?: string | null
}

/** 创建/轮换响应：完整 key 仅此一次返回 */
export interface KeySecretResp {
  key: string
  row: ApiKeyRow
}

export interface KeyUpdateResp {
  row: ApiKeyRow
}

export interface OkResp {
  ok: boolean
}

// ---------------------------------------------------------------------------
// upstreams（/api/upstreams）
// ---------------------------------------------------------------------------

/** 协议取值：4 标准 + 4 图片 */
export type ProtocolName =
  | 'openai_chat'
  | 'openai_responses'
  | 'anthropic'
  | 'gemini'
  | 'images_openai'
  | 'images_gemini'
  | 'images_dashscope_sync'
  | 'images_dashscope_async'

/** 嵌套覆盖结构：请求头/请求体 add/set，value 为任意 JSON */
export interface Overrides {
  headers?: { add?: Record<string, unknown>; set?: Record<string, unknown> }
  body?: { add?: Record<string, unknown>; set?: Record<string, unknown> }
}

export interface UpstreamOut {
  id: string
  name: string
  kind: string
  base_url: string
  /** 代替 api_key 明文/密文：是否已设置 */
  has_api_key: boolean
  protocols: ProtocolName[]
  enabled: boolean
  timeout_ms: number
  breaker_threshold: number
  probe_model: string | null
  consecutive_failures: number
  disabled_by: string | null
  cooldown_until: string | null
  use_proxy: boolean
  proxy_id: string | null
  /** 可含 protocol_priority / overrides / media_base_url */
  extra: UpstreamExtra
  /** 模型同步策略：manual=仅手动 / auto=跟随上游自动更新托管路由 */
  model_sync: 'manual' | 'auto'
  /** 自动同步排除名单 */
  model_exclude: string[]
  /** 最近一次成功拉取的模型列表缓存 */
  models_cache: string[]
  models_fetched_at: string | null
  /** Codex OAuth：仅状态，不含 token */
  has_oauth: boolean
  oauth_account_id: string | null
  oauth_expires_at: string | null
  created_at: string
  updated_at: string
}

export interface UpstreamExtra {
  /** 分协议 base_url 覆盖（gateway 调用时按协议选用） */
  protocol_base_urls?: Record<string, string> | null
  protocol_priority?: ProtocolName[]
  overrides?: Overrides
  media_base_url?: string | null
  /** 能力矩阵（M10.3）：三态——缺省=假设支持，false=明确不支持（路由过滤） */
  capabilities?: {
    stream?: boolean | null
    tools?: boolean | null
    vision?: boolean | null
    max_context?: number | null
  }
}

export interface UpstreamIn {
  name: string
  kind?: string | null
  base_url: string
  /** 创建：非空即加密；更新：未传/"***"=保持，""=清除，其他=新明文 */
  api_key?: string | null
  protocols?: ProtocolName[] | null
  enabled?: boolean | null
  timeout_ms?: number | null
  breaker_threshold?: number | null
  probe_model?: string | null
  use_proxy?: boolean | null
  proxy_id?: string | null
  extra?: UpstreamExtra | null
  model_sync?: 'manual' | 'auto' | null
  model_exclude?: string[] | null
  /** Codex 渠道：粘贴 auth.json 原文；未传/"***"=保持，""=清除 */
  auth_json?: string | null
}

export interface TestResp {
  ok: boolean
  status?: number
  latency_ms: number
  error?: string
}

/** 查看上游明文 API Key（管理员二次验证后，仅本次展示） */
export interface UpstreamRevealReq {
  password: string
  /** 已启用 TOTP 时必填（6 位数字） */
  totp_code?: string
}

export interface UpstreamRevealResp {
  api_key: string
}

/** 渠道模型列表项（实时拉取 + 路由状态标注） */
export interface UpstreamModelItem {
  name: string
  /** new=未路由 / routed_manual=本渠道手动路由 / routed_auto=本渠道托管路由 /
   *  routed_other=已被其他渠道占用 / excluded=排除名单内 */
  status: 'new' | 'routed_manual' | 'routed_auto' | 'routed_other' | 'excluded'
}

export interface UpstreamModelsResp {
  items: UpstreamModelItem[]
  fetched_at: string
  model_sync: 'manual' | 'auto'
  model_exclude: string[]
}

/** 模型同步/手动添加报告 */
export interface ModelSyncReport {
  fetched: number
  desired: number
  excluded: number
  added: string[]
  removed: string[]
  skipped: string[]
  kept: number
}

// ---------------------------------------------------------------------------
// presets（/api/presets）
// ---------------------------------------------------------------------------

export interface Preset {
  name: string
  display_name: string
  kind: string
  base_url: string
  protocols: ProtocolName[]
  media_base_url: string | null
  /** 分协议 base_url 覆盖（多根供应商，如 DeepSeek 的 Anthropic 根） */
  protocol_base_urls?: Record<string, string>
  description: string
}

export interface ProvisionReq {
  api_key: string
  name?: string | null
}

export interface ProvisionResp {
  upstream: Record<string, unknown> & { id: string; name: string }
  test: TestResp
}

// ---------------------------------------------------------------------------
// model-routes（/api/model-routes，PUT = 裸数组全量替换）
// ---------------------------------------------------------------------------

export interface RouteItem {
  model_pattern: string
  upstream_id: string
  override_model?: string | null
  priority?: number | null
  weight?: number | null
  enabled?: boolean | null
  retries?: number | null
  retry_status_codes?: number[] | null
  lock_upstream?: boolean | null
  sort_order?: number | null
  /** 透传托管标记（'auto'）；新建手动路由无需传 */
  managed_by?: string | null
}

export interface RouteOut {
  id: string
  model_pattern: string
  upstream_id: string
  upstream_name: string
  override_model: string | null
  priority: number
  weight: number
  enabled: boolean
  retries: number
  retry_status_codes: number[]
  lock_upstream: boolean
  sort_order: number
  /** NULL=手动；'auto'=渠道模型同步托管 */
  managed_by: string | null
  created_at: string
  updated_at: string
}

// ---------------------------------------------------------------------------
// model aliases（/api/aliases，M10.1）
// ---------------------------------------------------------------------------

export interface ModelAliasRow {
  id: string
  alias: string
  model: string
  enabled: boolean
  created_at: string
  updated_at: string
}

export interface AliasIn {
  alias: string
  model: string
  enabled?: boolean
}

// ---------------------------------------------------------------------------
// proxies（/api/proxies）
// ---------------------------------------------------------------------------

export interface ProxyOut {
  id: string
  name: string
  /** http | https | socks5 */
  kind: string
  host: string
  port: number
  username: string | null
  /** 代替明文密码：是否已设置 */
  has_password: boolean
  no_proxy: string[]
  enabled: boolean
  created_at: string
  updated_at: string
}

export interface ProxyIn {
  name: string
  kind?: string
  host: string
  port?: number
  username?: string | null
  /** 更新语义同 api_key：未传/"***"=保持，""=清除 */
  password?: string | null
  no_proxy?: string[] | null
  enabled?: boolean | null
}

export interface ProxyTestResp {
  ok: boolean
  status?: number
  latency_ms?: number
  error?: string
  note?: string
  probe_url?: string
}

// ---------------------------------------------------------------------------
// settings / config（/api/settings、/api/config）
// ---------------------------------------------------------------------------

export interface SettingItem {
  /** 点分键，如 gateway.display_currency */
  key: string
  /** 生效值（已按 file/ui/env 优先级合并；secret 项为 "***"） */
  value: string | number | boolean | string[] | null
  /** file | ui | env */
  source: 'file' | 'ui' | 'env'
  restart_required: boolean
  secret: boolean
}

export interface SettingsView {
  settings: SettingItem[]
}

/** PUT /api/settings 请求：键 → 值；值 "***" = 保持原值跳过 */
export type SettingsPutReq = { settings: Record<string, string | number | boolean | string[] | null> }

/** GET /api/config 返回的整文件掩码视图（宽松类型；深层字段见各业务段） */
export interface ConfigFileView {
  server?: Record<string, unknown>
  database?: Record<string, unknown>
  gateway?: Record<string, unknown>
  admin?: Record<string, unknown>
  upstreams?: unknown[]
  model_routes?: unknown[]
  price_rules?: unknown[]
  fx_rates?: unknown[]
  fx_auto_fetch?: Record<string, unknown>
  price_import?: Record<string, unknown>
  media_download?: Record<string, unknown>
  media_poller?: Record<string, unknown>
  proxy?: Record<string, unknown>
  proxies?: unknown[]
  update_check?: Record<string, unknown>
  [k: string]: unknown
}

/** PUT /api/config：可直接传完整 ConfigFile 对象；掩码字段 "***"=保持原值 */
export type ConfigPutReq = ConfigFileView | { config: ConfigFileView }

// ---------------------------------------------------------------------------
// logs（/api/logs）
// ---------------------------------------------------------------------------

export interface LogQuery {
  from_ts?: string
  to_ts?: string
  key_id?: string
  upstream_id?: string
  model?: string
  stream?: boolean
  /** HTTP 状态码精确过滤 */
  status?: number
  request_id?: string
  degraded?: boolean
  page?: number
  page_size?: number
}

export interface UsageLogRow {
  id: number
  request_id: string
  ts: string
  key_id: string | null
  model: string
  /** 客户端原始入口模型（经别名解析时与 model 不同，未走别名为 null） */
  requested_model: string | null
  upstream_id: string | null
  protocol_in: string
  protocol_out: string
  /** 转换链描述（passthrough/convert 等） */
  convert_mode: string
  stream: boolean
  prompt_tokens: number | null
  completion_tokens: number | null
  cache_write_tokens: number | null
  cache_read_tokens: number | null
  images: number | null
  image_size: string | null
  video_seconds: string | null
  video_resolution: string | null
  video_task_type: string | null
  latency_ms: number | null
  /** HTTP 状态码 */
  status: number
  error: string | null
  retry_count: number
  ttfb_ms: number | null
  degraded: boolean
  pricing_source: string | null
  cost_cny: string | null
  cost_usd: string | null
  /** price_used / fx_snapshot / usage_raw / debug_payload：JSON 对象或数组 */
  price_used: unknown
  fx_snapshot: unknown
  usage_raw: unknown
  debug_payload: unknown
  /** 白名单请求头摘要（user-agent 等调试头，键为小写；绝不含 Authorization/Cookie 等敏感头；旧数据为 null） */
  request_headers: Record<string, string> | null
}

export interface LogItem extends UsageLogRow {
  key_name: string | null
  key_prefix: string | null
  upstream_name: string | null
}

export interface PartitionInfo {
  name: string
  from_ts: string
  to_ts: string
  size_bytes: number
  row_estimate: number
}

export interface CleanupReq {
  /** RFC3339；仅删 to_ts <= before 的分区 */
  before: string
  dry_run: boolean
}

/** dry-run 预览汇总：待删明细的规模与用量/成本合计 */
export interface CleanupSummary {
  log_rows: number
  prompt_tokens: number
  completion_tokens: number
  cache_write_tokens: number
  cache_read_tokens: number
  cost_cny: string
  cost_usd: string
}

export interface CleanupResp {
  dry_run: boolean
  /** dry_run=true 时为将被删分区预览 */
  dropped: PartitionInfo[]
  /** dry_run 时为待删聚合行数，执行时为已删行数 */
  hourly_rows?: number
  /** 实际生效截止时间（按分区边界对齐）；null = 没有可清理的分区 */
  effective_before?: string | null
  /** 仅 dry_run 返回；执行时为 null */
  summary?: CleanupSummary | null
}

/** 日志列表响应：页码模式返回 total；游标模式 total=null、返回 next_cursor */
export interface LogListResp {
  items: LogItem[]
  /** 游标模式（带 cursor 请求）为 null */
  total: number | null
  page: number
  page_size: number
  /** 游标模式下还有下一页时的续页游标；页码模式恒 null */
  next_cursor?: string | null
}

// ---------------------------------------------------------------------------
// stats（/api/stats）
// ---------------------------------------------------------------------------

export type StatsCurrency = 'CNY' | 'USD'
export type SeriesGranularity = 'hour' | 'day'
export type SeriesDimension = '' | 'model' | 'key' | 'upstream' | 'protocol'

export interface StatsQuery {
  from_ts?: string
  to_ts?: string
  key_id?: string
  upstream_id?: string
  model?: string
  protocol?: string
  currency?: StatsCurrency
  granularity?: SeriesGranularity
  dimension?: SeriesDimension
}

export interface StatsSummary {
  requests: number
  errors: number
  success_rate: number
  prompt_tokens: number
  completion_tokens: number
  total_tokens: number
  cost_cny: string | null
  cost_usd: string | null
  /** 按展示币种列合计（已舍入） */
  cost_display: string | null
  /** 展示币种列为 NULL 但有成本的条数（未计价混入） */
  cost_na_count: number
  avg_latency_ms: number | null
  p50_ms: number | null
  p95_ms: number | null
  p99_ms: number | null
  source: string
}

export interface SeriesPoint {
  bucket: string
  dimension: string
  requests: number
  errors: number
  prompt_tokens: number
  completion_tokens: number
  cost_cny: string | null
  cost_usd: string | null
  cost_display: string | null
  cost_na_count: number
}

export interface SeriesResp {
  currency: StatsCurrency
  points: SeriesPoint[]
}

// ---------------------------------------------------------------------------
// pricing（/api/pricing）
// ---------------------------------------------------------------------------

export type PriceUnit =
  | 'token_in'
  | 'token_out'
  | 'token_cache_write'
  | 'token_cache_read'
  | 'image'
  | 'video_second'

/** 分段：时间条件（星期 + 窗口，start>end=跨午夜）与上下文 token 区间，按数组顺序第一命中 */
export interface PriceSegment {
  name?: string | null
  price: string
  /** mon..sun 小写三字母 */
  weekdays?: string[] | null
  /** [["09:00","12:00"], ...] */
  windows?: [string, string][] | null
  min_prompt_tokens?: number | null
  max_prompt_tokens?: number | null
}

export interface PriceRuleRow {
  id: string
  upstream_id: string
  model_id: string
  unit: PriceUnit
  currency: StatsCurrency
  base_price: string
  /** manual | import */
  source: string
  /** 维度对象（图片/视频，可空） */
  dimensions: Record<string, unknown> | null
  /** 服务端归一化（勿手填） */
  dimension_key: string
  segments: PriceSegment[] | null
  /** prompt_tokens | total_tokens */
  context_basis: string
  effective_from: string | null
  effective_to: string | null
  priority: number
  sort_order: number
  enabled: boolean
  created_at: string
  updated_at: string
}

export interface RuleItem extends PriceRuleRow {
  upstream_name: string | null
}

export interface PriceRuleIn {
  upstream_id: string
  model_id: string
  unit: PriceUnit
  currency: StatsCurrency
  base_price: string
  dimensions?: Record<string, unknown> | null
  segments?: PriceSegment[] | null
  context_basis?: string | null
  effective_from?: string | null
  effective_to?: string | null
  priority?: number | null
  sort_order?: number | null
  enabled?: boolean | null
}

/** PUT 时显式 null 可清空 dimensions/segments/effective_* */
export type PriceRuleUpdate = Partial<PriceRuleIn>

export interface PreviewReq {
  upstream_id: string
  model_id: string
  /** 计费时间（缺省 now） */
  at?: string | null
  prompt_tokens?: number | null
  completion_tokens?: number | null
  cache_write_tokens?: number | null
  cache_read_tokens?: number | null
  images?: number | null
  image_size?: string | null
  video_seconds?: string | null
  video_resolution?: string | null
  video_task_type?: string | null
}

export interface PreviewLine {
  unit: PriceUnit
  currency: StatsCurrency
  /** token 为原始数（非 /1M）；video=向上取整秒数(≥1) */
  quantity: string
  /** 每 1M token / 每张 / 每秒 */
  price: string
  cost: string
  rule_id: string
  matched_segment: string | null
}

export interface PreviewResp {
  lines: PreviewLine[]
  cost_cny: string | null
  cost_usd: string | null
  price_used: unknown[]
  fx_snapshot: unknown[]
  priced: boolean
}

export interface SuggestGroup {
  upstream_id: string
  upstream_name: string
  rules: PriceRuleRow[]
  updated_at: string
}

export interface SuggestResp {
  items: SuggestGroup[]
}

export interface UnpricedItem {
  upstream_id: string
  upstream_name: string
  model_id: string
}

export interface UnpricedResp {
  items: UnpricedItem[]
}

/** 导出响应为纯文本（XML/JSON），无 Content-Disposition */
export type PriceExportFormat = 'xml' | 'json'

export interface ImportReport {
  total: number
  succeeded: number
  skipped: number
  failed: { index: number; reason: string }[]
  dry_run: boolean
}

// ---------------------------------------------------------------------------
// fx（/api/fx）
// ---------------------------------------------------------------------------

export interface FxRow {
  currency_from: string
  currency_to: string
  rate: string
  /** manual | auto */
  source: 'manual' | 'auto'
  fetched_at: string | null
  updated_at: string
  /** auto 且超陈旧阈值才为 true */
  stale: boolean
}

export interface PutFxItem {
  from: string
  to: string
  rate: number
}

export interface FxPutResp {
  ok: boolean
  updated: number
}

export interface FxRefreshResp {
  ok: boolean
  report: { updated: number; pairs: string[] }
}

// ---------------------------------------------------------------------------
// audit（/api/audit）
// ---------------------------------------------------------------------------

export interface AuditQuery {
  page?: number
  page_size?: number
  action?: string
  object_type?: string
}

export interface AuditRow {
  id: number
  admin_id: string | null
  admin_name: string | null
  action: string
  object_type: string | null
  object_id: string | null
  /** JSON 对象 */
  summary: Record<string, unknown> | null
  ip: string | null
  created_at: string
}

// ---------------------------------------------------------------------------
// system（/api/system，M8 后端补丁）
// ---------------------------------------------------------------------------

/** 运维状态摘要（/api/system/status，M10.4） */
export interface SystemStatusResp {
  version: string
  started_at: string
  uptime_secs: number
  database: { ok: boolean; latency_ms: number }
  settings_sources: Record<string, number>
  logging: {
    queue_used: number | null
    queue_capacity: number | null
    overflow_total: number
    wal_files: number
    wal_bytes: number
  }
  media_poller: { interval_secs: number; max_age_hours: number; pending_tasks: number }
  breakers: {
    name: string
    consecutive_failures: number
    disabled_by: string | null
    cooldown_until: string | null
  }[]
  recent_errors: { ts: string; model: string; status: number; error: string | null }[]
}

export interface VersionResp {
  version: string
  started_at: string
}

/** 反向代理与部署诊断（/api/system/diagnostics，M15 §6.5） */
export interface DiagnosticsResp {
  version: string
  request_id: string
  /** 部署前缀（子路径部署支持落地后填充；当前恒为空） */
  deployment_prefix: string
  request: {
    host: string | null
    scheme: string
    user_agent: string | null
    x_forwarded_for: string | null
    x_real_ip: string | null
    x_forwarded_host: string | null
    x_forwarded_proto: string | null
    x_forwarded_port: string | null
  }
  client: {
    peer_addr: string | null
    peer_ip: string | null
    /** TCP 对端是否命中 server.trusted_proxies */
    peer_trusted: boolean
    client_ip: string | null
    /** 客户端 IP 来源：peer / xff / x-real-ip */
    source: string
  }
  trusted_proxies: string[]
  hints: string[]
}

export interface CheckUpdateReq {
  /** 仅本次检查覆盖 */
  use_proxy?: boolean
  proxy_id?: string
}

export interface CheckUpdateResp {
  current_version: string
  latest_version: string
  release_name: string
  published_at: string | null
  html_url: string
  update_available: boolean
  release_notes: string
}
