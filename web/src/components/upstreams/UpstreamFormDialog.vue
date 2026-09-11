<template>
  <el-dialog
    v-model="visible"
    :title="isEdit ? '编辑上游' : '新建上游'"
    class="dlg-wide long-form"
    :close-on-click-modal="false"
    :before-close="onBeforeClose"
    destroy-on-close
    @closed="onClosed"
  >
    <el-form
      ref="formRef"
      :model="form"
      :rules="rules"
      :label-position="labelPosition"
      :label-width="labelPosition === 'right' ? '120px' : undefined"
      scroll-to-error
      @submit.prevent
      @keyup.enter="onFormEnter"
    >
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
          <el-option v-for="k in UPSTREAM_KINDS" :key="k" :value="k" :label="k">
            <div class="kind-option">
              <span>{{ k }}</span>
              <span v-if="KIND_HINTS[k]" class="kind-hint">{{ KIND_HINTS[k] }}</span>
            </div>
          </el-option>
        </el-select>
      </el-form-item>

      <el-form-item label="Base URL" prop="base_url">
        <el-input v-model="form.base_url" placeholder="如 https://api.openai.com/v1" clearable />
      </el-form-item>

      <el-form-item label="分协议地址">
        <div class="pbu-block">
          <div v-for="proto in pbuProtocols" :key="proto" class="pbu-row">
            <span class="pbu-label">{{ PROTOCOL_LABELS[proto] ?? proto }}</span>
            <el-input
              v-model="form.protocolBaseUrls[proto]"
              :placeholder="`（可选）${PROTOCOL_LABELS[proto] ?? proto} 请求的独立根地址；留空 = 用 Base URL`"
              clearable
            />
          </div>
          <div v-if="!pbuProtocols.length" class="hint">
            先选择协议；同一 Key 下不同协议根地址不同（如 DeepSeek 的 Anthropic 根）时可在此分别覆盖。
          </div>
          <div v-else class="hint">留空即回落 Base URL；同一套 API Key，多协议各自独立地址。</div>
        </div>
      </el-form-item>

      <!-- Codex 渠道：OAuth 凭证（auth.json 粘贴） -->
      <template v-if="isCodex">
        <el-form-item label="OAuth 凭证">
          <div class="api-key-block">
            <div v-if="isEdit && form.hasOauth" class="oauth-state">
              <el-tag type="success" size="small">已授权</el-tag>
              <span class="hint">账号 {{ form.oauthAccountId || '-' }}</span>
              <span class="hint">令牌有效期至 {{ form.oauthExpiresAt ? fmtTime(form.oauthExpiresAt) : '-' }}</span>
              <el-button size="small" :loading="oauthRefreshing" @click="refreshOAuth">手动刷新</el-button>
            </div>
            <el-input
              v-model="form.auth_json"
              type="textarea"
              :rows="5"
              class="mono"
              :placeholder="
                isEdit && form.hasOauth
                  ? '留空保持现有凭证；粘贴新的 auth.json 覆盖'
                  : '粘贴 codex CLI 的 ~/.codex/auth.json 全文（含 tokens.access_token / refresh_token）'
              "
            />
            <el-alert
              v-if="authJsonInvalid"
              type="warning"
              :closable="false"
              show-icon
              class="auth-json-warn"
              title="auth.json 格式异常：无法解析为 JSON 或缺少 tokens 字段，请确认粘贴的是完整文件"
            />
            <div class="hint" style="margin-top: 4px">
              获取方式：本机安装 codex CLI 执行 <span class="mono">codex login</span>
              后复制 ~/.codex/auth.json。凭证加密存储，到期自动刷新；内置授权流程（免 CLI）后续版本提供。
            </div>
          </div>
        </el-form-item>
      </template>
      <el-form-item v-else label="API Key" prop="api_key">
        <div v-if="isEdit" class="api-key-block">
          <div class="api-key-row">
            <el-tag v-if="form.hasApiKey" type="success" size="small" class="api-key-tag">已设置</el-tag>
            <el-tag v-else type="info" size="small" class="api-key-tag">未设置</el-tag>
            <el-input
              v-model="form.api_key"
              type="password"
              show-password
              autocomplete="new-password"
              data-lpignore="true"
              data-1p-ignore
              class="api-key-input"
              :placeholder="form.hasApiKey ? '留空保持不变' : '输入新的 Key'"
              clearable
            />
            <el-tooltip content="勾选后保存将移除已存储的 Key" placement="top">
              <el-checkbox v-model="form.clearKey">清除</el-checkbox>
            </el-tooltip>
          </div>
        </div>
        <el-input
          v-else
          v-model="form.api_key"
          type="password"
          show-password
          autocomplete="new-password"
          data-lpignore="true"
          data-1p-ignore
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

      <template v-if="isEdit">
        <el-divider content-position="left">模型与路由</el-divider>

        <el-form-item label="同步策略">
          <div style="width: 100%">
            <el-radio-group v-model="form.model_sync">
              <el-radio-button value="manual">手动管理</el-radio-button>
              <el-radio-button value="auto">跟随上游自动更新</el-radio-button>
            </el-radio-group>
            <div class="hint" style="margin-top: 6px">
              自动模式：按系统间隔（model_sync.interval_minutes）拉取上游模型列表并全量对账——新模型自动建路由、
              消失模型自动删托管路由；手动路由（含其他渠道）绝不受影响。保存后生效。
            </div>
          </div>
        </el-form-item>

        <el-form-item v-if="form.model_sync === 'auto'" label="排除名单">
          <el-select
            v-model="form.model_exclude"
            multiple
            filterable
            allow-create
            default-first-option
            placeholder="输入模型名回车加入：同步时跳过，已托管的会移除"
            style="width: 100%"
          >
            <el-option v-for="m in form.model_exclude" :key="m" :value="m" :label="m" />
          </el-select>
        </el-form-item>

        <el-form-item label="模型列表">
          <div class="models-block">
            <div class="models-toolbar">
              <el-button size="small" :loading="modelsLoading" @click="fetchModels">拉取模型列表</el-button>
              <el-input
                v-if="modelItems.length"
                v-model="modelSearch"
                size="small"
                placeholder="搜索模型名"
                clearable
                class="model-search"
                :prefix-icon="Search"
                @keyup.enter.stop
              />
              <el-select
                v-if="modelItems.length && !usingCache"
                v-model="modelStatusFilter"
                size="small"
                class="model-filter"
              >
                <el-option label="全部状态" value="all" />
                <el-option label="仅未路由" value="new" />
                <el-option label="已路由（本渠道）" value="routed" />
                <el-option label="占用 / 排除" value="other" />
              </el-select>
              <span v-if="modelItems.length" class="hint">{{ modelStatsText }}</span>
            </div>
            <div class="models-meta">
              <span v-if="modelsFetchedAt && !usingCache" class="hint">最近拉取：{{ fmtTime(modelsFetchedAt) }}</span>
              <span v-if="usingCache" class="hint">以下为缓存列表，点击「拉取模型列表」获取最新状态</span>
            </div>
            <el-alert v-if="modelsError" type="error" :closable="false" :title="modelsError" style="margin-bottom: 8px" />
            <el-checkbox-group v-if="filteredItems.length && !usingCache" v-model="selectedModels">
              <div v-for="it in filteredItems" :key="it.name" class="model-row">
                <el-tooltip :disabled="!rowDisabled(it)" :content="rowDisabledReason(it)" placement="top">
                  <el-checkbox :value="it.name" :disabled="rowDisabled(it)" class="model-check">
                    <span class="model-name">{{ it.name }}</span>
                  </el-checkbox>
                </el-tooltip>
                <el-icon
                  class="model-copy"
                  role="button"
                  aria-label="复制模型名"
                  @click.stop="copyText(it.name, '已复制模型名')"
                >
                  <CopyDocument />
                </el-icon>
                <el-tag size="small" :type="MODEL_TAG[it.status]" effect="plain">{{ MODEL_STATUS[it.status] }}</el-tag>
              </div>
            </el-checkbox-group>
            <div v-else-if="filteredItems.length && usingCache" class="cache-list">
              <el-tag v-for="m in filteredItems" :key="m.name" size="small" effect="plain" style="margin: 2px 4px 2px 0">
                {{ m.name }}
              </el-tag>
            </div>
            <div v-else-if="modelItems.length" class="hint">无匹配「{{ modelSearch }}」的模型。</div>
            <div v-else class="hint">
              尚未拉取；点击「拉取模型列表」查看该渠道可用模型，或在下方手动输入模型 ID。
            </div>
            <div v-if="modelItems.length && !usingCache" class="models-actions">
              <template v-if="form.model_sync === 'auto'">
                <el-button size="small" type="primary" :loading="syncing" @click="syncNow">立即同步（全量对账）</el-button>
              </template>
              <template v-else>
                <el-button size="small" :disabled="!newModelNames.length" @click="selectedModels = [...newModelNames]">
                  全选新模型（{{ newModelNames.length }}）
                </el-button>
                <el-button
                  size="small"
                  type="primary"
                  :disabled="!selectedModels.length"
                  :loading="syncing"
                  @click="addSelected"
                >
                  为选中模型生成路由（{{ selectedModels.length }}）
                </el-button>
              </template>
            </div>
            <div v-if="modelItems.length && !usingCache" class="hint models-instant-hint">
              「生成路由 / 立即同步」立即写入服务端，与底部「保存」无关；「保存」仅提交上方表单配置。
            </div>
            <el-alert
              v-if="syncReport"
              type="success"
              :closable="false"
              style="margin-top: 8px"
              :title="syncReportText"
            />
            <!-- 手动输入模型 ID：无 /models 能力或拉取失败的渠道的兜底入口 -->
            <div class="manual-add">
              <div class="manual-add-head">
                <span class="manual-add-title">手动输入模型 ID</span>
                <span class="hint">拉取失败或上游无模型列表接口时可直接输入；支持逗号 / 空格 / 换行批量粘贴</span>
              </div>
              <div class="manual-add-row">
                <el-input
                  v-model="manualInput"
                  size="small"
                  placeholder="如 gpt-5.2, claude-opus-4-6（回车添加）"
                  clearable
                  @keyup.enter.stop="addManualModels"
                />
                <el-button size="small" :disabled="!manualInput.trim()" @click="addManualModels">添加</el-button>
              </div>
              <div v-if="manualPending.length" class="manual-pending">
                <el-tag
                  v-for="m in manualPending"
                  :key="m"
                  size="small"
                  closable
                  class="manual-tag"
                  @close="removeManual(m)"
                >
                  {{ m }}
                </el-tag>
                <el-button size="small" type="primary" :loading="syncing" @click="submitManual">
                  为以上模型生成路由（{{ manualPending.length }}）
                </el-button>
              </div>
            </div>
          </div>
        </el-form-item>
      </template>
      <el-form-item v-else label="模型与路由">
        <span class="hint">保存后可在编辑页管理：拉取上游模型列表、勾选生成路由或开启自动跟随更新。</span>
      </el-form-item>

      <el-divider content-position="left">运行</el-divider>

      <el-row :gutter="16">
        <el-col :xs="24" :sm="12">
          <el-form-item label="超时（ms）" prop="timeout_ms">
            <el-input-number v-model="form.timeout_ms" :min="0" :step="1000" style="width: 200px" />
          </el-form-item>
        </el-col>
        <el-col :xs="24" :sm="12">
          <el-form-item label="熔断阈值" prop="breaker_threshold">
            <el-input-number v-model="form.breaker_threshold" :min="0" :step="1" style="width: 200px" />
          </el-form-item>
        </el-col>
        <el-col :xs="24" :sm="12">
          <el-form-item label="探测模型" prop="probe_model">
            <el-input v-model="form.probe_model" placeholder="（可选）连通性测试使用的模型名" clearable />
          </el-form-item>
        </el-col>
        <el-col :xs="24" :sm="12">
          <el-form-item label="启用" prop="enabled">
            <el-switch v-model="form.enabled" />
          </el-form-item>
        </el-col>
      </el-row>

      <el-divider content-position="left">代理</el-divider>

      <el-row :gutter="16">
        <el-col :xs="24" :sm="12">
          <el-form-item label="使用代理" prop="use_proxy">
            <el-switch v-model="form.use_proxy" />
          </el-form-item>
        </el-col>
        <el-col :xs="24" :sm="12">
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
        </el-col>
      </el-row>

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
      <el-button @click="onCancel">取消</el-button>
      <el-button type="primary" :loading="saving" @click="submit">保存</el-button>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { fmtTime } from '@/utils/format'
