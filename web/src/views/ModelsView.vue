<template>
  <div class="page models-page">
    <div class="page-head">
      <p class="page-desc">
        上游、路由、别名与价格的统一工作台：先接上游，再把模型分发给调用方，最后按模型定价。
      </p>
    </div>

    <div class="models-layout">
      <SectionNav :items="NAV_ITEMS" :active="activeTab" @select="switchTab" />

      <div class="models-content">
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
import { onMounted, reactive, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { Coin, Connection, Operation, Switch } from '@element-plus/icons-vue'
import SectionNav, { type SectionNavItem } from '@/components/common/SectionNav.vue'
import UpstreamsPanel from '@/views/models/UpstreamsPanel.vue'
import RoutesPanel from '@/views/models/RoutesPanel.vue'
import AliasesPanel from '@/views/models/AliasesPanel.vue'
import PricingPanel from '@/views/models/PricingPanel.vue'

const TAB_KEYS = ['upstreams', 'routes', 'aliases', 'pricing'] as const
type TabKey = (typeof TAB_KEYS)[number]

const NAV_ITEMS: SectionNavItem[] = [
  { key: 'upstreams', label: '上游供应商', icon: Connection },
  { key: 'routes', label: '模型路由', icon: Operation },
  { key: 'aliases', label: '模型别名', icon: Switch },
  { key: 'pricing', label: '价格管理', icon: Coin },
]

const route = useRoute()
const router = useRouter()

const activeTab = ref<TabKey>('upstreams')
const visited = reactive(new Set<TabKey>(['upstreams']))

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