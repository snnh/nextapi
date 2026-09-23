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
            <el-tag v-if="p.auth_kind === 'oauth'" size="small" type="warning" effect="plain">
              OAuth 授权
            </el-tag>
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
      <!-- OAuth 渠道（Devin）：免 API Key，用 Devin 账号授权换取会话凭证 -->
      <el-form-item v-if="isOauth" label="Devin 授权">
        <div class="api-key-block">
          <div class="hint">
            凭证为 devin CLI 的 session token（<span class="mono">devin-session-token$…</span>）：
            点击「开始授权」在浏览器登录 Devin 账号，把页面给出的授权码粘贴回来兑换，
            token 将随渠道一起加密保存。未授权也可先建渠道，之后到上游列表补授权。
          </div>
          <div class="oauth-state">
            <el-button size="small" :loading="devinAuthLoading" @click="devinStartAuth">
              开始授权
            </el-button>
            <el-input
              v-model="devinCode"
              size="small"
              class="devin-code"
              placeholder="粘贴授权码（code）"
              clearable
            />
            <el-button
              size="small"
              type="primary"
              :loading="devinExchangeLoading"
              :disabled="!devinCode.trim()"
              @click="devinExchange"
            >
              兑换凭证
            </el-button>
            <el-tag v-if="apiKey.trim()" size="small" type="success" effect="plain">
              已获取 session token
            </el-tag>
          </div>
        </div>
      </el-form-item>
      <el-form-item v-else label="API Key" required>
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
      <el-form-item v-if="domainChoices.length > 1" label="接入域名">
        <el-radio-group v-model="domainMode">
          <el-radio-button v-for="d in domainChoices" :key="d.value" :value="d.value">
            {{ d.label }}
          </el-radio-button>
        </el-radio-group>
      </el-form-item>
      <template v-if="useWorkspace">
        <el-form-item label="Workspace ID" required>
          <el-input
            v-model="workspaceId"
            placeholder="如 llm-xxxxxx（控制台「业务空间详情」或 API Key 弹窗的 API Host）"
            autocomplete="off"
            clearable
          />
        </el-form-item>
        <el-form-item label="地域" required>
          <el-select v-model="region" style="width: 100%">
            <el-option
              v-for="r in workspaceDomain?.regions ?? []"
              :key="r.id"
              :label="`${r.name}（${r.id}）`"
              :value="r.id"
            />
          </el-select>
        </el-form-item>
      </template>
      <el-form-item label="Base URL">
        <el-input :model-value="baseUrlPreview" readonly />
      </el-form-item>
      <el-form-item
        v-for="(url, proto) in protocolBaseUrlPreviews"
        :key="proto"
        :label="`${PROTOCOL_LABEL(String(proto))} 地址`"
      >
        <el-input :model-value="String(url)" readonly />
      </el-form-item>
      <el-form-item v-if="mediaBaseUrlPreview" label="媒体地址（图像原生）">
        <el-input :model-value="mediaBaseUrlPreview" readonly />
      </el-form-item>
      <div v-if="domainHint" class="hint domain-hint">{{ domainHint }}</div>
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
      >
        <div v-if="isOauth" class="hint">{{ devinAccountHint }}</div>
      </el-alert>
      <el-alert
        v-else
        type="error"
        :closable="false"
        show-icon
        :title="result?.test.error || '连通测试失败'"
      >
        <div class="hint">
          {{
            isOauth && !apiKey.trim()
              ? '上游已创建但尚未绑定凭证：到「上游」列表点击该渠道的「Devin 授权」完成授权后，再点「测试」与「拉取模型」。'
              : '上游已创建，可稍后到列表页修改凭证或重试连通测试。'
          }}
        </div>
      </el-alert>

      <div v-if="isOauth && !apiKey.trim()" class="hint">
        未授权渠道不会参与路由：请先完成 Devin 授权，再回到「上游」页拉取模型并建立路由。
      </div>

      <div v-if="!isOauth || apiKey.trim()" class="group-title">模型发现</div>
      <div v-if="isOauth && !apiKey.trim()" />
      <div v-else-if="modelsLoading" class="hint">正在拉取模型列表…</div>
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
  ProvisionReq,
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

