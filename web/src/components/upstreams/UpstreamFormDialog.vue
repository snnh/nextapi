<template>
  <el-dialog
    v-model="visible"
    :title="isEdit ? '编辑上游' : '新建上游'"
    width="860px"
    :close-on-click-modal="false"
    destroy-on-close
  >
    <el-form ref="formRef" :model="form" :rules="rules" label-width="120px" @submit.prevent>
      <el-divider content-position="left">基础</el-divider>

      <el-form-item label="名称" prop="name">
        <el-input v-model="form.name" placeholder="上游名称（唯一）" clearable />
      </el-form-item>

      <el-form-item label="类型（kind）" prop="kind">
        <el-select
          v-model="form.kind"
          filterable
          allow-create
          default-first-option
          placeholder="选择或输入类型"
          style="width: 100%"
        >
          <el-option v-for="k in UPSTREAM_KINDS" :key="k" :value="k" :label="k" />
        </el-select>
      </el-form-item>

      <el-form-item label="Base URL" prop="base_url">
        <el-input v-model="form.base_url" placeholder="如 https://api.openai.com/v1" clearable />
      </el-form-item>

      <el-form-item label="API Key" prop="api_key">
        <div v-if="isEdit" class="api-key-block">
          <div class="api-key-state">
            <el-tag v-if="form.hasApiKey" type="success" size="small">已设置</el-tag>
            <el-tag v-else type="info" size="small">未设置</el-tag>
          </div>
          <el-input
            v-model="form.api_key"
            type="password"
            show-password
            :placeholder="form.hasApiKey ? '留空保持不变' : '输入新的 Key'"
            clearable
          />
          <el-checkbox v-model="form.clearKey">清除 Key（提交空串）</el-checkbox>
        </div>
        <el-input
          v-else
          v-model="form.api_key"
          type="password"
          show-password
          placeholder="（可选）无 Key 也可保存，网关调用会失败"
          clearable
        />
      </el-form-item>

      <el-form-item label="协议" prop="protocols">
        <el-select
          v-model="form.protocols"
          multiple
          collapse-tags
          collapse-tags-tooltip
          placeholder="选择支持的协议"
          style="width: 100%"
        >
          <el-option-group label="标准协议">
            <el-option v-for="p in STANDARD_PROTOCOLS" :key="p" :label="PROTOCOL_LABELS[p]" :value="p" />
          </el-option-group>
          <el-option-group label="图片协议（媒体请求）">
            <el-option v-for="p in IMAGE_PROTOCOLS" :key="p" :label="PROTOCOL_LABELS[p]" :value="p" />
          </el-option-group>
        </el-select>
      </el-form-item>

      <el-form-item v-if="standardSelected.length" label="优先级">
        <ProtocolPriorityEditor v-model="form.protocolPriority" :selected="standardSelected" />
      </el-form-item>

      <el-divider content-position="left">运行</el-divider>

      <el-form-item label="超时（ms）" prop="timeout_ms">
        <el-input-number v-model="form.timeout_ms" :min="0" :step="1000" style="width: 200px" />
      </el-form-item>

      <el-form-item label="熔断阈值" prop="breaker_threshold">
        <el-input-number v-model="form.breaker_threshold" :min="0" :step="1" style="width: 200px" />
      </el-form-item>

      <el-form-item label="探测模型" prop="probe_model">
        <el-input v-model="form.probe_model" placeholder="（可选）连通性测试使用的模型名" clearable />
      </el-form-item>

      <el-form-item label="启用" prop="enabled">
        <el-switch v-model="form.enabled" />
      </el-form-item>

      <el-divider content-position="left">代理</el-divider>

      <el-form-item label="使用代理" prop="use_proxy">
        <el-switch v-model="form.use_proxy" />
      </el-form-item>

      <el-form-item label="代理" prop="proxy_id">
        <el-select
          v-model="form.proxy_id"
          clearable
          :disabled="!form.use_proxy"
          placeholder="（跟随系统默认代理）"
          style="width: 100%"
        >
          <el-option label="（跟随系统默认代理）" value="" />
          <el-option v-for="p in proxies" :key="p.id" :value="p.id" :label="p.name" />
        </el-select>
      </el-form-item>

      <el-divider content-position="left">高级</el-divider>

      <el-collapse v-model="advancedOpen">
        <el-collapse-item name="advanced">
          <template #title>覆盖项（overrides）与媒体地址</template>
          <el-form-item label="覆盖项" label-width="120px">
            <OverridesEditor v-model="form.overrides" />
          </el-form-item>
          <el-form-item label="媒体地址" label-width="120px" prop="media_base_url">
            <el-input
              v-model="form.media_base_url"
              placeholder="（可选）图片媒体请求基础地址，如阿里百炼原生 DashScope 根；留空回退 base_url"
              clearable
            />
          </el-form-item>
          <el-form-item label="能力矩阵" label-width="120px">
            <div class="caps-grid">
              <span class="caps-label">流式</span>
              <el-select v-model="form.capStream" style="width: 170px">
                <el-option label="未设置（假设支持）" value="unset" />
                <el-option label="明确支持" value="on" />
                <el-option label="不支持（路由过滤）" value="off" />
              </el-select>
              <span class="caps-label">工具调用</span>
              <el-select v-model="form.capTools" style="width: 170px">
                <el-option label="未设置（假设支持）" value="unset" />
                <el-option label="明确支持" value="on" />
                <el-option label="不支持（路由过滤）" value="off" />
              </el-select>
              <span class="caps-label">视觉（图片输入）</span>
              <el-select v-model="form.capVision" style="width: 170px">
                <el-option label="未设置（假设支持）" value="unset" />
                <el-option label="明确支持" value="on" />
                <el-option label="不支持（路由过滤）" value="off" />
              </el-select>
              <span class="caps-label">最大上下文</span>
              <el-input-number
                v-model="form.capMaxContext"
                :min="0"
                :step="8192"
                placeholder="tokens"
                style="width: 170px"
              />
            </div>
            <div class="hint" style="margin-top: 6px">
              标记「不支持」的能力，带该需求的请求路由时自动跳过此上游；最大上下文仅记录展示，暂不参与过滤。
            </div>
          </el-form-item>
        </el-collapse-item>
      </el-collapse>
    </el-form>

    <template #footer>
      <el-button @click="visible = false">取消</el-button>
      <el-button type="primary" :loading="saving" @click="submit">保存</el-button>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import { ElMessage, type FormInstance, type FormRules } from 'element-plus'
