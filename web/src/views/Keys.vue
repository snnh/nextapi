<template>
  <div class="page" v-loading="pageLoading">
    <div class="toolbar">
      <el-button type="primary" :icon="Plus" @click="openCreate">新建密钥</el-button>
      <el-button :icon="Refresh" @click="loadKeys">刷新</el-button>
      <el-input v-model="searchText" class="search-input" clearable placeholder="按名称 / 前缀搜索">
        <template #prefix>
          <el-icon><Search /></el-icon>
        </template>
      </el-input>
      <div class="spacer" />
      <div class="hint" style="max-width: 560px">
        网关 Key 格式为 <span class="mono">sk-nx-...</span>。完整 Key 仅在创建/轮换时展示一次，请立即保存；数据库只存储哈希，之后任何页面均无法再次查看明文。
      </div>
    </div>

    <el-card shadow="never">
      <div v-if="listError" class="error-state">
        <p class="error-msg">密钥列表加载失败：{{ listError }}</p>
        <el-button type="primary" :icon="Refresh" @click="loadKeys">重新加载</el-button>
      </div>
      <template v-else>
      <el-table :data="pagedKeys" :row-key="(row: ApiKeyRow) => row.id" border>
        <template #empty>
          <el-empty
            :description="
              searchText.trim()
                ? '未找到与搜索条件匹配的密钥'
                : '暂无密钥，点击右上角「新建密钥」创建'
            "
          />
        </template>

        <el-table-column label="名称" min-width="140">
          <template #default="{ row }">
            <span v-if="row.name && row.name.trim()">{{ row.name }}</span>
            <span v-else class="muted">(未命名)</span>
          </template>
        </el-table-column>

        <el-table-column label="前缀" width="190">
          <template #default="{ row }">
            <el-tooltip content="sk-nx- 前缀，仅识别用" placement="top">
              <span class="mono">{{ row.prefix }}</span>
            </el-tooltip>
          </template>
        </el-table-column>

        <el-table-column label="启用" width="80" align="center">
          <template #default="{ row }">
            <el-switch
              v-model="row.enabled"
              :loading="togglingSet.has(row.id)"
              @change="(val: boolean) => toggleEnabled(row, val)"
            />
          </template>
        </el-table-column>

        <el-table-column label="模型白名单" min-width="220">
          <template #default="{ row }">
            <template v-if="!row.models || row.models.length === 0">
              <el-tag size="small" type="info">全部模型</el-tag>
            </template>
            <template v-else>
              <div class="tag-group">
                <el-tag v-for="(m, i) in row.models.slice(0, 3)" :key="'m' + i" size="small" effect="plain">
                  {{ m }}
                </el-tag>
                <el-tooltip v-if="row.models.length > 3" :content="row.models.join('、')" placement="top">
                  <el-tag size="small" type="info">+{{ row.models.length - 3 }}</el-tag>
                </el-tooltip>
              </div>
            </template>
          </template>
        </el-table-column>

        <el-table-column label="限流" min-width="130">
          <template #default="{ row }">
            <span>{{ rateText(row) }}</span>
          </template>
        </el-table-column>

        <el-table-column label="配额" min-width="170">
          <template #default="{ row }">
            <span v-if="row.quota_limit != null">{{ quotaText(row) }}</span>
            <span v-else>—</span>
          </template>
        </el-table-column>

        <el-table-column label="到期时间" width="190">
          <template #default="{ row }">
            <template v-if="row.expires_at">
              <span>{{ fmtTime(row.expires_at) }}</span>
              <el-tag v-if="isExpired(row.expires_at)" size="small" type="danger" style="margin-left: 6px">已过期</el-tag>
            </template>
            <span v-else>—</span>
          </template>
        </el-table-column>

        <el-table-column label="调试" width="130">
          <template #default="{ row }">
            <template v-if="row.debug_enabled">
              <el-tooltip v-if="row.debug_expires_at" :content="`调试记录到期：${fmtTime(row.debug_expires_at)}`" placement="top">
                <el-tag size="small" type="success">开启</el-tag>
              </el-tooltip>
              <el-tag v-else size="small" type="success">开启</el-tag>
            </template>
            <span v-else>—</span>
          </template>
        </el-table-column>

        <el-table-column label="最近使用" width="180">
          <template #default="{ row }">
            <span>{{ fmtTime(row.last_used_at) }}</span>
          </template>
        </el-table-column>

        <el-table-column label="操作" width="260" fixed="right">
          <template #default="{ row }">
            <el-button size="small" @click="openEdit(row)">编辑</el-button>
            <el-button size="small" :loading="rotatingId === row.id" @click="rotateKey(row)">轮换</el-button>
            <el-tooltip content="完整 Key 仅创建/轮换时展示一次，此处仅复制前缀用于识别" placement="top">
              <el-button size="small" @click="copyPrefix(row)">复制前缀</el-button>
            </el-tooltip>
            <el-button
              size="small"
              type="danger"
              :loading="deletingSet.has(row.id)"
              :disabled="deletingSet.has(row.id)"
              @click="removeKey(row)"
            >
              删除
            </el-button>
          </template>
        </el-table-column>
      </el-table>
      <div class="pager">
        <el-pagination
          v-model:current-page="currentPage"
          v-model:page-size="pageSize"
          :total="filteredKeys.length"
          :page-sizes="[10, 20, 50]"
          :layout="pagerLayout"
          background
        />
      </div>
      </template>
    </el-card>

    <!-- 新建 / 编辑弹窗 -->
    <el-dialog
      v-model="dialogVisible"
      :title="dialogTitle"
      class="dlg-wide long-form"
      :close-on-click-modal="false"
      :close-on-press-escape="!showingSecret"
      :show-close="!showingSecret"
      :before-close="handleBeforeClose"
      destroy-on-close
      @closed="onDialogClosed"
    >
      <!-- 完整 Key 展示（创建/轮换成功） -->
      <template v-if="secretKey">
        <div class="secret-view">
          <el-alert
            type="error"
            :closable="false"
            show-icon
            title="请立即保存完整 Key"
            description="关闭后将无法再次查看完整 Key；数据库仅存储哈希，页面与后端均无法再次取得明文。"
          />
          <div class="secret-key mono">{{ secretKey }}</div>
          <div class="secret-actions">
            <el-button type="primary" :icon="CopyDocument" @click="copySecret">一键复制密钥</el-button>
            <el-button :icon="CopyDocument" @click="copyCurl">复制调用示例</el-button>
          </div>

          <el-divider content-position="left">调用信息</el-divider>
          <el-descriptions :column="1" border size="small" class="secret-desc">
            <el-descriptions-item label="网关地址">
              <CopyText :text="gatewayAddr" />
            </el-descriptions-item>
            <el-descriptions-item label="模型名">
              填写「模型」页中的对外模型名（如 <span class="mono">gpt-4o-mini</span>），别名同样可用
            </el-descriptions-item>
          </el-descriptions>
          <pre class="code">{{ curlExample }}</pre>
          <div class="hint">
            兼容 OpenAI / Anthropic / Gemini 官方 SDK：把 SDK 的 base_url 指向网关地址、api_key 填入本密钥即可。
          </div>
        </div>
      </template>

      <!-- 表单（新建 / 编辑） -->
      <el-form v-else ref="formRef" :model="form" :rules="rules" label-width="130px" @submit.prevent>
        <el-divider content-position="left">基本信息</el-divider>

        <el-form-item label="名称">
          <el-input v-model="form.name" placeholder="密钥名称（可为空，空名显示「(未命名)」）" clearable />
        </el-form-item>

        <el-form-item v-if="isEdit" label="启用">
          <el-switch v-model="form.enabled" />
        </el-form-item>

        <el-divider content-position="left">模型与限流</el-divider>

        <el-form-item label="模型白名单">
          <el-input
            v-model="form.modelsText"
            type="textarea"
            :rows="3"
            :disabled="form.modelsClear"
            :placeholder="
              isEdit
                ? '每行一个模型，支持 * 通配；留空 = 保持现有白名单；勾选下方复选框 = 清除'
                : '每行一个模型，支持 * 通配；留空 = 全部模型'
            "
          />
          <el-checkbox v-if="isEdit" v-model="form.modelsClear">清除模型白名单（恢复全部模型）</el-checkbox>
        </el-form-item>

        <el-form-item label="速率限制">
          <div class="rate-row">
            <span class="rate-label">RPM</span>
            <el-input-number v-model="form.rpm" :min="0" :precision="0" :step="1" :controls="false" placeholder="留空不限" />
          </div>
          <div class="rate-row" style="margin-top: 8px">
            <span class="rate-label">TPM</span>
            <el-input-number v-model="form.tpm" :min="0" :precision="0" :step="1" :controls="false" placeholder="留空不限" />
          </div>
        </el-form-item>

        <el-divider content-position="left">配额与时效</el-divider>

        <el-form-item label="用量上限" prop="quota_limit">
          <div v-if="form.quotaClear" class="hint">已清除（不限），如需重新设置请取消勾选「清除配额」。</div>
          <template v-else>
            <div class="rate-row">
              <el-input-number v-model="form.quota_limit" :min="0" :precision="0" :step="1000" :controls="false" placeholder="留空 = 不限" />
              <el-select v-model="form.quota_unit" class="quota-select" placeholder="单位">
                <el-option v-for="o in QUOTA_UNIT_OPTIONS" :key="o.value" :value="o.value" :label="o.label" />
              </el-select>
              <el-select v-model="form.quota_window" class="quota-select" placeholder="窗口">
                <el-option v-for="o in QUOTA_WINDOW_OPTIONS" :key="o.value" :value="o.value" :label="o.label" />
              </el-select>
            </div>
            <div class="hint">设定了用量上限时必须同时选择单位与窗口；清空上限 = 不限。</div>
          </template>
          <el-checkbox v-if="isEdit" v-model="form.quotaClear">清除配额（不限）</el-checkbox>
        </el-form-item>

        <el-form-item label="到期时间">
          <el-date-picker
            v-model="form.expires_at"
            type="datetime"
            value-format="YYYY-MM-DD HH:mm:ss"
            :disabled="form.expiresClear"
            :disabled-date="disablePastDate"
            placeholder="留空 = 永不过期"
            style="width: 320px"
          />
          <el-checkbox v-if="isEdit" v-model="form.expiresClear">清除到期时间（永不过期）</el-checkbox>
        </el-form-item>

        <el-divider content-position="left">透传授权</el-divider>

        <el-form-item label="允许上游透传">
          <el-switch v-model="form.allow_upstream_passthrough" />
        </el-form-item>

        <el-form-item label="透传上游">
          <el-input
            v-model="form.passthroughText"
            type="textarea"
            :rows="3"
            :disabled="form.passthroughClear"
            placeholder="每行一个上游名；需网关 Key 显式授权的上游名列表，/v1/upstream/{name} 透传用"
          />
          <el-checkbox v-if="isEdit" v-model="form.passthroughClear">清除透传上游</el-checkbox>
        </el-form-item>

        <el-divider content-position="left">调试</el-divider>

        <el-form-item label="调试模式">
          <el-switch v-model="form.debug_enabled" />
        </el-form-item>

        <el-form-item label="调试到期">
          <el-date-picker
            v-model="form.debug_expires_at"
            type="datetime"
            value-format="YYYY-MM-DD HH:mm:ss"
            :disabled-date="disablePastDate"
            placeholder="可清空；到期自动关闭"
            style="width: 320px"
          />
          <div class="hint" style="width: 100%">
            调试模式完整记录请求/响应（脱敏后），最长时间受运行参数 log_debug_ttl_minutes 限制，到时自动关闭。
          </div>
        </el-form-item>
      </el-form>

      <template #footer>
        <template v-if="secretKey">
          <el-button @click="closeSecret">完成</el-button>
          <el-button type="primary" @click="gotoModels">去配置模型</el-button>
        </template>
        <template v-else>
          <el-button @click="handleCancel">取消</el-button>
          <el-button type="primary" :loading="saving" @click="submit">保存</el-button>
        </template>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage, type FormInstance, type FormRules } from 'element-plus'
