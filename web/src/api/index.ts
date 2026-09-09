// ============================================================================
// 分组 API 函数（M8 契约冻结：页面只允许调用这些函数，禁止裸 axios）
// 路径前缀统一 /api（除 login 外全部需 JWT，由 http.ts 注入）
// ============================================================================
import http from './http'
import type {
  ApiKeyRow,
  AuditQuery,
  AuditRow,
  ChangePasswordReq,
  CheckUpdateReq,
  CheckUpdateResp,
  CleanupReq,
  CleanupResp,
  ConfigFileView,
  ConfigPutReq,
  DiagnosticsResp,
  FxRefreshResp,
  FxRow,
  ImportReport,
  KeyCreateReq,
  KeySecretResp,
  KeyUpdateResp,
  LoginReq,
  LoginResp,
  TotpSetupResp,
  TotpStatusResp,
  LogItem,
  LogListResp,
  OkResp,
  Paged,
  PreviewReq,
  PreviewResp,
  Preset,
  PriceExportFormat,
  PriceRuleIn,
  PriceRuleUpdate,
  ProxyIn,
  ProxyOut,
  ProxyTestResp,
  ProvisionReq,
  ProvisionResp,
  RouteItem,
  RouteOut,
  ModelAliasRow,
  AliasIn,
  RuleItem,
  SettingsPutReq,
  SettingsView,
  StatsQuery,
  StatsSummary,
  SeriesResp,
  SuggestResp,
  TestResp,
  UnpricedItem,
  UpstreamIn,
  UpstreamOut,
  UpstreamRevealReq,
  UpstreamRevealResp,
  SystemStatusResp,
  VersionResp,
  UpstreamModelsResp,
  ModelSyncReport,
} from './types'

// ---------------------------------------------------------------------------
// auth
// ---------------------------------------------------------------------------
export const authApi = {
  login: (body: LoginReq) => http.post<LoginResp>('/api/auth/login', body).then((r) => r.data),
  changePassword: (body: ChangePasswordReq) =>
    http.put<OkResp>('/api/auth/password', body).then((r) => r.data),
  totpStatus: () => http.get<TotpStatusResp>('/api/auth/totp').then((r) => r.data),
  totpSetup: (password: string) =>
    http.post<TotpSetupResp>('/api/auth/totp/setup', { password }).then((r) => r.data),
  totpEnable: (code: string) =>
    http.post<OkResp>('/api/auth/totp/enable', { code }).then((r) => r.data),
  totpDisable: (password: string, code: string) =>
    http.post<OkResp>('/api/auth/totp/disable', { password, code }).then((r) => r.data),
}

// ---------------------------------------------------------------------------
// keys（整批无分页）
// ---------------------------------------------------------------------------
export const keyApi = {
  list: () => http.get<{ items: ApiKeyRow[] }>('/api/keys').then((r) => r.data.items),
  create: (body: KeyCreateReq) => http.post<KeySecretResp>('/api/keys', body).then((r) => r.data),
  update: (id: string, body: KeyCreateReq) =>
    http.put<KeyUpdateResp>(`/api/keys/${id}`, body).then((r) => r.data.row),
  remove: (id: string) => http.delete<OkResp>(`/api/keys/${id}`).then((r) => r.data),
  rotate: (id: string) =>
    http.post<KeySecretResp>(`/api/keys/${id}/rotate`).then((r) => r.data),
}

// ---------------------------------------------------------------------------
// upstreams
// ---------------------------------------------------------------------------
export const upstreamApi = {
  list: () => http.get<{ items: UpstreamOut[] }>('/api/upstreams').then((r) => r.data.items),
  create: (body: UpstreamIn) => http.post<UpstreamOut>('/api/upstreams', body).then((r) => r.data),
  update: (id: string, body: UpstreamIn) =>
    http.put<UpstreamOut>(`/api/upstreams/${id}`, body).then((r) => r.data),
  remove: (id: string) => http.delete<OkResp>(`/api/upstreams/${id}`).then((r) => r.data),
  test: (id: string) => http.post<TestResp>(`/api/upstreams/${id}/test`).then((r) => r.data),
  /** 安全验证（管理员密码 + 可选 TOTP）通过后查看明文 API Key；仅本次展示 */
  revealKey: (id: string, body: UpstreamRevealReq) =>
    http.post<UpstreamRevealResp>(`/api/upstreams/${id}/reveal-key`, body).then((r) => r.data),
  /** 实时拉取上游模型列表（含路由状态标注） */
  fetchModels: (id: string) =>
    http.get<UpstreamModelsResp>(`/api/upstreams/${id}/models`).then((r) => r.data),
  /** 自动模式：立即全量对账托管路由 */
  syncModels: (id: string) =>
    http.post<ModelSyncReport>(`/api/upstreams/${id}/models/sync`).then((r) => r.data),
  /** Codex：手动刷新 OAuth access_token */
  refreshOAuth: (id: string) =>
    http.post<{ ok: boolean; account_id: string }>(`/api/upstreams/${id}/oauth/refresh`).then((r) => r.data),
  /** 手动模式：为勾选模型创建手动路由 */
  addModelRoutes: (id: string, models: string[]) =>
    http.post<ModelSyncReport>(`/api/upstreams/${id}/models/routes`, { models }).then((r) => r.data),
}

