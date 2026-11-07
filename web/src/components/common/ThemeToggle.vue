<template>
  <el-dropdown trigger="click" @command="onCommand">
    <el-button
      class="theme-toggle"
      text
      :title="`主题：${currentLabel}`"
      :aria-label="`切换主题，当前${currentLabel}`"
      aria-haspopup="menu"
    >
      <el-icon><component :is="currentIcon" /></el-icon>
    </el-button>
    <template #dropdown>
      <el-dropdown-menu>
        <el-dropdown-item
          v-for="option in options"
          :key="option.value"
          :command="option.value"
          :class="{ 'is-current': mode === option.value }"
        >
          <span class="theme-item">
            <el-icon><component :is="option.icon" /></el-icon>
            <span>{{ option.label }}</span>
            <el-icon v-if="mode === option.value" class="theme-check"><Check /></el-icon>
          </span>
        </el-dropdown-item>
      </el-dropdown-menu>
    </template>
  </el-dropdown>
</template>

<script setup lang="ts">
import { computed, type Component } from 'vue'
import { Check, Monitor, Moon, Sunny } from '@element-plus/icons-vue'
import { useTheme, type ThemeMode } from '@/composables/useTheme'

interface ThemeOption {
  value: ThemeMode
  label: string
  icon: Component
}

const options: ThemeOption[] = [
  { value: 'light', label: '浅色', icon: Sunny },
  { value: 'dark', label: '深色', icon: Moon },
  { value: 'system', label: '跟随系统', icon: Monitor },
]

const { mode, setMode } = useTheme()

const currentLabel = computed(() => options.find((o) => o.value === mode.value)?.label ?? '浅色')
const currentIcon = computed(() => options.find((o) => o.value === mode.value)?.icon ?? Sunny)

function onCommand(command: string | number | object) {
  if (command === 'light' || command === 'dark' || command === 'system') {
    setMode(command)
  }
}
</script>

<style scoped>
.theme-toggle {
  width: 32px;
  height: 32px;
  padding: 0;
  color: var(--nx-text-2);
  border-radius: var(--nx-radius);
}
.theme-toggle:hover {
  color: var(--nx-text);
  background: var(--nx-hover);
}
.theme-toggle :deep(.el-icon) {
  font-size: var(--el-font-size-large);
}
.theme-item {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 120px;
}
.theme-check {
  margin-left: auto;
  color: var(--nx-accent);
}
@media (max-width: 800px) {
  .theme-toggle {
    width: 40px;
    height: 40px;
  }
}
</style>