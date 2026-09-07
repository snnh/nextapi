<template>
  <div ref="el" :style="{ height, width: '100%' }" />
</template>

<script setup lang="ts">
// ECharts 封装：传入 option（notMerge 全量替换，适合 series 数量变化场景），自动 resize
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import * as echarts from 'echarts'
import type { EChartsOption } from 'echarts'

const props = withDefaults(
  defineProps<{
    option: EChartsOption
    height?: string
    loading?: boolean
  }>(),
  { height: '320px', loading: false },
)

const el = ref<HTMLDivElement>()
let chart: echarts.ECharts | null = null
let observer: ResizeObserver | null = null

// 全局色板：对齐石墨主题（OWC 配色），规避 ECharts 默认色板中的蓝/紫
const PALETTE = ['#3d444c', '#0f7b6c', '#c27a16', '#b42332', '#67777f', '#7a9e8f', '#8a6d3b', '#9aa7ae']

onMounted(() => {
  if (!el.value) return
  chart = echarts.init(el.value)
  chart.setOption({ color: PALETTE, ...props.option }, { notMerge: true })
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

watch(
  () => props.loading,
  (l) => {
    if (!chart) return
    if (l) chart.showLoading({ text: '加载中…' })
    else chart.hideLoading()
  },
)

onBeforeUnmount(() => {
  observer?.disconnect()
  chart?.dispose()
  chart = null
})

defineExpose({ getChart: () => chart })
</script>
