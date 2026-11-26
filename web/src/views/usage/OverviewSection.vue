<template>
  <div class="section-body">
    <!-- 段头：只放段名与段级操作；时间范围等设置留在筛选卡里 -->
    <div class="section-head">
      <div class="section-head-main">
        <h2 class="section-title">用量统计</h2>
        <span class="hint">{{ granularityHint }}</span>
      </div>
      <div class="section-head-actions">
        <el-button :icon="Refresh" :loading="loading" title="刷新统计" aria-label="刷新统计" @click="onQuery">
          <span v-if="!isMobile">刷新</span>
        </el-button>
        <el-button
          type="primary"
          plain
          :icon="Download"
          :loading="exporting"
          :disabled="!hasData"
          title="导出 CSV"
          aria-label="导出 CSV"
          @click="exportCsv"
        >
          <span v-if="!isMobile">导出 CSV</span>
        </el-button>
      </div>
    </div>

    <!-- 筛选：一行紧凑工具栏（自动查询 + 手动刷新 / 重置） -->
    <el-card shadow="never" class="section-card filter-card">
      <!-- 移动端：默认收起，只留「预设 · 结果数」与筛选/查询，把半屏还给数据 -->
      <div v-if="isMobile" class="filter-mobile-bar">
        <span class="filter-meta">{{ presetLabel }} · 共 {{ fmtInt(summary?.requests) }} 次请求</span>
        <span class="spacer" />
        <el-button class="filter-toggle" @click="mobileFilterOpen = !mobileFilterOpen">
          筛选
          <span v-if="activeFilterCount" class="filter-count">{{ activeFilterCount }}</span>
          <el-icon class="filter-caret" :class="{ 'is-open': mobileFilterOpen }"><ArrowDown /></el-icon>
        </el-button>
        <el-button v-if="!mobileFilterOpen" type="primary" :loading="loading" @click="onQuery">查询</el-button>
      </div>

      <div v-show="!isMobile || mobileFilterOpen">
        <!-- 时间：预设 + 实际区间 -->
        <div class="filter-row">
          <el-select v-model="rangePreset" class="filter-preset" aria-label="时间范围" @change="applyPreset">
            <el-option label="今日" value="today" />
            <el-option label="近 7 天" value="7d" />
            <el-option label="近 30 天" value="30d" />
            <el-option label="近 90 天" value="90d" />
            <el-option label="全部时间" value="all" />
            <el-option label="自定义" value="custom" />
          </el-select>
          <el-date-picker
            v-model="range"
            class="filter-range"
            type="datetimerange"
            :shortcuts="timeShortcuts"
            format="MM-DD HH:mm"
            range-separator="~"
            start-placeholder="开始时间"
            end-placeholder="结束时间"
            clearable
          />
        </div>

        <!-- 维度与范围条件：移动端两列，桌面一行 -->
        <div class="filter-row is-main">
          <span v-if="!isMobile" class="filter-meta">共 {{ fmtInt(summary?.requests) }} 次请求</span>
          <el-select v-model="dimension" class="filter-select" placeholder="不分组（总量）" aria-label="分组维度">
            <el-option v-for="o in DIMENSION_OPTIONS" :key="o.value" :label="o.label" :value="o.value" />
          </el-select>
          <el-select v-model="currency" class="filter-currency" aria-label="币种">
            <el-option v-for="c in CURRENCIES" :key="c" :label="c" :value="c" />
          </el-select>
          <el-select v-model="keyId" class="filter-select" filterable clearable placeholder="全部密钥" aria-label="密钥">
            <el-option v-for="k in keys" :key="k.id" :label="keyLabel(k)" :value="k.id" />
          </el-select>
          <el-select v-model="upstreamId" class="filter-select" filterable clearable placeholder="全部上游" aria-label="上游">
            <el-option v-for="u in upstreams" :key="u.id" :label="u.name" :value="u.id" />
          </el-select>
          <span class="spacer" />
          <template v-if="!isMobile">
            <el-button type="primary" :loading="loading" @click="onQuery">查询</el-button>
            <el-button :disabled="!hasActiveFilters" @click="onReset">重置</el-button>
          </template>
        </div>

        <!-- 移动端：操作放展开区底部，改完条件就地执行 -->
        <div v-if="isMobile" class="filter-actions-mobile">
          <el-button :disabled="!hasActiveFilters" @click="onReset">重置</el-button>
          <el-button type="primary" :loading="loading" @click="onQuery">查询</el-button>
        </div>
      </div>
    </el-card>

    <div>
      <!-- 汇总卡（跟着筛选范围走，不再单独成「梗概」段）；刷新时不遮罩，只补骨架 -->
      <StatCards :summary="summary" :currency="currency" :points="points" :loading="loading" />

      <!-- 趋势图 -->
      <el-card shadow="never" class="section-card">
        <template #header>
          <div class="chart-head">
            <span>{{ chartTitle }}</span>
            <div class="metric-switch">
              <el-radio-group v-model="metric" size="small">
                <el-radio-button value="requests">请求数</el-radio-button>
                <el-radio-button value="tokens">Token 数</el-radio-button>
                <el-radio-button value="cache">缓存命中</el-radio-button>
                <el-radio-button value="hit">命中率</el-radio-button>
                <el-radio-button value="cost">成本</el-radio-button>
              </el-radio-group>
            </div>
          </div>
        </template>
        <SkeletonBlock v-if="chartSkeleton" height="320px" radius="10px" />
        <EChart v-else-if="hasData" :option="chartOption" height="320px" />
        <el-empty v-else :description="emptyText" :image-size="90" />
        <div class="hint metric-note">
          Token = 输入 + 输出 + 缓存读 + 缓存写；命中率只统计输入侧。
        </div>
      </el-card>

      <div class="stats-split" :class="{ 'is-single': !dimension }">
      <!-- 维度明细表 -->
      <el-card v-if="dimension" shadow="never" class="section-card">
        <template #header>
          <div class="card-head-line">
            <span>维度明细</span>
            <span class="hint">{{ dimLabelText }}</span>
          </div>
        </template>
        <el-table :data="detailRows" max-height="420">
          <el-table-column v-if="!isMobile && !isCompact" label="时间" :width="splitLayout ? 116 : 140">
            <template #default="{ row }">{{ row.bucketText }}</template>
          </el-table-column>
          <el-table-column label="维度" :min-width="isMobile ? 118 : isCompact ? 126 : 140" show-overflow-tooltip>
            <template #default="{ row }">
              <span>{{ row.dimLabel }}</span>
              <span v-if="isMobile || isCompact" class="hint dim-time">{{ row.bucketText }}</span>
            </template>
          </el-table-column>
          <el-table-column label="请求数" :width="splitLayout ? 84 : isMobile ? 72 : isCompact ? 78 : 104" align="right">
            <template #default="{ row }">{{ fmtInt(row.requests) }}</template>
          </el-table-column>
          <el-table-column v-if="!isTablet" label="错误" :width="splitLayout ? 68 : 88" align="right">
            <template #default="{ row }">{{ fmtInt(row.errors) }}</template>
          </el-table-column>
          <el-table-column v-if="!isMobile && !isCompact" label="Token（含缓存）" :width="splitLayout ? 120 : 148" align="right">
            <template #default="{ row }">{{ fmtTokens(totalTokensOf(row)) }}</template>
          </el-table-column>
          <el-table-column v-if="!isTablet" label="缓存命中" :width="splitLayout ? 100 : 116" align="right">
            <template #default="{ row }">{{ fmtTokens(row.cache_read_tokens ?? 0) }}</template>
          </el-table-column>
          <el-table-column v-if="!isTablet" label="命中率" :width="splitLayout ? 80 : 96" align="right">
            <template #default="{ row }">{{ fmtPctOrNull(hitRateOf(row)) }}</template>
          </el-table-column>
          <el-table-column label="成本" :width="isMobile ? 96 : isCompact ? 108 : 128" align="right">
            <template #default="{ row }">{{ fmtMoney(row.cost_display) }}</template>
          </el-table-column>
        </el-table>
        <div class="table-foot">
          <span class="hint">共 {{ detailRows.length }} 行</span>
          <span class="hint">时间桶 {{ hourMode ? '按小时' : '按天' }}</span>
        </div>
      </el-card>

      <!-- 模型排行（P5-a 高级统计）：区间内 Top-N 模型，点击模型名钻取到日志页 -->
      <el-card shadow="never" class="section-card" v-loading="rankLoading">
        <template #header>
          <div class="rank-header">
            <span>模型排行 · Top {{ rankLimit }}</span>
            <div class="rank-actions">
              <el-select v-model="rankOrder" size="small" class="rank-sort" aria-label="排行排序">
                <el-option label="总 Token" value="total_tokens" />
                <el-option label="请求数" value="requests" />
                <el-option label="成本" value="cost" />
                <el-option label="缓存命中" value="cache_read" />
                <el-option label="错误数" value="errors" />
              </el-select>
              <el-select v-model="rankLimit" size="small" class="rank-limit" aria-label="排行条数">
                <el-option v-for="n in [10, 20, 50, 100]" :key="n" :label="`Top ${n}`" :value="n" />
              </el-select>
              <el-tooltip content="导出排行 CSV" placement="top">
                <el-button
                  size="small"
                  plain
                  :icon="Download"
                  :loading="exportingRank"
                  :disabled="!rankRows.length"
                  aria-label="导出排行 CSV"
                  @click="exportRankCsv"
                />
              </el-tooltip>
            </div>
          </div>
        </template>
        <el-table v-if="rankRows.length" :data="rankRows" max-height="420">
          <el-table-column label="模型" :min-width="isMobile ? 118 : isCompact ? 140 : splitLayout ? 130 : 180" show-overflow-tooltip>
            <template #default="{ row }">
              <el-link type="primary" :underline="false" @click="drillToLogs(row.model)">
                {{ row.model || '—' }}
              </el-link>
            </template>
          </el-table-column>
          <el-table-column label="请求数" :width="isMobile ? 72 : isCompact ? 78 : 100" align="right">
            <template #default="{ row }">{{ fmtInt(row.requests) }}</template>
          </el-table-column>
          <el-table-column v-if="!isTablet && !splitLayout" label="错误" width="84" align="right">
            <template #default="{ row }">{{ fmtInt(row.errors) }}</template>
          </el-table-column>
          <el-table-column v-if="!isMobile && !isCompact" label="Token（含缓存）" :width="splitLayout ? 112 : 150" align="right">
            <template #default="{ row }">{{ fmtTokens(row.total_tokens) }}</template>
          </el-table-column>
          <el-table-column v-if="!isTablet && !splitLayout" label="缓存命中" width="116" align="right">
            <template #default="{ row }">{{ fmtTokens(row.cache_read_tokens) }}</template>
          </el-table-column>
          <el-table-column v-if="!isTablet && !splitLayout" label="命中率" width="96" align="right">
            <template #default="{ row }">{{ fmtPctOrNull(row.cache_hit_rate) }}</template>
          </el-table-column>
          <el-table-column v-if="!isTablet && !splitLayout" label="平均延迟" width="104" align="right">
            <template #default="{ row }">{{ fmtDur(row.avg_latency_ms) }}</template>
          </el-table-column>
          <el-table-column label="成本" :width="isMobile ? 96 : isCompact ? 108 : 128" align="right">
            <template #default="{ row }">
              {{ fmtMoney(row.cost_display) }}
              <span v-if="row.cost_na_count > 0 && !isMobile" class="hint">（{{ row.cost_na_count }} 未计价）</span>
            </template>
          </el-table-column>
        </el-table>
        <ErrorState
          v-else-if="rankError"
          title="模型排行加载失败"
          :message="rankError"
          @retry="loadRank"
        />
        <EmptyState v-else title="当前条件下暂无模型数据" />
      </el-card>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