import { ElMessage, ElMessageBox, type FormInstance, type FormRules } from 'element-plus'
import { CopyDocument, Search } from '@element-plus/icons-vue'
import { copyText } from '@/utils/clipboard'
import { useDirtyGuard } from '@/composables/useDirtyGuard'
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
  ModelSyncReport,
  Overrides,
  ProtocolName,
  ProxyOut,
  UpstreamExtra,
  UpstreamIn,
  UpstreamModelItem,
  UpstreamOut,
} from '@/api/types'
import OverridesEditor from './OverridesEditor.vue'
import ProtocolPriorityEditor from './ProtocolPriorityEditor.vue'

defineProps<{ proxies: ProxyOut[] }>()
const emit = defineEmits<{ (e: 'saved'): void }>()

const IMAGE_PROTOCOLS = PROTOCOL_OPTIONS.filter((o) => o.value.startsWith('images_')).map((o) => o.value)

/** Codex 渠道默认值（与后端 upstream/codex.rs 常量一致） */
const CODEX_DEFAULT_BASE_URL = 'https://chatgpt.com/backend-api/codex'

const visible = ref(false)
const saving = ref(false)
const isEdit = ref(false)
const editingId = ref('')
const advancedOpen = ref<string[]>([])
const formRef = ref<FormInstance>()

