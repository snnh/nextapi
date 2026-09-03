<template>
  <div class="page login-page">
    <el-card class="login-card" shadow="always">
      <div class="login-head">
        <div class="logo">NextAPI</div>
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
import { ElMessage, type FormInstance, type FormRules } from 'element-plus'
import { Lock, User } from '@element-plus/icons-vue'
import { useAuthStore } from '@/stores/auth'
import { errMsg } from '@/api/http'

const router = useRouter()
const route = useRoute()
const auth = useAuthStore()

const formRef = ref<FormInstance>()
const loading = ref(false)
const form = reactive({
  username: '',
  password: '',
})

const rules: FormRules = {
  username: [{ required: true, message: '请输入用户名', trigger: 'blur' }],
  password: [{ required: true, message: '请输入密码', trigger: 'blur' }],
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
    await auth.login(form.username, form.password)
    const redirect = (route.query.redirect as string) || '/dashboard'
    router.push(redirect)
  } catch (e) {
    ElMessage.error(errMsg(e))
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
  background: linear-gradient(135deg, #0f2027 0%, #203a43 50%, #2c5364 100%);
}

.login-card {
  width: 400px;
  max-width: 92vw;
  border-radius: 12px;
  border: none;
}

.login-head {
  text-align: center;
  margin-bottom: 24px;
}

.logo {
  font-size: 34px;
  font-weight: 800;
  letter-spacing: 1px;
  background: linear-gradient(90deg, #409eff, #67c23a);
  -webkit-background-clip: text;
  background-clip: text;
  color: transparent;
}

.title {
  margin: 12px 0 4px;
  font-size: 22px;
  font-weight: 600;
  color: #303133;
}

.subtitle {
  font-size: 13px;
  color: #909399;
}

.login-btn {
  width: 100%;
}
</style>
