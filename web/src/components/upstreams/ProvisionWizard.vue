<template>
  <el-dialog
    v-model="visible"
    title="接入上游向导"
    width="660px"
    :close-on-click-modal="false"
    destroy-on-close
    @closed="reset"
  >
    <el-steps :active="step" align-center finish-status="success" class="steps">
      <el-step title="选择供应商" />
      <el-step title="填写凭证" />
      <el-step title="连通与模型" />
    </el-steps>

    <!-- 1. 选择供应商 -->
    <div v-if="step === 0" class="step-body">
      <el-input
        v-model="keyword"
        placeholder="搜索供应商名称 / 类型"
        clearable
        :prefix-icon="Search"
        style="margin-bottom: 12px"
      />
      <div class="preset-list">
        <div
          v-for="p in filteredPresets"
          :key="p.name"
          class="preset-item"
          :class="{ active: picked?.name === p.name }"
          role="button"
          tabindex="0"
          @click="pick(p)"
          @keyup.enter="pick(p)"
        >
          <div class="preset-head">
            <span class="preset-name">{{ p.display_name }}</span>
            <el-tag size="small" type="info">{{ p.kind }}</el-tag>
            <el-icon v-if="picked?.name === p.name" class="check"><CircleCheckFilled /></el-icon>
          </div>
          <div class="hint">{{ p.description }}</div>
          <div class="tag-group">
            <el-tag
              v-for="proto in p.protocols"
              :key="proto"
              size="small"
              type="primary"
              effect="plain"
            >
              {{ PROTOCOL_LABEL(proto) }}
            </el-tag>
          </div>
        </div>
        <el-empty v-if="!filteredPresets.length" description="没有匹配的供应商预设" :image-size="80" />
      </div>
    </div>

    <!-- 2. 填写凭证 -->
    <el-form v-else-if="step === 1" label-width="100px" class="step-body" @submit.prevent>
      <el-form-item label="供应商">
        <div class="hint">
          {{ picked?.display_name }}（{{ picked?.kind }}）
        </div>
      </el-form-item>
      <el-form-item label="上游名称" required>
        <el-input v-model="name" placeholder="用于区分同一供应商的多个账号" clearable />
      </el-form-item>
      <el-form-item label="API Key" required>
        <el-input
          v-model="apiKey"
          type="password"
          show-password
          autocomplete="new-password"
          data-lpignore="true"
          data-1p-ignore
          placeholder="必填，加密存储，之后仅可二次验证查看"
          clearable
        />
      </el-form-item>
      <el-form-item label="Base URL">
        <el-input :model-value="picked?.base_url" readonly />
      </el-form-item>
      <el-form-item
        v-for="(url, proto) in picked?.protocol_base_urls"
        :key="proto"
        :label="`${PROTOCOL_LABEL(String(proto))} 地址`"
      >
        <el-input :model-value="String(url)" readonly />
      </el-form-item>
      <div v-if="conflict" class="hint conflict">
        名称「{{ name }}」已存在，请修改名称后重试。
      </div>
    </el-form>

    <!-- 3. 连通与模型 -->
    <div v-else class="step-body">
      <el-alert
        v-if="result?.test.ok"
        type="success"
        :closable="false"
        show-icon
        :title="`连通正常 HTTP ${result.test.status} · ${result.test.latency_ms}ms`"
      />
      <el-alert
        v-else
        type="error"
        :closable="false"
        show-icon
        :title="result?.test.error || '连通测试失败'"
      >
        <div class="hint">上游已创建，可稍后到列表页修改凭证或重试连通测试。</div>
      </el-alert>

      <div class="group-title">模型发现</div>
      <div v-if="modelsLoading" class="hint">正在拉取模型列表…</div>
      <el-alert
        v-else-if="modelsErr"
        type="warning"
        :closable="false"
        show-icon
        :title="`拉取模型失败：${modelsErr}`"
      >
        <el-button size="small" :icon="Refresh" style="margin-top: 6px" @click="fetchModels">
          重试拉取
        </el-button>
      </el-alert>
      <template v-else>
        <div class="hint">
          发现 <b>{{ models.length }}</b> 个模型，其中 <b>{{ newCount }}</b> 个尚未建立路由。
        </div>
        <div class="sync-actions">
          <template v-if="modelSync === 'auto'">
            <div class="hint">
              该渠道为「自动同步」：托管路由会跟随上游模型列表增删。
            </div>
            <el-button
              type="primary"
              size="small"
              :loading="syncRunning"
              @click="runSync"
            >
              立即同步模型
            </el-button>
          </template>
          <template v-else>
            <div class="hint">
              该渠道为「手动同步」：勾选创建路由请到「上游详情 → 拉取模型」，或在此一键创建全部新模型路由。
            </div>
            <el-button
              type="primary"
              size="small"
              :loading="syncRunning"
              :disabled="newCount === 0"
              @click="createAllRoutes"
            >
              为 {{ newCount }} 个新模型创建路由
            </el-button>
          </template>
        </div>
        <el-alert
          v-if="syncReport"
          type="success"
          :closable="false"
          show-icon
          class="report"
          :title="`同步完成：新增 ${syncReport.added.length} · 移除 ${syncReport.removed.length} · 保留 ${syncReport.kept}`"
        />
      </template>
    </div>

    <template #footer>
      <template v-if="step === 0">
        <el-button @click="visible = false">取消</el-button>
        <el-button type="primary" :disabled="!picked" @click="step = 1">下一步</el-button>
      </template>
      <template v-else-if="step === 1">
        <el-button @click="step = 0">上一步</el-button>
        <el-button type="primary" :loading="submitting" @click="submit">接入并测试</el-button>
      </template>
      <template v-else>
        <el-button @click="finish">完成</el-button>
        <el-button type="primary" @click="gotoRoutes">下一步：配置模型路由</el-button>
      </template>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { ElMessage } from 'element-plus'
