<template>
  <div class="page">
    <div class="toolbar">
      <el-radio-group v-model="range">
        <el-radio-button value="today">今天</el-radio-button>
        <el-radio-button value="7d">近 7 天</el-radio-button>
      </el-radio-group>
      <div class="spacer" />
      <el-radio-group v-model="currency" size="small">
        <el-radio-button value="CNY">CNY</el-radio-button>
        <el-radio-button value="USD">USD</el-radio-button>
      </el-radio-group>
    </div>

    <div v-loading="loading">
      <el-row :gutter="16">
        <el-col v-for="m in metricCards" :key="m.label" :xs="24" :sm="12" :md="8" :lg="4">
          <el-card class="stat-card" shadow="never">
            <div class="stat-label">{{ m.label }}</div>
            <div class="stat-value">{{ m.value }}</div>
            <div v-if="m.sub" class="stat-sub hint">{{ m.sub }}</div>
          </el-card>
        </el-col>
        <el-col :xs="24" :sm="12" :md="8" :lg="4">
          <el-card class="stat-card" shadow="never">
            <div class="stat-label">延迟</div>
            <div class="stat-value delay">
              <span>P50 <b>{{ fmtDur(summary?.p50_ms) }}</b></span>
              <span>P95 <b>{{ fmtDur(summary?.p95_ms) }}</b></span>
              <span>P99 <b>{{ fmtDur(summary?.p99_ms) }}</b></span>
            </div>
          </el-card>
        </el-col>
      </el-row>

      <el-card class="chart-card" shadow="never">
        <template #header>请求数 &amp; 成本趋势（{{ rangeLabel }}）</template>
        <EChart v-if="totalSeries.length" :option="lineOption" :loading="loading" height="320px" />
        <el-empty v-else description="暂无数据" :image-size="90" />
      </el-card>

      <el-row :gutter="16" class="top-row">
        <el-col :xs="24" :md="12">
          <el-card class="top-card" shadow="never">
            <template #header>模型 Top 5</template>
            <el-table v-if="modelTop.length" :data="modelTop" size="small">
              <el-table-column prop="name" label="名称" min-width="120" show-overflow-tooltip />
              <el-table-column label="请求数" width="90" align="right">
                <template #default="{ row }">{{ fmtInt(row.requests) }}</template>
              </el-table-column>
              <el-table-column label="Token 数" width="110" align="right">
                <template #default="{ row }">{{ fmtInt(row.tokens) }}</template>
              </el-table-column>
              <el-table-column label="成本" width="110" align="right">
                <template #default="{ row }">{{ fmtMoney(row.cost) }}</template>
              </el-table-column>
            </el-table>
            <el-empty v-else description="暂无数据" :image-size="80" />
          </el-card>
        </el-col>
        <el-col :xs="24" :md="12">
          <el-card class="top-card" shadow="never">
            <template #header>上游 Top 5</template>
            <el-table v-if="upstreamTop.length" :data="upstreamTop" size="small">
              <el-table-column prop="name" label="名称" min-width="120" show-overflow-tooltip />
              <el-table-column label="请求数" width="90" align="right">
                <template #default="{ row }">{{ fmtInt(row.requests) }}</template>
              </el-table-column>
              <el-table-column label="Token 数" width="110" align="right">
                <template #default="{ row }">{{ fmtInt(row.tokens) }}</template>
              </el-table-column>
              <el-table-column label="成本" width="110" align="right">
                <template #default="{ row }">{{ fmtMoney(row.cost) }}</template>
              </el-table-column>
            </el-table>
            <el-empty v-else description="暂无数据" :image-size="80" />
          </el-card>
        </el-col>
      </el-row>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import dayjs from 'dayjs'
import { ElMessage } from 'element-plus'
import EChart from '@/components/EChart.vue'
import type { EChartsOption } from 'echarts'
import { statsApi } from '@/api'
import { errMsg } from '@/api/http'
import type { SeriesDimension, SeriesGranularity, SeriesPoint, StatsCurrency, StatsQuery, StatsSummary } from '@/api/types'
import { fmtDur, fmtInt, fmtMoney, fmtPct } from '@/utils/format'

type Range = 'today' | '7d'

const range = ref<Range>('today')
const currency = ref<StatsCurrency>('CNY')
const loading = ref(false)

const summary = ref<StatsSummary | null>(null)
const totalSeries = ref<SeriesPoint[]>([])
const modelSeries = ref<SeriesPoint[]>([])
const upstreamSeries = ref<SeriesPoint[]>([])

const rangeLabel = computed(() => (range.value === '7d' ? '近 7 天' : '今天'))

const granularity = computed<SeriesGranularity>(() => (range.value === '7d' ? 'day' : 'hour'))

