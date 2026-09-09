<template>
  <div class="page" v-loading="pageLoading">
    <div class="page-intro">
      <div>
        <h2>模型</h2>
        <p>按对外模型名查看可调用情况（上游、价格、别名、健康度），或切换到路由视图编辑转发规则。</p>
      </div>
      <div class="intro-summary">
        <span><b>{{ uniqueModelCount }}</b> 个模型</span>
        <span><b>{{ drafts.length }}</b> 条路由</span>
        <span><b>{{ enabledCount }}</b> 条启用</span>
      </div>
    </div>

    <div class="toolbar">
      <el-radio-group v-model="viewMode">
        <el-radio-button value="models">模型视图</el-radio-button>
        <el-radio-button value="routes">路由视图</el-radio-button>
      </el-radio-group>

      <template v-if="viewMode === 'models'">
        <el-input
          v-model="modelKeyword"
          placeholder="搜索模型名 / 上游 / 别名"
          clearable
          :prefix-icon="Search"
          style="width: 260px"
        />
        <el-button :icon="Refresh" @click="refresh">刷新</el-button>
        <div class="spacer" />
        <div class="hint" v-if="unpricedModelCount > 0">
          有 {{ unpricedModelCount }} 个模型未配置价格（仍可调用，但不计成本）
        </div>
        <template v-if="dirtyCount > 0">
          <div class="hint">有 {{ dirtyCount }} 条修改未保存</div>
          <el-button @click="discardLocal">放弃修改</el-button>
          <el-button type="primary" :loading="saving" @click="saveAll">保存全部</el-button>
        </template>
        <el-button v-else type="primary" :icon="Plus" @click="openCreate">新增路由</el-button>
      </template>

      <template v-else>
        <el-button type="primary" :icon="Plus" @click="openCreate">新增规则</el-button>
        <el-button :icon="Refresh" @click="refresh">刷新</el-button>
        <el-button @click="discardLocal">放弃本地修改</el-button>
        <div class="spacer" />
        <div class="hint" v-if="dirtyCount > 0">有 {{ dirtyCount }} 条修改未保存</div>
        <el-button type="primary" :loading="saving" @click="saveAll">
          保存全部（{{ drafts.length }} 条）
        </el-button>
      </template>
    </div>

    <el-alert
      v-if="viewMode === 'routes'"
      type="info"
      :closable="false"
      show-icon
      style="margin-bottom: 16px"
    >
      <template #title>模型 → 上游绑定（模型名支持 <span class="mono">*</span> 通配）</template>
      <div class="hint">
        <div>· 优先级数字小者优先；同优先级按权重加权。</div>
        <div>· 「保存全部」将整体替换后端路由表（PUT 裸数组）。</div>
        <div>· 重试 / 熔断参数只在路由表维护。</div>
        <div>· 锁定上游后，失败时不会自动故障转移。</div>
      </div>
    </el-alert>

    <!-- ===== 模型视图：按对外模型名聚合 ===== -->
    <el-card v-if="viewMode === 'models'" shadow="never">
      <el-table :data="filteredOverviewRows" border v-loading="pageLoading">
        <template #empty>
          <el-empty :description="modelsEmptyText">
            <el-button type="primary" :icon="Plus" @click="openCreate">新增路由</el-button>
          </el-empty>
        </template>

        <el-table-column label="对外模型名" min-width="230" show-overflow-tooltip>
          <template #default="{ row }">
            <div class="cell-inline">
              <CopyText :text="row.model" :truncate="28" />
              <el-tag v-if="row.wildcard" size="small" type="warning" effect="plain">通配</el-tag>
              <el-tag v-if="row.autoManaged" size="small" type="primary" effect="plain">自动</el-tag>
            </div>
          </template>
        </el-table-column>

        <el-table-column label="上游" min-width="210">
          <template #default="{ row }">
            <div class="tag-group">
              <el-tag
                v-for="n in row.upstreamNames.slice(0, 2)"
                :key="n"
                size="small"
                effect="plain"
              >
                {{ n }}
              </el-tag>
              <el-tooltip
                v-if="row.upstreamNames.length > 2"
                :content="row.upstreamNames.join('、')"
                placement="top"
              >
                <el-tag size="small" type="info" effect="plain">
                  +{{ row.upstreamNames.length - 2 }}
                </el-tag>
              </el-tooltip>
            </div>
          </template>
        </el-table-column>

        <el-table-column label="优先级" width="90" align="center">
          <template #default="{ row }">
            <span v-if="row.minPriority != null">{{ row.minPriority }}</span>
            <span v-else class="muted">—</span>
          </template>
        </el-table-column>

        <el-table-column label="路由" width="96" align="center">
          <template #default="{ row }">
            <span :class="{ 'text-warn': row.enabledCount === 0 }">
              {{ row.enabledCount }}/{{ row.totalCount }}
            </span>
          </template>
        </el-table-column>

        <el-table-column label="价格" width="118" align="center">
          <template #default="{ row }">
            <el-tooltip :content="priceTooltip(row)" placement="top">
              <el-tag :type="priceTagType(row.priceState)" size="small" effect="plain">
                {{ priceLabel(row.priceState) }}
              </el-tag>
            </el-tooltip>
          </template>
        </el-table-column>

        <el-table-column label="别名" width="80" align="center">
          <template #default="{ row }">
            <span v-if="row.aliases.length">{{ row.aliases.length }}</span>
            <span v-else class="muted">—</span>
          </template>
        </el-table-column>

        <el-table-column label="状态" width="120" align="center">
          <template #default="{ row }">
            <el-tooltip :content="row.healthText" placement="top">
              <el-tag :type="healthTagType(row.health)" size="small" effect="plain">
                {{ healthLabel(row.health) }}
              </el-tag>
            </el-tooltip>
          </template>
        </el-table-column>

        <el-table-column label="操作" width="90" fixed="right" align="center">
          <template #default="{ row }">
            <el-button link type="primary" @click="openDetail(row)">详情</el-button>
          </template>
        </el-table-column>
      </el-table>
    </el-card>

    <!-- ===== 路由视图：现有编辑表格 ===== -->
    <el-card v-else shadow="never">
      <el-table :data="drafts" border>
        <template #empty>
          <el-empty description="暂无模型路由，点击左上角「新增规则」创建" />
        </template>

        <el-table-column label="模型模式" min-width="220" show-overflow-tooltip>
          <template #default="{ row }">
            <div class="cell-inline">
              <CopyText :text="row.model_pattern" :truncate="24" />
              <el-tooltip
                v-if="row.managed_by === 'auto'"
                content="由渠道模型同步托管：自动跟随上游模型列表增删；手动编辑请先关闭该渠道的自动跟随"
                placement="top"
              >
                <el-tag size="small" type="primary" effect="plain">自动</el-tag>
              </el-tooltip>
              <el-tag v-if="isRowDirty(row)" size="small" type="warning" effect="plain">未保存</el-tag>
            </div>
          </template>
        </el-table-column>

        <el-table-column label="上游" min-width="180" show-overflow-tooltip>
          <template #default="{ row }">
            <CopyText :text="row.upstreamName" :truncate="24" />
          </template>
        </el-table-column>

        <el-table-column label="覆盖模型名" min-width="140" show-overflow-tooltip>
          <template #default="{ row }">
            <span v-if="row.override_model">{{ row.override_model }}</span>
            <span v-else class="muted">—</span>
          </template>
        </el-table-column>

        <el-table-column prop="priority" label="优先级" width="90" align="center" />
        <el-table-column prop="weight" label="权重" width="80" align="center" />

        <el-table-column label="启用" width="80" align="center">
          <template #default="{ row }">
            <el-tag :type="row.enabled ? 'success' : 'info'" size="small">
              {{ row.enabled ? '启用' : '禁用' }}
            </el-tag>
          </template>
        </el-table-column>

        <el-table-column label="重试次数" width="90" align="center">
          <template #default="{ row }">{{ row.retries }}</template>
        </el-table-column>

        <el-table-column label="重试状态码" min-width="180">
          <template #default="{ row }">
            <div v-if="row.retry_status_codes && row.retry_status_codes.length" class="tag-group">
              <el-tag
                v-for="c in row.retry_status_codes"
                :key="c"
                size="small"
                type="warning"
                effect="plain"
              >
                {{ c }}
              </el-tag>
            </div>
            <span v-else class="muted">默认</span>
          </template>
        </el-table-column>

        <el-table-column label="锁定上游" width="90" align="center">
          <template #default="{ row }">
            <el-tag v-if="row.lock_upstream" size="small" type="warning">锁定</el-tag>
            <span v-else class="muted">—</span>
          </template>
        </el-table-column>

        <el-table-column label="排序" width="70" align="center">
          <template #default="{ row }">{{ row.sort_order }}</template>
        </el-table-column>

        <el-table-column label="更新时间" width="180">
          <template #default="{ row }">
            <span>{{ fmtTime(row.updatedAt) }}</span>
          </template>
        </el-table-column>

        <el-table-column label="操作" width="150" fixed="right">
          <template #default="{ $index }">
            <el-button size="small" @click="openEdit($index)">编辑</el-button>
            <el-button size="small" type="danger" @click="removeRow($index)">删除</el-button>
          </template>
        </el-table-column>
      </el-table>
    </el-card>

    <!-- 新增 / 编辑弹窗 -->
    <el-dialog
      v-model="dialogVisible"
      :title="dialogTitle"
      class="dlg"
      :close-on-click-modal="false"
      destroy-on-close
      :before-close="(done: () => void) => dirtyGuard.confirmClose(done)"
    >
      <el-form ref="formRef" :model="form" :rules="rules" label-width="120px" @submit.prevent>
        <el-divider content-position="left">基础</el-divider>

        <el-form-item label="模型模式" prop="model_pattern">
          <el-input
            v-model="form.model_pattern"
            placeholder="支持 * 通配，如 gpt-4*"
            clearable
          />
        </el-form-item>

        <el-form-item label="上游" prop="upstream_id">
          <el-select
            v-model="form.upstream_id"
            filterable
            placeholder="请选择上游"
            style="width: 100%"
          >
            <el-option v-for="u in upstreams" :key="u.id" :value="u.id" :label="u.name" />
          </el-select>
        </el-form-item>

        <el-form-item label="覆盖模型名" prop="override_model">
          <el-input
            v-model="form.override_model"
            placeholder="留空=不覆盖，透传时使用原模型名"
            clearable
          />
        </el-form-item>

        <el-divider content-position="left">路由参数</el-divider>

        <el-form-item label="优先级" prop="priority">
          <el-input-number v-model="form.priority" :min="0" :step="1" :precision="0" style="width: 200px" />
          <div class="hint" style="width: 100%">数字小者优先；同优先级按权重加权。</div>
        </el-form-item>

        <el-form-item label="权重" prop="weight">
          <el-input-number v-model="form.weight" :min="1" :step="1" :precision="0" style="width: 200px" />
        </el-form-item>

        <el-form-item label="启用" prop="enabled">
          <el-switch v-model="form.enabled" />
        </el-form-item>

        <el-form-item label="排序" prop="sort_order">
          <el-input-number v-model="form.sort_order" :min="0" :step="1" :precision="0" style="width: 200px" />
        </el-form-item>

        <el-divider content-position="left">重试 / 熔断</el-divider>

        <el-form-item label="重试次数" prop="retries">
          <el-input-number v-model="form.retries" :min="0" :step="1" :precision="0" style="width: 200px" />
        </el-form-item>

        <el-form-item label="重试状态码" prop="retry_status_codes">
          <el-input
            v-model="form.retry_status_codes"
            placeholder="逗号分隔，留空=默认 429,500,502,503,504"
            clearable
            @blur="warnIgnoredStatusCodes"
          />
        </el-form-item>

        <el-form-item label="锁定上游" prop="lock_upstream">
          <el-switch v-model="form.lock_upstream" />
          <div class="hint" style="width: 100%">锁定后失败不自动故障转移。</div>
        </el-form-item>
      </el-form>

      <template #footer>
        <el-button @click="onCancelDialog">取消</el-button>
        <el-button type="primary" :loading="saving" @click="submitDialog">保存</el-button>
      </template>
    </el-dialog>

    <!-- 模型详情抽屉（模型视图） -->
    <ModelDetailDrawer
      v-model:visible="detailVisible"
      :row="detailRow"
      :upstreams="upstreams"
      @edit-route="onEditRoute"
      @add-route="onAddRoute"
      @goto-pricing="gotoPricing"
      @goto-aliases="gotoAliases"
    />
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { useRouter } from 'vue-router'
import { ElMessage, ElMessageBox, type FormInstance, type FormRules } from 'element-plus'
import { Plus, Refresh, Search } from '@element-plus/icons-vue'
import { aliasApi, pricingApi, routeApi, upstreamApi } from '@/api'
import { errMsg } from '@/api/http'
import { fmtTime } from '@/utils/format'
import CopyText from '@/components/common/CopyText.vue'
import ModelDetailDrawer from '@/components/models/ModelDetailDrawer.vue'
import { useDirtyGuard } from '@/composables/useDirtyGuard'
import {
  buildModelOverview,
  type ModelHealth,
  type ModelOverviewRow,
  type ModelPriceState,
} from '@/composables/useModelOverview'
import type { ModelAliasRow, RouteItem, RouteOut, RuleItem, UpstreamOut } from '@/api/types'