import { CopyDocument, Plus, Refresh, Search } from '@element-plus/icons-vue'
import dayjs from 'dayjs'
import { keyApi } from '@/api'
import { errMsg } from '@/api/http'
import { useDirtyGuard } from '@/composables/useDirtyGuard'
import { confirmDanger } from '@/utils/confirm'
import { copyText } from '@/utils/clipboard'
import { QUOTA_UNIT_LABELS, QUOTA_WINDOW_LABELS } from '@/utils/consts'
import { gatewayBase } from '@/utils/base'
import { fmtInt, fmtTime } from '@/utils/format'
import CopyText from '@/components/common/CopyText.vue'
import type { ApiKeyRow, KeyCreateReq, QuotaUnit, QuotaWindow } from '@/api/types'

/** 更新写体：KeyCreateReq 冻结类型未含 enabled，但契约 §4.3 要求行内/编辑启用开关走 PUT enabled */
type KeyWriteReq = KeyCreateReq & { enabled?: boolean }

const keys = ref<ApiKeyRow[]>([])
const pageLoading = ref(false)
/** 列表加载失败信息（非空时展示失败态 + 重试，而不是空表格） */
const listError = ref('')
const router = useRouter()
const togglingSet = reactive(new Set<string>())
const deletingSet = reactive(new Set<string>())
const rotatingId = ref('')

