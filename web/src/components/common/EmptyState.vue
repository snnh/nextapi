<template>
  <div class="empty-state" role="status">
    <el-icon class="empty-icon"><component :is="icon" /></el-icon>
    <p class="empty-title">{{ title }}</p>
    <p v-if="description" class="empty-desc">{{ description }}</p>
    <div v-if="$slots.actions" class="empty-actions">
      <slot name="actions" />
    </div>
  </div>
</template>

<script setup lang="ts">
import { type Component } from 'vue'
import { Box } from '@element-plus/icons-vue'

withDefaults(
  defineProps<{
    title?: string
    description?: string
    icon?: Component
  }>(),
  { title: '暂无数据', icon: Box },
)
</script>

<style scoped>
.empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 32px 16px;
  text-align: center;
}
.empty-icon {
  font-size: 28px;
  color: var(--nx-text-placeholder);
}
.empty-title {
  margin: 10px 0 0;
  color: var(--nx-text);
  font-size: var(--nx-font-body);
  font-weight: 500;
}
.empty-desc {
  margin: 4px 0 0;
  max-width: 460px;
  color: var(--nx-text-3);
  font-size: var(--nx-font-small);
  line-height: 1.6;
}
.empty-actions {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
  justify-content: center;
  margin-top: 14px;
}
</style>