const metricCards = computed(() => {
  const s = summary.value
  return [
    { label: '请求数', value: fmtInt(s?.requests), sub: '' },
    { label: '成功率', value: fmtPct(s?.success_rate), sub: '' },
    { label: '总 Token 数', value: fmtInt(s?.total_tokens), sub: '' },
    {
      label: `成本（${currency.value}）`,
      value: fmtMoney(s?.cost_display),
      sub: s && s.cost_na_count > 0 ? `${s.cost_na_count} 条未计价` : '',
    },
  ]
})

interface TopRow {
  name: string
  requests: number
  tokens: number
  cost: number
}

function aggregateDim(points: SeriesPoint[]): TopRow[] {
  const map = new Map<string, TopRow>()
  for (const p of points) {
    const name = p.dimension || '—'
    const row = map.get(name) ?? { name, requests: 0, tokens: 0, cost: 0 }
    row.requests += p.requests
    row.tokens += (p.prompt_tokens ?? 0) + (p.completion_tokens ?? 0)
    row.cost += Number(p.cost_display ?? 0)
    map.set(name, row)
  }
  return [...map.values()].sort((a, b) => b.requests - a.requests).slice(0, 5)
}

const modelTop = computed(() => aggregateDim(modelSeries.value))
const upstreamTop = computed(() => aggregateDim(upstreamSeries.value))

function fmtBucket(bucket: string, g: SeriesGranularity): string {
  const d = dayjs(bucket)
  if (!d.isValid()) return bucket
  return g === 'hour' ? d.format('HH:mm') : d.format('MM-DD')
}

const lineOption = computed<EChartsOption>(() => {
  const pts = totalSeries.value
  const labels = pts.map((p) => fmtBucket(p.bucket, granularity.value))
  const requests = pts.map((p) => p.requests)
  const costs = pts.map((p) => (p.cost_display != null ? Number(p.cost_display) : null))
  return {
    tooltip: { trigger: 'axis' },
    legend: { data: ['请求数', `成本(${currency.value})`] },
    grid: { left: 10, right: 24, top: 36, bottom: 10, containLabel: true },
    xAxis: { type: 'category', boundaryGap: false, data: labels },
    yAxis: [
      { type: 'value', name: '请求数' },
      { type: 'value', name: `成本(${currency.value})` },
    ],
    series: [
      { name: '请求数', type: 'line', smooth: true, connectNulls: true, yAxisIndex: 0, data: requests },
      { name: `成本(${currency.value})`, type: 'line', smooth: true, connectNulls: true, yAxisIndex: 1, data: costs },
    ],
  }
})

function baseQuery(): StatsQuery {
  const q: StatsQuery = { currency: currency.value }
  if (range.value === '7d') {
    q.from_ts = dayjs().subtract(7, 'day').toISOString()
  }
  return q
}

function seriesQuery(dimension?: SeriesDimension): StatsQuery {
  const q = baseQuery()
  q.granularity = granularity.value
  if (dimension) q.dimension = dimension
  return q
}

let reqSeq = 0

async function loadAll() {
  const my = ++reqSeq
  loading.value = true
  try {
    const [sum, total, model, upstream] = await Promise.all([
      statsApi.summary(baseQuery()),
      statsApi.series(seriesQuery()),
      statsApi.series(seriesQuery('model')),
      statsApi.series(seriesQuery('upstream')),
    ])
    if (my !== reqSeq) return
    summary.value = sum
    totalSeries.value = total.points
    modelSeries.value = model.points
    upstreamSeries.value = upstream.points
  } catch (e) {
    if (my !== reqSeq) return
    summary.value = null
    totalSeries.value = []
    modelSeries.value = []
    upstreamSeries.value = []
    ElMessage.error(errMsg(e))
  } finally {
    if (my === reqSeq) loading.value = false
  }
}

onMounted(loadAll)
watch([range, currency], () => loadAll())
</script>

<style scoped>
.stat-card {
  margin-bottom: 16px;
  height: 100%;
  box-sizing: border-box;
}

.stat-label {
  font-size: 13px;
  color: #6b7280;
  margin-bottom: 8px;
}

.stat-value {
  font-size: 24px;
  font-weight: 600;
  color: #1f2329;
  line-height: 1.2;
}

.stat-value.delay {
  display: flex;
  flex-wrap: wrap;
  gap: 6px 16px;
  font-size: 14px;
  font-weight: 400;
}

.stat-value.delay b {
  font-weight: 600;
  color: #1f2329;
}

.stat-sub {
  margin-top: 6px;
}

.chart-card {
  margin-bottom: 16px;
}

.top-row {
  margin-top: 0;
}

.top-card {
  margin-bottom: 16px;
}
</style>