/** 默认重试状态码（可空=默认此列表） */
const DEFAULT_RETRY_CODES = [429, 500, 502, 503, 504]

/** 本地草稿行：可编辑字段取自 RouteItem；展示字段（上游名、时间、id）随行维护 */
interface RouteRow extends RouteItem {
  id: string | null
  upstreamName: string
  createdAt: string | null
  updatedAt: string | null
}

// —— 状态 ——
const pageLoading = ref(false)
const saving = ref(false)
const upstreams = ref<UpstreamOut[]>([])
/** 价格规则（模型视图判定「是否已计价」） */
const prices = ref<RuleItem[]>([])
/** 模型别名（模型视图展示别名关系） */
const aliases = ref<ModelAliasRow[]>([])
/** 最近一次后端提交结果（用于 diff / 放弃本地修改） */
const serverCommit = ref<RouteOut[]>([])
/** 本地草稿（可编辑工作副本） */
const drafts = ref<RouteRow[]>([])

// —— 模型视图状态 ——
const router = useRouter()
const viewMode = ref<'models' | 'routes'>('models')
const modelKeyword = ref('')
const detailVisible = ref(false)
const detailRow = ref<ModelOverviewRow | null>(null)

const enabledCount = computed(() => drafts.value.filter((r) => r.enabled).length)
const uniqueModelCount = computed(() => new Set(drafts.value.map((r) => r.model_pattern)).size)

