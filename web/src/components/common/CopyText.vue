<template>
  <span class="copy-text">
    <el-tooltip :disabled="!tooltip" :content="text" placement="top" :show-after="200">
      <span class="copy-text-value mono">{{ display }}</span>
    </el-tooltip>
    <el-tooltip content="复制" placement="top">
      <el-icon class="copy-btn" role="button" aria-label="复制" @click.stop="onCopy"><CopyDocument /></el-icon>
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
  cursor: pointer;
  color: #9ca3af;
  font-size: 13px;
  padding: 2px;
  border-radius: 4px;
}
.copy-btn:hover {
  color: #1f2329;
  background: #f3f4f6;
}
</style>