// ---------------------------------------------------------------------------
// presets
// ---------------------------------------------------------------------------
export const presetApi = {
  list: () => http.get<{ items: Preset[] }>('/api/presets').then((r) => r.data.items),
  provision: (name: string, body: ProvisionReq) =>
    http.post<ProvisionResp>(`/api/presets/${encodeURIComponent(name)}/provision`, body).then((r) => r.data),
}

// ---------------------------------------------------------------------------
// model-routes（PUT 裸数组全量替换）
// ---------------------------------------------------------------------------
export const routeApi = {
  list: () => http.get<{ items: RouteOut[] }>('/api/model-routes').then((r) => r.data.items),
  replaceAll: (items: RouteItem[]) =>
    http.put<{ items: RouteOut[] }>('/api/model-routes', items).then((r) => r.data.items),
}

// ---------------------------------------------------------------------------
// model aliases（M10.1）
// ---------------------------------------------------------------------------
export const aliasApi = {
  list: () => http.get<{ items: ModelAliasRow[] }>('/api/aliases').then((r) => r.data.items),
  create: (input: AliasIn) => http.post<ModelAliasRow>('/api/aliases', input).then((r) => r.data),
  update: (id: string, input: AliasIn) =>
    http.put<ModelAliasRow>(`/api/aliases/${id}`, input).then((r) => r.data),
  remove: (id: string) => http.delete(`/api/aliases/${id}`).then((r) => r.data),
}

// ---------------------------------------------------------------------------
// proxies
// ---------------------------------------------------------------------------
export const proxyApi = {
  list: () => http.get<{ items: ProxyOut[] }>('/api/proxies').then((r) => r.data.items),
  create: (body: ProxyIn) => http.post<ProxyOut>('/api/proxies', body).then((r) => r.data),
  update: (id: string, body: ProxyIn) =>
    http.put<ProxyOut>(`/api/proxies/${id}`, body).then((r) => r.data),
  remove: (id: string) => http.delete<OkResp>(`/api/proxies/${id}`).then((r) => r.data),
  test: (id: string) => http.post<ProxyTestResp>(`/api/proxies/${id}/test`).then((r) => r.data),
}

// ---------------------------------------------------------------------------
// settings / config
// ---------------------------------------------------------------------------
export const settingsApi = {
  view: () => http.get<SettingsView>('/api/settings').then((r) => r.data.settings),
  put: (settings: SettingsPutReq['settings']) =>
    http.put<SettingsView>('/api/settings', { settings }).then((r) => r.data.settings),
}

export const configApi = {
  get: () => http.get<{ config: ConfigFileView }>('/api/config').then((r) => r.data.config),
  put: (body: ConfigPutReq) => http.put<{ config: ConfigFileView }>('/api/config', body).then((r) => r.data.config),
  reload: () => http.post<OkResp>('/api/config/reload').then((r) => r.data),
}

// ---------------------------------------------------------------------------
// logs
// ---------------------------------------------------------------------------
function clean<T extends Record<string, unknown>>(q: T): Record<string, string | number | boolean> {
  const out: Record<string, string | number | boolean> = {}
  for (const [k, v] of Object.entries(q)) {
    if (v !== undefined && v !== null && v !== '') out[k] = v as string | number | boolean
  }
  return out
}

export interface LogListQuery {
  from_ts?: string
  to_ts?: string
  key_id?: string
  upstream_id?: string
  model?: string
  stream?: boolean
  status?: number
  status_group?: 'success' | '4xx' | '5xx' | '429' | 'degraded'
  request_id?: string
  degraded?: boolean
  page?: number
  page_size?: number
  /** 游标分页（深页优先）：提供时忽略 page/OFFSET */
  cursor?: string
}

