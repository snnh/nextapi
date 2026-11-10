export interface ChartTheme {
  palette: string[]
  axis: string
  split: string
  tooltipBg: string
  tooltipBorder: string
  tooltipText: string
  loadingMask: string
}

const FALLBACK: ChartTheme = {
  palette: ['#2fa45e', '#7d6238', '#4f6b8a', '#96424a', '#6b5d73', '#8a7a4a', '#3e7e83', '#5c6b73'],
  axis: '#637168',
  split: '#e1e7e2',
  tooltipBg: '#ffffff',
  tooltipBorder: '#e1e7e2',
  tooltipText: '#1b231e',
  loadingMask: 'rgba(255,255,255,.78)',
}

function readVar(style: CSSStyleDeclaration, name: string, fallback: string): string {
  return style.getPropertyValue(name).trim() || fallback
}

/** 从当前 html 主题读取图表 token（切换主题后重新调用） */
export function readChartTheme(): ChartTheme {
  if (typeof window === 'undefined') return FALLBACK
  const style = getComputedStyle(document.documentElement)
  return {
    palette: FALLBACK.palette.map((fallback, index) =>
      readVar(style, `--nx-chart-${index + 1}`, fallback),
    ),
    axis: readVar(style, '--nx-chart-axis', FALLBACK.axis),
    split: readVar(style, '--nx-chart-split', FALLBACK.split),
    tooltipBg: readVar(style, '--nx-chart-tooltip-bg', FALLBACK.tooltipBg),
    tooltipBorder: readVar(style, '--nx-chart-tooltip-border', FALLBACK.tooltipBorder),
    tooltipText: readVar(style, '--nx-chart-tooltip-text', FALLBACK.tooltipText),
    loadingMask: readVar(style, '--nx-chart-loading-mask', FALLBACK.loadingMask),
  }
}