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

/** 提取后端统一错误体 message；无则回退状态文案。响应带 x-request-id 时追加 request_id 便于与后端日志关联 */
export function errMsg(e: unknown): string {
  if (axios.isAxiosError(e)) {
    const ax = e as AxiosError<ApiErrorBody>
    let msg: string
    const bodyMsg = ax.response?.data?.error?.message
    if (bodyMsg) msg = bodyMsg
    else if (ax.code === 'ECONNABORTED') msg = '请求超时'
    else if (!ax.response) msg = '网络错误，无法连接服务器'
    else msg = `请求失败（HTTP ${ax.response.status}）`
    const requestId = ax.response?.headers?.['x-request-id']
    if (typeof requestId === 'string' && requestId) msg += `（request_id: ${requestId}）`
    return msg
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
      // 跳转登录时保留当前 path+search 作为 redirect，登录成功后跳回
      if (window.location.pathname !== '/login') {
        const redirect = encodeURIComponent(window.location.pathname + window.location.search)
        window.location.href = `/login?redirect=${redirect}`
      }
    }
    return Promise.reject(error)
  },
)

export default http