import {
  DEFAULT_PRIORITY,
  PROTOCOL_LABELS,
  PROTOCOL_OPTIONS,
  STANDARD_PROTOCOLS,
  UPSTREAM_KINDS,
} from '@/utils/consts'
import { upstreamApi } from '@/api'
import { errMsg } from '@/api/http'
import type {
  Overrides,
  ProtocolName,
  ProxyOut,
  UpstreamExtra,
  UpstreamIn,
  UpstreamOut,
} from '@/api/types'
import OverridesEditor from './OverridesEditor.vue'
import ProtocolPriorityEditor from './ProtocolPriorityEditor.vue'

defineProps<{ proxies: ProxyOut[] }>()
const emit = defineEmits<{ (e: 'saved'): void }>()

const IMAGE_PROTOCOLS = PROTOCOL_OPTIONS.filter((o) => o.value.startsWith('images_')).map((o) => o.value)

const visible = ref(false)
const saving = ref(false)
const isEdit = ref(false)
const editingId = ref('')
const advancedOpen = ref<string[]>([])
const formRef = ref<FormInstance>()

interface FormState {
  name: string
  kind: string
  base_url: string
  api_key: string
  hasApiKey: boolean
  clearKey: boolean
  protocols: ProtocolName[]
  protocolPriority: ProtocolName[]
  timeout_ms: number
  breaker_threshold: number
  probe_model: string
  enabled: boolean
  use_proxy: boolean
  proxy_id: string
  overrides: Overrides
  media_base_url: string
  capStream: 'unset' | 'on' | 'off'
  capTools: 'unset' | 'on' | 'off'
  capVision: 'unset' | 'on' | 'off'
  capMaxContext: number | undefined
}

function defaultForm(): FormState {
  return {
    name: '',
    kind: '',
    base_url: '',
    api_key: '',
    hasApiKey: false,
    clearKey: false,
    protocols: [],
    protocolPriority: [...DEFAULT_PRIORITY],
    timeout_ms: 300000,
    breaker_threshold: 5,
    probe_model: '',
    enabled: true,
    use_proxy: false,
    proxy_id: '',
    overrides: {},
    media_base_url: '',
    capStream: 'unset',
    capTools: 'unset',
    capVision: 'unset',
    capMaxContext: undefined,
  }
}

const form = reactive<FormState>(defaultForm())

const standardSelected = computed<ProtocolName[]>(() =>
  STANDARD_PROTOCOLS.filter((p) => form.protocols.includes(p)),
)

function computePriority(selected: ProtocolName[], base?: ProtocolName[]): ProtocolName[] {
  const selSet = new Set(selected)
  const baseArr: ProtocolName[] = base?.length ? base : [...DEFAULT_PRIORITY]
  const idx = (arr: ProtocolName[]) => (p: ProtocolName) => {
    const i = arr.indexOf(p)
    return i === -1 ? arr.length : i
  }
  const selOrdered = STANDARD_PROTOCOLS.filter((p) => selSet.has(p)).sort(
    (a, b) => idx(baseArr)(a) - idx(baseArr)(b),
  )
  const unselOrdered = STANDARD_PROTOCOLS.filter((p) => !selSet.has(p)).sort(
    (a, b) => idx(DEFAULT_PRIORITY)(a) - idx(DEFAULT_PRIORITY)(b),
  )
  return [...selOrdered, ...unselOrdered]
}