import { CircleCheckFilled, Refresh, Search } from '@element-plus/icons-vue'
import { presetApi, upstreamApi } from '@/api'
import { errMsg } from '@/api/http'
import { PROTOCOL_LABEL } from '@/utils/consts'
import type {
  ModelSyncReport,
  Preset,
  ProvisionResp,
  UpstreamModelItem,
} from '@/api/types'

const props = defineProps<{
  modelValue: boolean
  presets: Preset[]
  /** 从预设卡片直接进入时预选的供应商 */
  initialPreset?: Preset | null
}>()

const emit = defineEmits<{
  'update:modelValue': [boolean]
  /** 上游已创建/同步完成，父级刷新列表 */
  saved: []
  'goto-routes': []
}>()

const visible = computed({
  get: () => props.modelValue,
  set: (v: boolean) => emit('update:modelValue', v),
})

// —— 步骤状态 ——
const step = ref(0)
const keyword = ref('')
const picked = ref<Preset | null>(null)

// —— 凭证 ——
const name = ref('')
const apiKey = ref('')
const submitting = ref(false)
const conflict = ref(false)
const result = ref<ProvisionResp | null>(null)

// —— 模型发现与同步 ——
const modelsLoading = ref(false)
const modelsErr = ref('')
const models = ref<UpstreamModelItem[]>([])
const modelSync = ref<'manual' | 'auto'>('manual')
const syncRunning = ref(false)
const syncReport = ref<ModelSyncReport | null>(null)

const filteredPresets = computed(() => {
  const kw = keyword.value.trim().toLowerCase()
  if (!kw) return props.presets
  return props.presets.filter(
    (p) =>
      p.display_name.toLowerCase().includes(kw) ||
      p.name.toLowerCase().includes(kw) ||
      p.kind.toLowerCase().includes(kw),
  )
})

const newCount = computed(() => models.value.filter((m) => m.status === 'new').length)

const upstreamId = computed(() => result.value?.upstream?.id ?? '')

watch(
  () => props.modelValue,
  (v) => {
    if (!v) return
    // 打开时：带预选则直接进入凭证步骤，否则从选择供应商开始
    const p = props.initialPreset ?? null
    picked.value = p
    step.value = p ? 1 : 0
    name.value = p?.name ?? ''
    apiKey.value = ''
    conflict.value = false
    result.value = null
    models.value = []
    modelsErr.value = ''
    syncReport.value = null
  },
)

