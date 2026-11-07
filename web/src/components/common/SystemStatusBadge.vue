<template>
  <el-popover placement="bottom-end" :width="360" trigger="click" popper-class="status-pop">
    <template #reference>
      <button
        type="button"
        class="status-badge"
        :class="`status-${level}`"
        :title="statusTitle"
        :aria-label="`系统状态：${statusLabel}。${statusTitle}`"
      >
        <span class="dot" aria-hidden="true" />
        <span class="label">{{ statusLabel }}</span>
      </button>
    </template>

    <div class="pop">
      <div class="pop-head">
        <span class="pop-title">系统状态</span>
        <el-button link type="primary" :loading="loading" :icon="Refresh" @click="refresh">
          刷新
        </el-button>
      </div>

      <div class="rows">
        <div class="row">
          <span class="k">后端</span>
          <span class="v">
            <StatusTag :label="healthz ? '可用' : '不可用'" :tone="healthz ? 'success' : 'danger'" />
          </span>
        </div>
        <div class="row">
          <span class="k">数据库</span>
          <span class="v">
            <StatusTag :label="readyz ? '就绪' : '不可用'" :tone="readyz ? 'success' : 'danger'" />
            <span v-if="status" class="hint">{{ status.database.latency_ms }}ms</span>
          </span>
        </div>
        <div class="row">
          <span class="k">版本</span>
          <span class="v mono">{{ status?.version || version || '-' }}</span>
        </div>
        <div class="row">
          <span class="k">运行时长</span>
          <span class="v">{{ status ? fmtUptime(status.uptime_secs) : '-' }}</span>
        </div>
        <div class="row">
          <span class="k">日志队列</span>
          <span class="v">
            <template v-if="status?.logging.queue_capacity != null">
              {{ status.logging.queue_used }} / {{ status.logging.queue_capacity }}
              <span class="hint">（溢出 {{ status.logging.overflow_total }}）</span>
            </template>
            <span v-else class="hint">同步直写</span>
          </span>
        </div>
        <div class="row">
          <span class="k">WAL 兜底</span>
          <span class="v">
            {{ status ? `${status.logging.wal_files} 个 / ${fmtBytes(status.logging.wal_bytes)}` : '-' }}
          </span>
        </div>
        <div class="row">
          <span class="k">媒体任务</span>
          <span class="v">
            待回联 {{ status?.media_poller.pending_tasks ?? '-' }}
          </span>
        </div>
        <div class="row">
          <span class="k">异常上游</span>
          <span class="v">
            <template v-if="status?.breakers.length">
              <StatusTag
                v-for="b in status.breakers"
                :key="b.name"
                :label="b.name"
                tone="warning"
                class="tag-gap"
              />
            </template>
            <span v-else class="hint">无</span>
          </span>
        </div>
        <div v-if="status?.recent_errors.length" class="row errors">
          <span class="k">最近错误</span>
          <div class="v">
            <div v-for="(e, i) in status.recent_errors.slice(0, 3)" :key="i" class="err">
              <span class="mono">{{ e.status }}</span>
              <span class="err-model">{{ e.model }}</span>
              <span class="err-msg" :title="e.error || ''">{{ e.error || '-' }}</span>
            </div>
          </div>
        </div>
      </div>

      <div class="pop-foot">
        <span class="hint">{{ lastUpdated ? `更新于 ${lastUpdated}` : '尚未获取状态' }}</span>
        <div class="foot-actions">
          <el-button link type="primary" @click="gotoLogs">请求日志</el-button>
          <el-button link type="primary" @click="gotoSettings">系统设置</el-button>
        </div>
      </div>
    </div>
  </el-popover>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { Refresh } from '@element-plus/icons-vue'
import { systemApi } from '@/api'
import { appBase } from '@/utils/base'
import { fmtBytes, fmtUptime } from '@/utils/format'
import StatusTag from '@/components/common/StatusTag.vue'
import type { SystemStatusResp } from '@/api/types'

const router = useRouter()
const healthz = ref(true)
const readyz = ref(true)
const status = ref<SystemStatusResp | null>(null)
const version = ref('')
const loading = ref(false)
const lastUpdated = ref('')

