<template>
  <div class="page" v-loading="pageLoading">
    <el-tabs v-model="activeTab" @tab-change="onTabChange">
      <!-- ===== Tab1 价格规则 ===== -->
      <el-tab-pane label="价格规则" name="rules">
        <div class="toolbar">
          <el-select
            v-model="filterUpstream"
            filterable
            clearable
            placeholder="按上游筛选"
            style="width: 240px"
            @change="loadRules"
          >
            <el-option v-for="u in upstreams" :key="u.id" :value="u.id" :label="u.name" />
          </el-select>
          <el-input
            v-model="filterModel"
            placeholder="按模型筛选"
            clearable
            style="width: 240px"
            @keyup.enter="loadRules"
            @clear="loadRules"
          />
          <el-button type="primary" :icon="Search" @click="loadRules">查询</el-button>
          <el-button :icon="Refresh" @click="resetFilter">重置</el-button>
          <div class="spacer" />
          <el-button type="primary" :icon="Plus" @click="openCreate">新建规则</el-button>
        </div>

        <el-card shadow="never">
          <el-table :data="rules" empty-text="暂无价格规则" v-loading="rulesLoading">
            <el-table-column prop="model_id" label="模型" min-width="180" show-overflow-tooltip />
            <el-table-column label="上游" min-width="150" show-overflow-tooltip>
              <template #default="{ row }">{{ row.upstream_name || '-' }}</template>
            </el-table-column>
            <el-table-column label="计价单位" min-width="170">
              <template #default="{ row }">{{ UNIT_LABEL(row.unit) }}</template>
            </el-table-column>
            <el-table-column label="币种" width="80" align="center">
              <template #default="{ row }">{{ row.currency }}</template>
            </el-table-column>
            <el-table-column label="基础单价" min-width="120" align="right">
              <template #default="{ row }">
                <span class="mono">{{ fmtMoney(row.base_price) }}</span>
              </template>
            </el-table-column>
            <el-table-column label="分段" min-width="110" align="center">
              <template #default="{ row }">
                <el-tooltip v-if="row.segments && row.segments.length" placement="top">
                  <template #content>
                    <div v-for="(s, i) in row.segments" :key="i" class="seg-tip">{{ segTitle(s) }}</div>
                  </template>
                  <el-tag size="small" type="primary" effect="plain">{{ row.segments.length }} 段</el-tag>
                </el-tooltip>
                <span v-else class="hint">—</span>
              </template>
            </el-table-column>
            <el-table-column label="维度" min-width="120">
              <template #default="{ row }">
                <el-tooltip
                  v-if="row.dimensions && Object.keys(row.dimensions).length"
                  placement="top"
                >
                  <template #content>
                    <div v-for="(v, k) in row.dimensions" :key="k" class="seg-tip">
                      {{ k }} = {{ fmtDimensionValue(v) }}
                    </div>
                  </template>
                  <span class="mono">{{ row.dimension_key || '维度' }}</span>
                </el-tooltip>
                <span v-else class="hint">—</span>
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
            <el-table-column label="有效期" min-width="220">
              <template #default="{ row }">{{ effectiveRange(row) }}</template>
            </el-table-column>
            <el-table-column label="操作" width="160" fixed="right">
              <template #default="{ row }">
                <el-button size="small" @click="openEdit(row)">编辑</el-button>
                <el-button size="small" type="danger" @click="removeRule(row)">删除</el-button>
              </template>
            </el-table-column>
          </el-table>
        </el-card>
      </el-tab-pane>

      <!-- ===== Tab2 未定价模型 ===== -->
      <el-tab-pane label="未定价模型" name="unpriced" lazy>
        <el-alert
          type="info"
          :closable="false"
          show-icon
          title="未设置价格即不计价：成本记 NULL 并在列表/统计中提示未定价。"
          style="margin-bottom: 14px"
        />
        <el-card shadow="never">
          <el-table :data="unpricedItems" empty-text="暂无未定价模型" v-loading="unpricedLoading">
            <el-table-column label="上游" min-width="180">
              <template #default="{ row }">{{ row.upstream_name }}</template>
            </el-table-column>
            <el-table-column prop="model_id" label="模型" min-width="200" show-overflow-tooltip />
            <el-table-column label="操作" width="140">
              <template #default="{ row }">
                <el-button size="small" type="primary" :icon="PriceTag" @click="priceUpstream(row)">
                  为此定价
                </el-button>
              </template>
            </el-table-column>
          </el-table>
        </el-card>
      </el-tab-pane>

      <!-- ===== Tab3 价格试算 ===== -->
      <el-tab-pane label="价格试算" name="preview" lazy>
        <el-card shadow="never" class="preview-form-card">
          <el-form label-width="150px" @submit.prevent>
            <el-form-item label="上游" required>
              <el-select v-model="previewForm.upstream_id" filterable placeholder="请选择上游" style="width: 300px">
                <el-option v-for="u in upstreams" :key="u.id" :value="u.id" :label="u.name" />
              </el-select>
            </el-form-item>
            <el-form-item label="模型 ID" required>
              <el-input v-model="previewForm.model_id" placeholder="如 gpt-4o" clearable style="width: 300px" />
            </el-form-item>
            <el-form-item label="计费时间 (at)">
              <el-date-picker
                v-model="previewForm.at"
                type="datetime"
                value-format="YYYY-MM-DD HH:mm:ss"
                placeholder="（可选）默认当前时间"
                clearable
                style="width: 300px"
              />
              <span class="hint" style="margin-left: 10px">留空 = 现在</span>
            </el-form-item>

            <el-divider content-position="left">用量（数字，可空）</el-divider>
            <el-form-item label="输入 tokens">
              <el-input-number v-model="previewForm.prompt_tokens" :min="0" :controls="false" style="width: 200px" />
            </el-form-item>
            <el-form-item label="输出 tokens">
              <el-input-number v-model="previewForm.completion_tokens" :min="0" :controls="false" style="width: 200px" />
            </el-form-item>
            <el-form-item label="缓存写 tokens">
              <el-input-number v-model="previewForm.cache_write_tokens" :min="0" :controls="false" style="width: 200px" />
            </el-form-item>
            <el-form-item label="缓存读 tokens">
              <el-input-number v-model="previewForm.cache_read_tokens" :min="0" :controls="false" style="width: 200px" />
            </el-form-item>
            <el-form-item label="图片数量">
              <el-input-number v-model="previewForm.images" :min="0" :controls="false" style="width: 200px" />
            </el-form-item>

            <el-divider content-position="left">媒体参数（文本，可空）</el-divider>
            <el-form-item label="图片尺寸">
              <el-input v-model="previewForm.image_size" placeholder="如 1024x1024" clearable style="width: 300px" />
            </el-form-item>
            <el-form-item label="视频秒数">
              <el-input-number v-model="previewForm.video_seconds" :min="0" :max="86400" :precision="3"
                :controls="false" placeholder="如 10（支持小数）" style="width: 300px" />
            </el-form-item>
            <el-form-item label="视频分辨率">
              <el-input v-model="previewForm.video_resolution" placeholder="如 1080p" clearable style="width: 300px" />
            </el-form-item>
            <el-form-item label="视频任务类型">
              <el-input v-model="previewForm.video_task_type" placeholder="如 text2video" clearable style="width: 300px" />
            </el-form-item>

            <el-form-item>
              <el-button type="primary" :icon="Search" :loading="previewing" @click="runPreview">试算</el-button>
              <el-button :icon="RefreshLeft" @click="resetPreview">清空</el-button>
            </el-form-item>
          </el-form>
        </el-card>

        <el-card shadow="never" style="margin-top: 16px">
          <template #header>
            <div class="card-head"><span>试算结果</span></div>
          </template>
          <PreviewPanel :resp="previewResp" />
        </el-card>
      </el-tab-pane>

      <!-- ===== Tab4 导入/导出 ===== -->
      <el-tab-pane label="导入 / 导出" name="import" lazy>
        <el-card shadow="never" style="margin-bottom: 16px">
          <template #header>
            <div class="card-head"><span>导出价格</span></div>
          </template>
          <div class="hint" style="margin-bottom: 12px">
            导出的文件包含全部价格规则，字段结构参考：<span class="mono">input_per_m / output_per_m / cache_* / image / video</span>，可用于备份或迁移到其它实例。
          </div>
          <div class="export-row">
            <el-radio-group v-model="exportFormat">
              <el-radio-button value="xml">XML</el-radio-button>
              <el-radio-button value="json">JSON</el-radio-button>
            </el-radio-group>
            <el-button type="primary" :icon="Download" :loading="exporting" @click="doExport">
              导出
            </el-button>
            <span class="hint">文件名 nextapi-prices.{{ exportFormat }}</span>
          </div>
        </el-card>

        <ImportPanel />
      </el-tab-pane>

      <!-- ===== Tab5 汇率 ===== -->
      <el-tab-pane label="汇率" name="fx" lazy>
        <FxPanel />
      </el-tab-pane>
    </el-tabs>

    <!-- 规则新建/编辑弹窗 -->
    <RuleFormDialog ref="formDialogRef" :upstreams="upstreams" @saved="onRuleSaved" />
  </div>
