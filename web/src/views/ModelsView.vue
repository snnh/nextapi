<template>
  <div class="page models-page">
    <div class="models-layout">
      <SectionNav
        v-model:list-open="navListOpen"
        :items="NAV_ITEMS"
        :active="activeTab"
        @select="onNavSelect"
      />

      <div v-if="!isCompact || !navListOpen" class="models-content">
        <!-- 已访问过的子页才挂载（v-show 保留筛选与滚动状态） -->
        <div v-if="visited.has('upstreams')" v-show="activeTab === 'upstreams'">
          <UpstreamsPanel />
        </div>
        <div v-if="visited.has('routes')" v-show="activeTab === 'routes'">
          <RoutesPanel />
        </div>
        <div v-if="visited.has('aliases')" v-show="activeTab === 'aliases'">
          <AliasesPanel />
        </div>
        <div v-if="visited.has('pricing')" v-show="activeTab === 'pricing'">
          <PricingPanel />
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { nextTick, onMounted, reactive, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { Coin, Connection, Operation, Switch } from '@element-plus/icons-vue'
import SectionNav, { type SectionNavItem } from '@/components/common/SectionNav.vue'
import { useBreakpoint } from '@/composables/useBreakpoint'
import UpstreamsPanel from '@/views/models/UpstreamsPanel.vue'
import RoutesPanel from '@/views/models/RoutesPanel.vue'
import AliasesPanel from '@/views/models/AliasesPanel.vue'
import PricingPanel from '@/views/models/PricingPanel.vue'

const TAB_KEYS = ['upstreams', 'routes', 'aliases', 'pricing'] as const
type TabKey = (typeof TAB_KEYS)[number]

const NAV_ITEMS: SectionNavItem[] = [
  { key: 'upstreams', label: '上游供应商', icon: Connection, desc: '实际提供模型的上游与额度' },
  { key: 'routes', label: '模型路由', icon: Operation, desc: '对外模型到上游的转发规则' },
  { key: 'aliases', label: '模型别名', icon: Switch, desc: '同一个模型的其它对外名字' },
  { key: 'pricing', label: '价格管理', icon: Coin, desc: '单价、分段规则与汇率' },
]

const route = useRoute()
const router = useRouter()

const activeTab = ref<TabKey>('upstreams')
const visited = reactive(new Set<TabKey>(['upstreams']))
/** 窄屏：是否停在子页列表（false 时显示当前子页） */
const navListOpen = ref(true)
const { isCompact } = useBreakpoint()

function tabFromQuery(): TabKey {
  const raw = route.query.tab
  const v = typeof raw === 'string' ? raw : ''
  return (TAB_KEYS as readonly string[]).includes(v) ? (v as TabKey) : 'upstreams'
}

function switchTab(key: string) {
  if (!(TAB_KEYS as readonly string[]).includes(key)) return
  activeTab.value = key as TabKey
  visited.add(key as TabKey)
}

/** 窄屏点子页卡片：切成功后收起列表，回到内容顶部 */
function onNavSelect(key: string) {
  switchTab(key)
  navListOpen.value = false
  nextTick(() => window.scrollTo({ top: 0, behavior: 'smooth' }))
}

// 页签同步到 URL（刷新 / 深链 / 分享保持）
watch(activeTab, (v) => {
  if (route.query.tab === v) return
  router.replace({ query: { ...route.query, tab: v } })
})

onMounted(() => {
  const initial = tabFromQuery()
  activeTab.value = initial
  visited.add(initial)
  // query 中存在但非法 → 移除，避免污染 URL
  const raw = route.query.tab
  if (typeof raw === 'string' && raw !== initial) {
    const q = { ...route.query }
    delete q.tab
    router.replace({ query: q })
  }
  // 窄屏从深链（合法 tab）进入时，直接展示内容而不是分类列表
  if (isCompact.value && raw === initial) navListOpen.value = false
})
</script>

<style scoped>
.models-layout {
  display: grid;
  grid-template-columns: 190px minmax(0, 1fr);
  gap: var(--nx-space-4);
  align-items: start;
}

.models-content {
  min-width: 0;
}

@media (max-width: 1000px) {
  .models-layout {
    grid-template-columns: 1fr;
  }
}
</style>