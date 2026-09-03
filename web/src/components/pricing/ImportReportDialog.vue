<template>
  <el-dialog
    :model-value="modelValue"
    title="导入报告"
    width="640px"
    :close-on-click-modal="false"
    @update:model-value="(v: boolean) => emit('update:modelValue', v)"
  >
    <template v-if="report">
      <el-alert
        v-if="report.dry_run"
        type="warning"
        :closable="false"
        show-icon
        title="预览模式：此次未落库，确认无误后请点击「确认导入」"
        style="margin-bottom: 12px"
      />
      <el-alert
        v-else
        :type="report.failed.length ? 'warning' : 'success'"
        :closable="false"
        show-icon
        :title="`导入完成：共 ${report.total} 条，成功 ${report.succeeded}，跳过 ${report.skipped}，失败 ${report.failed.length}`"
        style="margin-bottom: 12px"
      />

      <div class="report-stats">
        <div class="stat">
          <span class="stat-num">{{ report.total }}</span>
          <span class="stat-label">总数</span>
        </div>
        <div class="stat">
          <span class="stat-num success">{{ report.succeeded }}</span>
          <span class="stat-label">成功</span>
        </div>
        <div class="stat">
          <span class="stat-num skip">{{ report.skipped }}</span>
          <span class="stat-label">跳过</span>
        </div>
        <div class="stat">
          <span class="stat-num" :class="report.failed.length ? 'danger' : 'success'">
            {{ report.failed.length }}
          </span>
          <span class="stat-label">失败</span>
        </div>
      </div>

      <template v-if="report.failed.length">
        <el-divider content-position="left">失败明细</el-divider>
        <el-table :data="report.failed" size="small" max-height="280">
          <el-table-column prop="index" label="序号" width="80" align="right" />
          <el-table-column prop="reason" label="原因" show-overflow-tooltip />
        </el-table>
      </template>
    </template>
    <el-empty v-else description="无报告数据" :image-size="60" />

    <template #footer>
      <el-button @click="emit('update:modelValue', false)">关闭</el-button>
      <el-button
        v-if="report && report.dry_run"
        type="primary"
        :loading="confirmLoading"
        @click="emit('confirm')"
      >
        确认导入
      </el-button>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
import type { ImportReport } from '@/api/types'

defineProps<{
  modelValue: boolean
  report: ImportReport | null
  confirmLoading?: boolean
}>()
const emit = defineEmits<{
  (e: 'update:modelValue', v: boolean): void
  (e: 'confirm'): void
}>()
</script>

<style scoped>
.report-stats {
  display: flex;
  gap: 16px;
  margin-bottom: 8px;
}
.stat {
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 10px 18px;
  background: #f5f7fa;
  border-radius: 8px;
}
.stat-num {
  font-size: 22px;
  font-weight: 600;
  color: #303133;
}
.stat-num.success {
  color: #67c23a;
}
.stat-num.skip {
  color: #e6a23c;
}
.stat-num.danger {
  color: #f56c6c;
}
.stat-label {
  font-size: 12px;
  color: #909399;
  margin-top: 2px;
}
</style>
