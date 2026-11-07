<template>
  <div class="error-state" role="alert">
    <el-icon class="error-icon"><WarningFilled /></el-icon>
    <p class="error-title">{{ title }}</p>
    <p class="error-msg">{{ message }}</p>
    <div class="error-actions">
      <el-button type="primary" :icon="Refresh" @click="emit('retry')">重试</el-button>
      <el-button :icon="CopyDocument" @click="copy">复制错误</el-button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { CopyDocument, Refresh, WarningFilled } from '@element-plus/icons-vue'
import { copyText } from '@/utils/clipboard'

const props = withDefaults(
  defineProps<{
    title?: string
    message: string
  }>(),
  { title: '加载失败' },
)

const emit = defineEmits<{ retry: [] }>()

function copy() {
  copyText(props.message, '错误信息已复制')
}
</script>

<style scoped>
.error-state {
  padding: 28px 16px;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 8px;
  text-align: center;
}
.error-icon {
  font-size: 28px;
  color: var(--nx-danger);
}
.error-title {
  margin: 0;
  color: var(--nx-text);
  font-size: var(--nx-font-title);
  font-weight: 600;
}
.error-msg {
  margin: 0;
  max-width: 720px;
  color: var(--nx-danger);
  font-size: var(--nx-font-small);
  line-height: 1.6;
  word-break: break-all;
}
.error-actions {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
  justify-content: center;
  margin-top: 6px;
}
</style>