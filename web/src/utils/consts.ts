// 常量表：协议/单位/币种等中英映射与选项（M8 契约冻结）
import type { PriceUnit, ProtocolName, QuotaUnit, QuotaWindow } from '@/api/types'

// —— 协议 ——
export const PROTOCOL_LABELS: Record<ProtocolName, string> = {
  openai_chat: 'OpenAI Chat',
  openai_responses: 'OpenAI Responses',
  anthropic: 'Anthropic Messages',
  gemini: 'Gemini',
  images_openai: '图片 · OpenAI',
  images_gemini: '图片 · Gemini',
  images_dashscope_sync: '图片 · 阿里云同步',
  images_dashscope_async: '图片 · 阿里云异步',
}

export const PROTOCOL_OPTIONS: { value: ProtocolName; label: string }[] = (
  Object.keys(PROTOCOL_LABELS) as ProtocolName[]
).map((v) => ({ value: v, label: PROTOCOL_LABELS[v] }))

export const STANDARD_PROTOCOLS: ProtocolName[] = ['openai_chat', 'openai_responses', 'anthropic', 'gemini']

/** 标准协议默认优先级（后端缺省） */
export const DEFAULT_PRIORITY: ProtocolName[] = ['openai_chat', 'anthropic', 'gemini', 'openai_responses']

export const PROTOCOL_LABEL = (p: string): string =>
  PROTOCOL_LABELS[p as ProtocolName] ?? p

// —— 计价单位 ——
export const UNIT_LABELS: Record<PriceUnit, string> = {
  token_in: '输入 token（每 1M）',
  token_out: '输出 token（每 1M）',
  token_cache_write: '缓存写入（每 1M）',
  token_cache_read: '缓存读取（每 1M）',
  image: '图片（每张）',
  video_second: '视频（每秒）',
}

export const UNIT_OPTIONS: { value: PriceUnit; label: string }[] = (
  Object.keys(UNIT_LABELS) as PriceUnit[]
).map((v) => ({ value: v, label: UNIT_LABELS[v] }))

export const UNIT_SHORT: Record<PriceUnit, string> = {
  token_in: '输入',
  token_out: '输出',
  token_cache_write: '缓存写',
  token_cache_read: '缓存读',
  image: '图片',
  video_second: '视频秒',
}

export const UNIT_LABEL = (u: string): string => UNIT_LABELS[u as PriceUnit] ?? u

// —— 币种 ——
export const CURRENCIES = ['CNY', 'USD'] as const

// —— 配额 ——
export const QUOTA_UNIT_LABELS: Record<QuotaUnit, string> = {
  tokens: 'Token 数',
  cost_cny: '成本（CNY）',
  cost_usd: '成本（USD）',
}

export const QUOTA_WINDOW_LABELS: Record<QuotaWindow, string> = {
  daily: '每天',
  monthly: '每月',
  total: '累计',
}

// —— 星期（mon..sun ↔ 中文），分段时间条件用 ——
export const WEEKDAYS: { value: string; label: string }[] = [
  { value: 'mon', label: '一' },
  { value: 'tue', label: '二' },
  { value: 'wed', label: '三' },
  { value: 'thu', label: '四' },
  { value: 'fri', label: '五' },
  { value: 'sat', label: '六' },
  { value: 'sun', label: '日' },
]

export const WEEKDAY_LABEL = (w: string): string =>
  WEEKDAYS.find((x) => x.value === w)?.label ?? w

/** 上游 kind 建议选项（后端不做白名单校验，仅为表单引导） */
export const UPSTREAM_KINDS = [
  'openai_compatible',
  'openai',
  'anthropic',
  'gemini',
  'aliyun',
  'baidu',
  'kimi',
  'tencent',
  'zhipu',
  'volcano',
  'openrouter',
  'zenmux',
  'deepseek',
  'codex',
  'custom',
]

/** 代理类型 */
export const PROXY_KINDS = [
  { value: 'http', label: 'HTTP' },
  { value: 'https', label: 'HTTPS' },
  { value: 'socks5', label: 'SOCKS5' },
]

/** 维度键建议 key（图片/视频价格维度） */
export const DIMENSION_KEY_HINTS = ['image_size', 'width', 'height', 'resolution', 'task_type']

// —— stats ——
export const DIMENSION_OPTIONS = [
  { value: '', label: '不分组（总量）' },
  { value: 'model', label: '按模型' },
  { value: 'key', label: '按 Key' },
  { value: 'upstream', label: '按上游' },
  { value: 'protocol', label: '按协议' },
] as const

/** protocol_in wire 值 → 展示名（日志/统计用） */
export const PROTOCOL_IN_LABELS: Record<string, string> = {
  openai_chat: 'Chat',
  openai_responses: 'Responses',
  anthropic: 'Anthropic',
  gemini: 'Gemini',
}

// —— 状态码语义 ——
export function statusType(s: number): 'success' | 'warning' | 'danger' | 'info' {
  if (s < 400) return 'success'
  if (s < 500) return 'warning'
  return 'danger'
}