export const logApi = {
  list: (q: LogListQuery) =>
    http.get<LogListResp>('/api/logs', { params: clean({ ...q }) }).then((r) => r.data),
  detail: (requestId: string) =>
    http.get<LogItem>(`/api/logs/${encodeURIComponent(requestId)}`).then((r) => r.data),
  cleanup: (body: CleanupReq) =>
    http.post<CleanupResp>('/api/logs/cleanup', body).then((r) => r.data),
  /** CSV 导出：返回 blob（服务端 Content-Disposition: usage_logs.csv） */
  exportCsv: async (q: LogListQuery): Promise<Blob> => {
    const r = await http.get('/api/logs/export.csv', { params: clean({ ...q }), responseType: 'blob' })
    return r.data as Blob
  },
}

// ---------------------------------------------------------------------------
// stats
// ---------------------------------------------------------------------------
export const statsApi = {
  summary: (q: StatsQuery) =>
    http.get<StatsSummary>('/api/stats/summary', { params: clean({ ...q }) }).then((r) => r.data),
  series: (q: StatsQuery) =>
    http.get<SeriesResp>('/api/stats/series', { params: clean({ ...q }) }).then((r) => r.data),
}

// ---------------------------------------------------------------------------
// pricing
// ---------------------------------------------------------------------------
export const pricingApi = {
  list: (upstream_id?: string, model_id?: string) =>
    http
      .get<{ items: RuleItem[] }>('/api/pricing', { params: clean({ upstream_id, model_id }) })
      .then((r) => r.data.items),
  create: (body: PriceRuleIn) => http.post<RuleItem>('/api/pricing', body).then((r) => r.data),
  update: (id: string, body: PriceRuleUpdate) =>
    http.put<RuleItem>(`/api/pricing/${id}`, body).then((r) => r.data),
  remove: (id: string) => http.delete<OkResp>(`/api/pricing/${id}`).then((r) => r.data),
  preview: (body: PreviewReq) => http.post<PreviewResp>('/api/pricing/preview', body).then((r) => r.data),
  suggest: (upstream_id: string, model_id: string) =>
    http
      .get<SuggestResp>('/api/pricing/suggest', { params: clean({ upstream_id, model_id }) })
      .then((r) => r.data),
  unpriced: () => http.get<{ items: UnpricedItem[] }>('/api/pricing/unpriced').then((r) => r.data.items),
  /** 导出：返回 XML/JSON 纯文本 */
  exportText: async (format: PriceExportFormat) => {
    const r = await http.get<string>('/api/pricing/export', { params: { format }, responseType: 'text' })
    return r.data as string
  },
  /** 本地文件导入（multipart 字段名 file） */
  importFile: async (file: File, dry_run: boolean): Promise<ImportReport> => {
    const fd = new FormData()
    fd.append('file', file)
    // 不显式设置 Content-Type：浏览器会自动带 boundary（手写会丢 boundary 导致解析失败）
    const r = await http.post<ImportReport>('/api/pricing/import', fd, { params: { dry_run } })
    return r.data
  },
  /** 网络链接导入 */
  importUrl: (url: string, dry_run: boolean) =>
    http.post<ImportReport>('/api/pricing/import', { url, dry_run }).then((r) => r.data),
}

// ---------------------------------------------------------------------------
// fx
// ---------------------------------------------------------------------------
export const fxApi = {
  list: () => http.get<{ items: FxRow[] }>('/api/fx').then((r) => r.data.items),
  put: (items: { from: string; to: string; rate: number }[]) =>
    http.put<{ ok: boolean; updated: number }>('/api/fx', items).then((r) => r.data),
  refresh: () => http.post<FxRefreshResp>('/api/fx/refresh').then((r) => r.data),
}

// ---------------------------------------------------------------------------
// audit
// ---------------------------------------------------------------------------
export const auditApi = {
  list: (q: AuditQuery) =>
    http.get<Paged<AuditRow>>('/api/audit', { params: clean({ ...q }) }).then((r) => r.data),
}

// ---------------------------------------------------------------------------
// system（M8 后端补丁）
// ---------------------------------------------------------------------------
export const systemApi = {
  version: () => http.get<VersionResp>('/api/system/version').then((r) => r.data),
  status: () => http.get<SystemStatusResp>('/api/system/status').then((r) => r.data),
  /** 反向代理与部署诊断（非敏感信息） */
  diagnostics: () => http.get<DiagnosticsResp>('/api/system/diagnostics').then((r) => r.data),
  checkUpdate: (body?: CheckUpdateReq) =>
    http.post<CheckUpdateResp>('/api/system/check-update', body ?? {}).then((r) => r.data),
}