// —— 搜索 + 客户端分页 ——
const searchText = ref('')
const currentPage = ref(1)
const pageSize = ref(10)
/** 窄屏（移动端）分页简化：不带每页条数选择 */
const narrowViewport = ref(window.innerWidth < 768)
function onViewportResize() {
  narrowViewport.value = window.innerWidth < 768
}
onMounted(() => window.addEventListener('resize', onViewportResize))
onBeforeUnmount(() => window.removeEventListener('resize', onViewportResize))

const pagerLayout = computed(() =>
  narrowViewport.value ? 'total, prev, pager, next' : 'total, sizes, prev, pager, next',
)

const filteredKeys = computed<ApiKeyRow[]>(() => {
  const q = searchText.value.trim().toLowerCase()
  if (!q) return keys.value
  return keys.value.filter((r) => {
    const name = (r.name || '').toLowerCase()
    const prefix = (r.prefix || '').toLowerCase()
    return name.includes(q) || prefix.includes(q)
  })
})

const pagedKeys = computed<ApiKeyRow[]>(() => {
  const start = (currentPage.value - 1) * pageSize.value
  return filteredKeys.value.slice(start, start + pageSize.value)
})

watch(searchText, () => {
  currentPage.value = 1
})
watch(pageSize, () => {
  currentPage.value = 1
})
// 删除 / 刷新后数据变少时回退页码
watch(filteredKeys, (list) => {
  const maxPage = Math.max(1, Math.ceil(list.length / pageSize.value))
  if (currentPage.value > maxPage) currentPage.value = maxPage
})