const MODEL_STATUS: Record<UpstreamModelItem['status'], string> = {
  new: '未路由',
  routed_manual: '已路由·手动',
  routed_auto: '已路由·自动',
  routed_other: '他渠道占用',
  excluded: '已排除',
}
const MODEL_TAG: Record<UpstreamModelItem['status'], 'success' | 'info' | 'warning' | 'danger' | 'primary'> = {
  new: 'info',
  routed_manual: 'success',
  routed_auto: 'primary',
  routed_other: 'warning',
  excluded: 'danger',
}

/** kind 下拉选项的补充说明（仅常见项，未列出的不显示） */
const KIND_HINTS: Record<string, string> = {
  openai_compatible: 'OpenAI 兼容接口（多数国产 / 聚合渠道）',
  codex: 'ChatGPT Codex OAuth 渠道（粘贴 auth.json）',
  custom: '自定义类型标识',
}

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
  model_sync: 'manual' | 'auto'
  model_exclude: string[]
  /** 分协议 base_url 覆盖（proto → url；空串=不覆盖） */
  protocolBaseUrls: Record<string, string>
  /** Codex：auth.json 原文（仅传输用，不展示回读） */
  auth_json: string
  hasOauth: boolean
  oauthAccountId: string
  oauthExpiresAt: string
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
    model_sync: 'manual',
    model_exclude: [],
    protocolBaseUrls: {},
    auth_json: '',
    hasOauth: false,
    oauthAccountId: '',
    oauthExpiresAt: '',
  }
}

