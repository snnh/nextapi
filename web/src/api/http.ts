// axios 实例：统一鉴权头、401 处理、错误消息提取。
// 页面代码禁止直接 import axios——一律走 api/index.ts 的分组函数。
import axios, { AxiosError } from 'axios'
import type { ApiErrorBody } from './types'

export const TOKEN_KEY = 'nextapi_token'
/** 登录用户名持久化键（auth store 与 401 清理共用，避免循环 import） */
export const USER_KEY = 'nextapi_user'

export function getToken(): string | null {
  return localStorage.getItem(TOKEN_KEY)
}

export function setToken(t: string) {
  localStorage.setItem(TOKEN_KEY, t)
}

export function clearToken() {
  localStorage.removeItem(TOKEN_KEY)
}

/** 提取后端统一错误体 message；无则回退状态文案 */
export function errMsg(e: unknown): string {
  if (axios.isAxiosError(e)) {
    const ax = e as AxiosError<ApiErrorBody>
    const msg = ax.response?.data?.error?.message
    if (msg) return msg
    if (ax.code === 'ECONNABORTED') return '请求超时'
    if (!ax.response) return '网络错误，无法连接服务器'
    return `请求失败（HTTP ${ax.response.status}）`
  }
  return e instanceof Error ? e.message : String(e)
}

const http = axios.create({
  baseURL: import.meta.env.VITE_API_BASE || undefined,
  timeout: 30000,
  headers: { 'Content-Type': 'application/json' },
})

http.interceptors.request.use((cfg) => {
  const t = getToken()
  if (t) cfg.headers.Authorization = `Bearer ${t}`
  return cfg
})

http.interceptors.response.use(
  (resp) => resp,
  (error: AxiosError<ApiErrorBody>) => {
    if (error.response?.status === 401 && !error.config?.url?.includes('/auth/login')) {
      clearToken()
      localStorage.removeItem(USER_KEY)
      if (window.location.pathname !== '/login') {
        window.location.href = '/login'
      }
    }
    return Promise.reject(error)
  },
)

export default http