// —— 弹窗状态 ——
const dialogVisible = ref(false)
const saving = ref(false)
const isEdit = ref(false)
const editingRow = ref<ApiKeyRow | null>(null)
const secretKey = ref('')

const QUOTA_UNIT_OPTIONS = (Object.keys(QUOTA_UNIT_LABELS) as QuotaUnit[]).map((v) => ({
  value: v,
  label: QUOTA_UNIT_LABELS[v],
}))
const QUOTA_WINDOW_OPTIONS = (Object.keys(QUOTA_WINDOW_LABELS) as QuotaWindow[]).map((v) => ({
  value: v,
  label: QUOTA_WINDOW_LABELS[v],
}))

interface FormState {
  name: string
  enabled: boolean
  modelsText: string
  modelsClear: boolean
  rpm: number | null
  tpm: number | null
  quota_limit: number | null
  quota_unit: QuotaUnit | null
  quota_window: QuotaWindow | null
  quotaClear: boolean
  allow_upstream_passthrough: boolean
  passthroughText: string
  passthroughClear: boolean
  debug_enabled: boolean
  debug_expires_at: string | null
  expires_at: string | null
  expiresClear: boolean
}

function defaultForm(): FormState {
  return {
    name: '',
    enabled: true,
    modelsText: '',
    modelsClear: false,
    rpm: null,
    tpm: null,
    quota_limit: null,
    quota_unit: 'tokens',
    quota_window: 'monthly',
    quotaClear: false,
    allow_upstream_passthrough: false,
    passthroughText: '',
    passthroughClear: false,
    debug_enabled: false,
    debug_expires_at: null,
    expires_at: null,
    expiresClear: false,
  }
}