/** 草稿 → 聚合视图输入（未保存草稿用合成 id，保证逐行价格判定键稳定） */
function overviewRoutes(): RouteOut[] {
  return drafts.value.map((r, i) => ({
    id: r.id ?? `draft-${i}`,
    model_pattern: r.model_pattern,
    upstream_id: r.upstream_id,
    upstream_name: r.upstreamName,
    override_model: r.override_model ?? null,
    priority: r.priority ?? 10,
    weight: r.weight ?? 1,
    enabled: r.enabled ?? true,
    retries: r.retries ?? 2,
    retry_status_codes: r.retry_status_codes ?? [...DEFAULT_RETRY_CODES],
    lock_upstream: r.lock_upstream ?? false,
    sort_order: r.sort_order ?? 0,
    managed_by: r.managed_by ?? null,
    created_at: r.createdAt ?? '',
    updated_at: r.updatedAt ?? '',
  }))
}

/** 模型视图行（基于本地草稿实时聚合，未保存改动也能看到） */
const overviewRows = computed(() =>
  buildModelOverview(overviewRoutes(), upstreams.value, prices.value, aliases.value),
)

const filteredOverviewRows = computed(() => {
  const kw = modelKeyword.value.trim().toLowerCase()
  if (!kw) return overviewRows.value
  return overviewRows.value.filter(
    (r) =>
      r.model.toLowerCase().includes(kw) ||
      r.upstreamNames.some((n) => n.toLowerCase().includes(kw)) ||
      r.aliases.some((a) => a.alias.toLowerCase().includes(kw)),
  )
})

