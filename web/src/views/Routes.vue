<template>
  <div class="page" v-loading="pageLoading">
    <el-alert type="info" :closable="false" show-icon style="margin-bottom: 16px">
      <template #title>模型 → 上游绑定（模型名支持 <span class="mono">*</span> 通配）</template>
      <div class="hint">
        <div>· 优先级数字小者优先；同优先级按权重加权。</div>
        <div>· 「保存全部」将整体替换后端路由表（PUT 裸数组）。</div>
        <div>· 重试 / 熔断参数只在路由表维护。</div>
        <div>· 锁定上游后，失败时不会自动故障转移。</div>
      </div>
    </el-alert>

    <div class="toolbar">
      <el-button type="primary" :icon="Plus" @click="openCreate">新增规则</el-button>
      <el-button :icon="Refresh" @click="refresh">刷新</el-button>
      <el-button @click="discardLocal">放弃本地修改</el-button>
      <div class="spacer" />
      <div class="hint" v-if="dirtyCount > 0">有 {{ dirtyCount }} 条修改未保存</div>
      <el-button type="primary" :loading="saving" @click="saveAll">
        保存全部（{{ drafts.length }} 条）
      </el-button>
    </div>

    <el-card shadow="never">
      <el-table :data="drafts" border>
        <template #empty>
          <el-empty description="暂无模型路由，点击右上角「新增规则」创建" />
        </template>

        <el-table-column label="模型模式" min-width="180" show-overflow-tooltip>
          <template #default="{ row }">
            <span class="mono">{{ row.model_pattern }}</span>
            <el-tooltip
              v-if="row.managed_by === 'auto'"
              content="由渠道模型同步托管：自动跟随上游模型列表增删；手动编辑请先关闭该渠道的自动跟随"
              placement="top"
            >
              <el-tag size="small" type="primary" effect="plain" style="margin-left: 6px">自动</el-tag>
            </el-tooltip>
            <el-tag
              v-if="isRowDirty(row)"
              size="small"
              type="warning"
              effect="plain"
              style="margin-left: 6px"
            >
              未保存
            </el-tag>
          </template>
        </el-table-column>

        <el-table-column label="上游" min-width="160" show-overflow-tooltip>
          <template #default="{ row }">
            <span>{{ row.upstreamName }}</span>
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
      width="640px"
      :close-on-click-modal="false"
      destroy-on-close
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
          />
        </el-form-item>

        <el-form-item label="锁定上游" prop="lock_upstream">
          <el-switch v-model="form.lock_upstream" />
          <div class="hint" style="width: 100%">锁定后失败不自动故障转移。</div>
        </el-form-item>
      </el-form>

      <template #footer>
        <el-button @click="dialogVisible = false">取消</el-button>
        <el-button type="primary" :loading="saving" @click="submitDialog">保存</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { ElMessage, ElMessageBox, type FormInstance, type FormRules } from 'element-plus'
import { Plus, Refresh } from '@element-plus/icons-vue'
import { routeApi, upstreamApi } from '@/api'
import { errMsg } from '@/api/http'
import { fmtTime } from '@/utils/format'
import type { RouteItem, RouteOut, UpstreamOut } from '@/api/types'

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
/** 最近一次后端提交结果（用于 diff / 放弃本地修改） */
const serverCommit = ref<RouteOut[]>([])
/** 本地草稿（可编辑工作副本） */
const drafts = ref<RouteRow[]>([])

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
        if (v == null || !(v > 0)) cb(new Error('权重需大于 0'))
        else cb()
      },
      trigger: 'change',
    },
  ],
}

const dialogTitle = computed(() => (isEdit.value ? '编辑规则' : '新增规则'))

const dirtyCount = computed(() => drafts.value.filter((r) => isRowDirty(r)).length)

// —— 数据加载 ——
async function loadData() {
  pageLoading.value = true
  try {
    const routes = await routeApi.list()
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
      upstreamName: r.upstream_name,
      createdAt: r.created_at,
      updatedAt: r.updated_at,
    }))
  } catch (e) {
    ElMessage.error(errMsg(e))
  }
  try {
    upstreams.value = await upstreamApi.list()
  } catch (e) {
    ElMessage.error(errMsg(e))
  }
  pageLoading.value = false
}

onMounted(loadData)

// —— 弹窗打开 / 回填 ——
function openCreate() {
  isEdit.value = false
  dialogIndex.value = -1
  Object.assign(form, defaultForm())
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
  dialogVisible.value = false
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
    if (r.weight == null || !(r.weight > 0)) {
      ElMessage.warning(`第 ${i + 1} 条：权重需大于 0`)
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
  if (drafts.value.length === 0) {
    try {
      await ElMessageBox.confirm('当前无任何规则，保存将清空全部模型路由。确定继续？', '保存确认', {
        type: 'warning',
      })
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
</script>

<style scoped>
.muted {
  color: #c0c4cc;
}
</style>
