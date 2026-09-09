<template>
  <div class="page" v-loading="pageLoading">
    <div class="page-card tabs-card">
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
            placeholder="按模型名过滤（本地搜索）"
            clearable
            style="width: 240px"
          />
          <el-button type="primary" :icon="Search" @click="loadRules">查询</el-button>
          <el-button :icon="Refresh" @click="resetFilter">重置</el-button>
          <div class="spacer" />
          <el-button type="primary" :icon="Plus" @click="openModelPrice()">按模型定价</el-button>
          <el-tooltip content="单条规则（图片/视频维度、有效期等高级项）" placement="top">
            <el-button :icon="Setting" @click="openCreate">高级规则</el-button>
          </el-tooltip>
        </div>

        <el-card shadow="never">
          <el-table :data="pageGroups" :empty-text="rulesEmptyText" v-loading="rulesLoading" row-key="key">
            <el-table-column type="expand">
              <template #default="{ row }">
                <div class="rule-expand">
                  <el-table :data="row.rules" size="small">
                    <el-table-column label="计价单位" min-width="150">
                      <template #default="{ row: r }">{{ UNIT_LABEL(r.unit) }}</template>
                    </el-table-column>
                    <el-table-column label="币种" width="70" align="center">
                      <template #default="{ row: r }">{{ r.currency }}</template>
                    </el-table-column>
                    <el-table-column label="基础单价" min-width="110" align="right">
                      <template #default="{ row: r }"><span class="mono">{{ fmtMoney(r.base_price) }}</span></template>
                    </el-table-column>
                    <el-table-column label="分段" width="90" align="center">
                      <template #default="{ row: r }">
                        <el-tooltip v-if="r.segments && r.segments.length" placement="top">
                          <template #content>
                            <div v-for="(s, i) in r.segments" :key="i" class="seg-tip">{{ segTitle(s) }}</div>
                          </template>
                          <el-tag size="small" type="primary" effect="plain">{{ r.segments.length }} 段</el-tag>
                        </el-tooltip>
                        <span v-else class="hint">—</span>
                      </template>
                    </el-table-column>
                    <el-table-column label="维度" min-width="120">
                      <template #default="{ row: r }">
                        <el-tooltip v-if="r.dimensions && Object.keys(r.dimensions).length" placement="top">
                          <template #content>
                            <div v-for="(v, k) in r.dimensions" :key="k" class="seg-tip">{{ k }} = {{ fmtDimensionValue(v) }}</div>
                          </template>
                          <span class="mono">{{ r.dimension_key || '维度' }}</span>
                        </el-tooltip>
                        <span v-else class="hint">—</span>
                      </template>
                    </el-table-column>
                    <el-table-column label="启用" width="70" align="center">
                      <template #default="{ row: r }">
                        <el-switch
                          v-model="r.enabled"
                          size="small"
                          :loading="togglingSet.has(r.id)"
                          @change="(val: boolean) => toggleEnabled(r, val)"
                        />
                      </template>
                    </el-table-column>
                    <el-table-column label="有效期" min-width="200">
                      <template #default="{ row: r }">{{ effectiveRange(r) }}</template>
                    </el-table-column>
                    <el-table-column label="操作" width="150" fixed="right">
                      <template #default="{ row: r }">
                        <el-button size="small" @click="openEdit(r)">高级编辑</el-button>
                        <el-button size="small" type="danger" @click="removeRule(r)">删除</el-button>
                      </template>
                    </el-table-column>
                  </el-table>
                </div>
              </template>
            </el-table-column>
            <el-table-column label="模型" min-width="220">
              <template #default="{ row }">
                <CopyText :text="row.model_id" :truncate="28" />
              </template>
            </el-table-column>
            <el-table-column label="上游" min-width="140" show-overflow-tooltip>
              <template #default="{ row }">{{ row.upstream_name || '-' }}</template>
            </el-table-column>
            <el-table-column v-for="u in TOKEN_UNITS" :key="u" min-width="120" align="right">
              <template #header>
                <el-tooltip :content="UNIT_LABEL(u)" placement="top">
                  <span>{{ UNIT_SHORT[u] }} /1M</span>
                </el-tooltip>
              </template>
              <template #default="{ row }">
                <template v-if="row.unitMap[u]">
                  <span class="mono">{{ fmtMoney(row.unitMap[u].base_price) }}</span>
                  <span class="hint"> {{ row.unitMap[u].currency }}</span>
                  <el-tag
                    v-if="row.unitMap[u].segments?.length"
                    size="small"
                    effect="plain"
                    style="margin-left: 4px"
                  >
                    {{ row.unitMap[u].segments.length }}段
                  </el-tag>
                  <el-tag
                    v-if="!row.unitMap[u].enabled"
                    size="small"
                    type="info"
                    effect="plain"
                    style="margin-left: 4px"
                  >
                    已停用
                  </el-tag>
                </template>
                <span v-else class="hint">—</span>
              </template>
            </el-table-column>
            <el-table-column label="其他" width="90" align="center">
              <template #default="{ row }">
                <el-tag v-if="row.otherCount" size="small" type="info" effect="plain">{{ row.otherCount }} 条</el-tag>
                <span v-else class="hint">—</span>
              </template>
            </el-table-column>
            <el-table-column label="操作" width="150" fixed="right">
              <template #default="{ row }">
                <el-button size="small" type="primary" @click="openModelPrice(row)">编辑定价</el-button>
              </template>
            </el-table-column>
          </el-table>
          <div class="table-foot">
            <el-pagination
              v-model:current-page="page"
              v-model:page-size="pageSize"
              :page-sizes="[10, 20, 50]"
              :total="filteredGroups.length"
              layout="total, sizes, prev, pager, next, jumper"
              background
              small
            />
          </div>
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
            <el-table-column label="操作" min-width="300">
              <template #default="{ row }">
                <el-button size="small" type="primary" :icon="PriceTag" @click="priceUpstream(row)">
                  为此定价
                </el-button>
                <el-button size="small" :icon="View" @click="viewModelRules(row)">查看该模型已有规则</el-button>
              </template>
            </el-table-column>
          </el-table>
        </el-card>
      </el-tab-pane>

      <!-- ===== Tab3 价格试算 ===== -->
      <el-tab-pane label="价格试算" name="preview" lazy>
        <el-card shadow="never" class="preview-form-card">
          <el-form
            ref="previewFormRef"
            :model="previewForm"
            :rules="previewRules"
            label-width="150px"
            @submit.prevent
            @keyup.enter="onPreviewEnter"
          >
            <el-form-item label="上游" prop="upstream_id">
              <el-select v-model="previewForm.upstream_id" filterable placeholder="请选择上游" style="width: 300px">
                <el-option v-for="u in upstreams" :key="u.id" :value="u.id" :label="u.name" />
              </el-select>
            </el-form-item>
            <el-form-item label="模型 ID" prop="model_id">
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
          <el-alert
            v-if="previewStale"
            type="warning"
            :closable="false"
            show-icon
            title="参数已变更，结果未刷新"
            description="下方展示的是上一次试算结果，请修改参数后重新试算。"
            style="margin-bottom: 14px"
          />
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
    </div>

    <!-- 规则新建/编辑弹窗 -->
    <RuleFormDialog ref="formDialogRef" :upstreams="upstreams" @saved="onRuleSaved" />
    <!-- 按模型定价弹窗 -->
    <ModelPriceDialog ref="modelDialogRef" :upstreams="upstreams" @saved="onRuleSaved" />
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import { ElMessage, type FormInstance, type FormRules } from 'element-plus'
import { Download, Plus, PriceTag, Refresh, RefreshLeft, Search, Setting, View } from '@element-plus/icons-vue'
import dayjs from 'dayjs'
import { pricingApi, upstreamApi } from '@/api'
import { errMsg } from '@/api/http'
import { UNIT_LABEL, UNIT_SHORT } from '@/utils/consts'
import { fmtMoney } from '@/utils/format'
import { downloadText } from '@/utils/download'
import { confirmDanger } from '@/utils/confirm'
import CopyText from '@/components/common/CopyText.vue'
import type {
  PreviewReq,
  PreviewResp,
  PriceExportFormat,
  PriceSegment,
  PriceUnit,
  RuleItem,
  UnpricedItem,
  UpstreamOut,
} from '@/api/types'
import { fmtDimensionValue } from '@/components/pricing/types'
import RuleFormDialog from '@/components/pricing/RuleFormDialog.vue'
import ModelPriceDialog from '@/components/pricing/ModelPriceDialog.vue'
import PreviewPanel from '@/components/pricing/PreviewPanel.vue'
import ImportPanel from '@/components/pricing/ImportPanel.vue'
import FxPanel from '@/components/pricing/FxPanel.vue'

