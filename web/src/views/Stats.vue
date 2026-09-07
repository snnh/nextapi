<template>
  <div class="page">
    <!-- 顶部工具栏：说明 + CSV 导出 -->
    <div class="toolbar">
      <div class="hint">按时间范围 / 维度 / 币种查询统计；粒度由后端自动选择。</div>
      <div class="spacer" />
      <el-button type="primary" plain :icon="Download" :disabled="!hasData" @click="exportCsv">
        导出 CSV
      </el-button>
    </div>

    <!-- 筛选工具栏 -->
    <el-card shadow="never" class="page-card">
      <div class="filter-grid">
        <div class="f-item">
          <span class="f-label">时间范围</span>
          <el-date-picker
            v-model="range"
            type="datetimerange"
            :shortcuts="timeShortcuts"
            range-separator="至"
            start-placeholder="开始时间"
            end-placeholder="结束时间"
            clearable
            style="width: 360px"
          />
        </div>
        <div class="f-item">
          <span class="f-label">维度</span>
          <el-select v-model="dimension" style="width: 140px">
            <el-option v-for="o in DIMENSION_OPTIONS" :key="o.value" :label="o.label" :value="o.value" />
          </el-select>
        </div>
        <div class="f-item">
          <span class="f-label">币种</span>
          <el-select v-model="currency" style="width: 92px">
            <el-option v-for="c in CURRENCIES" :key="c" :label="c" :value="c" />
          </el-select>
        </div>
        <div class="filter-actions">
          <el-button type="primary" :icon="Search" :loading="loading" @click="loadData">查询</el-button>
        </div>
      </div>
      <div class="hint granularity-hint">{{ granularityHint }}</div>
    </el-card>

    <div v-loading="loading">
      <!-- 汇总卡 -->
      <StatCards :summary="summary" :currency="currency" />

      <!-- 趋势图 -->
      <el-card shadow="never" class="page-card">
        <template #header>{{ chartTitle }}</template>
        <EChart v-if="hasData" :option="chartOption" height="360px" />
        <el-empty v-else description="暂无数据" :image-size="90" />
        <div class="metric-switch">
          <span class="hint">切换指标：</span>
          <el-radio-group v-model="metric" size="small">
            <el-radio-button value="requests">请求数</el-radio-button>
            <el-radio-button value="tokens">Token 数</el-radio-button>
            <el-radio-button value="cost">成本</el-radio-button>
          </el-radio-group>
        </div>
      </el-card>

      <!-- 维度明细表 -->
      <el-card v-if="dimension" shadow="never" class="page-card">
        <template #header>维度明细</template>
        <el-table :data="detailRows" stripe max-height="420">
          <el-table-column label="时间" width="140">
            <template #default="{ row }">{{ row.bucketText }}</template>
          </el-table-column>
          <el-table-column prop="dimLabel" label="维度" min-width="140" show-overflow-tooltip />
          <el-table-column label="请求数" width="104" align="right">
            <template #default="{ row }">{{ fmtInt(row.requests) }}</template>
          </el-table-column>
          <el-table-column label="错误" width="88" align="right">
            <template #default="{ row }">{{ fmtInt(row.errors) }}</template>
          </el-table-column>
          <el-table-column label="Token 数" width="128" align="right">
            <template #default="{ row }">{{ fmtInt((row.prompt_tokens ?? 0) + (row.completion_tokens ?? 0)) }}</template>
          </el-table-column>
          <el-table-column label="成本" width="128" align="right">
            <template #default="{ row }">{{ fmtMoney(row.cost_display) }}</template>
          </el-table-column>
        </el-table>
      </el-card>
    </div>
  </div>
</template>

