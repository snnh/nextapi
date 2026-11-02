<template>
  <span class="copy-text">
    <el-tooltip :disabled="!tooltip" :content="text" placement="top" :show-after="200">
      <span class="copy-text-value mono">{{ display }}</span>
    </el-tooltip>
    <el-tooltip content="复制" placement="top">
      <el-button
        class="copy-btn"
        link
        :icon="CopyDocument"
        aria-label="复制"
        title="复制"
        @click.stop="onCopy"
      />
    </el-tooltip>
  </span>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { CopyDocument } from '@element-plus/icons-vue'
import { copyText } from '@/utils/clipboard'

const props = withDefaults(
  defineProps<{
    /** 完整文本（复制内容与 tooltip 展示） */
    text: string
    /** 截断展示长度（超过则中间省略）；<=0 不截断 */
    truncate?: number
    /** hover 是否展示完整文本 tooltip */
    tooltip?: boolean
  }>(),
  { truncate: 0, tooltip: true },
)

const display = computed(() => {
  const t = props.text
  const n = props.truncate
  if (!n || n <= 0 || t.length <= n) return t
  const head = Math.ceil(n / 2)
  const tail = Math.floor(n / 2)
  return `${t.slice(0, head)}…${t.slice(t.length - tail)}`
})

function onCopy() {
  copyText(props.text)
}
</script>

<style scoped>
.copy-text {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  min-width: 0;
}
.copy-text-value {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.copy-btn {
  flex-shrink: 0;
  width: 24px;
  height: 24px;
  padding: 0;
  color: var(--el-text-color-secondary);
  border-radius: var(--el-border-radius-small);
}
.copy-btn:hover {
  color: var(--el-text-color-primary);
  background: var(--el-fill-color-light);
}
@media (max-width: 768px) {
  .copy-btn {
    width: 32px;
    height: 32px;
  }
}
</style>
