<template>
  <div class="page" v-loading="pageLoading">
    <div class="page-head">
      <p class="page-desc">
        网关 Key 格式为 <span class="mono">sk-nx-...</span>。完整 Key 仅在创建/轮换时展示一次，请立即保存；数据库只存储哈希，之后任何页面均无法再次查看明文。
      </p>
      <div class="page-head-actions">
        <el-button type="primary" :icon="Plus" @click="openCreate">新建密钥</el-button>
        <el-button :icon="Refresh" :loading="pageLoading" @click="loadKeys">刷新</el-button>
      </div>
    </div>

    <div class="toolbar">
      <el-input
        v-model="searchText"
        class="search-input"
        clearable
        :prefix-icon="Search"
        placeholder="按名称 / 前缀搜索"
      />
      <el-select v-model="statusFilter" class="status-filter" aria-label="状态筛选">
        <el-option label="全部状态" value="all" />
        <el-option label="启用中" value="enabled" />
        <el-option label="已停用" value="disabled" />
        <el-option label="已过期" value="expired" />
      </el-select>
    </div>

    <el-card shadow="never" class="section-card">
      <ErrorState v-if="listError" :message="`密钥列表加载失败：${listError}`" @retry="loadKeys" />
      <template v-else>
        <el-table :data="pagedKeys" :row-key="(row: ApiKeyRow) => row.id">
          <template #empty>
            <EmptyState
              v-if="hasFilter"
              title="没有匹配的密钥"
              description="试试清空搜索或切换状态筛选。"
              :icon="Search"
            >
              <template #actions>
                <el-button @click="clearFilters">清空筛选</el-button>
              </template>
            </EmptyState>
            <EmptyState
              v-else
              title="还没有创建密钥"
              description="创建后即可用 sk-nx-... 调用网关；完整 Key 只展示一次，请及时保存。"
              :icon="Key"
            >
              <template #actions>
                <el-button type="primary" :icon="Plus" @click="openCreate">新建密钥</el-button>
              </template>
            </EmptyState>
          </template>

          <!-- 名称：前缀/白名单/限额/到期/调试折到副行，减少桌面列数 -->
          <el-table-column label="名称" :min-width="isNarrow ? 60 : isTablet ? 190 : 240">
            <template #default="{ row }">
              <div class="key-name">
                <span v-if="row.name && row.name.trim()">{{ row.name }}</span>
                <span v-else class="muted">(未命名)</span>
                <el-tag v-if="rotatingId === row.id" size="small" type="info" effect="plain">轮换中</el-tag>
              </div>
              <div class="meta-list key-meta">
                <span class="meta-item mono">{{ row.prefix }}</span>
                <span class="meta-item">{{ modelsSummary(row) }}</span>
                <span v-if="(isNarrow || isTablet) && row.quota_limit != null" class="meta-item">
                  {{ quotaText(row) }}
                </span>
                <span v-if="row.debug_enabled" class="meta-item">调试开启</span>
                <span v-if="row.expires_at" class="meta-item" :class="{ 'is-expired': isExpired(row.expires_at) }">
                  到期 {{ fmtTime(row.expires_at) }}<template v-if="isExpired(row.expires_at)">（已过期）</template>
                </span>
              </div>
              <div v-if="isNarrow" class="key-rate">{{ rateText(row) }}</div>
            </template>
          </el-table-column>

          <el-table-column label="启用" :width="isNarrow ? 70 : isTablet ? 56 : 80" align="center">
            <template #default="{ row }">
              <el-switch
                v-model="row.enabled"
                :loading="togglingSet.has(row.id)"
                :aria-label="`启用密钥 ${row.name || row.prefix}`"
                @change="(val: boolean) => toggleEnabled(row, val)"
              />
            </template>
          </el-table-column>

          <el-table-column v-if="!isNarrow" label="限流" :width="isTablet ? 130 : 150">
            <template #default="{ row }">
              <span>{{ rateText(row) }}</span>
            </template>
          </el-table-column>

          <el-table-column v-if="!isNarrow && !isTablet" label="配额" min-width="170">
            <template #default="{ row }">
              <span v-if="row.quota_limit != null">{{ quotaText(row) }}</span>
              <span v-else>—</span>
            </template>
          </el-table-column>

          <el-table-column v-if="!isNarrow && !isTablet" label="最近使用" width="180">
            <template #default="{ row }">{{ fmtTime(row.last_used_at) }}</template>
          </el-table-column>

          <!-- 操作：桌面编辑 + 更多；平板/手机收进下拉 -->
          <el-table-column
            label="操作"
            :width="isNarrow || isTablet ? 88 : 150"
            :fixed="isNarrow ? false : 'right'"
          >
            <template #default="{ row }">
              <el-dropdown
                v-if="isNarrow || isTablet"
                trigger="click"
                @command="(c: string) => onRowAction(c, row)"
              >
                <el-button size="small">
                  操作<el-icon class="el-icon--right"><ArrowDown /></el-icon>
                </el-button>
                <template #dropdown>
                  <el-dropdown-menu>
                    <el-dropdown-item command="edit">编辑</el-dropdown-item>
                    <el-dropdown-item command="rotate">轮换</el-dropdown-item>
                    <el-dropdown-item command="copy">复制前缀</el-dropdown-item>
                    <el-dropdown-item command="delete" divided style="color: var(--nx-danger)">删除</el-dropdown-item>
                  </el-dropdown-menu>
                </template>
              </el-dropdown>
              <RowActions v-else label="密钥操作">
                <el-button size="small" @click="openEdit(row)">编辑</el-button>
                <el-dropdown trigger="click" @command="(c: string) => onRowAction(c, row)">
                  <el-button size="small">
                    更多<el-icon class="el-icon--right"><ArrowDown /></el-icon>
                  </el-button>
                  <template #dropdown>
                    <el-dropdown-menu>
                      <el-dropdown-item command="rotate">轮换</el-dropdown-item>
                      <el-dropdown-item command="copy">复制前缀</el-dropdown-item>
                      <el-dropdown-item command="delete" divided style="color: var(--nx-danger)">删除</el-dropdown-item>
                    </el-dropdown-menu>
                  </template>
                </el-dropdown>
              </RowActions>
            </template>
          </el-table-column>
        </el-table>

        <TablePager
          v-model:page="currentPage"
          v-model:size="pageSize"
          :total="filteredKeys.length"
          :page-sizes="[10, 20, 50]"
        />
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
import { computed, onMounted, reactive, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage, type FormInstance, type FormRules } from 'element-plus'
import { ArrowDown, CopyDocument, Key, Plus, Refresh, Search } from '@element-plus/icons-vue'
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
import EmptyState from '@/components/common/EmptyState.vue'
import ErrorState from '@/components/common/ErrorState.vue'
import RowActions from '@/components/common/RowActions.vue'
import TablePager from '@/components/common/TablePager.vue'
import { useBreakpoint } from '@/composables/useBreakpoint'
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
type StatusFilter = 'all' | 'enabled' | 'disabled' | 'expired'
const statusFilter = ref<StatusFilter>('all')
const currentPage = ref(1)
const pageSize = ref(10)
/** 断点：桌面完整列 / 平板隐藏配额与最近使用 / 窄屏卡片式三列 */
const { isMobile: isNarrow, isTablet } = useBreakpoint()

