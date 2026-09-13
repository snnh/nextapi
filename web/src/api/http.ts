// axios 实例：统一鉴权头、401 处理、错误消息提取。
// 页面代码禁止直接 import axios——一律走 api/index.ts 的分组函数。
import axios, { AxiosError } from 'axios'
import type { ApiErrorBody } from './types'
import { apiBase, appBase, appPath } from '@/utils/base'

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
  // 子路径部署（M15 §6.4）：API 前缀随部署前缀变化，不再写死 /api
  baseURL: apiBase(),
  timeout: 30000,
  // 刻意不设全局 Content-Type：axios v1 一旦看到 application/json，就会把 FormData
  // 请求体交给 formDataToJSON 序列化（File 变 `{}`）——文件导入因此变成 JSON `{"file":{}}`，
  // 后端按 JSON 解析后报「参数错误: 请输入 url」。对象请求体由 axios 自动带
  // application/json，multipart 必须留空 Content-Type 让浏览器补 boundary。
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
      // 跳转登录时保留当前 path+search 作为 redirect（含部署前缀），登录成功后跳回
      const base = appBase()
      if (window.location.pathname !== appPath('login')) {
        // redirect 存「去掉部署前缀」的路径，登录成功后由 router.push 还原
        const rel = window.location.pathname.startsWith(base)
          ? window.location.pathname.slice(base.length)
          : window.location.pathname.replace(/^\//, '')
        const redirect = encodeURIComponent(`/${rel}${window.location.search}`)
        window.location.href = `${appPath('login')}?redirect=${redirect}`
      }
    }
    return Promise.reject(error)
  },
)

export default http