</template>

<script setup lang="ts">
import { onMounted, reactive, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { Download, Plus, PriceTag, Refresh, RefreshLeft, Search } from '@element-plus/icons-vue'
import dayjs from 'dayjs'
import { pricingApi, upstreamApi } from '@/api'
import { errMsg } from '@/api/http'
import { UNIT_LABEL } from '@/utils/consts'
import { fmtDate, fmtMoney } from '@/utils/format'
import { downloadText } from '@/utils/download'
import type {
  PreviewReq,
  PreviewResp,
  PriceExportFormat,
  PriceSegment,
  RuleItem,
  UnpricedItem,
  UpstreamOut,
} from '@/api/types'
import { fmtDimensionValue } from '@/components/pricing/types'
import RuleFormDialog from '@/components/pricing/RuleFormDialog.vue'
import PreviewPanel from '@/components/pricing/PreviewPanel.vue'
import ImportPanel from '@/components/pricing/ImportPanel.vue'
import FxPanel from '@/components/pricing/FxPanel.vue'

const upstreams = ref<UpstreamOut[]>([])
const pageLoading = ref(false)

const activeTab = ref('rules')
const formDialogRef = ref<InstanceType<typeof RuleFormDialog>>()

async function loadUpstreams() {
  try {
    upstreams.value = await upstreamApi.list()
  } catch (e) {
    ElMessage.error(errMsg(e))
  }
}

// —— Tab1 价格规则 ——
const rules = ref<RuleItem[]>([])
const rulesLoading = ref(false)
const filterUpstream = ref('')
const filterModel = ref('')
const togglingSet = reactive(new Set<string>())

async function loadRules() {
  rulesLoading.value = true
  try {
    rules.value = await pricingApi.list(
      filterUpstream.value || undefined,
      filterModel.value.trim() || undefined,
    )
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    rulesLoading.value = false
  }
}

function resetFilter() {
  filterUpstream.value = ''
  filterModel.value = ''
  loadRules()
}

function segTitle(s: PriceSegment): string {
  const name = s.name?.trim() ? s.name.trim() : '(未命名)'
  return `${name}: ${s.price}`
}

function effectiveRange(row: RuleItem): string {
  const from = fmtDate(row.effective_from)
  const to = fmtDate(row.effective_to)
  return `${from} ~ ${to}`
}

function openCreate() {
  formDialogRef.value?.open()
}

function openEdit(row: RuleItem) {
  formDialogRef.value?.open(row)
}

async function toggleEnabled(row: RuleItem, val: boolean) {
  togglingSet.add(row.id)
  try {
    const updated = await pricingApi.update(row.id, { enabled: val })
    Object.assign(row, updated)
    ElMessage.success(val ? '已启用' : '已禁用')
  } catch (e) {
    row.enabled = !val
    ElMessage.error(errMsg(e))
  } finally {
    togglingSet.delete(row.id)
  }
}

async function removeRule(row: RuleItem) {
  try {
    await ElMessageBox.confirm(
      `确定删除「${row.model_id}」在「${row.upstream_name || ''}」的价格规则？`,
      '删除规则',
      { type: 'warning' },
    )
  } catch {
    return
  }
  try {
    await pricingApi.remove(row.id)
    ElMessage.success('已删除')
    loadRules()
  } catch (e) {
    ElMessage.error(errMsg(e))
  }
}

function onRuleSaved() {
  loadRules()
  // 未定价列表可能已被定价
  loadUnpriced()
}

// —— Tab2 未定价模型 ——
const unpricedItems = ref<UnpricedItem[]>([])
const unpricedLoading = ref(false)

async function loadUnpriced() {
  unpricedLoading.value = true
  try {
    unpricedItems.value = await pricingApi.unpriced()
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    unpricedLoading.value = false
  }
}

function priceUpstream(row: UnpricedItem) {
  formDialogRef.value?.open(undefined, { upstream_id: row.upstream_id, model_id: row.model_id })
}

// —— Tab3 价格试算 ——
const previewForm = reactive({
  upstream_id: '',
  model_id: '',
  at: '',
  prompt_tokens: null as number | null,
  completion_tokens: null as number | null,
  cache_write_tokens: null as number | null,
  cache_read_tokens: null as number | null,
  images: null as number | null,
  image_size: '',
  video_seconds: null as number | null,
  video_resolution: '',
  video_task_type: '',
})
const previewing = ref(false)
const previewResp = ref<PreviewResp | null>(null)

function buildPreviewReq(): PreviewReq {
  const body: PreviewReq = {
    upstream_id: previewForm.upstream_id,
    model_id: previewForm.model_id.trim(),
  }
  if (previewForm.at) body.at = dayjs(previewForm.at).toISOString()
  const numKeys = ['prompt_tokens', 'completion_tokens', 'cache_write_tokens', 'cache_read_tokens', 'images'] as const
  for (const k of numKeys) {
    const v = previewForm[k]
    if (v !== null && v !== undefined) body[k] = v
  }
  // 视频秒数：数字输入，非空时转 Decimal 字符串提交（后端按秒计价，支持小数）
  if (previewForm.video_seconds !== null && previewForm.video_seconds !== undefined) {
    body.video_seconds = String(previewForm.video_seconds)
  }
  const strKeys = ['image_size', 'video_resolution', 'video_task_type'] as const
  for (const k of strKeys) {
    const v = previewForm[k]
    if (v && v.trim()) body[k] = v.trim()
  }
  return body
}

async function runPreview() {
  if (!previewForm.upstream_id) {
    ElMessage.warning('请选择上游')
    return
  }
  if (!previewForm.model_id.trim()) {
    ElMessage.warning('请输入模型 ID')
    return
  }
  previewing.value = true
  try {
    previewResp.value = await pricingApi.preview(buildPreviewReq())
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    previewing.value = false
  }
}

function resetPreview() {
  previewForm.upstream_id = ''
  previewForm.model_id = ''
  previewForm.at = ''
  previewForm.prompt_tokens = null
  previewForm.completion_tokens = null
  previewForm.cache_write_tokens = null
  previewForm.cache_read_tokens = null
  previewForm.images = null
  previewForm.image_size = ''
  previewForm.video_seconds = null
  previewForm.video_resolution = ''
  previewForm.video_task_type = ''
  previewResp.value = null
}

// —— Tab4 导出 ——
const exportFormat = ref<PriceExportFormat>('xml')
const exporting = ref(false)

async function doExport() {
  exporting.value = true
  try {
    const text = await pricingApi.exportText(exportFormat.value)
    downloadText(
      text,
      `nextapi-prices.${exportFormat.value}`,
      exportFormat.value === 'json'
        ? 'application/json;charset=utf-8'
        : 'application/xml;charset=utf-8',
    )
    ElMessage.success('已导出')
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    exporting.value = false
  }
}

// —— Tab 切换 ——
function onTabChange(name: string | number) {
  if (String(name) === 'unpriced' && !unpricedItems.value.length) {
    loadUnpriced()
  }
}

onMounted(() => {
  pageLoading.value = true
  Promise.all([loadUpstreams(), loadRules()]).finally(() => {
    pageLoading.value = false
  })
})
</script>

<style scoped>
.preview-form-card {
  max-width: 900px;
}
.export-row {
  display: flex;
  align-items: center;
  gap: 14px;
  flex-wrap: wrap;
}
.card-head {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 14px;
  font-weight: 600;
  color: #303133;
}
.seg-tip {
  font-size: 12px;
  line-height: 1.6;
  white-space: nowrap;
}
</style>
