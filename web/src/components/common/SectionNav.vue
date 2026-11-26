<template>
  <!-- 二级导航：宽屏左侧粘性竖列 / 窄屏（≤1000）分类卡片列表 → 详情
       /models、/settings 共用；窄屏进入详情后由父级把 listOpen 置 false，
       导航自身退化为一行「‹ 当前项」返回条 -->
  <nav v-if="!isCompact" class="section-nav" aria-label="子页面导航">
    <button
      v-for="it in items"
      :key="it.key"
      type="button"
      class="nav-item"
      :class="{ 'is-active': it.key === active }"
      :aria-current="it.key === active ? 'page' : undefined"
      @click="onSelect(it.key)"
    >
      <el-icon><component :is="it.icon" /></el-icon>
      <span class="nav-label">{{ it.label }}</span>
      <span v-if="it.badge" class="nav-badge" :title="it.badgeTitle">{{ it.badge }}</span>
    </button>
  </nav>

  <div v-else-if="listOpen" class="nav-cards">
    <div v-for="g in groups" :key="g.id" class="nav-card-group">
      <button
        v-for="it in g.items"
        :key="it.key"
        type="button"
        class="nav-card"
        @click="onCardSelect(it.key)"
      >
        <span class="nav-card-icon">
          <el-icon><component :is="it.icon" /></el-icon>
        </span>
        <span class="nav-card-body">
          <span class="nav-card-title">{{ it.label }}</span>
          <span v-if="it.desc" class="nav-card-sub">{{ it.desc }}</span>
        </span>
        <span v-if="it.badge" class="nav-card-badge" :title="it.badgeTitle">{{ it.badge }}</span>
        <el-icon class="nav-card-arrow"><ArrowRight /></el-icon>
      </button>
    </div>
  </div>

  <button v-else type="button" class="detail-back" @click="listOpen = true">
    <el-icon><ArrowLeft /></el-icon>
    <span>{{ activeLabel }}</span>
  </button>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import type { Component } from 'vue'
import { ArrowLeft, ArrowRight } from '@element-plus/icons-vue'
import { useBreakpoint } from '@/composables/useBreakpoint'

export interface SectionNavItem {
  key: string
  label: string
  icon: Component
  /** 窄屏卡片副标题（一行说明） */
  desc?: string
  /** 窄屏卡片分组：同组卡片并成一张卡，组间留白 */
  group?: string
  /** 状态徽标（异常数、未保存数等）；0/空不显示 */
  badge?: number | string
  badgeTitle?: string
}

const props = defineProps<{
  items: SectionNavItem[]
  active: string
}>()

const emit = defineEmits<{
  (e: 'select', key: string): void
}>()

/** 窄屏是否停在分类列表（父级 v-model:list-open 控制，未绑定时本地生效） */
const listOpen = defineModel<boolean>('listOpen', { default: true })

const { isCompact } = useBreakpoint()

/** 按 group 聚合成卡片组（未声明 group 的并成一组） */
const groups = computed(() => {
  const out: { id: string; items: SectionNavItem[] }[] = []
  for (const it of props.items) {
    const id = it.group ?? ''
    const last = out[out.length - 1]
    if (last && last.id === id) last.items.push(it)
    else out.push({ id, items: [it] })
  }
  return out
})

const activeLabel = computed(() => props.items.find((it) => it.key === props.active)?.label ?? '返回')

function onSelect(key: string) {
  if (key === props.active) return
  emit('select', key)
}

/** 窄屏点卡片：只发出选择，由父级决定是否收起列表（可能被未保存确认拦下） */
function onCardSelect(key: string) {
  emit('select', key)
}
</script>

<style scoped>
.section-nav {
  position: sticky;
  top: 16px;
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 8px;
  border: 1px solid var(--nx-border);
  border-radius: var(--nx-radius-lg);
  background: var(--nx-surface);
}

.nav-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 9px 10px;
  border: 0;
  border-radius: var(--nx-radius);
  background: transparent;
  color: var(--nx-text-2);
  font-size: var(--nx-font-small);
  text-align: left;
  cursor: pointer;
  transition: background-color var(--nx-dur-fast) var(--nx-ease),
    color var(--nx-dur-fast) var(--nx-ease);
}
.nav-item:hover {
  background: var(--nx-hover);
  color: var(--nx-text);
}
.nav-item.is-active {
  background: var(--nx-accent-soft);
  color: var(--nx-accent);
  font-weight: 600;
}
.nav-item:focus-visible {
  outline: 2px solid var(--nx-focus-ring);
  outline-offset: -2px;
}

.nav-label {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.nav-badge {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 16px;
  height: 16px;
  margin-left: auto;
  padding: 0 5px;
  border-radius: var(--nx-radius-round);
  background: var(--nx-accent-soft);
  color: var(--nx-accent);
  font-size: var(--nx-font-caption);
  font-weight: 600;
  line-height: 1;
  font-variant-numeric: tabular-nums;
}

/* ---------- 窄屏：分类卡片列表 ---------- */
.nav-cards {
  display: flex;
  flex-direction: column;
  gap: var(--nx-space-4);
}

.nav-card-group {
  display: flex;
  flex-direction: column;
  overflow: hidden;
  border: 1px solid var(--nx-border-subtle);
  border-radius: var(--nx-radius-lg);
  background: var(--nx-surface);
}

.nav-card {
  display: flex;
  align-items: center;
  gap: var(--nx-space-3);
  width: 100%;
  min-height: 64px;
  padding: 10px 16px;
  border: 0;
  background: transparent;
  color: inherit;
  text-align: left;
  cursor: pointer;
}
.nav-card + .nav-card {
  border-top: 1px solid var(--nx-border-subtle);
}
.nav-card:hover,
.nav-card:active {
  background: var(--nx-hover);
}
.nav-card:focus-visible {
  outline: 2px solid var(--nx-focus-ring);
  outline-offset: -2px;
}

.nav-card-icon {
  display: inline-flex;
  flex: none;
  align-items: center;
  justify-content: center;
  width: 36px;
  height: 36px;
  border-radius: var(--nx-radius-md);
  color: var(--nx-accent);
  background: color-mix(in srgb, currentColor 12%, transparent);
  font-size: var(--nx-font-lg);
}

.nav-card-body {
  display: flex;
  flex: 1;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.nav-card-title {
  color: var(--nx-text);
  font-size: var(--nx-font-title);
  font-weight: 500;
  line-height: 1.3;
}

.nav-card-sub {
  overflow: hidden;
  color: var(--nx-text-3);
  font-size: var(--nx-font-small);
  line-height: 1.3;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.nav-card-arrow {
  flex: none;
  color: var(--nx-text-disabled);
  font-size: var(--nx-font-title);
}

.nav-card-badge {
  display: inline-flex;
  flex: none;
  align-items: center;
  justify-content: center;
  min-width: 20px;
  height: 20px;
  padding: 0 6px;
  border-radius: var(--nx-radius-round);
  background: var(--nx-warning);
  color: var(--nx-on-accent);
  font-size: var(--nx-font-caption);
  font-weight: 600;
}

/* 窄屏详情态：返回条 */
.detail-back {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 4px 8px 4px 0;
  border: 0;
  background: transparent;
  color: var(--nx-accent);
  font-size: var(--nx-font-small);
  cursor: pointer;
}
.detail-back:hover {
  text-decoration: underline;
}
</style>