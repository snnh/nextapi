<template>
  <el-row :gutter="16">
    <el-col v-for="m in cards" :key="m.label" :xs="24" :sm="12" :md="8" :lg="4">
      <el-card class="stat-card" shadow="hover">
        <div class="stat-label">{{ m.label }}</div>
        <div class="stat-value">{{ m.value }}</div>
        <div v-if="m.sub" class="stat-sub hint">{{ m.sub }}</div>
      </el-card>
    </el-col>
    <el-col :xs="24" :sm="12" :md="8" :lg="4">
      <el-card class="stat-card" shadow="hover">
        <div class="stat-label">延迟</div>
        <div class="stat-value delay">
          <span>P50 <b>{{ fmtDur(summary?.p50_ms) }}</b></span>
          <span>P95 <b>{{ fmtDur(summary?.p95_ms) }}</b></span>
          <span>P99 <b>{{ fmtDur(summary?.p99_ms) }}</b></span>
        </div>
      </el-card>
    </el-col>
  </el-row>
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

const cards = computed(() => {
  const s = props.summary
  return [
    { label: '请求数', value: fmtInt(s?.requests), sub: '' },
    { label: '成功率', value: fmtPct(s?.success_rate), sub: '' },
    { label: '总 Tokens', value: fmtInt(s?.total_tokens), sub: '' },
    {
      label: `成本（${props.currency}）`,
      value: fmtMoney(s?.cost_display),
      sub: s && s.cost_na_count > 0 ? `${s.cost_na_count} 条未计价` : '',
    },
  ]
})
</script>

<style scoped>
.stat-card {
  margin-bottom: 16px;
}

.stat-label {
  font-size: 13px;
  color: #909399;
  margin-bottom: 8px;
}

.stat-value {
  font-size: 24px;
  font-weight: 600;
  color: #303133;
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
  color: #303133;
}

.stat-sub {
  margin-top: 6px;
}
</style>
