import { computed, readonly, ref } from 'vue'

export type Density = 'compact' | 'standard' | 'relaxed'

const STORAGE_KEY = 'nextapi.density'
/** 首次访问默认紧凑：网关以看数据为主，一屏多放内容 */
const DEFAULT_DENSITY: Density = 'compact'

const density = ref<Density>(DEFAULT_DENSITY)
let initialized = false

function isDensity(v: unknown): v is Density {
  return v === 'compact' || v === 'standard' || v === 'relaxed'
}

function readStored(): Density {
  try {
    const value = localStorage.getItem(STORAGE_KEY)
    if (isDensity(value)) return value
  } catch {
    /* 隐私模式下 localStorage 不可用 */
  }
  return DEFAULT_DENSITY
}

function applyDensity(next: Density) {
  const root = document.documentElement
  // standard 不写属性：直接回落到 theme.css 的 :root 默认值
  if (next === 'standard') root.removeAttribute('data-density')
  else root.setAttribute('data-density', next)
}

/** 幂等初始化：index.html 已做首屏引导，这里只同步内存状态 */
export function initDensity() {
  if (initialized) return
  initialized = true
  density.value = readStored()
  applyDensity(density.value)
}

export function setDensity(next: Density) {
  density.value = next
  applyDensity(next)
  try {
    localStorage.setItem(STORAGE_KEY, next)
  } catch {
    /* 存储失败不影响当前会话 */
  }
}

export const DENSITY_OPTIONS: { value: Density; label: string; hint: string }[] = [
  { value: 'compact', label: '紧凑', hint: '一屏显示更多行' },
  { value: 'standard', label: '标准', hint: '默认间距与字号' },
  { value: 'relaxed', label: '宽松', hint: '更大的行距，适合大屏' },
]

export function useDensity() {
  return {
    density: readonly(density),
    currentLabel: computed(() => DENSITY_OPTIONS.find((o) => o.value === density.value)?.label ?? '标准'),
    setDensity,
  }
}