const isCodex = computed(() => form.kind === 'codex')
const oauthRefreshing = ref(false)
/** codex 预填提示每次弹窗只提示一次 */
const codexTipShown = ref(false)

/** auth.json 简单预检：非空但无法解析为含 tokens 的 JSON 时提示（不阻断提交） */
const authJsonInvalid = computed(() => {
  const t = form.auth_json.trim()
  if (!t) return false
  try {
    const obj = JSON.parse(t)
    return !obj || typeof obj !== 'object' || !('tokens' in obj)
  } catch {
    return true
  }
})

// ---- 移动端：label 顶部对齐，弹窗可读性更好 ----
const MOBILE_MQ = '(max-width: 768px)'
const isMobile = ref(false)
let mq: MediaQueryList | null = null
const onMq = (e: MediaQueryListEvent) => (isMobile.value = e.matches)
onMounted(() => {
  mq = window.matchMedia(MOBILE_MQ)
  isMobile.value = mq.matches
  mq.addEventListener('change', onMq)
})
onBeforeUnmount(() => mq?.removeEventListener('change', onMq))
const labelPosition = computed<'right' | 'top'>(() => (isMobile.value ? 'top' : 'right'))

// ---- 模型与路由（编辑态） ----
const modelsLoading = ref(false)
const syncing = ref(false)
const modelItems = ref<UpstreamModelItem[]>([])
const selectedModels = ref<string[]>([])
const modelsFetchedAt = ref('')
const modelsError = ref('')
const usingCache = ref(false)
const syncReport = ref<ModelSyncReport | null>(null)
/** 模型名搜索 / 状态筛选 */
const modelSearch = ref('')
const modelStatusFilter = ref<'all' | 'new' | 'routed' | 'other'>('all')
/** 手动输入模型 ID：输入框与待生成路由的暂存列表 */
const manualInput = ref('')
const manualPending = ref<string[]>([])

