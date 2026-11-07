<template>
  <div class="page login-page">
    <div class="login-theme"><ThemeToggle /></div>
    <el-card class="login-card" shadow="never">
      <div class="login-head">
        <div class="logo">next</div>
        <h1 class="title">NextAPI 管理后台</h1>
        <div class="subtitle">自托管 LLM 网关</div>
      </div>
      <p v-if="errorMsg" class="login-error" role="alert" aria-live="polite">{{ errorMsg }}</p>
      <el-form
        ref="formRef"
        :model="form"
        :rules="rules"
        size="large"
        label-position="top"
        @submit.prevent="handleLogin"
      >
        <el-form-item label="用户名" prop="username">
          <el-input
            v-model="form.username"
            name="username"
            autocomplete="username"
            placeholder="用户名"
            :prefix-icon="User"
            clearable
          />
        </el-form-item>
        <el-form-item label="密码" prop="password">
          <el-input
            v-model="form.password"
            name="current-password"
            autocomplete="current-password"
            type="password"
            placeholder="密码"
            :prefix-icon="Lock"
            show-password
          />
        </el-form-item>
        <el-form-item v-if="needTotp" label="二次验证码" prop="totpCode">
          <el-input
            ref="totpInput"
            v-model="form.totpCode"
            name="one-time-code"
            autocomplete="one-time-code"
            inputmode="numeric"
            placeholder="认证器 6 位数字"
            :prefix-icon="Key"
            maxlength="6"
            clearable
          />
        </el-form-item>
        <el-form-item>
          <el-button class="login-btn" type="primary" native-type="submit" :loading="loading">登录</el-button>
        </el-form-item>
      </el-form>
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { nextTick, reactive, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import axios, { AxiosError } from 'axios'
import type { FormInstance, FormRules } from 'element-plus'
import { Key, Lock, User } from '@element-plus/icons-vue'
import { useAuthStore } from '@/stores/auth'
import { errMsg } from '@/api/http'
import ThemeToggle from '@/components/common/ThemeToggle.vue'
import type { ApiErrorBody } from '@/api/types'

const router = useRouter()
const route = useRoute()
const auth = useAuthStore()

const formRef = ref<FormInstance>()
const totpInput = ref<{ focus: () => void } | null>(null)
const loading = ref(false)
/** 后端返回 totp_required 后展示验证码输入框 */
const needTotp = ref(false)
const errorMsg = ref('')
const form = reactive({
  username: '',
  password: '',
  totpCode: '',
})

const rules: FormRules = {
  username: [{ required: true, message: '请输入用户名', trigger: 'blur' }],
  password: [{ required: true, message: '请输入密码', trigger: 'blur' }],
}

/** 提取后端错误体的机器可读码 */
function errCode(e: unknown): string | null {
  if (axios.isAxiosError(e)) {
    return (e as AxiosError<ApiErrorBody>).response?.data?.error?.code ?? null
  }
  return null
}

async function handleLogin() {
  if (loading.value || !formRef.value) return
  try {
    await formRef.value.validate()
  } catch {
    return
  }
  errorMsg.value = ''
  loading.value = true
  try {
    await auth.login(form.username, form.password, form.totpCode || undefined)
    const redirect = (route.query.redirect as string) || '/dashboard'
    router.push(redirect)
  } catch (e) {
    const code = errCode(e)
    if (code === 'totp_required') {
      // 密码已通过，进入二次验证步骤
      needTotp.value = true
      errorMsg.value = '该账号已启用二次验证，请输入认证器验证码'
      await nextTick()
      totpInput.value?.focus()
    } else if (code === 'totp_invalid') {
      needTotp.value = true
      form.totpCode = ''
      errorMsg.value = '验证码错误或已过期，请重新输入'
      await nextTick()
      totpInput.value?.focus()
    } else {
      // 普通失败（多为密码错误）：重置二次验证展示，避免残留误导
      needTotp.value = false
      form.totpCode = ''
      errorMsg.value = errMsg(e)
    }
  } finally {
    loading.value = false
  }
}
</script>

<style scoped>
.login-page {
  position: relative;
  max-width: none;
  min-height: 100vh;
  padding: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background: radial-gradient(ellipse at 50% 0%, var(--nx-accent-soft), var(--nx-canvas) 62%);
}

.login-theme {
  position: absolute;
  top: 16px;
  right: 16px;
}

.login-card {
  width: 380px;
  max-width: 92vw;
  border-radius: var(--nx-radius-lg);
  border: 1px solid var(--nx-border);
  background: var(--nx-surface);
  box-shadow: none;
}

.login-head {
  text-align: center;
  margin-bottom: 24px;
}

.logo {
  display: inline-grid;
  place-items: center;
  height: 42px;
  padding: 0 16px;
  border-radius: var(--nx-radius);
  background: var(--nx-accent-solid);
  color: var(--nx-on-accent);
  font-size: var(--nx-font-lg);
  font-weight: 800;
  letter-spacing: 1px;
}

.title {
  margin: 10px 0 4px;
  font-size: var(--nx-font-lg);
  font-weight: 600;
  color: var(--nx-text);
}

.subtitle {
  font-size: var(--nx-font-small);
  color: var(--nx-text-3);
}

.login-error {
  margin: 0 0 14px;
  padding: 8px 12px;
  border: 1px solid var(--nx-danger-border);
  border-radius: var(--nx-radius);
  background: var(--nx-danger-soft);
  color: var(--nx-danger);
  font-size: var(--nx-font-small);
  line-height: 1.6;
}

.login-btn {
  width: 100%;
}

@media (max-width: 800px) {
  .login-theme {
    top: 8px;
    right: 8px;
  }
}
</style>