const modelsEmptyText = computed(() =>
  modelKeyword.value.trim() ? '未找到匹配的模型，请调整搜索关键词' : '暂无模型路由，点击「新增路由」创建',
)

/** 未配 / 部分配价格的模型数（用于工具栏提示） */
const unpricedModelCount = computed(
  () =>
    overviewRows.value.filter((r) => r.priceState === 'unpriced' || r.priceState === 'partial')
      .length,
)

function priceLabel(s: ModelPriceState): string {
  switch (s) {
    case 'priced':
      return '已配置'
    case 'partial':
      return '部分配置'
    case 'unpriced':
      return '未配置'
    default:
      return '待定'
  }
}

function priceTagType(s: ModelPriceState): 'success' | 'warning' | 'info' {
  if (s === 'priced') return 'success'
  if (s === 'unknown') return 'info'
  return 'warning'
}

function priceTooltip(row: ModelOverviewRow): string {
  if (row.priceState === 'unknown') {
    return '通配路由未设置覆盖模型名，无法逐条判定价格；请为实际模型配置价格'
  }
  if (row.priceState === 'priced') return '该模型下所有路由均已配置价格'
  return `已配置 ${row.pricedRouteCount}/${row.determinateRouteCount} 条路由；未配置仍可调用但不计成本`
}