/** 搜索 + 状态筛选后的展示列表 */
const filteredItems = computed(() => {
  let list = modelItems.value
  const q = modelSearch.value.trim().toLowerCase()
  if (q) list = list.filter((m) => m.name.toLowerCase().includes(q))
  if (!usingCache.value && modelStatusFilter.value !== 'all') {
    list = list.filter((m) => {
      if (modelStatusFilter.value === 'new') return m.status === 'new'
      if (modelStatusFilter.value === 'routed') return m.status === 'routed_manual' || m.status === 'routed_auto'
      return m.status === 'routed_other' || m.status === 'excluded'
    })
  }
  return list
})

const modelStatsText = computed(() => {
  const total = modelItems.value.length
  if (!total) return ''
  if (usingCache.value) return `共 ${total} 个（缓存）`
  const n = modelItems.value.filter((m) => m.status === 'new').length
  return `共 ${total} · 未路由 ${n}`
})

/** 勾选禁用判定与原因（auto 模式全禁，由同步策略管理） */
function rowDisabled(it: UpstreamModelItem): boolean {
  return form.model_sync === 'auto' || it.status !== 'new'
}
function rowDisabledReason(it: UpstreamModelItem): string {
  if (form.model_sync === 'auto') return '自动模式下由同步策略管理，无需勾选'
  const map: Record<UpstreamModelItem['status'], string> = {
    new: '',
    routed_manual: '已有本渠道手动路由',
    routed_auto: '已有本渠道自动托管路由',
    routed_other: '已被其他渠道路由占用',
    excluded: '在排除名单内（同步时跳过）',
  }
  return map[it.status]
}

/** 手动输入解析：逗号/空格/换行/中英文分号分隔；去空白去重，已在列表中的直接勾选或跳过 */
function addManualModels() {
  const parts = manualInput.value
    .split(/[\s,，;；]+/)
    .map((s) => s.trim())
    .filter(Boolean)
  if (!parts.length) return
  let added = 0
  let selected = 0
  let existed = 0
  for (const name of parts) {
    if (manualPending.value.includes(name)) {
      existed++
      continue
    }
    const item = modelItems.value.find((m) => m.name === name)
    if (item && item.status === 'new' && form.model_sync !== 'auto') {
      if (!selectedModels.value.includes(name)) {
        selectedModels.value.push(name)
        selected++
      } else {
        existed++
      }
      continue
    }
    if (item) {
      existed++
      continue
    }
    manualPending.value.push(name)
    added++
  }
  manualInput.value = ''
  const msgs: string[] = []
  if (added) msgs.push(`已添加 ${added} 个待生成`)
  if (selected) msgs.push(`已在列表中勾选 ${selected} 个`)
  if (existed) msgs.push(`${existed} 个已存在或重复，已跳过`)
  if (msgs.length) {
    ElMessage({ type: added || selected ? 'success' : 'info', message: msgs.join('；') })
  }
}

