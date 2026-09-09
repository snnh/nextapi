// ============================================================================
// 模型工作台聚合（M13.4.5）
// 把「模型路由 + 上游 + 价格 + 别名」四份后端数据在前端聚合为「对外模型」视图。
// 纯函数，便于单测；不新增后端接口，遵循 M8 契约（页面只调用分组 API 函数）。
// ============================================================================
import type { ModelAliasRow, RouteOut, RuleItem, UpstreamOut } from '@/api/types'

/** 模型行价格状态：已配置 / 部分路由已配置 / 未配置 / 无法判定（通配路由无覆盖模型名） */
export type ModelPriceState = 'priced' | 'partial' | 'unpriced' | 'unknown'

/** 模型行健康度：ok=全部正常；warn=上游停用/冷却中；error=上游熔断或已被自动禁用/缺失 */
export type ModelHealth = 'ok' | 'warn' | 'error'

/** 模型工作台一行：以 model_pattern 聚合的多条路由 */
export interface ModelOverviewRow {
  /** 对外模型名（= model_pattern） */
  model: string
  /** 是否含通配符 */
  wildcard: boolean
  /** 该模型下的全部路由 */
  routes: RouteOut[]
  /** 关联上游名（去重，保持路由顺序） */
  upstreamNames: string[]
  /** 启用路由数 / 总路由数 */
  enabledCount: number
  totalCount: number
  /** 最小优先级（数字小者优先）；无路由时为 null */
  minPriority: number | null
  /** 是否存在渠道模型同步托管路由 */
  autoManaged: boolean
  priceState: ModelPriceState
  /** 已配价格的路由数（仅精确判定部分） */
  pricedRouteCount: number
  /** 可精确判定价格的路由数 */
  determinateRouteCount: number
  /** 逐条路由的价格判定：true=已配 / false=未配 / 'unknown'=通配且无覆盖模型名 */
  priceByRoute: Record<string, boolean | 'unknown'>
  /** 指向该模型的别名 */
  aliases: ModelAliasRow[]
  health: ModelHealth
  /** 健康度说明（异常时给出原因） */
  healthText: string
  /** 最近更新时间（取路由最大 updated_at） */
  updatedAt: string | null
}

/** 单条上游健康度（前端近似判定，最终以网关熔断状态机为准） */
export function upstreamHealth(u: UpstreamOut | undefined): ModelHealth {
  if (!u) return 'error'
  if (u.disabled_by) return 'error'
  if (!u.enabled) return 'warn'
  if (u.cooldown_until && new Date(u.cooldown_until).getTime() > Date.now()) return 'warn'
  if (u.breaker_threshold > 0 && u.consecutive_failures >= u.breaker_threshold) return 'error'
  return 'ok'
}

const HEALTH_RANK: Record<ModelHealth, number> = { ok: 0, warn: 1, error: 2 }

/** 路由实际使用的上游模型名：覆盖模型名优先，否则用对外模型名 */
export function effectiveModelId(route: RouteOut): string {
  return route.override_model?.trim() || route.model_pattern
}

/** 价格规则是否命中某条路由（精确匹配 upstream_id + model_id，且规则启用） */
export function isRoutePriced(route: RouteOut, rules: RuleItem[]): boolean {
  const modelId = effectiveModelId(route)
  return rules.some(
    (r) => r.enabled && r.upstream_id === route.upstream_id && r.model_id === modelId,
  )
}

/** 单条路由的价格状态：无法判定（通配且无覆盖模型名）/ 已配 / 未配 */
export function routePriceState(
  route: RouteOut,
  rules: RuleItem[],
): boolean | 'unknown' {
  if (effectiveModelId(route).includes('*')) return 'unknown'
  return isRoutePriced(route, rules)
}

/** 聚合：路由表 → 模型行（按 model_pattern 分组，模型名升序） */
export function buildModelOverview(
  routes: RouteOut[],
  upstreams: UpstreamOut[],
  rules: RuleItem[],
  aliases: ModelAliasRow[],
): ModelOverviewRow[] {
  const upstreamMap = new Map(upstreams.map((u) => [u.id, u]))
  const byPattern = new Map<string, RouteOut[]>()
  for (const r of routes) {
    const list = byPattern.get(r.model_pattern)
    if (list) list.push(r)
    else byPattern.set(r.model_pattern, [r])
  }

  const rows: ModelOverviewRow[] = []
  for (const [model, list] of byPattern) {
    const upstreamNames: string[] = []
    let health: ModelHealth = 'ok'
    const reasons: string[] = []
    for (const r of list) {
      if (!upstreamNames.includes(r.upstream_name)) upstreamNames.push(r.upstream_name)
      const u = upstreamMap.get(r.upstream_id)
      const h = upstreamHealth(u)
      if (HEALTH_RANK[h] > HEALTH_RANK[health]) health = h
      if (h === 'error') {
        const why = !u
          ? '上游已删除'
          : u.disabled_by
            ? `上游「${u.name}」已被自动禁用`
            : `上游「${u.name}」连续失败达阈值`
        if (!reasons.includes(why)) reasons.push(why)
      } else if (h === 'warn' && u) {
        const why = !u.enabled ? `上游「${u.name}」已停用` : `上游「${u.name}」熔断冷却中`
        if (!reasons.includes(why)) reasons.push(why)
      }
    }

    // 价格：仅对能确定实际模型名的路由判定（含 * 的通配且无覆盖模型名 → 无法判定）
    let determinate = 0
    let priced = 0
    const priceByRoute: Record<string, boolean | 'unknown'> = {}
    for (const r of list) {
      const st = routePriceState(r, rules)
      priceByRoute[r.id] = st
      if (st === 'unknown') continue
      determinate++
      if (st) priced++
    }
    let priceState: ModelPriceState
    if (determinate === 0) priceState = 'unknown'
    else if (priced === determinate) priceState = 'priced'
    else if (priced > 0) priceState = 'partial'
    else priceState = 'unpriced'

    const updatedAt = list.reduce<string | null>(
      (acc, r) => (!acc || r.updated_at > acc ? r.updated_at : acc),
      null,
    )

    rows.push({
      model,
      wildcard: model.includes('*'),
      routes: list,
      upstreamNames,
      enabledCount: list.filter((r) => r.enabled).length,
      totalCount: list.length,
      minPriority: list.length ? Math.min(...list.map((r) => r.priority)) : null,
      autoManaged: list.some((r) => r.managed_by === 'auto'),
      priceState,
      pricedRouteCount: priced,
      determinateRouteCount: determinate,
      priceByRoute,
      aliases: aliases.filter((a) => a.model === model),
      health,
      healthText: reasons.length ? reasons.join('；') : '正常',
      updatedAt,
    })
  }

  rows.sort((a, b) => a.model.localeCompare(b.model))
  return rows
}