function healthLabel(h: ModelHealth): string {
  return h === 'ok' ? '正常' : h === 'warn' ? '部分异常' : '异常'
}

function healthTagType(h: ModelHealth): 'success' | 'warning' | 'danger' {
  return h === 'ok' ? 'success' : h === 'warn' ? 'warning' : 'danger'
}

/** 打开模型详情抽屉 */
function openDetail(row: ModelOverviewRow) {
  detailRow.value = row
  detailVisible.value = true
}

/** 详情内编辑某条路由：切回路由视图并打开该行编辑弹窗 */
function onEditRoute(route: RouteOut) {
  const idx = drafts.value.findIndex((r, i) => (r.id ?? `draft-${i}`) === route.id)
  if (idx < 0) return
  viewMode.value = 'routes'
  detailVisible.value = false
  openEdit(idx)
}

/** 详情内为该模型新增路由：切回路由视图并预填模型名 */
function onAddRoute(model: string) {
  viewMode.value = 'routes'
  detailVisible.value = false
  openCreate()
  form.model_pattern = model
}

function gotoPricing(model: string) {
  router.push({ name: 'pricing', query: { model } })
}

function gotoAliases(model: string) {
  router.push({ name: 'aliases', query: { model } })
}

// —— 弹窗状态 ——
const dialogVisible = ref(false)
const dialogIndex = ref(-1)
const isEdit = ref(false)

interface FormState {
  model_pattern: string
  upstream_id: string
  override_model: string
  priority: number
  weight: number
  enabled: boolean
  retries: number
  retry_status_codes: string
  lock_upstream: boolean
  sort_order: number
}

function defaultForm(): FormState {
  return {
    model_pattern: '',
    upstream_id: '',
    override_model: '',
    priority: 10,
    weight: 1,
    enabled: true,
    retries: 2,
    retry_status_codes: '',
    lock_upstream: false,
    sort_order: 0,
  }
}

const form = reactive<FormState>(defaultForm())
const formRef = ref<FormInstance>()

const rules: FormRules = {
  model_pattern: [{ required: true, message: '请输入模型模式', trigger: 'blur' }],
  upstream_id: [{ required: true, message: '请选择上游', trigger: 'change' }],
  weight: [
    {
      validator: (_r, v: number, cb) => {
        if (v == null || !(v >= 1)) cb(new Error('权重需 ≥ 1'))
        else cb()
      },
      trigger: 'change',
    },
  ],
}