<script setup lang="ts">
// 统计报表页（M8 §4.8）：汇总卡 + 趋势图 + 维度明细 + CSV 导出
import { computed, onMounted, ref, watch } from 'vue'
import dayjs from 'dayjs'
import { ElMessage } from 'element-plus'
import { Download, Search } from '@element-plus/icons-vue'
import EChart from '@/components/EChart.vue'
import StatCards from '@/components/stats/StatCards.vue'
import type { EChartsOption } from 'echarts'
import { statsApi } from '@/api'
import { errMsg } from '@/api/http'
import type {
  SeriesDimension,
  SeriesPoint,
  StatsCurrency,
  StatsQuery,
  StatsSummary,
} from '@/api/types'
import { CURRENCIES, DIMENSION_OPTIONS, PROTOCOL_IN_LABELS } from '@/utils/consts'
import { downloadBlob } from '@/utils/download'
import { fmtInt, fmtMoney } from '@/utils/format'

type Metric = 'requests' | 'tokens' | 'cost'

// —— 筛选状态 ——
const range = ref<[Date, Date] | null>([
  dayjs().subtract(7, 'day').toDate(),
  dayjs().toDate(),
])
const dimension = ref<SeriesDimension>('')
const currency = ref<StatsCurrency>('CNY')
const metric = ref<Metric>('requests')

// —— 数据状态 ——
const loading = ref(false)
const summary = ref<StatsSummary | null>(null)
const points = ref<SeriesPoint[]>([])

const timeShortcuts = [
  { text: '今天', value: () => [dayjs().startOf('day').toDate(), dayjs().toDate()] },
  { text: '近 7 天', value: () => [dayjs().subtract(7, 'day').toDate(), dayjs().toDate()] },
  { text: '近 30 天', value: () => [dayjs().subtract(30, 'day').toDate(), dayjs().toDate()] },
  { text: '近 90 天', value: () => [dayjs().subtract(90, 'day').toDate(), dayjs().toDate()] },
]

// —— 粒度自动推断（仅用于前端 UI 提示与 x 轴格式，不随请求发送） ——
const spanHours = computed(() => {
  if (!range.value) return 7 * 24
  return dayjs(range.value[1]).diff(dayjs(range.value[0]), 'hour')
})
const spanDays = computed(() => Math.round(spanHours.value / 24))
const hourMode = computed(() => spanHours.value <= 48)

const granularityHint = computed(() => {
  if (hourMode.value) return '区间 ≤2 天：自动按小时粒度。'
  if (spanDays.value > 7) {
    return '区间 >7 天：自动按天粒度。未筛选 Key/上游/协议时使用预聚合，维度归并为 model。'
  }
  return '区间 >2 天：自动按天粒度。'
})

// —— 数据加载（请求序号防旧响应覆盖） ——
const sortedPoints = computed<SeriesPoint[]>(() =>
  [...points.value].sort((a, b) => a.bucket.localeCompare(b.bucket)),
)
const hasData = computed(() => sortedPoints.value.length > 0)

function buildBaseQuery(): StatsQuery {
  const q: StatsQuery = { currency: currency.value }
  if (range.value) {
    q.from_ts = dayjs(range.value[0]).toISOString()
    q.to_ts = dayjs(range.value[1]).toISOString()
  }
  return q
}

function buildSeriesQuery(): StatsQuery {
  const q = buildBaseQuery()
  const d = dimension.value
  if (d) q.dimension = d
  return q
}

let reqSeq = 0

async function loadData() {
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
  } catch (e) {
    if (my !== reqSeq) return
    summary.value = null
    points.value = []
    ElMessage.error(errMsg(e))
  } finally {
    if (my === reqSeq) loading.value = false
  }
}

onMounted(loadData)
watch([range, currency, dimension], () => loadData())

// —— 展示辅助 ——
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