/** 状态等级：error=后端不可用；warn=数据库不可用或存在异常上游；ok=正常 */
const level = computed<'ok' | 'warn' | 'error'>(() => {
  if (!healthz.value) return 'error'
  if (!readyz.value) return 'warn'
  if (status.value?.breakers.length) return 'warn'
  return 'ok'
})

const statusLabel = computed(() =>
  level.value === 'ok' ? '运行正常' : level.value === 'warn' ? '部分异常' : '后端不可用',
)

const statusTitle = computed(() => {
  if (level.value === 'error') return '无法连接后端，请检查服务或反向代理'
  if (!readyz.value) return '数据库不可用'
  if (status.value?.breakers.length) return '存在异常/被禁用上游'
  return '系统运行正常'
})

async function probe(path: string): Promise<boolean> {
  try {
    const r = await fetch(`${appBase()}${path}`, { cache: 'no-store' })
    return r.ok
  } catch {
    return false
  }
}

async function refresh() {
  if (loading.value) return
  loading.value = true
  const [h, r] = await Promise.all([probe('healthz'), probe('readyz')])
  healthz.value = h
  readyz.value = r
  if (h) {
    try {
      status.value = await systemApi.status()
      version.value = status.value.version
    } catch {
      // 状态接口失败不改变可用性判定（可能只是 JWT 过期，由 http 层统一处理）
    }
  }
  lastUpdated.value = new Date().toLocaleTimeString('zh-CN', { hour12: false })
  loading.value = false
}

/** 秒级运行时长人性化 */

function gotoLogs() {
  router.push({ name: 'logs' })
}

function gotoSettings() {
  router.push({ name: 'settings' })
}

/** 60s 轮询（页面隐藏时跳过，避免后台标签页无谓请求） */
const TIMER_MS = 60_000
let timer: number | undefined
onMounted(() => {
  refresh()
  timer = window.setInterval(() => {
    if (document.visibilityState === 'visible') refresh()
  }, TIMER_MS)
})
onBeforeUnmount(() => {
  if (timer) window.clearInterval(timer)
})
</script>

<style scoped>
.status-badge {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 3px 10px;
  border: 1px solid transparent;
  border-radius: var(--nx-radius-round);
  font: inherit;
  font-size: var(--nx-font-caption);
  cursor: pointer;
  user-select: none;
  white-space: nowrap;
  background: none;
}
.status-badge:hover {
  border-color: var(--nx-border-strong);
}
.status-badge .dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  flex-shrink: 0;
  background: currentColor;
}
.status-ok {
  color: var(--nx-success);
  background: var(--nx-success-soft);
  border-color: var(--nx-success-border);
}
.status-warn {
  color: var(--nx-warning);
  background: var(--nx-warning-soft);
  border-color: var(--nx-warning-border);
}
.status-error {
  color: var(--nx-danger);
  background: var(--nx-danger-soft);
  border-color: var(--nx-danger-border);
}

.pop-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 8px;
}
.pop-title {
  font-size: var(--nx-font-body);
  font-weight: 600;
  color: var(--nx-text);
}
.rows {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.row {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  font-size: var(--nx-font-small);
}
.row .k {
  width: 72px;
  flex-shrink: 0;
  color: var(--nx-text-3);
}
.row .v {
  flex: 1;
  min-width: 0;
  color: var(--nx-text);
  word-break: break-all;
}
.row.errors .v {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.err {
  display: flex;
  gap: 6px;
  font-size: var(--nx-font-caption);
  line-height: 1.5;
}
.err-model {
  color: var(--nx-text-2);
  flex-shrink: 0;
}
.err-msg {
  color: var(--nx-text-3);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.tag-gap {
  margin-right: 4px;
}
.pop-foot {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-top: 10px;
  padding-top: 8px;
  border-top: 1px solid var(--nx-border-subtle);
}
.foot-actions {
  display: flex;
  gap: 4px;
}
@media (max-width: 800px) {
  .status-badge {
    padding: 3px 8px;
  }
}
</style>

<style>
.status-pop {
  max-width: calc(100vw - 24px);
}
</style>
