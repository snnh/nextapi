<template>
  <el-dialog
    v-model="visible"
    title="修改密码"
    width="440px"
    :close-on-click-modal="false"
    @closed="resetForm"
  >
    <el-form ref="formRef" :model="form" :rules="rules" label-width="90px" @submit.prevent>
      <el-form-item label="原密码" prop="old_password">
        <el-input v-model="form.old_password" type="password" show-password autocomplete="current-password" />
      </el-form-item>
      <el-form-item label="新密码" prop="new_password">
        <el-input v-model="form.new_password" type="password" show-password autocomplete="new-password" />
        <div v-if="pwdStrength !== null" class="pwd-strength">
          <el-progress
            :percentage="pwdStrength.pct"
            :stroke-width="6"
            :color="pwdStrength.color"
            :show-text="false"
            class="pwd-bar"
          />
          <span class="pwd-label">{{ pwdStrength.text }}</span>
        </div>
      </el-form-item>
      <el-form-item label="确认新密码" prop="confirm">
        <el-input v-model="form.confirm" type="password" show-password autocomplete="new-password" />
      </el-form-item>
    </el-form>
    <template #footer>
      <el-button @click="visible = false">取消</el-button>
      <el-button type="primary" :loading="saving" @click="submit">保存</el-button>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
import { computed, reactive, ref } from 'vue'
import { ElMessage, type FormInstance, type FormRules } from 'element-plus'
import { authApi } from '@/api'
import { errMsg } from '@/api/http'

const visible = ref(false)
const saving = ref(false)
const formRef = ref<FormInstance>()

const form = reactive({ old_password: '', new_password: '', confirm: '' })

const rules: FormRules = {
  old_password: [{ required: true, message: '请输入原密码', trigger: 'blur' }],
  new_password: [
    { required: true, message: '请输入新密码', trigger: 'blur' },
    { min: 8, message: '新密码至少 8 位', trigger: 'blur' },
  ],
  confirm: [
    { required: true, message: '请再次输入新密码', trigger: 'blur' },
    {
      validator: (_r, v: string, cb) => {
        if (v !== form.new_password) cb(new Error('两次输入不一致'))
        else cb()
      },
      trigger: 'blur',
    },
  ],
}

type PwdStrength = { pct: number; color: string; text: string } | null

/** 新密码强度即时提示：弱=仅长度达标；中=含字母+数字；强=含大小写+数字+符号 */
const pwdStrength = computed<PwdStrength>(() => {
  const pw = form.new_password
  if (!pw) return null
  const hasLetter = /[a-zA-Z]/.test(pw)
  const hasDigit = /\d/.test(pw)
  const hasUpper = /[A-Z]/.test(pw)
  const hasLower = /[a-z]/.test(pw)
  const hasSymbol = /[^a-zA-Z0-9]/.test(pw)
  if (hasUpper && hasLower && hasDigit && hasSymbol) {
    return { pct: 100, color: '#67c23a', text: '强（含大小写字母、数字与符号）' }
  }
  if (hasLetter && hasDigit) {
    return { pct: 66, color: '#409eff', text: '中（含字母与数字）' }
  }
  if (pw.length >= 8) {
    return { pct: 33, color: '#e6a23c', text: '弱（仅长度达标，建议混入字母与数字）' }
  }
  return { pct: 0, color: '#f56c6c', text: '长度不足 8 位' }
})

function open() {
  form.old_password = ''
  form.new_password = ''
  form.confirm = ''
  visible.value = true
}

/** 关闭（含成功）后清空表单，防密码驻留内存 */
function resetForm() {
  form.old_password = ''
  form.new_password = ''
  form.confirm = ''
}

async function submit() {
  if (!formRef.value) return
  try {
    await formRef.value.validate()
  } catch {
    return
  }
  saving.value = true
  try {
    await authApi.changePassword({ old_password: form.old_password, new_password: form.new_password })
    ElMessage.success('密码已修改，下次登录请使用新密码')
    visible.value = false
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    saving.value = false
  }
}

defineExpose({ open })
</script>

<style scoped>
.pwd-strength {
  width: 100%;
  display: flex;
  align-items: center;
  gap: 8px;
}
.pwd-bar {
  flex: 1;
  max-width: 160px;
}
.pwd-label {
  font-size: 12px;
  color: #909399;
}
</style>