const form = reactive<FormState>(defaultForm())
const formRef = ref<FormInstance>()

// 用量上限：限值存在时必须同时选择单位与窗口（内联报错定位于该字段）
const rules: FormRules = {
  quota_limit: [
    {
      validator: (_rule, _value, callback) => {
        if (form.quotaClear) {
          callback()
          return
        }
        const limit = form.quota_limit
        if (limit != null && !Number.isNaN(limit) && (!form.quota_unit || !form.quota_window)) {
          callback(new Error('设置用量上限时必须同时选择单位与窗口'))
          return
        }
        callback()
      },
      trigger: 'change',
    },
  ],
}

// —— 脏表单防丢（仅新建/编辑表单态参与；secretKey 展示态通过 props 禁用关闭）——
const { snapshot, disarm, confirmClose, confirmThen } = useDirtyGuard(() => form)

watch(
  () => form.modelsClear,
  (v) => {
    if (v) form.modelsText = ''
  },
)
watch(
  () => form.expiresClear,
  (v) => {
    if (v) form.expires_at = null
  },
)
watch(
  () => form.passthroughClear,
  (v) => {
    if (v) form.passthroughText = ''
  },
)
watch(
  () => form.quotaClear,
  (v) => {
    if (v) {
      form.quota_limit = null
      form.quota_unit = null
      form.quota_window = null
    }
  },
)

const dialogTitle = computed(() => {
  if (secretKey.value) return '完整密钥'
  return isEdit.value ? '编辑密钥' : '新建密钥'
})

/** 展示完整 Key（创建/轮换成功）时禁止 X / Esc 关闭，只能点「我已保存」 */
const showingSecret = computed(() => !!secretKey.value)

async function loadKeys() {
  pageLoading.value = true
  try {
    keys.value = await keyApi.list()
    listError.value = ''
  } catch (e) {
    listError.value = errMsg(e)
    ElMessage.error(errMsg(e))
  } finally {
    pageLoading.value = false
  }
}

onMounted(loadKeys)

// —— 表格列辅助 ——
function rowName(row: ApiKeyRow): string {
  return row.name && row.name.trim() ? row.name.trim() : '(未命名)'
}

function isExpired(iso: string): boolean {
  const d = dayjs(iso)
  return d.isValid() && d.isBefore(dayjs())
}

/** 禁止选择今天之前的日期（编辑回填的既有过去值不受影响，仅限制新选择） */
function disablePastDate(date: Date): boolean {
  const today = new Date()
  today.setHours(0, 0, 0, 0)
  return date.getTime() < today.getTime()
}

function rateText(row: ApiKeyRow): string {
  const parts: string[] = []
  if (row.rpm != null) parts.push(`RPM ${fmtInt(row.rpm)}`)
  if (row.tpm != null) parts.push(`TPM ${fmtInt(row.tpm)}`)
  return parts.length ? parts.join(' · ') : '—'
}

function quotaText(row: ApiKeyRow): string {
  const unit = row.quota_unit ? QUOTA_UNIT_LABELS[row.quota_unit] : ''
  const win = row.quota_window ? QUOTA_WINDOW_LABELS[row.quota_window] : ''
  return `${fmtInt(row.quota_limit)} ${unit} / ${win}`
}

// —— 行操作 ——
async function toggleEnabled(row: ApiKeyRow, val: boolean) {
  togglingSet.add(row.id)
  const body: KeyWriteReq = { enabled: val }
  try {
    const updated = await keyApi.update(row.id, body)
    Object.assign(row, updated)
    ElMessage.success(val ? '已启用' : '已禁用')
  } catch (e) {
    row.enabled = !val
    ElMessage.error(errMsg(e))
  } finally {
    togglingSet.delete(row.id)
  }
}

function parseLines(text: string): string[] {
  return text
    .split(/[\r\n,]+/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0)
}

