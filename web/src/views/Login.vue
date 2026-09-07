<template>
  <div class="page login-page">
    <el-card class="login-card" shadow="never">
      <div class="login-head">
        <div class="logo">next</div>
        <h1 class="title">NextAPI 管理后台</h1>
        <div class="subtitle">自托管 LLM 网关</div>
      </div>
      <el-form ref="formRef" :model="form" :rules="rules" size="large" @submit.prevent @keyup.enter="handleLogin">
        <el-form-item prop="username">
          <el-input v-model="form.username" placeholder="用户名" :prefix-icon="User" clearable />
        </el-form-item>
        <el-form-item prop="password">
          <el-input v-model="form.password" type="password" placeholder="密码" :prefix-icon="Lock" show-password />
        </el-form-item>
        <el-form-item v-if="needTotp" prop="totpCode">
          <el-input
            v-model="form.totpCode"
            placeholder="二次验证码（认证器 6 位数字）"
            :prefix-icon="Key"
            maxlength="6"
            clearable
          />
        </el-form-item>
        <el-form-item>
          <el-button class="login-btn" type="primary" :loading="loading" @click="handleLogin">登录</el-button>
        </el-form-item>
      </el-form>
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { reactive, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import axios, { AxiosError } from 'axios'
import { ElMessage, type FormInstance, type FormRules } from 'element-plus'
import { Key, Lock, User } from '@element-plus/icons-vue'
import { useAuthStore } from '@/stores/auth'
import { errMsg } from '@/api/http'
import type { ApiErrorBody } from '@/api/types'

const router = useRouter()
const route = useRoute()
const auth = useAuthStore()

const formRef = ref<FormInstance>()
const loading = ref(false)
/** 后端返回 totp_required 后展示验证码输入框 */
const needTotp = ref(false)
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
      ElMessage.warning('该账号已启用二次验证，请输入认证器验证码')
    } else if (code === 'totp_invalid') {
      needTotp.value = true
      form.totpCode = ''
      ElMessage.error('验证码错误或已过期，请重新输入')
    } else {
      ElMessage.error(errMsg(e))
    }
  } finally {
    loading.value = false
  }
}
</script>

<style scoped>
.login-page {
  min-height: 100vh;
  padding: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background: linear-gradient(160deg, #e9eef0 0%, #f1f4f5 55%, #f1f4f5 100%);
}

.login-card {
  width: 380px;
  max-width: 92vw;
  border-radius: 10px;
  border: 1px solid #e8eaee;
  box-shadow: 0 4px 24px rgba(16, 24, 40, .06);
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
  border-radius: 11px;
  background: #3d444c;
  color: #fff;
  font-size: 20px;
  font-weight: 800;
  letter-spacing: 1px;
  box-shadow: 0 3px 10px rgba(79, 70, 229, .35);
}

.title {
  margin: 10px 0 4px;
  font-size: 18px;
  font-weight: 600;
  color: #1f2329;
}

.subtitle {
  font-size: 13px;
  color: #9ca3af;
}

.login-btn {
  width: 100%;
}
</style>