// 统计报表页（M8 §4.8）：汇总卡 + 趋势图 + 维度明细 + CSV 导出
import { computed, nextTick, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import dayjs from 'dayjs'
import { ElMessage } from 'element-plus'
import { ArrowDown, Download, Refresh } from '@element-plus/icons-vue'
import EChart from '@/components/EChart.vue'
import StatCards from '@/components/stats/StatCards.vue'
import SkeletonBlock from '@/components/common/SkeletonBlock.vue'
import ErrorState from '@/components/common/ErrorState.vue'
import EmptyState from '@/components/common/EmptyState.vue'
import type { EChartsOption } from 'echarts'
import { keyApi, statsApi, upstreamApi } from '@/api'
import { errMsg } from '@/api/http'
import type {
  ApiKeyRow,
  ModelRankOrder,
  ModelRankRow,
  SeriesDimension,
  SeriesPoint,
  StatsCurrency,
  StatsQuery,
  StatsSummary,
  UpstreamOut,
} from '@/api/types'
import { CURRENCIES, DIMENSION_OPTIONS, PROTOCOL_IN_LABELS, RANGE_PRESET_LABELS } from '@/utils/consts'
import { downloadBlob } from '@/utils/download'
import { fmtDur, fmtInt, fmtMoney, fmtPct, fmtTokens } from '@/utils/format'
import { useBreakpoint } from '@/composables/useBreakpoint'

/** 趋势图指标：请求数 / 总 Token（含缓存）/ 缓存命中 Token / 缓存命中率 / 成本 */
type Metric = 'requests' | 'tokens' | 'cache' | 'hit' | 'cost'

// —— 筛选状态 ——
const range = ref<[Date, Date] | null>([
  dayjs().subtract(7, 'day').toDate(),
  dayjs().toDate(),
])
const dimension = ref<SeriesDimension>('')
const currency = ref<StatsCurrency>('CNY')

/** 时间范围预设（比 datetimerange 控件窄一半，保证筛选条一横条不折行） */
type RangePreset = 'today' | '7d' | '30d' | '90d' | 'all' | 'custom'
const rangePreset = ref<RangePreset>('7d')
let programmaticRange = false

function applyPreset(v: string | number | boolean | undefined) {
  const preset = typeof v === 'string' ? (v as RangePreset) : '7d'
  rangePreset.value = preset
  if (preset === 'custom') return
  programmaticRange = true
  if (preset === 'all') range.value = null
  else if (preset === 'today') range.value = [dayjs().startOf('day').toDate(), dayjs().toDate()]
  else {
    const days = preset === '7d' ? 7 : preset === '30d' ? 30 : 90
    range.value = [dayjs().subtract(days, 'day').toDate(), dayjs().toDate()]
  }
  void nextTick(() => {
    programmaticRange = false
  })
}
const keyId = ref('')
const upstreamId = ref('')
const keys = ref<ApiKeyRow[]>([])
const upstreams = ref<UpstreamOut[]>([])
const metric = ref<Metric>('requests')
const { isMobile, isCompact, isTablet } = useBreakpoint()

/** 并排布局：>1400 且有维度明细（与 CSS 的 min-width: 1401px 对齐） */
const splitLayout = computed(() => !!dimension.value && !isTablet.value)

// —— 模型排行（P5-a）——
const route = useRoute()
const router = useRouter()
const rankOrder = ref<ModelRankOrder>('total_tokens')
const rankLimit = ref(10)
const rankRows = ref<ModelRankRow[]>([])
/** 排行独立加载态（与整页 loading 解耦：排行慢不遮住已返回的主查询结果） */
const rankLoading = ref(false)
/** 排行加载失败原因（失败即清空行，空态常驻展示原因；错误期间禁用 CSV 导出） */
const rankError = ref('')

/** 总 Token（含缓存）：明细行与序列行同形状，统一在此计算 */
function totalTokensOf(r: {
  prompt_tokens?: number | null
  completion_tokens?: number | null
  cache_read_tokens?: number | null
  cache_write_tokens?: number | null
}): number {
  return (
    (r.prompt_tokens ?? 0) +
    (r.completion_tokens ?? 0) +
    (r.cache_read_tokens ?? 0) +
    (r.cache_write_tokens ?? 0)
  )
}

/** 输入侧缓存命中率 = cache_read / (prompt + cache_read + cache_write)；分母 0 → null。
 *  分量各自钳 0（与后端 stats::cache_hit_rate 同式），避免脏数据算出 >100%。 */
function hitRateOf(r: {
  prompt_tokens?: number | null
  cache_read_tokens?: number | null
  cache_write_tokens?: number | null
}): number | null {
  const cr = Math.max(0, r.cache_read_tokens ?? 0)
  const input =
    Math.max(0, r.prompt_tokens ?? 0) + cr + Math.max(0, r.cache_write_tokens ?? 0)
  if (input <= 0) return null
  return cr / input
}

/** 命中率展示：null → '-'（分母为 0 时不误报 0%） */
function fmtPctOrNull(v: number | null | undefined): string {
  return v === null || v === undefined ? '-' : fmtPct(v)
}

/** 钻取：跳转日志页并带上当前区间/筛选 + 模型名（Logs 页挂载时从 URL query 恢复） */
function drillToLogs(model: string) {
  const q: Record<string, string> = { model }
  if (range.value) {
    q.from_ts = dayjs(range.value[0]).toISOString()
    q.to_ts = dayjs(range.value[1]).toISOString()
  }
  if (keyId.value) q.key_id = keyId.value
  if (upstreamId.value) q.upstream_id = upstreamId.value
  router.push({ name: 'logs', query: q })
}

// —— 数据状态 ——
const loading = ref(false)
const exporting = ref(false)
/** 模型排行 CSV 导出中 */
const exportingRank = ref(false)
/** 是否成功加载过一次（区分「从未有数据」与「当前区间无数据」） */
const loadedOnce = ref(false)
const summary = ref<StatsSummary | null>(null)
const points = ref<SeriesPoint[]>([])

const timeShortcuts = [
  { text: '今天', value: () => [dayjs().startOf('day').toDate(), dayjs().toDate()] },
  { text: '近 7 天', value: () => [dayjs().subtract(7, 'day').toDate(), dayjs().toDate()] },
  { text: '近 30 天', value: () => [dayjs().subtract(30, 'day').toDate(), dayjs().toDate()] },
  { text: '近 90 天', value: () => [dayjs().subtract(90, 'day').toDate(), dayjs().toDate()] },
]

// —— 粒度/数据源自动推断（仅用于前端 UI 提示与 x 轴格式，不随请求发送） ——
/** 明细卡头显示当前维度（总览时留空，避免重复「总览」） */
const dimLabelText = computed(() => {
  if (!dimension.value) return ''
  return DIMENSION_OPTIONS.find((o) => o.value === dimension.value)?.label ?? ''
})

const spanHours = computed(() => {
  if (!range.value) return 7 * 24
  return dayjs(range.value[1]).diff(dayjs(range.value[0]), 'hour')
})
const hourMode = computed(() => spanHours.value <= 48)

/** 段头粒度提示：只讲用户关心的汇总粒度，后端实现不暴露 */
const granularityHint = computed(() => (hourMode.value ? '按小时汇总' : '按天汇总'))

/** 最近一次加载失败的原因（从未成功加载时用于空态提示） */
const loadError = ref('')
const emptyText = computed(() => {
  if (loadedOnce.value) return '当前条件下暂无数据'
  return loadError.value ? `加载失败：${loadError.value}` : '加载中…'
})

// —— URL query 同步（分享/刷新/返回保持筛选条件） ——
function qv(key: string): string {
  const v = route.query[key]
  if (Array.isArray(v)) return v[0] ?? ''
  return v ?? ''
}

function canonicalQuery(q: Record<string, string>): string {
  return Object.keys(q)
    .filter((k) => q[k] !== '')
    .sort()
    .map((k) => `${k}=${q[k]}`)
    .join('&')
}

function restoreFromQuery() {
  const from = qv('from_ts')
  const to = qv('to_ts')
  const d1 = new Date(from)
  const d2 = new Date(to)
  if (from && to && !Number.isNaN(d1.getTime()) && !Number.isNaN(d2.getTime())) {
    range.value = [d1, d2]
  }

  const dim = qv('dimension')
  if (DIMENSION_OPTIONS.some((o) => o.value === dim)) {
    dimension.value = dim as SeriesDimension
  }

  const cur = qv('currency')
  if ((CURRENCIES as readonly string[]).includes(cur)) {
    currency.value = cur as StatsCurrency
  }

  keyId.value = qv('key_id')
  upstreamId.value = qv('upstream_id')

  const m = qv('metric')
  if (['requests', 'tokens', 'cache', 'hit', 'cost'].includes(m)) {
    metric.value = m as Metric
  }

  const ro = qv('rank_order')
  if (['total_tokens', 'requests', 'cost', 'cache_read', 'errors'].includes(ro)) {
    rankOrder.value = ro as ModelRankOrder
  }

  const rl = Number.parseInt(qv('rank_limit'), 10)
  if ([10, 20, 50, 100].includes(rl)) {
    rankLimit.value = rl
  }
}

function syncUrl() {
  const q: Record<string, string> = {}
  if (range.value) {
    q.from_ts = dayjs(range.value[0]).toISOString()
    q.to_ts = dayjs(range.value[1]).toISOString()
  }
  if (dimension.value) q.dimension = dimension.value
  q.currency = currency.value
  if (keyId.value) q.key_id = keyId.value
  if (upstreamId.value) q.upstream_id = upstreamId.value
  q.metric = metric.value
  q.rank_order = rankOrder.value
  q.rank_limit = String(rankLimit.value)
  if (canonicalQuery(q) === canonicalQuery(route.query as Record<string, string>)) return
  router.replace({ path: route.path, query: q })
}

restoreFromQuery()

// —— 下拉选项加载 ——
async function loadFilterOptions() {
  try {
    const [k, u] = await Promise.all([keyApi.list(), upstreamApi.list()])
    keys.value = k
    upstreams.value = u
  } catch (e) {
    ElMessage.error(`Key/上游选项加载失败：${errMsg(e)}`)
  }
}

// —— 展示辅助 ——
const sortedPoints = computed<SeriesPoint[]>(() =>
  [...points.value].sort((a, b) => a.bucket.localeCompare(b.bucket)),
)
const hasData = computed(() => sortedPoints.value.length > 0)
/** 首次加载（尚无任何桶）时用骨架占位，刷新时保留旧图不闪白 */
const chartSkeleton = computed(() => loading.value && !hasData.value)
/** 是否存在非默认筛选：默认值时「重置」无意义，置灰避免误点 */
/** 移动端筛选区展开状态（默认收起，一次只看数据） */
const mobileFilterOpen = ref(false)
/** 生效的筛选条件数：移动端收起时也要能看到「有几条生效」 */
const activeFilterCount = computed(
  () =>
    [dimension.value, keyId.value, upstreamId.value].filter((v) => v !== '').length +
    (currency.value !== 'CNY' ? 1 : 0) +
    (rangePreset.value !== '7d' ? 1 : 0),
)
const presetLabel = computed(() => RANGE_PRESET_LABELS[rangePreset.value] ?? '自定义区间')

const hasActiveFilters = computed(
  () =>
    dimension.value !== '' ||
    keyId.value !== '' ||
    upstreamId.value !== '' ||
    currency.value !== 'CNY' ||
    rangePreset.value !== '7d',
)

function buildBaseQuery(): StatsQuery {
  const q: StatsQuery = { currency: currency.value }
  if (range.value) {
    q.from_ts = dayjs(range.value[0]).toISOString()
    q.to_ts = dayjs(range.value[1]).toISOString()
  }
  if (keyId.value) q.key_id = keyId.value
  if (upstreamId.value) q.upstream_id = upstreamId.value
  return q
}

function buildSeriesQuery(): StatsQuery {
  const q = buildBaseQuery()
  const d = dimension.value
  if (d) q.dimension = d
  return q
}

let reqSeq = 0
/** 排行请求的独立竞态序号（与主查询各自计数，互不干扰） */
let rankSeq = 0

// 查询期间不清空旧数据：失败保留旧 summary/points 仅提示，空态只在从未成功加载过时出现。
// 主查询与「模型排行」彻底解耦：排行恒走明细源（长区间可能慢/503），
// 若与主查询同批 await，会让已返回的 summary/series 被整页 loading 遮罩拖住不渲染。
async function loadMain() {
  const my = ++reqSeq
  loading.value = true
  try {
    const [sum, ser] = await Promise.all([
      statsApi.summary(buildBaseQuery()),
      statsApi.series(buildSeriesQuery()),
    ])
    if (my !== reqSeq) return
    summary.value = sum
    points.value = ser.points
    loadedOnce.value = true
    loadError.value = ''
  } catch (e) {
    if (my !== reqSeq) return
    // 保留旧数据，不重置为空态
    const m = errMsg(e)
    loadError.value = m
    ElMessage.error(m)
  } finally {
    if (my === reqSeq) loading.value = false
  }
}

/** 模型排行：独立加载态与独立竞态序号；失败清空旧行（避免表头排序/区间与表内数据不符） */
async function loadRank() {
  const my = ++rankSeq
  rankLoading.value = true
  try {
    // 排行与区间/Key/上游筛选一致（不受 dimension 影响：恒按模型聚合）
    const r = await statsApi.modelRank({
      ...buildBaseQuery(),
      order_by: rankOrder.value,
      limit: rankLimit.value,
    })
    if (my !== rankSeq) return
    rankRows.value = r.rows
    rankError.value = ''
  } catch (e) {
    if (my !== rankSeq) return
    rankRows.value = []
    rankError.value = errMsg(e)
    ElMessage.warning(`模型排行加载失败：${rankError.value}`)
  } finally {
    if (my === rankSeq) rankLoading.value = false
  }
}

/** 并发触发两路查询，互不阻塞（各自维护 loading 与错误态） */
function loadData() {
  syncUrl()
  void loadMain()
  void loadRank()
}

/** 「查询」按钮：手动立即触发（先取消待执行的自动防抖查询） */
let autoTimer: ReturnType<typeof setTimeout> | null = null
let rankTimer: ReturnType<typeof setTimeout> | null = null
function onQuery() {
  if (autoTimer) {
    clearTimeout(autoTimer)
    autoTimer = null
  }
  if (rankTimer) {
    clearTimeout(rankTimer)
    rankTimer = null
  }
  loadData()
}

function onReset() {
  if (autoTimer) clearTimeout(autoTimer)
  if (rankTimer) clearTimeout(rankTimer)
  applyPreset('7d')
  dimension.value = ''
  currency.value = 'CNY'
  keyId.value = ''
  upstreamId.value = ''
  metric.value = 'requests'
  rankOrder.value = 'total_tokens'
  rankLimit.value = 10
}

// 时间/币种/维度/Key/上游 变化 → 主查询 + 排行都刷新：300ms 防抖
watch(
  range,
  () => {
    if (!programmaticRange) rangePreset.value = 'custom'
  },
  { deep: true },
)

watch([range, currency, dimension, keyId, upstreamId], () => {
  if (autoTimer) clearTimeout(autoTimer)
  autoTimer = setTimeout(loadData, 300)
})

watch(metric, syncUrl)

// 排行的排序字段/条数变化 → 只刷新排行，不重拉 summary/series（后者可能走明细 + 百分位，代价高）
watch([rankOrder, rankLimit], () => {
  if (rankTimer) clearTimeout(rankTimer)
  rankTimer = setTimeout(() => {
    syncUrl()
    loadRank()
  }, 300)
})

onMounted(() => {
  loadFilterOptions()
  loadData()
})

function fmtBucket(bucket: string): string {
  const d = dayjs(bucket)
  if (!d.isValid()) return bucket
  return hourMode.value ? d.format('HH:mm') : d.format('MM-DD')
}

function fmtBucketFull(bucket: string): string {
  const d = dayjs(bucket)
  if (!d.isValid()) return bucket
  return hourMode.value ? d.format('MM-DD HH:mm') : d.format('YYYY-MM-DD')
}

function keyLabel(k: ApiKeyRow): string {
  return `${k.name || '(未命名)'} · ${k.prefix}`
}

const keyNameMap = computed(() => new Map(keys.value.map((k) => [k.id, k.name || k.prefix])))
const upstreamNameMap = computed(() => new Map(upstreams.value.map((u) => [u.id, u.name])))

/** 维度原始值 → 展示名：协议映射常量表；key/upstream 的 UUID 解析为名称，未知保留原值 */
function dimLabel(d: string): string {
  if (!d) return '—'
  if (dimension.value === 'protocol') return PROTOCOL_IN_LABELS[d] ?? d
  if (dimension.value === 'key') return keyNameMap.value.get(d) ?? d
  if (dimension.value === 'upstream') return upstreamNameMap.value.get(d) ?? d
  return d
}

function truncateLabel(s: string, max = 16): string {
  return s.length > max ? `${s.slice(0, max)}…` : s
}

// 仪表/图表标题
const chartTitle = computed(() => {
  const opt = DIMENSION_OPTIONS.find((o) => o.value === dimension.value)
  return `趋势 · ${opt?.label ?? '总量'}`
})

// —— 维度明细表（中文数值感知排序：模型名等含数字时稳定） ——
const detailCollator = new Intl.Collator('zh-Hans-CN', { numeric: true })

const detailRows = computed(() =>
  sortedPoints.value
    .map((p) => ({
      ...p,
      bucketText: fmtBucketFull(p.bucket),
      dimLabel: dimLabel(p.dimension),
    }))
    .sort((a, b) => detailCollator.compare(a.dimLabel, b.dimLabel) || a.bucket.localeCompare(b.bucket)),
)

// —— 趋势图 ——
const metricName = computed(() => {
  if (metric.value === 'tokens') return '总 Token 数（含缓存）'
  if (metric.value === 'cache') return '缓存命中 Token'
  if (metric.value === 'hit') return '缓存命中率'
  if (metric.value === 'cost') return `成本(${currency.value})`
  return '请求数'
})

/** 当前指标的数值（请求/总Token/缓存命中/命中率/成本），null 表示无值 */
function metricValue(p: SeriesPoint): number | null {
  if (metric.value === 'tokens') return totalTokensOf(p)
  if (metric.value === 'cache') return p.cache_read_tokens ?? 0
  if (metric.value === 'hit') return hitRateOf(p)
  if (metric.value === 'cost') return p.cost_display != null ? Number(p.cost_display) : null
  return p.requests
}

/** 指标值格式化：命中率百分比、成本货币千分位、计数千分位（tooltip / 图例共用） */
function fmtMetricValue(v: unknown): string {
  if (v === null || v === undefined || v === '') return '-'
  const n = Number(v)
  if (!Number.isFinite(n)) return String(v)
  if (metric.value === 'hit') return fmtPct(n)
  if (metric.value === 'cost') return fmtMoney(n)
  // Token 类指标统一 M/T 单位，与表格/KPI 卡同口径
  if (metric.value === 'tokens' || metric.value === 'cache') return fmtTokens(n)
  return fmtInt(n)
}

/** 坐标轴数字：命中率百分比、成本货币格式，其余大数缩写 + 千分位 */
function fmtAxisValue(n: number): string {
  if (metric.value === 'hit') return `${(n * 100).toFixed(0)}%`
  if (metric.value === 'cost') return fmtMoney(n)
  if (metric.value === 'tokens' || metric.value === 'cache') return fmtTokens(n)
  return n.toLocaleString('zh-CN')
}

interface TooltipParam {
  axisValueLabel?: string | number
  name?: string
  marker?: string
  seriesName?: string
  value?: unknown
}

/** 自定义 tooltip：请求数/Token 千分位、成本货币格式 */
function axisTooltip(params: unknown): string {
  const list = (Array.isArray(params) ? params : [params]) as TooltipParam[]
  const head = list.find((p) => p.axisValueLabel !== undefined)?.axisValueLabel
  const title = head !== undefined && head !== null ? String(head) : ''
  const rows: string[] = []
  for (const it of list) {
    const v = it.value
    if (v === null || v === undefined) continue
    const marker = it.marker ?? ''
    const name = it.seriesName ?? ''
    rows.push(`${marker}${name}<span style="float:right;margin-left:16px">${fmtMetricValue(v)}</span>`)
  }
  const body = rows.join('<br/>')
  if (!body) return title
  return title ? `${title}<br/>${body}` : body
}

const grid = { left: 10, right: 24, top: 40, bottom: 10, containLabel: true }

const chartOption = computed<EChartsOption>(() => {
  const pts = sortedPoints.value
  const labels = pts.map((p) => fmtBucket(p.bucket))

  // 单指标语义：每个 metric 只画自身序列；单轴即可，双轴仅未来出现双单位序列时才引入。
  const axisLabel = {
    formatter: (n: number) => fmtAxisValue(n),
  }

  if (!dimension.value) {
    let seriesName: string
    let seriesType: 'bar' | 'line'
    let data: (number | null)[]
    if (metric.value === 'requests') {
      seriesName = '请求数'
      seriesType = 'bar'
      data = pts.map((p) => p.requests)
    } else if (metric.value === 'tokens') {
      seriesName = '总 Token 数（含缓存）'
      seriesType = 'line'
      data = pts.map(totalTokensOf)
    } else if (metric.value === 'cache') {
      seriesName = '缓存命中 Token'
      seriesType = 'line'
      data = pts.map((p) => p.cache_read_tokens ?? 0)
    } else if (metric.value === 'hit') {
      // 命中率：单轴百分比（0–1 数值 + 轴/tooltip 百分比格式化），不引双轴
      seriesName = '缓存命中率'
      seriesType = 'line'
      data = pts.map(hitRateOf)
    } else {
      seriesName = `成本(${currency.value})`
      seriesType = 'line'
      data = pts.map((p) => (p.cost_display != null ? Number(p.cost_display) : null))
    }
    return {
      tooltip: { trigger: 'axis', formatter: axisTooltip },
      legend: { data: [seriesName], top: 8 },
      grid,
      xAxis: { type: 'category', boundaryGap: seriesType === 'bar', data: labels },
      yAxis: { type: 'value', axisLabel },
      series: [
        seriesType === 'bar'
          ? { name: seriesName, type: 'bar', barMaxWidth: 48, data }
          : { name: seriesName, type: 'line', smooth: true, connectNulls: true, data },
      ],
    }
  }

  // 分组：按维度聚合，取请求数 Top 8，当前指标单独成线
  // 维度分组返回的是 (bucket × dimension) 逐行展开：labels 必须先按 bucket 去重，
  // 否则同一时间点渲染 N 次且各线数据错位（发布审阅前端 M1）。
  const buckets = [...new Set(pts.map((p) => p.bucket))]
  const dimLabels = buckets.map(fmtBucket)

  const totals = new Map<string, number>()
  for (const p of pts) totals.set(p.dimension, (totals.get(p.dimension) ?? 0) + p.requests)
  const topDims = [...totals.entries()]
    .sort((a, b) => b[1] - a[1])
    .slice(0, 8)
    .map(([raw]) => ({ raw, label: dimLabel(raw) }))

  const series = topDims.map(({ raw, label }) => {
    const byBuck = new Map<string, number | null>()
    for (const p of pts) {
      if (p.dimension !== raw) continue
      byBuck.set(p.bucket, metricValue(p))
    }
    return {
      name: label,
      type: 'line' as const,
      smooth: true,
      connectNulls: true,
      data: buckets.map((b) => byBuck.get(b) ?? null),
    }
  })

  return {
    tooltip: { trigger: 'axis', formatter: axisTooltip },
    legend: {
      data: topDims.map((d) => d.label),
      formatter: (name: string) => truncateLabel(name),
      top: 8,
    },
    grid,
    xAxis: { type: 'category', boundaryGap: false, data: dimLabels },
    yAxis: { type: 'value', name: metricName.value, axisLabel },
    series,
  }
})

// —— CSV 导出 ——
function csvCell(v: unknown): string {
  if (v === null || v === undefined) return '""'
  let s = String(v)
  // 公式注入中和（CWE-1236）：= + - @ 及制表符开头前置单引号
  if (/^[=+\-@\t\r]/.test(s)) s = `'${s}`
  return `"${s.replace(/"/g, '""')}"`
}

/** 导出文件名：stats_起止日期_币种.csv */
function statsCsvFilename(): string {
  const fmt = 'YYYYMMDD'
  const f = range.value ? dayjs(range.value[0]).format(fmt) : dayjs().format(fmt)
  const t = range.value ? dayjs(range.value[1]).format(fmt) : dayjs().format(fmt)
  return `stats_${f}_${t}_${currency.value}.csv`
}

async function exportCsv() {
  exporting.value = true
  try {
    const header = [
      'bucket',
      'dimension',
      'requests',
      'errors',
      'prompt_tokens',
      'completion_tokens',
      'cache_read_tokens',
      'cache_write_tokens',
      'total_tokens',
      'cache_hit_rate',
      'cost_display',
    ]
    const lines = ['# nextapi stats（总 Token 含缓存读/写；命中率=cache_read/(prompt+cache_read+cache_write)）', header.map(csvCell).join(',')]
    for (const p of sortedPoints.value) {
      const hit = hitRateOf(p)
      lines.push(
        [
          p.bucket,
          dimLabel(p.dimension),
          p.requests,
          p.errors,
          p.prompt_tokens ?? '',
          p.completion_tokens ?? '',
          p.cache_read_tokens ?? '',
          p.cache_write_tokens ?? '',
          totalTokensOf(p),
          hit === null ? '' : hit.toFixed(6),
          p.cost_display ?? '',
        ]
          .map(csvCell)
          .join(','),
      )
    }
    const blob = new Blob(['\ufeff' + lines.join('\n')], { type: 'text/csv;charset=utf-8' })
    const filename = statsCsvFilename()
    downloadBlob(blob, filename)
    ElMessage.success(`已导出 ${filename}`)
  } catch (e) {
    ElMessage.error(`导出失败：${errMsg(e)}`)
  } finally {
    exporting.value = false
  }
}

/** 导出模型排行 CSV（与页面排行表列一致） */
async function exportRankCsv() {
  exportingRank.value = true
  try {
    const header = [
      'model',
      'requests',
      'errors',
      'prompt_tokens',
      'completion_tokens',
      'cache_read_tokens',
      'cache_write_tokens',
      'total_tokens',
      'cache_hit_rate',
      'avg_latency_ms',
      'cost_display',
    ]
    const lines = [
      `# nextapi stats model-rank（order_by=${rankOrder.value}, top ${rankLimit.value}）`,
      header.map(csvCell).join(','),
    ]
    for (const r of rankRows.value) {
      lines.push(
        [
          r.model,
          r.requests,
          r.errors,
          r.prompt_tokens,
          r.completion_tokens,
          r.cache_read_tokens,
          r.cache_write_tokens,
          r.total_tokens,
          r.cache_hit_rate === null ? '' : r.cache_hit_rate.toFixed(6),
          r.avg_latency_ms ?? '',
          r.cost_display ?? '',
        ]
          .map(csvCell)
          .join(','),
      )
    }
    const blob = new Blob(['\ufeff' + lines.join('\n')], { type: 'text/csv;charset=utf-8' })
    const filename = statsCsvFilename().replace(/^stats_/, 'stats_models_')
    downloadBlob(blob, filename)
    ElMessage.success(`已导出 ${filename}`)
  } catch (e) {
    ElMessage.error(`导出失败：${errMsg(e)}`)
  } finally {
    exportingRank.value = false
  }
}
// 供 /usage 页头「刷新全部」调用
defineExpose({ refresh: onQuery })
</script>

<style scoped>
.granularity-hint {
  margin-top: 12px;
}

.chart-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-wrap: wrap;
}