const dialogTitle = computed(() => (isEdit.value ? '编辑规则' : '新增规则'))

/** 脏表单守卫：弹窗内有未保存修改时关闭前二次确认 */
const dirtyGuard = useDirtyGuard(() => form)

const dirtyCount = computed(() => drafts.value.filter((r) => isRowDirty(r)).length)

// —— 数据加载 ——
async function loadData() {
  pageLoading.value = true
  const [routesRes, upstreamsRes, pricesRes, aliasesRes] = await Promise.allSettled([
    routeApi.list(),
    upstreamApi.list(),
    pricingApi.list(),
    aliasApi.list(),
  ])
  if (routesRes.status === 'fulfilled') {
    const routes = routesRes.value
    serverCommit.value = routes
    drafts.value = routes.map((r) => ({
      id: r.id,
      model_pattern: r.model_pattern,
      upstream_id: r.upstream_id,
      override_model: r.override_model,
      priority: r.priority,
      weight: r.weight,
      enabled: r.enabled,
      retries: r.retries,
      retry_status_codes: [...r.retry_status_codes],
      lock_upstream: r.lock_upstream,
      sort_order: r.sort_order,
      managed_by: r.managed_by,
      upstreamName: r.upstream_name,
      createdAt: r.created_at,
      updatedAt: r.updated_at,
    }))
  } else {
    ElMessage.error(errMsg(routesRes.reason))
  }
  if (upstreamsRes.status === 'fulfilled') upstreams.value = upstreamsRes.value
  else ElMessage.error(errMsg(upstreamsRes.reason))
  if (pricesRes.status === 'fulfilled') prices.value = pricesRes.value
  else ElMessage.error(errMsg(pricesRes.reason))
  if (aliasesRes.status === 'fulfilled') aliases.value = aliasesRes.value
  else ElMessage.error(errMsg(aliasesRes.reason))
  pageLoading.value = false
}

onMounted(loadData)

// —— 弹窗打开 / 回填 ——
function openCreate() {
  isEdit.value = false
  dialogIndex.value = -1
  Object.assign(form, defaultForm())
  dirtyGuard.snapshot()
  dialogVisible.value = true
}

function openEdit(index: number) {
  const row = drafts.value[index]
  if (!row) return
  isEdit.value = true
  dialogIndex.value = index
  Object.assign(form, {
    model_pattern: row.model_pattern,
    upstream_id: row.upstream_id,
    override_model: row.override_model ?? '',
    priority: row.priority ?? 10,
    weight: row.weight ?? 1,
    enabled: row.enabled ?? true,
    retries: row.retries ?? 2,
    retry_status_codes: codesToText(row.retry_status_codes),
    lock_upstream: row.lock_upstream ?? false,
    sort_order: row.sort_order ?? 0,
  })
  dirtyGuard.snapshot()
  dialogVisible.value = true
}

function codesToText(codes?: number[] | null): string {
  return (codes ?? []).join(',')
}

function parseCodes(text: string): number[] {
  const parts = text
    .split(/[,，\s]+/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0)
  if (!parts.length) return [...DEFAULT_RETRY_CODES]
  return parts.map((s) => Number(s)).filter((n) => !Number.isNaN(n))
}

/** 提取输入中会被 parseCodes 静默丢弃的非数字项（用于失焦即时提示，不阻断合法项） */
function ignoredStatusCodes(text: string): string[] {
  const parts = text
    .split(/[,，\s]+/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0)
  if (!parts.length) return []
  return parts.filter((s) => Number.isNaN(Number(s)))
}

/** 输入框失焦时：把被丢弃的非法状态码项提示出来 */
function warnIgnoredStatusCodes() {
  const bad = ignoredStatusCodes(form.retry_status_codes)
  if (bad.length) {
    ElMessage.warning(`已忽略非法状态码：${bad.join('、')}`)
  }
}