const upstreams = ref<UpstreamOut[]>([])
const pageLoading = ref(false)
const route = useRoute()

const activeTab = ref('rules')
const formDialogRef = ref<InstanceType<typeof RuleFormDialog>>()
const modelDialogRef = ref<InstanceType<typeof ModelPriceDialog>>()

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
// 模型名过滤词：客户端过滤（见 filteredGroups），并驱动「未定价 → 查看已有规则」跳转。
const filterModel = ref('')
// 客户端分页状态
const page = ref(1)
const pageSize = ref(20)
const togglingSet = reactive(new Set<string>())

// 请求序号：防筛选并发时旧响应覆盖（发布审阅前端 M2）
let rulesSeq = 0

async function loadRules() {
  const seq = ++rulesSeq
  rulesLoading.value = true
  try {
    // 上游走服务端过滤；模型名搜索在本地完成，避免每次击键打后端。
    const resp = await pricingApi.list(filterUpstream.value || undefined)
    if (seq !== rulesSeq) return
    rules.value = resp
  } catch (e) {
    if (seq !== rulesSeq) return
    ElMessage.error(errMsg(e))
  } finally {
    if (seq === rulesSeq) rulesLoading.value = false
  }
}

function resetFilter() {
  filterUpstream.value = ''
  filterModel.value = ''
  page.value = 1
  loadRules()
}