function pick(p: Preset) {
  picked.value = p
  if (!name.value.trim()) name.value = p.name
}

function reset() {
  step.value = 0
  keyword.value = ''
  picked.value = null
  name.value = ''
  apiKey.value = ''
  submitting.value = false
  conflict.value = false
  result.value = null
  models.value = []
  modelsErr.value = ''
  syncRunning.value = false
  syncReport.value = null
}

/** 接入：创建上游 + 自动连通测试，成功后自动进入模型发现 */
async function submit() {
  if (!picked.value || submitting.value) return
  if (!name.value.trim()) {
    ElMessage.warning('请输入上游名称')
    return
  }
  if (!apiKey.value.trim()) {
    ElMessage.warning('请填写 API Key')
    return
  }
  submitting.value = true
  conflict.value = false
  try {
    const resp = await presetApi.provision(picked.value.name, {
      api_key: apiKey.value,
      name: name.value.trim(),
    })
    result.value = resp
    step.value = 2
    emit('saved')
    if (resp.test.ok) ElMessage.success('上游已接入，连通正常')
    else ElMessage.warning('上游已创建，但连通测试未通过')
    fetchModels()
  } catch (e) {
    const msg = errMsg(e)
    if (msg.includes('已存在')) {
      conflict.value = true
      ElMessage.warning('名称已存在，请修改名称后重试')
    } else {
      ElMessage.error(msg)
    }
  } finally {
    submitting.value = false
  }
}

/** 拉取上游模型列表（用于展示发现数量与新增路由） */
async function fetchModels() {
  if (!upstreamId.value) return
  modelsLoading.value = true
  modelsErr.value = ''
  try {
    const r = await upstreamApi.fetchModels(upstreamId.value)
    models.value = r.items
    modelSync.value = r.model_sync
  } catch (e) {
    modelsErr.value = errMsg(e)
  } finally {
    modelsLoading.value = false
  }
}

/** 自动同步渠道：立即全量对账托管路由 */
async function runSync() {
  if (!upstreamId.value || syncRunning.value) return
  syncRunning.value = true
  try {
    syncReport.value = await upstreamApi.syncModels(upstreamId.value)
    ElMessage.success('同步完成')
    emit('saved')
    await fetchModels()
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    syncRunning.value = false
  }
}

/** 手动同步渠道：为全部未路由模型创建手动路由 */
async function createAllRoutes() {
  if (!upstreamId.value || syncRunning.value) return
  const names = models.value.filter((m) => m.status === 'new').map((m) => m.name)
  if (!names.length) return
  syncRunning.value = true
  try {
    syncReport.value = await upstreamApi.addModelRoutes(upstreamId.value, names)
    ElMessage.success(`已创建 ${syncReport.value.added.length} 条路由`)
    emit('saved')
    await fetchModels()
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    syncRunning.value = false
  }
}

function finish() {
  visible.value = false
}

function gotoRoutes() {
  visible.value = false
  emit('goto-routes')
}
</script>

<style scoped>
.steps {
  margin-bottom: 18px;
}
.step-body {
  min-height: 220px;
}
.preset-list {
  max-height: 380px;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.preset-item {
  border: 1px solid #e5e7eb;
  border-radius: 8px;
  padding: 10px 12px;
  cursor: pointer;
  transition: border-color 0.15s ease, background 0.15s ease;
}
.preset-item:hover {
  border-color: #c0c4cc;
  background: #fafbfc;
}
.preset-item.active {
  border-color: #409eff;
  background: #f2f8ff;
}
.preset-head {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 4px;
}
.preset-name {
  font-weight: 600;
  color: #1f2329;
}
.check {
  margin-left: auto;
  color: #409eff;
}
.hint {
  color: #6b7280;
  font-size: 12px;
  line-height: 1.7;
}
.conflict {
  color: #e6a23c;
  padding-left: 100px;
}
.group-title {
  margin: 16px 0 8px;
  font-size: 14px;
  font-weight: 600;
  color: #1f2329;
}
.sync-actions {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
  margin-top: 8px;
}
.report {
  margin-top: 12px;
}
</style>