// —— 接入域名（共享 / 统一域名 / 业务空间专属域名；预设未声明时不展示选择器）——
type DomainMode = 'shared' | 'unified' | 'workspace'
const domainMode = ref<DomainMode>('shared')
const workspaceId = ref('')
const region = ref('')

const workspaceDomain = computed(() => picked.value?.workspace_domain ?? null)
const unifiedDomain = computed(() => picked.value?.unified_domain ?? null)
const useWorkspace = computed(() => !!workspaceDomain.value && domainMode.value === 'workspace')

/** 域名选项：共享域名恒有；统一域名 / 业务空间专属域名按预设声明追加 */
const domainChoices = computed<{ value: DomainMode; label: string }[]>(() => {
  const out: { value: DomainMode; label: string }[] = [{ value: 'shared', label: '共享域名' }]
  if (unifiedDomain.value) {
    out.push({ value: 'unified', label: `${unifiedDomain.value.label}（推荐）` })
  }
  if (workspaceDomain.value) {
    out.push({ value: 'workspace', label: '业务空间专属域名' })
  }
  return out
})

const domainHint = computed(() => {
  if (useWorkspace.value) return workspaceDomain.value?.hint ?? ''
  if (domainMode.value === 'unified') return unifiedDomain.value?.hint ?? ''
  if (domainChoices.value.length > 1) {
    return '百炼共享域名（dashscope.aliyuncs.com）自 2026-09-30 起不再迭代新特性，建议改用上方的统一域名或业务空间专属域名。'
  }
  return ''
})

/** 模板渲染：替换 {workspaceId} / {region} 占位符（与后端 render_domain 一致） */
function renderTpl(tpl: string): string {
  return tpl.split('{workspaceId}').join(workspaceId.value.trim()).split('{region}').join(region.value)
}

/** 选择预设/打开时重置域名状态；有专属域名模板时默认选第一个地域 */
function initDomain(p: Preset | null) {
  domainMode.value = 'shared'
  workspaceId.value = ''
  region.value = p?.workspace_domain?.regions[0]?.id ?? ''
}

/** 当前模式的目标地址（shared 返回 null = 用预设默认；统一域名静态；专属域名渲染占位符） */
const activeDomain = computed<{
  base_url: string
  media_base_url: string | null
  protocol_base_urls: Record<string, string>
} | null>(() => {
  if (domainMode.value === 'unified' && unifiedDomain.value) {
    const u = unifiedDomain.value
    return {
      base_url: u.base_url,
      media_base_url: u.media_base_url,
      protocol_base_urls: u.protocol_base_urls,
    }
  }
  if (useWorkspace.value && workspaceDomain.value) {
    const w = workspaceDomain.value
    const roots: Record<string, string> = {}
    for (const [proto, tpl] of Object.entries(w.protocol_base_urls)) {
      roots[proto] = renderTpl(tpl)
    }
    return {
      base_url: renderTpl(w.base_url),
      media_base_url: w.media_base_url ? renderTpl(w.media_base_url) : null,
      protocol_base_urls: roots,
    }
  }
  return null
})

const baseUrlPreview = computed(() => {
  const p = picked.value
  if (!p) return ''
  return activeDomain.value?.base_url ?? p.base_url
})

const protocolBaseUrlPreviews = computed<Record<string, string>>(() => {
  const p = picked.value
  if (!p) return {}
  const out: Record<string, string> = { ...(p.protocol_base_urls ?? {}) }
  for (const [proto, url] of Object.entries(activeDomain.value?.protocol_base_urls ?? {})) {
    out[proto] = url
  }
  return out
})

const mediaBaseUrlPreview = computed(() => {
  const p = picked.value
  if (!p) return ''
  if (activeDomain.value) return activeDomain.value.media_base_url ?? ''
  return p.media_base_url ?? ''
})

