<template>
  <div class="update-panel">
    <el-card shadow="never" class="summary-card">
      <template #header>
        <div class="card-head"><span>更新检查现状</span></div>
      </template>
      <div v-if="updateItems.length" class="summary-grid">
        <div v-for="it in updateItems" :key="it.key" class="summary-item">
          <span class="summary-label">{{ it.label }}</span>
          <span class="summary-value mono">{{ it.value }}</span>
        </div>
      </div>
      <div v-else class="hint">暂无 update_check.* 配置项。</div>
      <div class="hint" style="margin-top: 12px">
        修改请到「运行参数 → 更新检查」分组。
      </div>
      <el-button type="primary" :loading="checking" :icon="Refresh" class="check-btn" @click="check">
        立即检查更新
      </el-button>
    </el-card>

    <el-alert
      v-if="needRepoHint"
      type="warning"
      :closable="false"
      show-icon
      title="未配置仓库：请先在「运行参数 → 更新检查」填写 update_check.repo（如 owner/nextapi）。"
      style="margin-bottom: 16px"
    />

    <el-card v-if="resp" shadow="never" class="result-card">
      <template #header>
        <div class="card-head"><span>检查结果</span></div>
      </template>

      <div class="version-row">
        <div class="version-cell">
          <span class="version-sub">当前版本</span>
          <span class="version-big mono">{{ resp.current_version }}</span>
        </div>
        <el-icon class="version-arrow" :size="18"><Right /></el-icon>
        <div class="version-cell">
          <span class="version-sub">最新版本</span>
          <span class="version-big mono">{{ resp.latest_version }}</span>
        </div>
        <el-tag :type="resp.update_available ? 'danger' : 'success'" size="large" effect="dark" class="version-tag">
          {{ resp.update_available ? '可更新' : '已是最新' }}
        </el-tag>
      </div>

      <el-descriptions :column="1" border class="release-desc">
        <el-descriptions-item label="发布名">
          <span class="mono">{{ resp.release_name || '—' }}</span>
        </el-descriptions-item>
        <el-descriptions-item label="发布时间">
          {{ fmtTime(resp.published_at) }}
        </el-descriptions-item>
        <el-descriptions-item label="发布页面">
          <el-link v-if="resp.html_url" type="primary" :href="resp.html_url" target="_blank" rel="noopener">
            {{ resp.html_url }}
          </el-link>
          <span v-else>—</span>
        </el-descriptions-item>
      </el-descriptions>

      <el-collapse v-if="resp.release_notes" class="notes-collapse">
        <el-collapse-item name="notes">
          <template #title>
            <span class="notes-title">更新说明</span>
          </template>
          <pre class="notes-pre">{{ resp.release_notes }}</pre>
        </el-collapse-item>
      </el-collapse>
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { Refresh, Right } from '@element-plus/icons-vue'
import type { AxiosError } from 'axios'
import { systemApi } from '@/api'
import { errMsg } from '@/api/http'
import { fmtTime } from '@/utils/format'
import type { CheckUpdateResp, SettingItem } from '@/api/types'
import { SETTING_FIELDS, shortKey } from './fields'

const props = defineProps<{ settings: SettingItem[] }>()

const checking = ref(false)
const resp = ref<CheckUpdateResp | null>(null)
const needRepoHint = ref(false)

const updateItems = computed(() => {
  return props.settings
    .filter((s) => s.key.startsWith('update_check.'))
    .map((s) => {
      const f = SETTING_FIELDS[s.key]
      return { key: s.key, label: f?.label ?? shortKey(s.key), value: fmtVal(s.value) }
    })
})

function fmtVal(v: SettingItem['value']): string {
  if (Array.isArray(v)) return v.length ? v.join(', ') : '[]'
  if (v === null || v === undefined || v === '') return '（未设置）'
  if (typeof v === 'boolean') return v ? '是' : '否'
  return String(v)
}

async function check() {
  checking.value = true
  needRepoHint.value = false
  try {
    resp.value = await systemApi.checkUpdate()
  } catch (e) {
    const status = (e as AxiosError).response?.status
    if (status === 400) needRepoHint.value = true
    ElMessage.error(errMsg(e))
  } finally {
    checking.value = false
  }
}
</script>

<style scoped>
.summary-card,
.result-card {
  margin-bottom: 16px;
}
.card-head {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 14px;
  font-weight: 600;
  color: #1f2329;
}
.summary-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
  gap: 10px;
}
.summary-item {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 8px 12px;
  background: #f5f7fa;
  border-radius: 6px;
}
.summary-label {
  font-size: 12px;
  color: #6b7280;
}
.summary-value {
  font-size: 14px;
  color: #1f2329;
  word-break: break-all;
}
.check-btn {
  margin-top: 14px;
}
.hint {
  font-size: 12px;
  color: #6b7280;
}
.version-row {
  display: flex;
  align-items: center;
  gap: 20px;
  flex-wrap: wrap;
  margin-bottom: 16px;
}
.version-cell {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.version-sub {
  font-size: 12px;
  color: #6b7280;
}
.version-big {
  font-size: 24px;
  font-weight: 600;
  color: #1f2329;
}
.version-arrow {
  color: #c0c4cc;
}
.version-tag {
  font-size: 15px;
}
.mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
}
.release-desc {
  margin-bottom: 12px;
}
.notes-collapse {
  margin-top: 4px;
}
.notes-title {
  font-size: 14px;
  font-weight: 600;
  color: #1f2329;
}
.notes-pre {
  margin: 0;
  white-space: pre-wrap;
  word-break: break-word;
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 13px;
  line-height: 1.7;
  background: #f5f7fa;
  padding: 12px;
  border-radius: 6px;
  max-height: 360px;
  overflow: auto;
}
</style>