function dimLabel(d: string): string {
  if (dimension.value === 'protocol' && PROTOCOL_IN_LABELS[d]) return PROTOCOL_IN_LABELS[d]
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

// —— 维度明细表 ——
const detailRows = computed(() =>
  sortedPoints.value
    .map((p) => ({
      ...p,
      bucketText: fmtBucketFull(p.bucket),
      dimLabel: dimLabel(p.dimension),
    }))
    .sort((a, b) => a.dimLabel.localeCompare(b.dimLabel) || a.bucket.localeCompare(b.bucket)),
)

// —— 趋势图 ——
const metricName = computed(() => {
  if (metric.value === 'tokens') return 'Token 数'
  if (metric.value === 'cost') return `成本(${currency.value})`
  return '请求数'
})

function metricValue(p: SeriesPoint): number | null {
  if (metric.value === 'tokens') return (p.prompt_tokens ?? 0) + (p.completion_tokens ?? 0)
  if (metric.value === 'cost') return p.cost_display != null ? Number(p.cost_display) : null
  return p.requests
}

const grid = { left: 10, right: 24, top: 40, bottom: 10, containLabel: true }

const chartOption = computed<EChartsOption>(() => {
  const pts = sortedPoints.value
  const labels = pts.map((p) => fmtBucket(p.bucket))

  if (!dimension.value) {
    if (metric.value === 'requests') {
      return {
        tooltip: { trigger: 'axis' },
        legend: { data: ['请求数', `成本(${currency.value})`], top: 8 },
        grid,
        xAxis: { type: 'category', boundaryGap: true, data: labels },
        yAxis: [
          { type: 'value' },
          { type: 'value', splitLine: { show: false } },
        ],
        series: [
          { name: '请求数', type: 'bar', barMaxWidth: 48, data: pts.map((p) => p.requests), yAxisIndex: 0 },
          {
            name: `成本(${currency.value})`,
            type: 'line',
            smooth: true,
            connectNulls: true,
            data: pts.map((p) => (p.cost_display != null ? Number(p.cost_display) : null)),
            yAxisIndex: 1,
          },
        ],
      }
    }
    if (metric.value === 'tokens') {
      return {
        tooltip: { trigger: 'axis' },
        legend: { data: ['总 Token 数'], top: 8 },
        grid,
        xAxis: { type: 'category', boundaryGap: false, data: labels },
        yAxis: { type: 'value' },
        series: [
          {
            name: '总 Token 数',
            type: 'line',
            smooth: true,
            data: pts.map((p) => (p.prompt_tokens ?? 0) + (p.completion_tokens ?? 0)),
          },
        ],
      }
    }
    const costData = pts.map((p) => (p.cost_display != null ? Number(p.cost_display) : null))
    return {
      tooltip: { trigger: 'axis' },
      legend: { data: [`成本(${currency.value})`], top: 8 },
      grid,
      xAxis: { type: 'category', boundaryGap: false, data: labels },
      yAxis: { type: 'value' },
      series: [
        {
          name: `成本(${currency.value})`,
          type: 'line',
          smooth: true,
          connectNulls: true,
          data: costData,
        },
      ],
    }
  }

  // 分组：按维度聚合，取请求数 Top 8，各指标单独成线
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
    tooltip: { trigger: 'axis' },
    legend: {
      data: topDims.map((d) => d.label),
      formatter: (name: string) => truncateLabel(name),
      top: 8,
    },
    grid,
    xAxis: { type: 'category', boundaryGap: false, data: dimLabels },
    yAxis: { type: 'value', name: metricName.value },
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

function exportCsv() {
  const header = ['bucket', 'dimension', 'requests', 'errors', 'prompt_tokens', 'completion_tokens', 'cost_display']
  const lines = ['# nextapi stats', header.map(csvCell).join(',')]
  for (const p of sortedPoints.value) {
    lines.push(
      [p.bucket, dimLabel(p.dimension), p.requests, p.errors, p.prompt_tokens ?? '', p.completion_tokens ?? '', p.cost_display ?? '']
        .map(csvCell)
        .join(','),
    )
  }
  const blob = new Blob(['\ufeff' + lines.join('\n')], { type: 'text/csv;charset=utf-8' })
  downloadBlob(blob, 'stats.csv')
  ElMessage.success('已导出 stats.csv')
}
</script>

<style scoped>
.filter-grid {
  display: flex;
  flex-wrap: wrap;
  gap: 14px 18px;
  align-items: center;
}

.f-item {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.f-label {
  color: #4b5563;
  font-size: 13px;
  white-space: nowrap;
}

.filter-actions {
  margin-left: auto;
}

.granularity-hint {
  margin-top: 12px;
}

.metric-switch {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 12px;
}
</style>