watch(
  () => [...form.protocols],
  () => {
    form.protocolPriority = computePriority(form.protocols, form.protocolPriority)
  },
)

const rules: FormRules = {
  name: [{ required: true, message: '请输入上游名称', trigger: 'blur' }],
  base_url: [
    { required: true, message: '请输入 base_url', trigger: 'blur' },
    {
      validator: (_r, v: string, cb) => {
        if (!v) return cb()
        if (!/^https?:\/\/.+/i.test(v)) return cb(new Error('请输入合法的 URL（http/https）'))
        cb()
      },
      trigger: 'blur',
    },
  ],
}

function open(upstream?: UpstreamOut) {
  isEdit.value = !!upstream
  editingId.value = upstream?.id ?? ''
  advancedOpen.value = []
  const base = defaultForm()
  if (upstream) {
    base.name = upstream.name
    base.kind = upstream.kind
    base.base_url = upstream.base_url
    base.hasApiKey = upstream.has_api_key
    base.protocols = [...upstream.protocols]
    base.protocolPriority = computePriority(upstream.protocols, upstream.extra?.protocol_priority)
    base.timeout_ms = upstream.timeout_ms
    base.breaker_threshold = upstream.breaker_threshold
    base.probe_model = upstream.probe_model ?? ''
    base.enabled = upstream.enabled
    base.use_proxy = upstream.use_proxy
    base.proxy_id = upstream.proxy_id ?? ''
    base.overrides = upstream.extra?.overrides ? JSON.parse(JSON.stringify(upstream.extra.overrides)) : {}
    base.media_base_url = upstream.extra?.media_base_url ?? ''
    const caps = upstream.extra?.capabilities
    base.capStream = caps?.stream === true ? 'on' : caps?.stream === false ? 'off' : 'unset'
    base.capTools = caps?.tools === true ? 'on' : caps?.tools === false ? 'off' : 'unset'
    base.capVision = caps?.vision === true ? 'on' : caps?.vision === false ? 'off' : 'unset'
    base.capMaxContext = caps?.max_context ?? undefined
  }
  Object.assign(form, base)
  visible.value = true
}

function buildExtra(): UpstreamExtra {
  const extra: UpstreamExtra = {}
  if (Object.keys(form.overrides).length) extra.overrides = form.overrides
  extra.protocol_priority = [...form.protocolPriority]
  extra.media_base_url = form.media_base_url.trim() || null
  const tri = (v: 'unset' | 'on' | 'off') => (v === 'unset' ? null : v === 'on')
  const caps: NonNullable<UpstreamExtra['capabilities']> = {
    stream: tri(form.capStream),
    tools: tri(form.capTools),
    vision: tri(form.capVision),
    max_context: form.capMaxContext || null,
  }
  // 全部未设置则不写 capabilities 键（保持 extra 干净）
  if (caps.stream !== null || caps.tools !== null || caps.vision !== null || caps.max_context) {
    extra.capabilities = caps
  }
  return extra
}

function buildBody(): UpstreamIn {
  const body: UpstreamIn = {
    name: form.name.trim(),
    base_url: form.base_url.trim(),
    kind: form.kind.trim() || null,
    protocols: form.protocols.length ? [...form.protocols] : null,
    enabled: form.enabled,
    timeout_ms: form.timeout_ms,
    breaker_threshold: form.breaker_threshold,
    probe_model: form.probe_model.trim() || null,
    use_proxy: form.use_proxy,
    proxy_id: form.use_proxy ? form.proxy_id || null : null,
    extra: buildExtra(),
  }
  if (isEdit.value) {
    if (form.clearKey) body.api_key = ''
    else if (form.api_key && form.api_key !== '***') body.api_key = form.api_key
  } else if (form.api_key && form.api_key.trim()) {
    body.api_key = form.api_key
  }
  return body
}

async function submit() {
  if (!formRef.value || saving.value) return
  try {
    await formRef.value.validate()
  } catch {
    return
  }
  saving.value = true
  try {
    const body = buildBody()
    if (isEdit.value) {
      await upstreamApi.update(editingId.value, body)
      ElMessage.success('上游已更新')
    } else {
      await upstreamApi.create(body)
      ElMessage.success('上游已创建')
    }
    visible.value = false
    form.api_key = '' // 敏感字段用完即清（发布审阅前端 M3）
    emit('saved')
  } catch (e) {
    ElMessage.error(errMsg(e))
    form.api_key = '' // 失败也清：避免已输入的新密钥长期驻留
  } finally {
    saving.value = false
  }
}

defineExpose({ open })
</script>

<style scoped>
.api-key-block {
  width: 100%;
}
.api-key-state {
  margin-bottom: 6px;
}
.caps-grid {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 8px 12px;
  align-items: center;
  width: 100%;
}
.caps-label {
  font-size: 13px;
  color: #6b7280;
}
</style>