function removeManual(name: string) {
  manualPending.value = manualPending.value.filter((m) => m !== name)
}

/** 手动输入的模型直接生成路由（立即写入服务端） */
async function submitManual() {
  if (!manualPending.value.length || syncing.value) return
  syncing.value = true
  modelsError.value = ''
  try {
    syncReport.value = await upstreamApi.addModelRoutes(editingId.value, [...manualPending.value])
    manualPending.value = []
    await fetchModels()
    emit('saved')
  } catch (e) {
    modelsError.value = errMsg(e)
  } finally {
    syncing.value = false
  }
}

const newModelNames = computed(() =>
  modelItems.value.filter((m) => m.status === 'new').map((m) => m.name),
)

const syncReportText = computed(() => {
  const r = syncReport.value
  if (!r) return ''
  const parts: string[] = []
  if (r.added.length) parts.push(`新增 ${r.added.length}`)
  if (r.removed.length) parts.push(`移除 ${r.removed.length}`)
  parts.push(`保留 ${r.kept}`)
  if (r.excluded) parts.push(`排除 ${r.excluded}`)
  if (r.skipped.length) parts.push(`跳过（已有路由）${r.skipped.length}`)
  return `操作完成：${parts.join(' · ')}`
})

function resetModels(upstream?: UpstreamOut) {
  modelsError.value = ''
  selectedModels.value = []
  syncReport.value = null
  modelsFetchedAt.value = upstream?.models_fetched_at ?? ''
  const cache = upstream?.models_cache ?? []
  if (cache.length) {
    modelItems.value = cache.map((name) => ({ name, status: 'new' as const }))
    usingCache.value = true
  } else {
    modelItems.value = []
    usingCache.value = false
  }
}

async function fetchModels() {
  modelsLoading.value = true
  modelsError.value = ''
  try {
    const resp = await upstreamApi.fetchModels(editingId.value)
    modelItems.value = resp.items
    modelsFetchedAt.value = resp.fetched_at
    usingCache.value = false
    selectedModels.value = []
  } catch (e) {
    modelsError.value = errMsg(e)
  } finally {
    modelsLoading.value = false
  }
}

async function syncNow() {
  if (syncing.value) return
  // 立即同步会先把当前表单的同步策略/排除名单等落库，脏表单时提前明示
  if (dirty.isDirty()) {
    try {
      await ElMessageBox.confirm(
        '立即同步将先保存当前的名称 / Base URL / 同步策略 / 排除名单，再执行全量对账。继续？',
        '立即同步',
        { type: 'info', confirmButtonText: '继续同步', cancelButtonText: '取消' },
      )
    } catch {
      return
    }
  }
  syncing.value = true
  modelsError.value = ''
  try {
    // 先把当前策略/排除落库，再对账（报告基于最新配置）
    await upstreamApi.update(editingId.value, {
      name: form.name.trim(),
      base_url: form.base_url.trim(),
      model_sync: form.model_sync,
      model_exclude: [...form.model_exclude],
    })
    syncReport.value = await upstreamApi.syncModels(editingId.value)
    await fetchModels()
    emit('saved')
  } catch (e) {
    modelsError.value = errMsg(e)
  } finally {
    syncing.value = false
  }
}

async function addSelected() {
  if (!selectedModels.value.length) return
  syncing.value = true
  modelsError.value = ''
  try {
    syncReport.value = await upstreamApi.addModelRoutes(editingId.value, selectedModels.value)
    await fetchModels()
    emit('saved')
  } catch (e) {
    modelsError.value = errMsg(e)
  } finally {
    syncing.value = false
  }
}