function upstreamNameOf(id: string): string {
  const u = upstreams.value.find((x) => x.id === id)
  if (u) return u.name
  const c = serverCommit.value.find((x) => x.upstream_id === id)
  return c ? c.upstream_name : id
}

function formToFields(f: FormState): RouteItem {
  return {
    model_pattern: f.model_pattern.trim(),
    upstream_id: f.upstream_id,
    override_model: f.override_model.trim() || null,
    priority: f.priority,
    weight: f.weight,
    enabled: f.enabled,
    retries: f.retries,
    retry_status_codes: parseCodes(f.retry_status_codes),
    lock_upstream: f.lock_upstream,
    sort_order: f.sort_order,
  }
}

async function submitDialog() {
  if (!formRef.value || saving.value) return
  try {
    await formRef.value.validate()
  } catch {
    return
  }
  const fields = formToFields(form)
  if (isEdit.value && dialogIndex.value >= 0) {
    const prev = drafts.value[dialogIndex.value]
    drafts.value[dialogIndex.value] = {
      ...fields,
      id: prev?.id ?? null,
      upstreamName: upstreamNameOf(fields.upstream_id),
      createdAt: prev?.createdAt ?? null,
      updatedAt: prev?.updatedAt ?? null,
    }
  } else {
    drafts.value.push({
      ...fields,
      id: null,
      upstreamName: upstreamNameOf(fields.upstream_id),
      createdAt: null,
      updatedAt: null,
    })
  }
  dirtyGuard.disarm()
  dialogVisible.value = false
  ElMessage.info('已加入草稿，需点击顶部「保存全部」生效')
}

/** 弹窗取消按钮：脏表单先二次确认再关闭 */
function onCancelDialog() {
  dirtyGuard.confirmThen(() => {
    dialogVisible.value = false
  })
}

// —— 删除 ——
async function removeRow(index: number) {
  const row = drafts.value[index]
  if (!row) return
  try {
    await ElMessageBox.confirm(
      `确定删除规则「${row.model_pattern}」？点击「保存全部」后生效。`,
      '删除规则',
      { type: 'warning' },
    )
  } catch {
    return
  }
  drafts.value.splice(index, 1)
  ElMessage.success('已从草稿删除，点击「保存全部」后生效')
}

// —— 保存全部 ——
function validateAll(): boolean {
  for (let i = 0; i < drafts.value.length; i++) {
    const r = drafts.value[i]
    if (!r.model_pattern.trim()) {
      ElMessage.warning(`第 ${i + 1} 条：模型模式不能为空`)
      return false
    }
    if (!r.upstream_id) {
      ElMessage.warning(`第 ${i + 1} 条：请选择上游`)
      return false
    }
    if (r.weight == null || !(r.weight >= 1)) {
      ElMessage.warning(`第 ${i + 1} 条：权重需 ≥ 1`)
      return false
    }
  }
  return true
}

function toRouteItem(row: RouteRow): RouteItem {
  return {
    model_pattern: row.model_pattern,
    upstream_id: row.upstream_id,
    override_model: row.override_model ?? null,
    priority: row.priority ?? 10,
    weight: row.weight ?? 1,
    enabled: row.enabled ?? true,
    retries: row.retries ?? 2,
    retry_status_codes: row.retry_status_codes ?? [...DEFAULT_RETRY_CODES],
    lock_upstream: row.lock_upstream ?? false,
    sort_order: row.sort_order ?? 0,
  }
}

async function saveAll() {
  if (saving.value) return
  if (!validateAll()) return
  // 保存前对比本地草稿与最近一次后端快照，向用户展示 diff 摘要
  const diff = computeDiff()
  if (diff.added + diff.modified + diff.deleted === 0) {
    ElMessage.info('无任何改动')
    return
  }
  if (drafts.value.length === 0) {
    // 0 条清空沿用原确认文案
    try {
      await ElMessageBox.confirm('当前无任何规则，保存将清空全部模型路由。确定继续？', '保存确认', {
        type: 'warning',
      })
    } catch {
      return
    }
  } else {
    try {
      await ElMessageBox.confirm(
        `检测到改动：新增 ${diff.added} · 修改 ${diff.modified} · 删除 ${diff.deleted}。点击确定将整体替换后端路由表。`,
        '保存全部',
        { type: 'warning' },
      )
    } catch {
      return
    }
  }
  saving.value = true
  try {
    const items = drafts.value.map(toRouteItem)
    await routeApi.replaceAll(items)
    ElMessage.success(`已替换全部 ${items.length} 条`)
    await loadData()
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    saving.value = false
  }
}

