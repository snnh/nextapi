<template>
  <!-- 二级导航：桌面左侧粘性导航 / 移动端顶部下拉（/models、/settings 共用） -->
  <nav class="section-nav" aria-label="子页面导航">
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

  <el-select
    class="section-nav-select"
    :model-value="active"
    aria-label="子页面导航"
    @change="onMobileChange"
  >
    <el-option v-for="it in items" :key="it.key" :label="it.label" :value="it.key" />
  </el-select>
</template>

<script setup lang="ts">
import type { Component } from 'vue'

export interface SectionNavItem {
  key: string
  label: string
  icon: Component
  /** 状态徽标（异常数、未定价数等）；0/空不显示 */
  badge?: number | string
  badgeTitle?: string
}

const props = defineProps<{
  items: SectionNavItem[]
  active: string
}>()

const emit = defineEmits<{ (e: 'select', key: string): void }>()

function onSelect(key: string) {
  if (key === props.active) return
  emit('select', key)
}

function onMobileChange(v: string | number | boolean | undefined) {
  if (typeof v === 'string') onSelect(v)
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
  background: var(--nx-warning-soft);
  color: var(--nx-warning);
  font-size: 10px;
  font-weight: 600;
  line-height: 1;
}

.section-nav-select {
  display: none;
  width: 100%;
}

@media (max-width: 1000px) {
  .section-nav {
    display: none;
  }
  .section-nav-select {
    display: block;
    margin-bottom: 12px;
  }
}
</style>