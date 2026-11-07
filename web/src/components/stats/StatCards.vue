<template>
  <!-- 统计汇总卡：与仪表盘视觉一致（CSS grid 自适应，含延迟分位卡） -->
  <!-- 加载中（summary 为 null）由格式化函数统一输出 '-'，避免把加载态误当「无数据」；
       父级 Stats.vue 通过 v-loading 展示整体加载遮罩，故此处不做骨架屏。 -->
  <div class="stat-grid">
    <el-card v-for="m in cards" :key="m.label" class="stat-card" shadow="never">
      <div class="stat-label">
        <el-tooltip :content="m.tip" placement="top">
          <span>{{ m.label }}</span>
        </el-tooltip>
      </div>
      <div class="stat-value">{{ m.value }}</div>
      <div v-if="m.sub" class="stat-sub hint">{{ m.sub }}</div>
    </el-card>

    <el-card class="stat-card" shadow="never">
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
import { STAT_LABEL_TIPS } from '@/utils/consts'

const props = defineProps<{
  summary: StatsSummary | null
  currency: StatsCurrency
}>()


interface CardItem {
  label: string
  value: string
  sub: string
  tip: string
}

/** Token 构成子标题（无数据时省略，避免全 0 噪声） */
function tokenSub(s: StatsSummary | null): string {
  if (!s) return ''
  const parts = [
    `输入 ${fmtInt(s.prompt_tokens)}`,
    `输出 ${fmtInt(s.completion_tokens)}`,
  ]
  if (s.cache_read_tokens > 0) parts.push(`缓存读 ${fmtInt(s.cache_read_tokens)}`)
  if (s.cache_write_tokens > 0) parts.push(`缓存写 ${fmtInt(s.cache_write_tokens)}`)
  return parts.join(' · ')
}

const cards = computed<CardItem[]>(() => {
  const s = props.summary
  return [
    { label: '请求数', value: fmtInt(s?.requests), sub: '', tip: STAT_LABEL_TIPS.requests },
    { label: '成功率', value: fmtPct(s?.success_rate), sub: '', tip: STAT_LABEL_TIPS.success },
    {
      label: '总 Tokens（含缓存）',
      value: fmtInt(s?.total_tokens),
      sub: tokenSub(s),
      tip: STAT_LABEL_TIPS.tokens,
    },
    {
      label: '缓存命中 Tokens',
      value: fmtInt(s?.cache_read_tokens),
      sub: '',
      tip: STAT_LABEL_TIPS.cacheRead,
    },
    {
      label: '缓存命中率',
      // 分母为 0（无输入 token）→ 后端 null → 展示 '-'，不误报 0%
      value: s?.cache_hit_rate != null ? fmtPct(s.cache_hit_rate) : '-',
      sub: '',
      tip: STAT_LABEL_TIPS.cacheHitRate,
    },
    {
      label: `成本（${props.currency}）`,
      value: fmtMoney(s?.cost_display),
      sub: s && s.cost_na_count > 0 ? `${s.cost_na_count} 条未计价` : '',
      tip: STAT_LABEL_TIPS.cost,
    },
  ]
})
</script>

<style scoped>
/* 固定 4 列：7 张卡（6 指标 + 延迟）排成两行；延迟卡整行，避免孤立一张卡 */
.stat-grid {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 16px;
  margin-bottom: 16px;
}

.stat-card:last-child {
  grid-column: 1 / -1;
}

@media (max-width: 1400px) {
  .stat-grid {
    grid-template-columns: repeat(3, minmax(0, 1fr));
  }
}

@media (max-width: 800px) {
  .stat-grid {
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 12px;
  }
  /* 移动端 Token 构成较长，独占一行 */
  .stat-card:nth-child(3) {
    grid-column: 1 / -1;
  }
}

.stat-card {
  min-width: 0; /* 允许内部省略号收缩 */
  box-sizing: border-box;
}

.stat-label {
  font-size: var(--nx-font-small);
  color: var(--el-text-color-secondary);
  margin-bottom: 8px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.stat-value {
  font-size: var(--nx-font-stat);
  font-weight: 600;
  color: var(--el-text-color-primary);
  line-height: 1.2;
  white-space: nowrap; /* 大数字单行溢出省略，防破卡 */
  overflow: hidden;
  text-overflow: ellipsis;
}

.stat-value.delay {
  display: flex;
  flex-wrap: wrap;
  gap: 6px 24px;
  font-size: var(--nx-font-body);
  font-weight: 400;
  white-space: normal;
  overflow: visible;
}

.stat-value.delay b {
  font-weight: 600;
  color: var(--el-text-color-primary);
}

.stat-sub {
  margin-top: 6px;
}
</style>