const form = reactive<FormState>(defaultForm())
/** 编辑时的原始 extra 快照：buildExtra 在其上合并表单管辖区字段，其余键（如
 *  protocol_base_urls）原样保留，避免保存即丢键 */
const extraSnapshot = ref<UpstreamExtra>({})

/** 脏表单防丢：Esc/X/取消关闭前二次确认（模型区勾选与手动输入也计入未保存意图） */
const dirty = useDirtyGuard(() => ({ form, sel: selectedModels.value, pend: manualPending.value }))

function onBeforeClose(done: () => void) {
  dirty.confirmClose(done)
}

function onCancel() {
  dirty.confirmThen(() => {
    visible.value = false
  })
}

/** 关闭后兜底清理敏感字段（取消路径不经 submit 清理） */
function onClosed() {
  form.api_key = ''
  form.auth_json = ''
}

/** Enter 提交：排除 textarea / 下拉 / 数字步进器内的回车 */
function onFormEnter(e: KeyboardEvent) {
  const t = e.target as HTMLElement | null
  if (!t) return
  if (t.tagName === 'TEXTAREA') return
  if (t.closest('.el-select') || t.closest('.el-input-number')) return
  submit()
}

const standardSelected = computed<ProtocolName[]>(() =>
  STANDARD_PROTOCOLS.filter((p) => form.protocols.includes(p)),
)

/** 分协议地址编辑行：已选标准协议 ∪ 已有覆盖值的协议 */
const pbuProtocols = computed<ProtocolName[]>(() => {
  const set = new Set<ProtocolName>(standardSelected.value)
  for (const k of Object.keys(form.protocolBaseUrls)) {
    if (form.protocolBaseUrls[k]?.trim()) set.add(k as ProtocolName)
  }
  return STANDARD_PROTOCOLS.filter((p) => set.has(p))
})

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

// 切到 codex：预填官方后端地址与 Responses 协议（可改）；切走不回滚用户已改内容
watch(
  () => form.kind,
  (k) => {
    if (k === 'codex') {
      let prefilled = false
      if (!form.base_url.trim() || form.base_url === CODEX_DEFAULT_BASE_URL) {
        if (form.base_url !== CODEX_DEFAULT_BASE_URL) prefilled = true
        form.base_url = CODEX_DEFAULT_BASE_URL
      }
      if (!form.protocols.length) {
        form.protocols = ['openai_responses']
        prefilled = true
      }
      if (prefilled && !codexTipShown.value) {
        codexTipShown.value = true
        ElMessage.info('已预填 Codex 官方后端地址与 Responses 协议，可按需修改')
      }
    }
  },
)

async function refreshOAuth() {
  oauthRefreshing.value = true
  try {
    const r = await upstreamApi.refreshOAuth(editingId.value)
    ElMessage.success(`令牌已刷新（账号 ${r.account_id}）`)
    emit('saved')
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    oauthRefreshing.value = false
  }
}

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
    base.model_sync = upstream.model_sync ?? 'manual'
    base.model_exclude = [...(upstream.model_exclude ?? [])]
    const pbu = upstream.extra?.protocol_base_urls ?? {}
    base.protocolBaseUrls = Object.fromEntries(
      Object.entries(pbu).map(([k, v]) => [k, String(v ?? '')]),
    )
    base.hasOauth = upstream.has_oauth
    base.oauthAccountId = upstream.oauth_account_id ?? ''
    base.oauthExpiresAt = upstream.oauth_expires_at ?? ''
  }
  Object.assign(form, base)
  extraSnapshot.value = upstream?.extra
    ? JSON.parse(JSON.stringify(upstream.extra))
    : {}
  resetModels(upstream)
  // 模型区交互态与提示复位
  modelSearch.value = ''
  modelStatusFilter.value = 'all'
  manualInput.value = ''
  manualPending.value = []
  codexTipShown.value = false
  visible.value = true
  // 回填完成后记录脏检查基线
  dirty.snapshot()
}

