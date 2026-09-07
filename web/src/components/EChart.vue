<template>
  <div ref="el" role="img" :aria-label="ariaLabel" :style="{ height, width: '100%' }" />
</template>

<script setup lang="ts">
// ECharts 封装：传入 option（notMerge 全量替换，适合 series 数量变化场景），自动 resize；
// loading 时调用 showLoading()/hideLoading()，并暴露 role="img" + aria-label 便于无障碍读屏。
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import * as echarts from 'echarts'
import type { EChartsOption } from 'echarts'

const props = withDefaults(
  defineProps<{
    option: EChartsOption
    height?: string
    loading?: boolean
    /** 图表的无障碍描述（role="img" 的 aria-label） */
    ariaLabel?: string
  }>(),
  { height: '320px', loading: false, ariaLabel: '统计图表' },
)

const el = ref<HTMLDivElement>()
let chart: echarts.ECharts | null = null
let observer: ResizeObserver | null = null

// 全局色板：对齐石墨主题（OWC 配色），规避 ECharts 默认色板中的蓝/紫
const PALETTE = ['#3d444c', '#0f7b6c', '#c27a16', '#b42332', '#67777f', '#7a9e8f', '#8a6d3b', '#9aa7ae']

function applyLoading() {
  if (!chart) return
  if (props.loading) chart.showLoading({ text: '加载中…' })
  else chart.hideLoading()
}

onMounted(() => {
  if (!el.value) return
  chart = echarts.init(el.value)
  chart.setOption({ color: PALETTE, ...props.option }, { notMerge: true })
  applyLoading() // 覆盖挂载前 loading 已为 true 的场景
  observer = new ResizeObserver(() => chart?.resize())
  observer.observe(el.value)
})

watch(
  () => props.option,
  (opt) => {
    if (chart) chart.setOption({ color: PALETTE, ...opt }, { notMerge: true })
  },
  { deep: true },
)

watch(() => props.loading, applyLoading)

onBeforeUnmount(() => {
  observer?.disconnect()
  chart?.dispose()
  chart = null
})

defineExpose({ getChart: () => chart })
</script>
