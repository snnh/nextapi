<template>
  <!-- 统计汇总卡：与仪表盘视觉一致（CSS grid 自适应，含延迟分位卡） -->
  <!-- 加载中（summary 为 null）由格式化函数统一输出 '-'，避免把加载态误当「无数据」；
       父级 Stats.vue 通过 v-loading 展示整体加载遮罩，故此处不做骨架屏。 -->
  <div class="stat-grid">
    <el-card v-for="m in cards" :key="m.label" class="stat-card" shadow="hover">
      <div class="stat-label">
        <el-tooltip :content="m.tip" placement="top">
          <span>{{ m.label }}</span>
        </el-tooltip>
      </div>
      <div class="stat-value">{{ m.value }}</div>
      <div v-if="m.sub" class="stat-sub hint">{{ m.sub }}</div>
    </el-card>

    <el-card class="stat-card" shadow="hover">
      <div class="stat-label">
        <el-tooltip content="响应耗时的分位延迟（P50/P95/P99）" placement="top">
          <span>延迟</span>
        </el-tooltip>
      </div>
      <div class="stat-value delay">
        <el-tooltip content="P50 分位延迟：50% 的请求 ≤ 该耗时" placement="top">
          <span>P50 <b>{{ fmtDur(summary?.p50_ms) }}</b></span>
        </el-tooltip>
        <el-tooltip content="P95 分位延迟：95% 的请求 ≤ 该耗时" placement="top">
          <span>P95 <b>{{ fmtDur(summary?.p95_ms) }}</b></span>
        </el-tooltip>
        <el-tooltip content="P99 分位延迟：99% 的请求 ≤ 该耗时" placement="top">
          <span>P99 <b>{{ fmtDur(summary?.p99_ms) }}</b></span>
        </el-tooltip>
      </div>
    </el-card>
  </div>
</template>

<script setup lang="ts">
// 统计汇总卡：与仪表盘视觉一致（请求数/成功率/Tokens/成本 + 延迟分位）
import { computed } from 'vue'
import type { StatsCurrency, StatsSummary } from '@/api/types'
import { fmtDur, fmtInt, fmtMoney, fmtPct } from '@/utils/format'

const props = defineProps<{
  summary: StatsSummary | null
  currency: StatsCurrency
}>()

// 指标释义（与 Dashboard.vue 同口径）
const LABEL_TIPS = {
  requests: '统计区间内的网关请求总数',
  success: '成功率 = 状态码 < 400 的请求占比',
  tokens: '总 Token 数 = 输入（提示）+ 输出（生成）token 合计',
  cost: '按所选币种折算的合计成本；未计价（无定价规则）条数不计入',
} as const

interface CardItem {
  label: string
  value: string
  sub: string
  tip: string
}

const cards = computed<CardItem[]>(() => {
  const s = props.summary
  return [
    { label: '请求数', value: fmtInt(s?.requests), sub: '', tip: LABEL_TIPS.requests },
    { label: '成功率', value: fmtPct(s?.success_rate), sub: '', tip: LABEL_TIPS.success },
    { label: '总 Tokens', value: fmtInt(s?.total_tokens), sub: '', tip: LABEL_TIPS.tokens },
    {
      label: `成本（${props.currency}）`,
      value: fmtMoney(s?.cost_display),
      sub: s && s.cost_na_count > 0 ? `${s.cost_na_count} 条未计价` : '',
      tip: LABEL_TIPS.cost,
    },
  ]
})
</script>

<style scoped>
/* 与仪表盘同款：自适应列数网格，避免 el-col 固定 4 格在宽屏留白 */
.stat-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(170px, 1fr));
  gap: 16px;
  margin-bottom: 16px;
}

.stat-card {
  min-width: 0; /* 允许内部省略号收缩 */
  box-sizing: border-box;
}

.stat-label {
  font-size: 13px;
  color: #6b7280;
  margin-bottom: 8px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.stat-value {
  font-size: 24px;
  font-weight: 600;
  color: #1f2329;
  line-height: 1.2;
  white-space: nowrap; /* 大数字单行溢出省略，防破卡 */
  overflow: hidden;
  text-overflow: ellipsis;
}

.stat-value.delay {
  display: flex;
  flex-wrap: wrap;
  gap: 6px 16px;
  font-size: 14px;
  font-weight: 400;
  white-space: normal;
  overflow: visible;
}

.stat-value.delay b {
  font-weight: 600;
  color: #1f2329;
}

.stat-sub {
  margin-top: 6px;
}
</style>