function buildExtra(): UpstreamExtra {
  // 以快照为底（保留 protocol_base_urls 等表单未覆盖的键）；表单管辖区以表单为准：
  // overrides / protocol_priority / media_base_url / capabilities 覆盖或清除
  const extra: UpstreamExtra = JSON.parse(JSON.stringify(extraSnapshot.value ?? {}))
  if (Object.keys(form.overrides).length) extra.overrides = form.overrides
  else delete extra.overrides
  const pbu = Object.fromEntries(
    Object.entries(form.protocolBaseUrls)
      .map(([k, v]) => [k, v.trim()] as const)
      .filter(([, v]) => v),
  )
  if (Object.keys(pbu).length) extra.protocol_base_urls = pbu
  else delete extra.protocol_base_urls
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
  } else {
    delete extra.capabilities
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
    model_sync: form.model_sync,
    model_exclude: [...form.model_exclude],
  }
  // Codex：auth_json 掩码语义（未传保持 / 空串清除 / 其他覆盖）
  if (isCodex.value) {
    if (isEdit.value) {
      if (form.auth_json.trim()) body.auth_json = form.auth_json
    } else if (form.auth_json.trim()) {
      body.auth_json = form.auth_json
    }
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
    dirty.disarm()
    visible.value = false
    form.api_key = '' // 敏感字段用完即清（发布审阅前端 M3）
    form.auth_json = ''
    emit('saved')
  } catch (e) {
    ElMessage.error(errMsg(e))
    form.api_key = '' // 失败也清：避免已输入的新密钥长期驻留
    form.auth_json = ''
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
.api-key-row {
  display: flex;
  align-items: center;
  gap: 10px;
  width: 100%;
}
.api-key-tag {
  flex-shrink: 0;
}
.api-key-input {
  flex: 1;
  min-width: 0;
}
.auth-json-warn {
  margin-top: 6px;
}
.kind-option {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}
.kind-hint {
  font-size: 12px;
  color: #9ca3af;
}
.oauth-state {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  margin-bottom: 8px;
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
.models-block {
  width: 100%;
}
.pbu-block {
  width: 100%;
}
.pbu-row {
  display: grid;
  grid-template-columns: 150px 1fr;
  gap: 10px;
  align-items: center;
  margin-bottom: 8px;
}
.pbu-label {
  font-size: 13px;
  color: #4b5563;
  text-align: right;
}
.models-toolbar {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  margin-bottom: 6px;
}
.model-search {
  width: 200px;
}
.model-filter {
  width: 150px;
}
.models-meta {
  margin-bottom: 8px;
  min-height: 18px;
}
.models-instant-hint {
  margin-top: 6px;
}
.model-copy {
  flex-shrink: 0;
  cursor: pointer;
  color: #9ca3af;
  font-size: 13px;
  margin-left: auto;
  margin-right: 6px;
  padding: 2px;
  border-radius: 4px;
}
.model-copy:hover {
  color: #1f2329;
  background: #f3f4f6;
}
.manual-add {
  margin-top: 14px;
  border-top: 1px dashed #e5e7eb;
  padding-top: 10px;
}
.manual-add-head {
  display: flex;
  align-items: baseline;
  gap: 10px;
  flex-wrap: wrap;
  margin-bottom: 8px;
}
.manual-add-title {
  font-size: 13px;
  font-weight: 600;
  color: #1f2329;
}
.manual-add-row {
  display: flex;
  gap: 8px;
}
.manual-pending {
  margin-top: 10px;
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 4px;
}
.manual-tag {
  margin-right: 2px;
}
.model-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 2px 0;
}
.model-check {
  margin-right: 0;
  min-width: 0;
}
.model-name {
  font-family: 'SFMono-Regular', Consolas, Menlo, monospace;
  font-size: 12px;
  word-break: break-all;
}
.models-block .el-checkbox-group {
  max-height: 260px;
  overflow-y: auto;
  border: 1px solid #eceef1;
  border-radius: 6px;
  padding: 6px 10px;
}
.cache-list {
  border: 1px dashed #e5e7eb;
  border-radius: 6px;
  padding: 8px 10px;
}
.models-actions {
  margin-top: 10px;
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}
</style>
