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
import { reactive, ref } from 'vue'
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
    {
      validator: (_r, v: string, cb) => {
        if (v !== form.new_password) cb(new Error('两次输入不一致'))
        else cb()
      },
      trigger: 'blur',
    },
  ],
}

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
