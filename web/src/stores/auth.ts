// 登录态 store：token + username，localStorage 持久化
import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { authApi } from '@/api'
import { clearToken, getToken, setToken } from '@/api/http'

export const USER_KEY = 'nextapi_user'

export const useAuthStore = defineStore('auth', () => {
  const token = ref<string | null>(getToken())
  const username = ref<string | null>(localStorage.getItem(USER_KEY))
  const loggedIn = computed(() => !!token.value)

  async function login(user: string, pass: string) {
    if (import.meta.env.VITE_UI_PREVIEW === 'true') {
      token.value = 'ui-preview-token'
      username.value = user || 'admin'
      setToken(token.value)
      localStorage.setItem(USER_KEY, username.value)
      return
    }
    const resp = await authApi.login({ username: user, password: pass })
    token.value = resp.token
    username.value = resp.username
    setToken(resp.token)
    localStorage.setItem(USER_KEY, resp.username)
  }

  function logout() {
    token.value = null
    username.value = null
    clearToken()
    localStorage.removeItem(USER_KEY)
  }

  return { token, username, loggedIn, login, logout }
})