.metric-switch {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
}

.metric-note {
  margin-top: 10px;
}



.dim-time {
  display: block;
  font-size: var(--nx-font-caption);
}

.rank-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-wrap: wrap;
}

.rank-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

@media (max-width: 800px) {
  .metric-switch :deep(.el-radio-button__inner) {
    padding: 5px 8px;
  }
}

/* ---------- 排版：设置区降级 + 明细/排行并排 ---------- */
.card-head-line {
  display: flex;
  align-items: baseline;
  gap: var(--nx-space-2);
  flex-wrap: wrap;
}

.stats-split {
  display: grid;
  grid-template-columns: minmax(0, 1fr);
  gap: var(--nx-space-4);
  margin-bottom: var(--nx-space-4);
}
.stats-split > .section-card {
  margin-bottom: 0;
  min-width: 0;
}
/* 宽屏才并排：明细 2/3 + 排行 1/3，窄屏保持纵向 */
@media (min-width: 1401px) {
  .stats-split {
    grid-template-columns: minmax(0, 2fr) minmax(0, 1fr);
    align-items: start;
  }
}


/* 并排容器：无维度明细时单列 */
.stats-split.is-single {
  grid-template-columns: minmax(0, 1fr);
}

/* 排行卡头：并排后宽度有限，控件收紧 */
.rank-header {
  flex-wrap: wrap;
  gap: var(--nx-space-2);
}
.rank-actions {
  gap: var(--nx-space-1);
}
.rank-sort {
  width: 116px;
}
.rank-limit {
  width: 84px;
}

/* 表尾：行数与口径 */
.table-foot {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--nx-space-2);
  margin-top: var(--nx-space-2);
  padding-top: var(--nx-space-2);
  border-top: 1px solid var(--nx-border-subtle);
}

/* 指标切换：窄屏横向滚动，不换行挤压图高 */
.metric-switch {
  max-width: 100%;
  overflow-x: auto;
  scrollbar-width: none;
}
.metric-switch::-webkit-scrollbar {
  display: none;
}


</style>
