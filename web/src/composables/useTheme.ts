import { computed, readonly, ref } from 'vue'

export type ThemeMode = 'light' | 'dark' | 'system'
type ResolvedTheme = 'light' | 'dark'

const STORAGE_KEY = 'nextapi.theme'
const SYSTEM_MQ = '(prefers-color-scheme: dark)'

const mode = ref<ThemeMode>('light')
const systemDark = ref(false)
const resolved = computed<ResolvedTheme>(() =>
  mode.value === 'system' ? (systemDark.value ? 'dark' : 'light') : mode.value,
)

let initialized = false
let mq: MediaQueryList | null = null

function readStored(): ThemeMode {
  try {
    const value = localStorage.getItem(STORAGE_KEY)
    if (value === 'light' || value === 'dark' || value === 'system') return value
  } catch {
    /* 隐私模式下 localStorage 不可用 */
  }
  return 'light'
}

function applyTheme() {
  const root = document.documentElement
  const isDark = resolved.value === 'dark'
  root.classList.toggle('dark', isDark)
  root.dataset.theme = resolved.value
  root.dataset.themeMode = mode.value
  root.style.colorScheme = resolved.value
  window.dispatchEvent(
    new CustomEvent('nextapi:theme-change', {
      detail: { mode: mode.value, resolved: resolved.value },
    }),
  )
}

function onSystemChange(event: MediaQueryListEvent) {
  systemDark.value = event.matches
  if (mode.value === 'system') applyTheme()
}

/** 幂等初始化：index.html 已做首屏防闪烁，这里只同步状态并监听系统主题 */
export function initTheme() {
  if (initialized) return
  initialized = true
  mode.value = readStored()
  if (typeof window.matchMedia === 'function') {
    mq = window.matchMedia(SYSTEM_MQ)
    systemDark.value = mq.matches
    if (typeof mq.addEventListener === 'function') {
      mq.addEventListener('change', onSystemChange)
    } else {
      mq.addListener(onSystemChange)
    }
  }
  applyTheme()
}

export function useTheme() {
  initTheme()

  function setMode(next: ThemeMode) {
    mode.value = next
    try {
      localStorage.setItem(STORAGE_KEY, next)
    } catch {
      /* 忽略：偏好仍作用于当前会话 */
    }
    applyTheme()
  }

  return {
    mode: readonly(mode),
    resolved: readonly(resolved),
    setMode,
  }
}