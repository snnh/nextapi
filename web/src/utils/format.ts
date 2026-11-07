// 格式化工具：时间/金额/百分比/字节/时长
import dayjs from 'dayjs'

/** null/undefined/空 → '-'；否则本地化 YYYY-MM-DD HH:mm:ss */
export function fmtTime(iso?: string | null): string {
  if (!iso) return '-'
  const d = dayjs(iso)
  return d.isValid() ? d.format('YYYY-MM-DD HH:mm:ss') : '-'
}

/** Decimal 字符串 → 展示（截断 6 位？后端已按 display_precision 舍入；此处仅格式化千分位） */
export function fmtMoney(v?: string | number | null): string {
  if (v === null || v === undefined || v === '') return '-'
  const n = typeof v === 'number' ? v : Number(v)
  if (Number.isNaN(n)) return String(v)
  return n.toLocaleString('zh-CN', { maximumFractionDigits: 6 })
}

/** 百分比 0.95 → 95.00% */
export function fmtPct(v?: number | null): string {
  if (v === null || v === undefined) return '-'
  return `${(v * 100).toFixed(2)}%`
}

/** 整数千分位 */
export function fmtInt(v?: number | null): string {
  if (v === null || v === undefined) return '-'
  return v.toLocaleString('zh-CN')
}

/** 字节人性化 */
export function fmtBytes(b?: number | null): string {
  if (b === null || b === undefined) return '-'
  if (b < 1024) return `${b} B`
  const units = ['KB', 'MB', 'GB', 'TB']
  let v = b
  let i = -1
  do {
    v /= 1024
    i += 1
  } while (v >= 1024 && i < units.length - 1)
  return `${v.toFixed(1)} ${units[i]}`
}

/** 毫秒时长（<1s 显示 ms，其余 s） */
export function fmtDur(ms?: number | null): string {
  if (ms === null || ms === undefined) return '-'
  if (ms < 1000) return `${ms} ms`
  return `${(ms / 1000).toFixed(1)} s`
}

/** 秒 → 运行时长：秒/分钟/小时/天，保留最高两级 */
export function fmtUptime(secs: number): string {
  if (secs < 60) return `${Math.floor(secs)} 秒`
  if (secs < 3600) return `${Math.floor(secs / 60)} 分钟`
  if (secs < 86400) return `${Math.floor(secs / 3600)} 小时 ${Math.floor((secs % 3600) / 60)} 分`
  return `${Math.floor(secs / 86400)} 天 ${Math.floor((secs % 86400) / 3600)} 小时`
}

/** 本地 00:00 起当天 ISO；用于默认「今天」区间 */
export function todayRange(): [string, string] {
  const start = dayjs().startOf('day')
  return [start.toISOString(), dayjs().toISOString()]
}
