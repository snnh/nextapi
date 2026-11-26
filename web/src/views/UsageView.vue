<template>
  <div class="page usage-page">
    <!-- 页头：说明 + 刷新全部（两段唯一的联动入口） -->
    <div class="page-head">
      <p class="page-desc">
        按时间范围查看请求量、Token、成本与延迟；页面底部可直接排查具体请求。
      </p>
      <div v-if="!isMobile" class="page-head-actions">
        <el-button :icon="Refresh" :loading="refreshing" @click="refreshAll">刷新全部</el-button>
      </div>
    </div>

    <!-- ① 用量统计 -->
    <div id="usage-overview" class="usage-section">
      <OverviewSection ref="overviewRef" />
    </div>

    <!-- ② 请求日志：进入视口前 200px 才挂载，避免首屏并发全部请求 -->
    <div id="usage-logs" ref="logsAnchor" class="usage-section">
      <LogsSection v-if="logsReady" ref="logsRef" />
      <div v-else class="lazy-block">
        <span class="hint">请求日志 · 滚动到此处自动加载</span>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref } from 'vue'
import { ElMessageBox } from 'element-plus'
import { Refresh } from '@element-plus/icons-vue'
import OverviewSection from '@/views/usage/OverviewSection.vue'
import LogsSection from '@/views/usage/LogsSection.vue'
import { useLazySection } from '@/composables/useLazySection'
import { useBreakpoint } from '@/composables/useBreakpoint'

const overviewRef = ref<InstanceType<typeof OverviewSection> | null>(null)
const logsRef = ref<InstanceType<typeof LogsSection> | null>(null)
const logsAnchor = ref<HTMLElement | null>(null)
const { ready: logsReady } = useLazySection(logsAnchor)
const { isMobile } = useBreakpoint()

const refreshing = ref(false)

/** 刷新全部：唯一跨段联动入口，先确认再执行 */
async function refreshAll() {
  try {
    await ElMessageBox.confirm('将重新加载「用量统计」与「请求日志」两段数据，继续？', '刷新全部', {
      type: 'info',
      confirmButtonText: '刷新',
      cancelButtonText: '取消',
    })
  } catch {
    return
  }
  refreshing.value = true
  try {
    await Promise.all([overviewRef.value?.refresh(), logsReady.value ? logsRef.value?.refresh() : undefined])
  } finally {
    refreshing.value = false
  }
}
</script>

<style scoped>
/* 移动端：页头与段头只保留必要信息，把首屏还给筛选与数据 */
@media (max-width: 800px) {
  /* 顶栏已给出页面名，说明文字在手机上只挤占首屏 */
  .usage-page :deep(.page-desc) {
    display: none;
  }
  .usage-page :deep(.page-head) {
    margin-bottom: var(--nx-space-3);
  }
  /* 段头改成一行：标题 + 图标按钮（按钮已在移动端去文字） */
  .usage-page :deep(.section-head) {
    flex-direction: row;
    align-items: center;
    flex-wrap: nowrap;
    margin-bottom: var(--nx-space-3);
  }
  .usage-page :deep(.section-head-main) {
    width: auto;
    min-width: 0;
  }
  .usage-page :deep(.section-head-actions) {
    width: auto;
    flex: none;
    margin-left: auto;
    flex-wrap: nowrap;
  }
  .usage-page :deep(.section-head .hint) {
    display: none;
  }
  /* 卡片内边距收紧：手机宽度下 16px 内边距过于奢侈 */
  .usage-page :deep(.el-card__body) {
    padding: var(--nx-space-4);
  }
}

.usage-page {
  --usage-section-gap: var(--nx-space-6);
}

.usage-section + .usage-section {
  margin-top: var(--usage-section-gap);
  padding-top: var(--nx-space-5);
  border-top: 1px solid var(--nx-border-subtle);
}

@media (max-width: 800px) {
  .usage-section + .usage-section {
    margin-top: var(--nx-space-4);
    padding-top: var(--nx-space-4);
  }
}

.lazy-block {
  display: flex;
  align-items: center;
  justify-content: center;
  min-height: 200px;
  border: 1px dashed var(--nx-border);
  border-radius: var(--nx-radius-md);
  background: var(--nx-subtle);
}
</style>