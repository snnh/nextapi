<template>
  <el-dialog
    :model-value="modelValue"
    :title="`重新授权 · ${row?.name ?? ''}`"
    width="600px"
    :close-on-click-modal="false"
    destroy-on-close
    @update:model-value="(v: boolean) => emit('update:modelValue', v)"
    @closed="reset"
  >
    <el-alert
      type="info"
      :closable="false"
      show-icon
      title="重新授权会换取新的 session token 并直接写回该渠道"
      description="适用于 Devin session token 过期/失效（列表标注「凭证失效」）。授权码沿用官方 Devin CLI 的 PKCE 流程，旧 token 会被新 token 覆盖。"
    />

    <ol class="steps">
      <li>
        <span>生成授权链接并在浏览器打开（使用你自己的 Devin 账号完成授权）</span>
        <div class="step-row">
          <el-button
            type="primary"
            :loading="starting"
            :icon="Link"
            @click="startAuth"
          >
            {{ verifier ? '重新生成授权链接' : '开始授权' }}
          </el-button>
          <el-button v-if="authorizeUrl" :icon="CopyDocument" @click="copyUrl">复制链接</el-button>
        </div>
        <div v-if="authorizeUrl" class="url mono">{{ authorizeUrl }}</div>
      </li>
      <li>
        <span>把授权完成后页面给出的授权码粘贴到下面，点击「换取并保存」</span>
        <el-input
          v-model="code"
          class="code-input"
          placeholder="粘贴 Devin CLI 授权码"
          clearable
          @keyup.enter="submit"
        />
      </li>
    </ol>

    <el-alert
      v-if="probeText"
      class="probe"
      type="success"
      :closable="false"
      show-icon
      :title="probeText"
    />

    <template #footer>
      <el-button @click="emit('update:modelValue', false)">关闭</el-button>
      <el-button
        type="primary"
        :loading="submitting"
        :disabled="!verifier || !code.trim()"
        @click="submit"
      >
        换取并保存
      </el-button>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
import { ref } from 'vue'
import { ElMessage } from 'element-plus'
import { CopyDocument, Link } from '@element-plus/icons-vue'
import { upstreamApi } from '@/api'
import { errMsg } from '@/api/http'
import { copyText } from '@/utils/clipboard'
import type { UpstreamOut } from '@/api/types'

const props = defineProps<{
  modelValue: boolean
  row: UpstreamOut | null
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', v: boolean): void
  (e: 'done'): void
}>()

const starting = ref(false)
const submitting = ref(false)
const verifier = ref('')
const authorizeUrl = ref('')
const code = ref('')
const probeText = ref('')

/** 第一步：生成 PKCE 会话（无状态；verifier 前端暂存，兑换时回传）并打开授权页 */
async function startAuth() {
  starting.value = true
  try {
    const r = await upstreamApi.devinPkceStart()
    verifier.value = r.code_verifier
    authorizeUrl.value = r.authorize_url
    window.open(r.authorize_url, '_blank')
    ElMessage.info('已打开 Devin 授权页；完成授权后把授权码粘贴回来')
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    starting.value = false
  }
}

function copyUrl() {
  copyText(authorizeUrl.value)
  ElMessage.success('授权链接已复制')
}

/** 第二步：授权码 → 新 token 写回该上游（后端顺带探测验证并解除失效隔离） */
async function submit() {
  if (!props.row) return
  if (!verifier.value) {
    ElMessage.warning('请先点击「开始授权」')
    return
  }
  const c = code.value.trim()
  if (!c) return
  submitting.value = true
  try {
    const r = await upstreamApi.devinReauth(props.row.id, {
      code: c,
      code_verifier: verifier.value,
      base_url: props.row.base_url || undefined,
    })
    const p = r.probe as { email?: string; plan?: string; models?: number }
    const parts: string[] = []
    if (p?.email) parts.push(`账号 ${p.email}`)
    if (p?.plan) parts.push(`套餐 ${p.plan}`)
    if (typeof p?.models === 'number') parts.push(`可用模型 ${p.models} 个`)
    probeText.value = parts.length ? `授权成功：${parts.join(' · ')}` : '授权成功，凭证已更新'
    ElMessage.success('已换新 token 并解除凭证失效标记')
    emit('done')
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    submitting.value = false
  }
}

function reset() {
  verifier.value = ''
  authorizeUrl.value = ''
  code.value = ''
  probeText.value = ''
}
</script>

<style scoped>
.steps {
  margin: 16px 0 0;
  padding-left: 18px;
  line-height: 1.9;
  font-size: 13px;
  color: var(--el-text-color-regular);
}

.step-row {
  margin: 6px 0;
}

.url {
  word-break: break-all;
  font-size: 12px;
  color: var(--el-text-color-secondary);
  background: var(--el-fill-color-light);
  padding: 6px 8px;
  border-radius: 4px;
}

.code-input {
  margin-top: 6px;
  max-width: 420px;
}

.probe {
  margin-top: 14px;
}
</style>