function segTitle(s: PriceSegment): string {
  const name = s.name?.trim() ? s.name.trim() : '(未命名)'
  return `${name}: ${s.price}`
}

// 展示精确到分钟：同日多个时间段的有效期规则可区分
function fmtMinute(iso?: string | null): string {
  if (!iso) return '-'
  const d = dayjs(iso)
  return d.isValid() ? d.format('YYYY-MM-DD HH:mm') : '-'
}

function effectiveRange(row: RuleItem): string {
  return `${fmtMinute(row.effective_from)} ~ ${fmtMinute(row.effective_to)}`
}

// —— 按模型分组（默认视图）：同供应商+同模型的多条单位规则聚合为一行 ——
const TOKEN_UNITS: PriceUnit[] = ['token_in', 'token_out', 'token_cache_write', 'token_cache_read']

interface ModelGroup {
  key: string
  upstream_id: string
  model_id: string
  upstream_name: string | null
  unitMap: Partial<Record<PriceUnit, RuleItem>>
  otherCount: number
  rules: RuleItem[]
}

const modelGroups = computed<ModelGroup[]>(() => {
  const map = new Map<string, ModelGroup>()
  for (const r of rules.value) {
    const key = `${r.upstream_id}|${r.model_id}`
    let g = map.get(key)
    if (!g) {
      g = {
        key,
        upstream_id: r.upstream_id,
        model_id: r.model_id,
        upstream_name: r.upstream_name,
        unitMap: {},
        otherCount: 0,
        rules: [],
      }
      map.set(key, g)
    }
    g.rules.push(r)
    if (TOKEN_UNITS.includes(r.unit)) {
      // 同单位多条（如不同有效期）：取启用的第一条
      if (!g.unitMap[r.unit] || (r.enabled && !g.unitMap[r.unit]!.enabled)) g.unitMap[r.unit] = r
    } else {
      g.otherCount++
    }
  }
  return [...map.values()].sort((a, b) => a.model_id.localeCompare(b.model_id))
})

// 客户端搜索：按模型名关键词过滤聚合行（大小写不敏感）
const filteredGroups = computed<ModelGroup[]>(() => {
  const kw = filterModel.value.trim().toLowerCase()
  if (!kw) return modelGroups.value
  return modelGroups.value.filter((g) => g.model_id.toLowerCase().includes(kw))
})

// 搜索无匹配时的空态文案
const rulesEmptyText = computed(() => {
  const kw = filterModel.value.trim()
  if (modelGroups.value.length && kw && !filteredGroups.value.length) {
    return `未找到匹配「${kw}」的价格规则`
  }
  return '暂无价格规则'
})

