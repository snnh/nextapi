<template>
  <div class="totp-panel">
    <el-alert type="info" :closable="false" class="tip">
      二次验证（TOTP）：启用后登录除密码外还需输入认证器（Google Authenticator / 1Password 等）的 6 位动态码。
      认证器丢失的恢复途径：在数据库执行
      <code>UPDATE admin_users SET totp_enabled = false, totp_secret_enc = NULL;</code>
    </el-alert>

    <div v-if="loading" v-loading="true" class="loading-box" />

    <template v-else>
      <!-- 状态行 -->
      <div class="status-row">
        <el-tag :type="status?.enabled ? 'success' : 'info'">
          {{ status?.enabled ? '已启用' : status?.pending ? '待确认' : '未启用' }}
        </el-tag>
        <span class="status-text">
          {{ status?.enabled ? '登录已受二次验证保护' : status?.pending ? '机密已生成，请完成验证以启用' : '当前登录仅依赖密码' }}
        </span>
      </div>

      <!-- 未启用：设置流程 -->
      <template v-if="!status?.enabled">
        <template v-if="!setup">
          <el-form inline @submit.prevent>
            <el-form-item label="当前密码">
              <el-input
                v-model="setupPassword"
                type="password"
                show-password
                style="width: 200px"
                placeholder="设置 TOTP 需验证密码"
              />
            </el-form-item>
            <el-form-item>
              <el-button type="primary" :loading="busy" :disabled="!setupPassword" @click="doSetup">
                生成 TOTP 机密
              </el-button>
            </el-form-item>
          </el-form>
        </template>
        <div v-else class="setup-box">
          <div class="qr-row">
            <img v-if="qrDataUrl" :src="qrDataUrl" class="qr" alt="TOTP 二维码" />
            <div class="secret-col">
              <div class="secret-label">无法扫码？手动输入机密：</div>
              <el-input :model-value="setup.secret" readonly class="secret-input">
                <template #append>
                  <el-button @click="copySecret">复制</el-button>
                </template>
              </el-input>
            </div>
          </div>
          <el-form inline @submit.prevent>
            <el-form-item label="验证码">
              <el-input
                v-model="enableCode"
                inputmode="numeric"
                placeholder="认证器 6 位数字"
                maxlength="6"
                style="width: 180px"
                @input="onEnableCodeInput"
              />
            </el-form-item>
            <el-form-item>
              <el-button type="primary" :loading="busy" :disabled="enableCode.length !== 6" @click="doEnable">
                验证并启用
              </el-button>
              <el-button :disabled="busy" @click="cancelSetup">取消</el-button>
            </el-form-item>
          </el-form>
        </div>
      </template>

      <!-- 已启用：禁用流程 -->
      <template v-else>
        <el-button v-if="!disabling" type="danger" plain @click="disabling = true">禁用 TOTP</el-button>
        <el-form v-else inline @submit.prevent>
          <el-form-item label="密码">
            <el-input v-model="disableForm.password" type="password" show-password style="width: 180px" />
          </el-form-item>
          <el-form-item label="验证码">
            <el-input
              v-model="disableForm.code"
              inputmode="numeric"
              placeholder="6 位数字"
              maxlength="6"
              style="width: 140px"
              @input="onDisableCodeInput"
            />
          </el-form-item>
          <el-form-item>
            <el-button
              type="danger"
              :loading="busy"
              :disabled="!disableForm.password || disableForm.code.length !== 6"
              @click="doDisable"
            >
              确认禁用
            </el-button>
            <el-button :disabled="busy" @click="disabling = false">取消</el-button>
          </el-form-item>
        </el-form>
      </template>
    </template>
  </div>
</template>

<script setup lang="ts">
import { onMounted, reactive, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { useRouter } from 'vue-router'
import QRCode from 'qrcode'
import { authApi } from '@/api'
import { errMsg } from '@/api/http'
import { useAuthStore } from '@/stores/auth'
import type { TotpSetupResp, TotpStatusResp } from '@/api/types'

const router = useRouter()
const auth = useAuthStore()

const loading = ref(false)
const busy = ref(false)
const setupPassword = ref("")
const status = ref<TotpStatusResp | null>(null)
const setup = ref<TotpSetupResp | null>(null)
const qrDataUrl = ref('')
const enableCode = ref('')
const disabling = ref(false)
const disableForm = reactive({ password: '', code: '' })

async function load() {
  loading.value = true
  try {
    status.value = await authApi.totpStatus()
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    loading.value = false
  }
}

async function doSetup() {
  busy.value = true
  try {
    setup.value = await authApi.totpSetup(setupPassword.value)
    setupPassword.value = ""
    qrDataUrl.value = await QRCode.toDataURL(setup.value.otpauth_url, { width: 180, margin: 1 })
    enableCode.value = ''
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    busy.value = false
  }
}

async function doEnable() {
  busy.value = true
  try {
    await authApi.totpEnable(enableCode.value)
    ElMessage.success('TOTP 已启用，所有会话已注销，请重新登录')
    relogin()
    return
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    busy.value = false
  }
}

async function doDisable() {
  busy.value = true
  try {
    await authApi.totpDisable(disableForm.password, disableForm.code)
    ElMessage.success('TOTP 已禁用，所有会话已注销，请重新登录')
    relogin()
    return
  } catch (e) {
    ElMessage.error(errMsg(e))
    disableForm.password = ''
  } finally {
    busy.value = false
  }
}

/**
 * enable/disable 成功后服务端已吊销全部会话：走 auth store 清登录态后回登录页。
 * （统一由 logout 清理 token/localStorage，避免直接操作 localStorage）
 */
function relogin() {
  auth.logout()
  router.push('/login')
}

/** 6 位验证码输入：仅保留数字 */
function digitsOnly(v: string): string {
  return v.replace(/\D/g, '')
}

function onEnableCodeInput(v: string) {
  enableCode.value = digitsOnly(v)
}

function onDisableCodeInput(v: string) {
  disableForm.code = digitsOnly(v)
}

function cancelSetup() {
  setup.value = null
  qrDataUrl.value = ''
  enableCode.value = ''
}

async function copySecret() {
  if (!setup.value) return
  try {
    await navigator.clipboard.writeText(setup.value.secret)
    ElMessage.success('已复制')
  } catch {
    ElMessage.warning('复制失败，请手动选择复制')
  }
}

onMounted(load)
</script>

<style scoped>
.totp-panel {
  max-width: 640px;
}
.tip {
  margin-bottom: 16px;
}
.tip code {
  background: rgba(0, 0, 0, 0.06);
  padding: 1px 4px;
  border-radius: 3px;
}
.loading-box {
  height: 120px;
}
.status-row {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 16px;
}
.status-text {
  color: #606266;
  font-size: 13px;
}
.setup-box {
  border: 1px solid #e4e7ed;
  border-radius: 8px;
  padding: 16px;
}
.qr-row {
  display: flex;
  gap: 20px;
  align-items: center;
  margin-bottom: 16px;
}
.qr {
  width: 180px;
  height: 180px;
  border: 1px solid #e4e7ed;
  border-radius: 4px;
}
.secret-col {
  flex: 1;
  min-width: 240px;
}
.secret-label {
  font-size: 13px;
  color: #606266;
  margin-bottom: 6px;
}
.secret-input {
  font-family: monospace;
}
</style>