function copyPrefix(row: ApiKeyRow) {
  copyText(row.prefix, '已复制前缀')
}

async function removeKey(row: ApiKeyRow) {
  if (deletingSet.has(row.id)) return
  const ok = await confirmDanger({
    title: '删除密钥',
    message: `将删除密钥「${rowName(row)}」。删除后使用该 Key 的客户端请求会立即全部失败，且不可恢复（需重新创建 Key）。`,
  })
  if (!ok) return
  deletingSet.add(row.id)
  try {
    await keyApi.remove(row.id)
    ElMessage.success('已删除')
    loadKeys()
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    deletingSet.delete(row.id)
  }
}

async function rotateKey(row: ApiKeyRow) {
  const ok = await confirmDanger({
    title: '轮换密钥',
    message: `将为密钥「${rowName(row)}」生成新 Key，旧 Key 立即失效；请同步更新所有使用该 Key 的客户端。`,
    confirmText: '确认轮换',
  })
  if (!ok) return
  rotatingId.value = row.id
  try {
    const resp = await keyApi.rotate(row.id)
    secretKey.value = resp.key
    isEdit.value = false
    editingRow.value = null
    dialogVisible.value = true
    ElMessage.success('密钥已轮换')
    loadKeys()
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    rotatingId.value = ''
  }
}

// —— 弹窗打开 / 回填 ——
function openCreate() {
  isEdit.value = false
  editingRow.value = null
  secretKey.value = ''
  Object.assign(form, defaultForm())
  dialogVisible.value = true
  snapshot()
}

function openEdit(row: ApiKeyRow) {
  isEdit.value = true
  editingRow.value = row
  secretKey.value = ''
  const base = defaultForm()
  base.name = row.name
  base.enabled = row.enabled
  base.modelsText = row.models?.length ? row.models.join('\n') : ''
  base.rpm = row.rpm
  base.tpm = row.tpm
  base.quota_limit = row.quota_limit
  base.quota_unit = row.quota_unit ?? null
  base.quota_window = row.quota_window ?? null
  base.allow_upstream_passthrough = row.allow_upstream_passthrough
  base.passthroughText = row.passthrough_upstreams?.length ? row.passthrough_upstreams.join('\n') : ''
  base.debug_enabled = row.debug_enabled
  base.debug_expires_at = row.debug_expires_at ? dayjs(row.debug_expires_at).format('YYYY-MM-DD HH:mm:ss') : null
  base.expires_at = row.expires_at ? dayjs(row.expires_at).format('YYYY-MM-DD HH:mm:ss') : null
  Object.assign(form, base)
  dialogVisible.value = true
  snapshot()
}

function onDialogClosed() {
  secretKey.value = ''
}

function handleBeforeClose(done: () => void) {
  // 完整 Key 展示态由 props 禁关，此处兜底直接放行（不参与脏表单守卫）
  if (showingSecret.value) {
    done()
    return
  }
  confirmClose(done)
}

async function handleCancel() {
  await confirmThen(() => {
    dialogVisible.value = false
  })
}

function closeSecret() {
  dialogVisible.value = false
  loadKeys()
}

// —— 提交 ——
function num(v: number | null | undefined): number | null {
  if (v == null || Number.isNaN(v)) return null
  return v
}

