<template>
  <div ref="el" role="img" :aria-label="ariaLabel" :style="{ height, width: '100%' }" />
</template>

<script setup lang="ts">
// ECharts 封装：传入 option（notMerge 全量替换，适合 series 数量变化场景），自动 resize；
// loading 时调用 showLoading()/hideLoading()，并暴露 role="img" + aria-label 便于无障碍读屏。
// 主题：从 --nx-chart-* 读取色板/坐标轴/tooltip，浅/深主题切换时自动重绘。
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import * as echarts from 'echarts'
import type { EChartsOption } from 'echarts'
import { useTheme } from '@/composables/useTheme'
import { readChartTheme, type ChartTheme } from '@/utils/chartTheme'

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
const { resolved } = useTheme()

type PlainObject = Record<string, unknown>

function isPlainObject(value: unknown): value is PlainObject {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

/** 只补默认值，不覆盖页面显式设置的轴/tooltip 配置 */
function themeAxis(axis: unknown, theme: ChartTheme): unknown {
  if (Array.isArray(axis)) return axis.map((item) => themeAxis(item, theme))
  if (!isPlainObject(axis)) return axis
  const source = axis
  const axisLine = isPlainObject(source.axisLine) ? source.axisLine : null
  const splitLine = isPlainObject(source.splitLine) ? source.splitLine : null
  return {
    ...source,
    axisLabel: isPlainObject(source.axisLabel)
      ? { color: theme.axis, ...source.axisLabel }
      : { color: theme.axis },
    axisLine: axisLine
      ? { ...axisLine, lineStyle: { color: theme.split, ...(isPlainObject(axisLine.lineStyle) ? axisLine.lineStyle : {}) } }
      : { lineStyle: { color: theme.split } },
    splitLine: splitLine
      ? { ...splitLine, lineStyle: { color: theme.split, ...(isPlainObject(splitLine.lineStyle) ? splitLine.lineStyle : {}) } }
      : { lineStyle: { color: theme.split } },
  }
}

function withTheme(option: EChartsOption): EChartsOption {
  const theme = readChartTheme()
  const tooltip = isPlainObject(option.tooltip) ? option.tooltip : {}
  const legend = isPlainObject(option.legend) ? option.legend : {}
  const textStyle = isPlainObject(option.textStyle) ? option.textStyle : {}
  const themed: PlainObject = {
    ...option,
    color: theme.palette,
    textStyle: { color: theme.axis, ...textStyle },
    tooltip: {
      backgroundColor: theme.tooltipBg,
      borderColor: theme.tooltipBorder,
      borderWidth: 1,
      textStyle: { color: theme.tooltipText },
      ...tooltip,
    },
    legend: { textStyle: { color: theme.axis }, ...legend },
  }
  if (option.xAxis !== undefined) themed.xAxis = themeAxis(option.xAxis, theme)
  if (option.yAxis !== undefined) themed.yAxis = themeAxis(option.yAxis, theme)
  return themed as EChartsOption
}

function applyLoading() {
  if (!chart) return
  if (props.loading) {
    const theme = readChartTheme()
    chart.showLoading('default', {
      text: '加载中…',
      color: theme.palette[0],
      textColor: theme.axis,
      maskColor: theme.loadingMask,
      zlevel: 0,
    })
  } else {
    chart.hideLoading()
  }
}

function render() {
  if (!chart) return
  chart.setOption(withTheme(props.option), { notMerge: true })
}

onMounted(() => {
  if (!el.value) return
  chart = echarts.init(el.value)
  render()
  applyLoading() // 覆盖挂载前 loading 已为 true 的场景
  observer = new ResizeObserver(() => chart?.resize())
  observer.observe(el.value)
})

watch(
  () => props.option,
  () => render(),
  { deep: true },
)

// 浅色/深色切换：重取 token 并重绘（不销毁实例）
watch(resolved, () => {
  render()
  applyLoading()
})

watch(() => props.loading, applyLoading)

onBeforeUnmount(() => {
  observer?.disconnect()
  chart?.dispose()
  chart = null
})

</script>