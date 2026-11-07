import { onBeforeUnmount, onMounted, readonly, ref } from 'vue'

const MOBILE_MQ = '(max-width: 800px)'
const COMPACT_MQ = '(max-width: 1000px)'
const TABLET_MQ = '(max-width: 1400px)'

/**
 * 全站断点（单一来源）：
 * - mobile  ≤800：卡片列表 / 单列筛选 / 抽屉导航
 * - compact ≤1000：平板窄区间，表格再隐藏低优先列
 * - tablet  ≤1400：紧凑表格列，隐藏次要列
 * - desktop >1400：完整列
 * CSS 媒体查询与本文件的 800 / 1400 保持一致；1000 仅用于表格列收敛。
 */
export function useBreakpoint() {
  const isMobile = ref(false)
  const isCompact = ref(false)
  const isTablet = ref(false)
  let mobileMq: MediaQueryList | null = null
  let compactMq: MediaQueryList | null = null
  let tabletMq: MediaQueryList | null = null

  const sync = () => {
    isMobile.value = mobileMq?.matches ?? false
    isCompact.value = compactMq?.matches ?? false
    isTablet.value = tabletMq?.matches ?? false
  }

  onMounted(() => {
    mobileMq = window.matchMedia(MOBILE_MQ)
    compactMq = window.matchMedia(COMPACT_MQ)
    tabletMq = window.matchMedia(TABLET_MQ)
    sync()
    mobileMq.addEventListener('change', sync)
    compactMq.addEventListener('change', sync)
    tabletMq.addEventListener('change', sync)
  })

  onBeforeUnmount(() => {
    mobileMq?.removeEventListener('change', sync)
    compactMq?.removeEventListener('change', sync)
    tabletMq?.removeEventListener('change', sync)
  })

  return {
    isMobile: readonly(isMobile),
    isCompact: readonly(isCompact),
    isTablet: readonly(isTablet),
  }
}