// —— OAuth 渠道（Devin）：CLI PKCE 无头授权（verifier 仅暂存内存，兑换后即弃）——
/** Devin 探测结果摘要（账号 / 套餐 / 可用模型数：Free 计划多模型被门控，只列可用数） */
const devinAccountHint = computed(() => {
  const t = result.value?.test
  if (!t?.ok) return ''
  const parts: string[] = []
  if (t.email) parts.push(`账号 ${t.email}`)
  if (t.plan) parts.push(`套餐 ${t.plan}`)
  if (typeof t.models === 'number') parts.push(`可用模型 ${t.models} 个`)
  return parts.join(' · ')
})

const isOauth = computed(() => picked.value?.auth_kind === 'oauth')
const devinPkce = ref<{ state: string; code_verifier: string } | null>(null)
const devinCode = ref('')
const devinAuthLoading = ref(false)
const devinExchangeLoading = ref(false)

/** 第一步：生成 PKCE 会话并在新窗口打开 Devin 授权页 */
async function devinStartAuth() {
  devinAuthLoading.value = true
  try {
    const r = await upstreamApi.devinPkceStart()
    devinPkce.value = { state: r.state, code_verifier: r.code_verifier }
    window.open(r.authorize_url, '_blank')
    ElMessage.info('已在新窗口打开 Devin 授权页；完成授权后把页面给出的授权码粘贴回来兑换')
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    devinAuthLoading.value = false
  }
}

/** 第二步：授权码 + verifier 兑换 session token（随渠道一起加密保存） */
async function devinExchange() {
  if (!devinPkce.value) {
    ElMessage.warning('请先点击「开始授权」')
    return
  }
  const code = devinCode.value.trim()
  if (!code) return
  devinExchangeLoading.value = true
  try {
    const r = await upstreamApi.devinPkceExchange({
      code,
      code_verifier: devinPkce.value.code_verifier,
      base_url: picked.value?.base_url || undefined,
    })
    apiKey.value = r.token
    devinCode.value = ''
    devinPkce.value = null
    ElMessage.success('兑换成功，session token 已就绪（接入后加密存储）')
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    devinExchangeLoading.value = false
  }
}

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
    devinPkce.value = null
    devinCode.value = ''
    initDomain(p)
  },
)

function pick(p: Preset) {
  picked.value = p
  initDomain(p)
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
  devinPkce.value = null
  devinCode.value = ''
  initDomain(null)
}

/** 接入：创建上游 + 自动连通测试，成功后自动进入模型发现 */
async function submit() {
  if (!picked.value || submitting.value) return
  if (!name.value.trim()) {
    ElMessage.warning('请输入上游名称')
    return
  }
  if (!isOauth.value && !apiKey.value.trim()) {
    ElMessage.warning('请填写 API Key')
    return
  }
  const wsId = workspaceId.value.trim()
  if (domainMode.value === 'workspace') {
    // 与后端 validate_workspace_id 同口径（1-63、首尾非连字符、仅字母数字连字符）
    if (!/^[A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?$/.test(wsId)) {
      ElMessage.warning('Workspace ID 非法：仅字母、数字与连字符，长度 1-63，首尾不能是连字符')
      return
    }
    if (!region.value) {
      ElMessage.warning('请选择业务空间所在地域')
      return
    }
  }
  submitting.value = true
  conflict.value = false
  try {
    const body: ProvisionReq = {
      api_key: apiKey.value.trim(),
      name: name.value.trim(),
    }
    if (domainChoices.value.length > 1) {
      body.domain = domainMode.value
    }
    if (domainMode.value === 'workspace') {
      body.workspace_id = wsId
      body.region = region.value
    }
    const resp = await presetApi.provision(picked.value.name, body)
    result.value = resp
    step.value = 2
    emit('saved')
    if (resp.test.ok) ElMessage.success('上游已接入，连通正常')
    else ElMessage.warning('上游已创建，但连通测试未通过')
    // OAuth 渠道未授权时不拉模型列表（避免必然失败），待授权后到上游页拉取
    if (!isOauth.value || apiKey.value.trim()) fetchModels()
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
.api-key-block {
  width: 100%;
}
.oauth-state {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 8px 0 6px;
  flex-wrap: wrap;
}
.devin-code {
  width: 260px;
}
.mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
}
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
.domain-hint {
  margin: -6px 0 10px;
  padding: 8px 10px;
  border-radius: 6px;
  background: #f4f8ff;
  color: #4b5563;
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
