// 价格管理组件共享的表单类型与转换工具（仅用于 components/pricing/*）
import type { PriceSegment, PriceUnit, StatsCurrency } from '@/api/types'

/** 维度键值编辑行 */
export interface DimKV {
  key: string
  value: string
}

/** 时间窗口：start/end 均为 HH:mm；start > end 表示跨午夜 */
export interface SegWindow {
  start: string
  end: string
}

/** 分段表单行（price 为数字便于 input-number，提交时转字符串） */
export interface SegmentForm {
  name: string
  price: number | null
  weekdays: string[]
  windows: SegWindow[]
  min_prompt_tokens: number | null
  max_prompt_tokens: number | null
}

/** 规则编辑/新建表单状态 */
export interface RuleFormState {
  upstream_id: string
  model_id: string
  unit: PriceUnit
  currency: StatsCurrency
  base_price: number | null
  context_basis: string
  dimensions: DimKV[]
  segments: SegmentForm[]
  /** 展示格式 YYYY-MM-DD HH:mm:ss；空串 = 不设置 */
  effective_from: string
  effective_to: string
  priority: number
  sort_order: number
  enabled: boolean
}

/** 分段表单 → 后端 PriceSegment */
export function segToPrice(s: SegmentForm): PriceSegment {
  return {
    name: s.name.trim() || null,
    price: numToDecimal(s.price ?? 0),
    weekdays: s.weekdays.length ? [...s.weekdays] : null,
    windows: s.windows.length ? s.windows.map((w) => [w.start, w.end] as [string, string]) : null,
    min_prompt_tokens: s.min_prompt_tokens,
    max_prompt_tokens: s.max_prompt_tokens,
  }
}

/** 数字 → 十进制字符串（保留至多 10 位小数，避免科学计数法如 1e-10） */
export function numToDecimal(v: number): string {
  if (!Number.isFinite(v)) return String(v)
  let s = v.toFixed(10)
  s = s.replace(/0+$/, '').replace(/\.$/, '')
  return s === '-0' ? '0' : s
}

/** 后端 PriceSegment → 分段表单行 */
export function priceToSegForm(p: PriceSegment): SegmentForm {
  return {
    name: p.name ?? '',
    price: p.price === null || p.price === undefined ? null : Number(p.price),
    weekdays: p.weekdays ? [...p.weekdays] : [],
    windows: p.windows ? p.windows.map((w) => ({ start: w[0], end: w[1] })) : [],
    min_prompt_tokens: p.min_prompt_tokens ?? null,
    max_prompt_tokens: p.max_prompt_tokens ?? null,
  }
}

/** 维度值：纯数字转 number，其余保留字符串（为空返回 null） */
export function parseDimValue(v: string): unknown {
  const t = v.trim()
  if (!t) return null
  if (/^-?\d+(\.\d+)?$/.test(t)) return Number(t)
  return t
}

/** 维度字符串（后端 Record<string, unknown> → 表格展示文本） */
export function fmtDimensionValue(v: unknown): string {
  if (v === null || v === undefined) return ''
  if (typeof v === 'object') {
    try {
      return JSON.stringify(v)
    } catch {
      return String(v)
    }
  }
  return String(v)
}

/** 是否为图片/视频单位（使用维度计价） */
export function isMediaUnit(u: PriceUnit): boolean {
  return u === 'image' || u === 'video_second'
}
