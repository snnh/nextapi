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

onMounted(() => {
  if (!el.value) return
  chart = echarts.init(el.value)
  chart.setOption(props.option, { notMerge: true })
  observer = new ResizeObserver(() => chart?.resize())
  observer.observe(el.value)
})

watch(
  () => props.option,
  (opt) => {
    if (chart) chart.setOption(opt, { notMerge: true })
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
