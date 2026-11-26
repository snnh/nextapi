<template>
  <el-dropdown trigger="click" @command="onCommand">
    <el-button
      class="density-toggle"
      text
      :title="`界面密度：${currentLabel}`"
      :aria-label="`界面密度，当前${currentLabel}`"
      aria-haspopup="menu"
    >
      <el-icon><Operation /></el-icon>
    </el-button>
    <template #dropdown>
      <el-dropdown-menu>
        <el-dropdown-item
          v-for="o in DENSITY_OPTIONS"
          :key="o.value"
          :command="o.value"
          :class="{ 'is-current': density === o.value }"
        >
          <span class="density-item">
            <span class="density-label">{{ o.label }}</span>
            <span class="density-hint">{{ o.hint }}</span>
            <el-icon v-if="density === o.value" class="density-check"><Check /></el-icon>
          </span>
        </el-dropdown-item>
      </el-dropdown-menu>
    </template>
  </el-dropdown>
</template>

<script setup lang="ts">
// 界面密度切换：紧凑 / 标准 / 宽松（只缩放间距与字号，不动颜色与圆角）
import { Operation, Check } from '@element-plus/icons-vue'
import { DENSITY_OPTIONS, useDensity, type Density } from '@/composables/useDensity'

const { density, currentLabel, setDensity } = useDensity()

function onCommand(value: string | number | object) {
  setDensity(value as Density)
}
</script>

<style scoped>
.density-item {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 168px;
}

.density-label {
  flex: none;
}

.density-hint {
  flex: 1;
  color: var(--nx-text-3);
  font-size: var(--nx-font-caption);
  white-space: nowrap;
}

.density-check {
  color: var(--nx-accent);
  flex: none;
}
</style>