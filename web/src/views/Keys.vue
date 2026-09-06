<template>
  <div class="page" v-loading="pageLoading">
    <div class="toolbar">
      <el-button type="primary" :icon="Plus" @click="openCreate">新建密钥</el-button>
      <el-button :icon="Refresh" @click="loadKeys">刷新</el-button>
      <div class="spacer" />
      <div class="hint" style="max-width: 560px">
        网关 Key 格式为 <span class="mono">sk-nx-...</span>。完整 Key 仅在创建/轮换时展示一次，请立即保存；数据库只存储哈希，之后任何页面均无法再次查看明文。
      </div>
    </div>

    <el-card shadow="never">
      <el-table :data="keys" :row-key="(row: ApiKeyRow) => row.id" border>
        <template #empty>
          <el-empty description="暂无密钥，点击右上角「新建密钥」创建" />
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
            <el-button size="small" type="danger" @click="removeKey(row)">删除</el-button>
          </template>
        </el-table-column>
      </el-table>
    </el-card>

    <!-- 新建 / 编辑弹窗 -->
    <el-dialog
      v-model="dialogVisible"
      :title="dialogTitle"
      width="860px"
      :close-on-click-modal="false"
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
          <el-button type="primary" :icon="CopyDocument" @click="copySecret">一键复制</el-button>
        </div>
      </template>

      <!-- 表单（新建 / 编辑） -->
      <el-form v-else label-width="130px" @submit.prevent>
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
            placeholder="每行一个模型，支持 * 通配；留空 = 全部模型"
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

        <el-form-item label="用量上限">
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
          <el-button type="primary" @click="closeSecret">我已保存</el-button>
        </template>
        <template v-else>
          <el-button @click="dialogVisible = false">取消</el-button>
          <el-button type="primary" :loading="saving" @click="submit">保存</el-button>
        </template>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { CopyDocument, Plus, Refresh } from '@element-plus/icons-vue'
import dayjs from 'dayjs'
import { keyApi } from '@/api'
import { errMsg } from '@/api/http'
import { QUOTA_UNIT_LABELS, QUOTA_WINDOW_LABELS } from '@/utils/consts'
import { fmtInt, fmtTime } from '@/utils/format'
import type { ApiKeyRow, KeyCreateReq, QuotaUnit, QuotaWindow } from '@/api/types'

/** 更新写体：KeyCreateReq 冻结类型未含 enabled，但契约 §4.3 要求行内/编辑启用开关走 PUT enabled */
type KeyWriteReq = KeyCreateReq & { enabled?: boolean }

const keys = ref<ApiKeyRow[]>([])
const pageLoading = ref(false)
const togglingSet = reactive(new Set<string>())
const rotatingId = ref('')

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

async function loadKeys() {
  pageLoading.value = true
  try {
    keys.value = await keyApi.list()
  } catch (e) {
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

async function copyPrefix(row: ApiKeyRow) {
  try {
    await navigator.clipboard.writeText(row.prefix)
    ElMessage.success('已复制前缀')
  } catch {
    ElMessage.error('复制失败')
  }
}

async function removeKey(row: ApiKeyRow) {
  try {
    await ElMessageBox.confirm(`确定删除密钥「${rowName(row)}」？删除后使用该 Key 的请求将失效。`, '删除密钥', {
      type: 'warning',
    })
  } catch {
    return
  }
  try {
    await keyApi.remove(row.id)
    ElMessage.success('已删除')
    loadKeys()
  } catch (e) {
    ElMessage.error(errMsg(e))
  }
}

async function rotateKey(row: ApiKeyRow) {
  try {
    await ElMessageBox.confirm(`确定轮换密钥「${rowName(row)}」？旧 Key 将立即失效。`, '轮换密钥', {
      type: 'warning',
    })
  } catch {
    return
  }
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
}

function onDialogClosed() {
  secretKey.value = ''
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
  if (form.quota_limit != null && (!form.quota_unit || !form.quota_window)) {
    ElMessage.warning('设置用量上限时必须同时选择单位与窗口')
    return
  }
  saving.value = true
  try {
    const body = buildBody()
    if (isEdit.value && editingRow.value) {
      await keyApi.update(editingRow.value.id, body)
      ElMessage.success('密钥已更新')
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

async function copySecret() {
  try {
    await navigator.clipboard.writeText(secretKey.value)
    ElMessage.success('已复制完整 Key')
  } catch {
    ElMessage.error('复制失败')
  }
}
</script>

<style scoped>
.muted {
  color: #c0c4cc;
}
.rate-row {
  display: flex;
  align-items: center;
  gap: 8px;
}
.rate-label {
  width: 44px;
  color: #606266;
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
.secret-key {
  font-size: 16px;
  line-height: 1.5;
  background: #f5f7fa;
  border: 1px solid #e4e7ed;
  border-radius: 6px;
  padding: 12px 16px;
  width: 100%;
  box-sizing: border-box;
  color: #303133;
}
</style>