function buildBody(): KeyWriteReq {
  const body: KeyWriteReq = {}
  body.name = form.name.trim() || null
  if (isEdit.value) body.enabled = form.enabled

  if (form.modelsClear) {
    body.models = null
  } else {
    const arr = parseLines(form.modelsText)
    if (arr.length) body.models = arr
    else if (editingRow.value?.models?.length) body.models = [...editingRow.value.models]
    else body.models = null
  }

  body.rpm = num(form.rpm)
  body.tpm = num(form.tpm)

  if (form.quotaClear) {
    body.quota_limit = null
    body.quota_unit = null
    body.quota_window = null
  } else if (form.quota_limit != null && !Number.isNaN(form.quota_limit)) {
    body.quota_limit = form.quota_limit
    body.quota_unit = form.quota_unit
    body.quota_window = form.quota_window
  } else if (editingRow.value?.quota_limit != null) {
    body.quota_limit = editingRow.value.quota_limit
    body.quota_unit = editingRow.value.quota_unit
    body.quota_window = editingRow.value.quota_window
  } else {
    body.quota_limit = null
    body.quota_unit = null
    body.quota_window = null
  }

  body.allow_upstream_passthrough = form.allow_upstream_passthrough

  if (form.passthroughClear) {
    body.passthrough_upstreams = null
  } else {
    const arr = parseLines(form.passthroughText)
    if (arr.length) body.passthrough_upstreams = arr
    else if (editingRow.value?.passthrough_upstreams?.length) body.passthrough_upstreams = [...editingRow.value.passthrough_upstreams]
    else body.passthrough_upstreams = null
  }

  body.debug_enabled = form.debug_enabled
  body.debug_expires_at = form.debug_expires_at ? dayjs(form.debug_expires_at).toISOString() : null

  if (form.expiresClear) {
    body.expires_at = null
  } else if (form.expires_at) {
    body.expires_at = dayjs(form.expires_at).toISOString()
  } else if (editingRow.value?.expires_at) {
    body.expires_at = editingRow.value.expires_at
  } else {
    body.expires_at = null
  }

  return body
}

async function submit() {
  if (saving.value) return
  try {
    await formRef.value?.validate()
  } catch {
    return
  }
  saving.value = true
  try {
    const body = buildBody()
    if (isEdit.value && editingRow.value) {
      await keyApi.update(editingRow.value.id, body)
      ElMessage.success('密钥已更新')
      disarm()
      dialogVisible.value = false
    } else {
      const resp = await keyApi.create(body)
      secretKey.value = resp.key
      ElMessage.success('密钥已创建')
    }
    loadKeys()
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    saving.value = false
  }
}

function copySecret() {
  copyText(secretKey.value, '已复制完整 Key')
}

// —— 一次性调用向导（创建/轮换成功后展示） ——
/** 网关调用地址：以浏览器当前 origin + 部署前缀为准（反代/子路径场景下即用户访问地址） */
const gatewayAddr = computed(() => gatewayBase())
const curlExample = computed(() =>
  [
    `curl ${gatewayAddr.value}/chat/completions \\`,
    `  -H "Authorization: Bearer ${secretKey.value}" \\`,
    `  -H "Content-Type: application/json" \\`,
    `  -d '{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}]}'`,
  ].join('\n'),
)

function copyCurl() {
  copyText(curlExample.value, '已复制调用示例')
}

/** 调用向导「去配置模型」：关闭弹窗并跳转模型页 */
function gotoModels() {
  dialogVisible.value = false
  router.push({ name: 'routes' })
}
</script>

<style scoped>
.muted {
  color: #c0c4cc;
}
.search-input {
  width: 240px;
}
.pager {
  display: flex;
  justify-content: flex-end;
  margin-top: 12px;
}
.error-state {
  padding: 30px 16px;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 14px;
}
.error-msg {
  margin: 0;
  color: #f56c6c;
  font-size: 13px;
  text-align: center;
  word-break: break-all;
}
.rate-row {
  display: flex;
  align-items: center;
  gap: 8px;
}
.rate-label {
  width: 44px;
  color: #4b5563;
  font-size: 13px;
}
.quota-select {
  width: 150px;
}
.secret-view {
  display: flex;
  flex-direction: column;
  gap: 16px;
  align-items: flex-start;
}
.secret-actions {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}
.secret-desc {
  width: 100%;
}
.code {
  width: 100%;
  box-sizing: border-box;
  margin: 0;
  padding: 10px 12px;
  background: #f7f8fa;
  border: 1px solid #e8eaee;
  border-radius: 6px;
  font-size: 12px;
  line-height: 1.6;
  white-space: pre-wrap;
  word-break: break-all;
}
.hint {
  color: #6b7280;
  font-size: 12px;
  line-height: 1.7;
}
.secret-key {
  font-size: 16px;
  line-height: 1.5;
  background: #f5f7fa;
  border: 1px solid #e4e7ed;
  border-radius: 6px;
  padding: 12px 16px;
  width: 100%;
  box-sizing: border-box;
  color: #1f2329;
}
</style>