// 客户端分页（10/20/50，默认 20）；页码越界时自动收敛
const pageGroups = computed<ModelGroup[]>(() => {
  const total = filteredGroups.value.length
  const maxPage = Math.max(1, Math.ceil(total / pageSize.value))
  if (page.value > maxPage) page.value = maxPage
  if (page.value < 1) page.value = 1
  const start = (page.value - 1) * pageSize.value
  return filteredGroups.value.slice(start, start + pageSize.value)
})

// 数据量变化时回到首页，避免停留在空页
watch(
  () => [filterUpstream.value, filterModel.value, pageSize.value] as const,
  () => {
    page.value = 1
  },
)

function openModelPrice(row?: ModelGroup) {
  modelDialogRef.value?.open(row ? { upstream_id: row.upstream_id, model_id: row.model_id } : undefined)
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
  const ok = await confirmDanger({
    title: '删除价格规则',
    message: `将删除「${row.model_id}」在「${row.upstream_name || '该上游'}」的价格规则。删除后该组合不再计价（成本记为 NULL），历史已产生成本不受影响。`,
  })
  if (!ok) return
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
  modelDialogRef.value?.open({ upstream_id: row.upstream_id, model_id: row.model_id })
}

// 「查看该模型已有规则」：切到规则 Tab，清掉上游过滤并把搜索词设为该模型，让跨上游的既有规则可见
function viewModelRules(row: UnpricedItem) {
  filterUpstream.value = ''
  filterModel.value = row.model_id
  page.value = 1
  activeTab.value = 'rules'
  loadRules()
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
const previewFormRef = ref<FormInstance>()
const previewRules: FormRules = {
  upstream_id: [{ required: true, message: '请选择上游', trigger: 'change' }],
  model_id: [{ required: true, message: '请输入模型 ID', trigger: 'blur' }],
}
const previewing = ref(false)
const previewResp = ref<PreviewResp | null>(null)
// 上次试算后参数又变更过 → 旧结果已过期，置 stale 提示
const previewStale = ref(false)
let previewRunKey = ''

// 参数快照（顺序固定），用于判定「参数已变更」
function previewParamKey(): string {
  return JSON.stringify(previewForm)
}

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
  if (!previewFormRef.value) return
  try {
    await previewFormRef.value.validate()
  } catch {
    return
  }
  const runKey = previewParamKey()
  previewing.value = true
  try {
    const resp = await pricingApi.preview(buildPreviewReq())
    previewResp.value = resp
    previewRunKey = runKey
    // 请求期间参数又被改动 → 结果视为过期，提示重新试算
    previewStale.value = previewParamKey() !== runKey
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    previewing.value = false
  }
}

// Enter 触发试算；排除 el-select/textarea 及日期/下拉等弹层控件（回车用于选中/确认时误触发）
function onPreviewEnter(e: KeyboardEvent) {
  const t = e.target as HTMLElement | null
  if (!t || typeof t.tagName !== 'string') return
  const tag = t.tagName.toLowerCase()
  if (tag === 'textarea') return
  if (
    t.closest(
      '.el-select, .el-select-dropdown, .el-date-editor, .el-picker-panel, .el-cascader, .el-cascader-panel',
    )
  ) {
    return
  }
  runPreview()
}

// 已有试算结果后任何参数变更 → 旧结果置为 stale
watch(previewParamKey, () => {
  if (previewResp.value && previewRunKey && previewParamKey() !== previewRunKey) {
    previewStale.value = true
  }
})

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
  previewFormRef.value?.clearValidate()
  previewRunKey = ''
  previewStale.value = false
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
  // 支持从模型详情跳转带入模型名（/pricing?model=xxx），直接定位到该模型的价格
  const qModel = typeof route.query.model === 'string' ? route.query.model.trim() : ''
  if (qModel) {
    activeTab.value = 'rules'
    filterModel.value = qModel
  }
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
.table-foot {
  display: flex;
  justify-content: flex-end;
  align-items: center;
  padding-top: 12px;
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
  color: #1f2329;
}
.seg-tip {
  font-size: 12px;
  line-height: 1.6;
  white-space: nowrap;
}
.rule-expand {
  padding: 4px 16px 8px 48px;
  background: #fafbfc;
}
</style>