/** 窄屏操作列下拉菜单分发（桌面端仍是行内按钮） */
function onRowAction(command: string, row: ApiKeyRow) {
  if (command === 'edit') openEdit(row)
  else if (command === 'rotate') rotateKey(row)
  else if (command === 'copy') copyPrefix(row)
  else if (command === 'delete') removeKey(row)
}

const filteredKeys = computed<ApiKeyRow[]>(() => {
  const q = searchText.value.trim().toLowerCase()
  return keys.value.filter((r) => {
    if (q) {
      const name = (r.name || '').toLowerCase()
      const prefix = (r.prefix || '').toLowerCase()
      if (!name.includes(q) && !prefix.includes(q)) return false
    }
    if (statusFilter.value === 'enabled') return r.enabled
    if (statusFilter.value === 'disabled') return !r.enabled
    if (statusFilter.value === 'expired') return !!r.expires_at && isExpired(r.expires_at)
    return true
  })
})

const hasFilter = computed(() => searchText.value.trim() !== '' || statusFilter.value !== 'all')

function clearFilters() {
  searchText.value = ''
  statusFilter.value = 'all'
}

function modelsSummary(row: ApiKeyRow): string {
  const models = row.models ?? []
  if (!models.length) return '全部模型'
  return models.length > 2 ? `${models.slice(0, 2).join(' · ')} +${models.length - 2}` : models.join(' · ')
}

const pagedKeys = computed<ApiKeyRow[]>(() => {
  const start = (currentPage.value - 1) * pageSize.value
  return filteredKeys.value.slice(start, start + pageSize.value)
})

watch([searchText, statusFilter], () => {
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
.key-name {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
  color: var(--nx-text);
  font-weight: 500;
  line-height: 1.5;
}
.key-meta {
  margin-top: 2px;
}
.key-rate {
  margin-top: 2px;
  color: var(--nx-text-3);
  font-size: var(--nx-font-caption);
}
.key-meta .is-expired {
  color: var(--nx-danger);
}
.status-filter {
  width: 140px;
  max-width: 100%;
}
/* 窄屏折到名称下方的副行（前缀 / 已过期标记） */
@media (max-width: 800px) {
  .status-filter {
    width: 100%;
  }
  .key-meta .meta-item {
    white-space: normal;
  }
}
.rate-row {
  display: flex;
  align-items: center;
  gap: 8px;
}
.rate-label {
  width: 44px;
  color: var(--el-text-color-regular);
  font-size: var(--nx-font-small);
}
.quota-select {
  width: 150px;
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
  background: var(--el-fill-color-light);
  border: 1px solid var(--el-border-color-light);
  border-radius: var(--nx-radius);
  font-size: var(--nx-font-caption);
  line-height: 1.6;
  white-space: pre-wrap;
  word-break: break-all;
}
</style>