// —— 刷新 / 放弃本地修改 ——
async function refresh() {
  if (!(await confirmIfDirty('当前有未保存的修改，刷新后不会提交，确定继续？'))) return
  await loadData()
  ElMessage.success('已刷新')
}

async function discardLocal() {
  if (dirtyCount.value === 0) {
    ElMessage.info('当前没有未保存的修改')
    return
  }
  try {
    await ElMessageBox.confirm('确定放弃所有本地未保存的修改？', '放弃修改', { type: 'warning' })
  } catch {
    return
  }
  await loadData()
  ElMessage.success('已放弃本地修改')
}

async function confirmIfDirty(msg: string): Promise<boolean> {
  if (dirtyCount.value === 0) return true
  try {
    await ElMessageBox.confirm(msg, '刷新确认', { type: 'warning' })
    return true
  } catch {
    return false
  }
}

// —— diff ——
function sameFields(a: RouteRow, b: RouteOut): boolean {
  return (
    a.model_pattern.trim() === b.model_pattern &&
    a.upstream_id === b.upstream_id &&
    (a.override_model ?? null) === b.override_model &&
    (a.priority ?? 10) === b.priority &&
    (a.weight ?? 1) === b.weight &&
    (a.enabled ?? true) === b.enabled &&
    (a.retries ?? 2) === b.retries &&
    (a.retry_status_codes ?? DEFAULT_RETRY_CODES).join(',') ===
      (b.retry_status_codes ?? DEFAULT_RETRY_CODES).join(',') &&
    (a.lock_upstream ?? false) === b.lock_upstream &&
    (a.sort_order ?? 0) === b.sort_order
  )
}

function isRowDirty(row: RouteRow): boolean {
  if (!row.id) return true
  const c = serverCommit.value.find((x) => x.id === row.id)
  if (!c) return true
  return !sameFields(row, c)
}

/** 本地草稿相对最近一次后端快照的 diff 摘要：新增 / 修改 / 删除 */
function computeDiff(): { added: number; modified: number; deleted: number } {
  const commitMap = new Map(serverCommit.value.map((c) => [c.id, c]))
  let added = 0
  let modified = 0
  for (const d of drafts.value) {
    const c = d.id ? commitMap.get(d.id) : undefined
    if (!c) {
      // 无 id（本会话新建）或后端快照中已不存在 → 新增
      added++
      continue
    }
    if (!sameFields(d, c)) modified++
  }
  const draftIds = new Set<string>()
  for (const d of drafts.value) {
    if (d.id != null) draftIds.add(d.id)
  }
  const deleted = serverCommit.value.filter((c) => !draftIds.has(c.id)).length
  return { added, modified, deleted }
}
</script>

<style scoped>
.page-intro {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 20px;
  margin-bottom: 16px;
}
.page-intro h2 { margin: 0; color: #1f2329; font-size: 20px; }
.page-intro p { margin: 6px 0 0; color: #6b7280; font-size: 13px; }
.intro-summary { display: flex; gap: 18px; color: #6b7280; font-size: 13px; white-space: nowrap; }
.intro-summary b { color: #1f2329; font-size: 16px; margin-right: 3px; }
@media (max-width: 768px) {
  .page-intro { flex-direction: column; }
  .intro-summary { white-space: normal; }
}
.muted {
  color: #c0c4cc;
}
.text-warn {
  color: #e6a23c;
}
.text-error {
  color: #f56c6c;
}
.cell-inline {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}
</style>
