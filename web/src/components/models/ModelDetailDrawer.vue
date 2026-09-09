<template>
  <el-drawer
    :model-value="visible"
    :title="row ? `模型详情 · ${row.model}` : '模型详情'"
    :size="drawerSize"
    destroy-on-close
    @update:model-value="(v: boolean) => emit('update:visible', v)"
  >
    <template v-if="row">
      <!-- 顶部状态摘要 -->
      <div class="head">
        <CopyText :text="row.model" />
        <el-tag v-if="row.wildcard" size="small" type="warning" effect="plain">通配</el-tag>
        <el-tag v-if="row.autoManaged" size="small" type="primary" effect="plain">自动托管</el-tag>
        <el-tag :type="healthTagType" size="small" effect="plain">{{ healthLabel }}</el-tag>
        <el-tag :type="priceTagType" size="small" effect="plain">{{ priceLabel }}</el-tag>
      </div>

      <!-- 1. 基本信息 -->
      <div class="group-title">基本信息</div>
      <el-descriptions :column="2" border size="small">
        <el-descriptions-item label="对外模型名">
          <CopyText :text="row.model" />
        </el-descriptions-item>
        <el-descriptions-item label="路由条数">
          {{ row.enabledCount }} 启用 / {{ row.totalCount }} 总计
        </el-descriptions-item>
        <el-descriptions-item label="最小优先级">
          <span v-if="row.minPriority != null">{{ row.minPriority }}</span>
          <span v-else class="muted">—</span>
        </el-descriptions-item>
        <el-descriptions-item label="别名数">{{ row.aliases.length }}</el-descriptions-item>
        <el-descriptions-item label="最近更新">{{ fmtTime(row.updatedAt) }}</el-descriptions-item>
        <el-descriptions-item label="健康状态">
          <span :class="{ 'text-error': row.health === 'error', 'text-warn': row.health === 'warn' }">
            {{ row.healthText }}
          </span>
        </el-descriptions-item>
      </el-descriptions>

      <!-- 2. 路由上游 -->
      <div class="group-title">
        路由上游
        <el-button
          class="group-action"
          link
          type="primary"
          :icon="Plus"
          @click="emit('add-route', row.model)"
        >
          新增路由
        </el-button>
      </div>
      <el-table :data="row.routes" border size="small">
        <el-table-column label="上游" min-width="150" show-overflow-tooltip>
          <template #default="{ row: r }">
            <span class="cell-inline">
              <CopyText :text="r.upstream_name" :truncate="18" />
              <el-tag
                v-if="healthOf(r) !== 'ok'"
                size="small"
                :type="healthOf(r) === 'error' ? 'danger' : 'warning'"
                effect="plain"
              >
                {{ healthOf(r) === 'error' ? '异常' : '停用' }}
              </el-tag>
            </span>
          </template>
        </el-table-column>
        <el-table-column label="实际模型" min-width="150" show-overflow-tooltip>
          <template #default="{ row: r }">
            <span v-if="r.override_model">{{ r.override_model }}</span>
            <span v-else class="muted">同模型名</span>
          </template>
        </el-table-column>
        <el-table-column prop="priority" label="优先级" width="80" align="center" />
        <el-table-column prop="weight" label="权重" width="70" align="center" />
        <el-table-column label="状态" width="86" align="center">
          <template #default="{ row: r }">
            <el-tag :type="r.enabled ? 'success' : 'info'" size="small">
              {{ r.enabled ? '启用' : '禁用' }}
            </el-tag>
          </template>
        </el-table-column>
        <el-table-column label="操作" width="76" align="center">
          <template #default="{ row: r }">
            <el-button link type="primary" @click="emit('edit-route', r)">编辑</el-button>
          </template>
        </el-table-column>
      </el-table>

      <!-- 3. 别名 -->
      <div class="group-title">
        别名
        <el-button
          class="group-action"
          link
          type="primary"
          :icon="Plus"
          @click="emit('goto-aliases', row.model)"
        >
          管理别名
        </el-button>
      </div>
      <el-table v-if="row.aliases.length" :data="row.aliases" border size="small">
        <el-table-column label="别名" min-width="160">
          <template #default="{ row: a }">
            <CopyText :text="a.alias" :truncate="24" />
          </template>
        </el-table-column>
        <el-table-column label="状态" width="90" align="center">
          <template #default="{ row: a }">
            <el-tag :type="a.enabled ? 'success' : 'info'" size="small">
              {{ a.enabled ? '启用' : '停用' }}
            </el-tag>
          </template>
        </el-table-column>
      </el-table>
      <el-alert
        v-else
        type="info"
        :closable="false"
        show-icon
        title="暂无别名：客户端可直接使用模型名请求；如需更友好的调用名可添加别名。"
      />

      <!-- 4. 价格 -->
      <div class="group-title">
        价格
        <el-button
          class="group-action"
          link
          type="primary"
          :icon="Coin"
          @click="emit('goto-pricing', row.model)"
        >
          价格管理
        </el-button>
      </div>
      <el-table :data="priceRows" border size="small">
        <el-table-column label="上游" min-width="140" show-overflow-tooltip>
          <template #default="{ row: p }">{{ p.upstreamName }}</template>
        </el-table-column>
        <el-table-column label="实际模型" min-width="150" show-overflow-tooltip>
          <template #default="{ row: p }">
            <CopyText :text="p.modelId" :truncate="22" />
          </template>
        </el-table-column>
        <el-table-column label="计价" width="110" align="center">
          <template #default="{ row: p }">
            <el-tag v-if="p.state === 'unknown'" size="small" type="info" effect="plain">
              通配待定
            </el-tag>
            <el-tag v-else :type="p.state ? 'success' : 'warning'" size="small" effect="plain">
              {{ p.state ? '已配置' : '未配置' }}
            </el-tag>
          </template>
        </el-table-column>
      </el-table>
      <div class="hint">未配置价格的模型仍可调用，但不产生成本统计（日志记为 NULL）。</div>

      <!-- 5. 调用示例 -->
      <div class="group-title">调用示例</div>
      <div class="hint">
        将 <span class="mono">&lt;API_KEY&gt;</span> 替换为「API 密钥」页创建的完整密钥。
      </div>
      <pre class="code">{{ curlExample }}</pre>
      <el-button size="small" :icon="CopyDocument" @click="copyCurl">复制示例</el-button>
    </template>
  </el-drawer>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { ElMessage } from 'element-plus'
import { Coin, CopyDocument, Plus } from '@element-plus/icons-vue'
import CopyText from '@/components/common/CopyText.vue'
import { copyText } from '@/utils/clipboard'
import { fmtTime } from '@/utils/format'
import type { RouteOut, UpstreamOut } from '@/api/types'
import { effectiveModelId, upstreamHealth, type ModelOverviewRow } from '@/composables/useModelOverview'

const props = defineProps<{
  visible: boolean
  row: ModelOverviewRow | null
  upstreams: UpstreamOut[]
}>()

const emit = defineEmits<{
  'update:visible': [boolean]
  'edit-route': [RouteOut]
  'add-route': [string]
  'goto-pricing': [string]
  'goto-aliases': [string]
}>()

const drawerSize = computed(() =>
  window.matchMedia('(max-width: 768px)').matches ? '100%' : '680px',
)

const upstreamMap = computed(() => new Map(props.upstreams.map((u) => [u.id, u])))

function healthOf(r: RouteOut) {
  return upstreamHealth(upstreamMap.value.get(r.upstream_id))
}

const healthLabel = computed(() => {
  if (!props.row) return ''
  return props.row.health === 'ok' ? '正常' : props.row.health === 'warn' ? '部分异常' : '异常'
})
const healthTagType = computed(() => {
  if (!props.row) return 'info'
  return props.row.health === 'ok' ? 'success' : props.row.health === 'warn' ? 'warning' : 'danger'
})

const priceLabel = computed(() => {
  switch (props.row?.priceState) {
    case 'priced':
      return '价格已配置'
    case 'partial':
      return '价格部分配置'
    case 'unpriced':
      return '未配置价格'
    default:
      return '价格待定'
  }
})
const priceTagType = computed(() => {
  switch (props.row?.priceState) {
    case 'priced':
      return 'success'
    case 'partial':
      return 'warning'
    case 'unpriced':
      return 'warning'
    default:
      return 'info'
  }
})

/** 价格明细：每条路由一行（实际模型名 + 是否已配价格） */
const priceRows = computed(() => {
  if (!props.row) return []
  return props.row.routes.map((r) => ({
    upstreamName: r.upstream_name,
    modelId: effectiveModelId(r),
    state: props.row!.priceByRoute[r.id] ?? 'unknown',
  }))
})

/** 网关调用地址：以浏览器当前 origin 为准（反代/自定义域名场景下即用户访问地址） */
const gatewayBase = computed(() => `${window.location.origin}/v1`)
const curlExample = computed(() => {
  const model = props.row?.model ?? '<MODEL>'
  return [
    `curl ${gatewayBase.value}/chat/completions \\`,
    `  -H "Authorization: Bearer <API_KEY>" \\`,
    `  -H "Content-Type: application/json" \\`,
    `  -d '{"model":"${model}","messages":[{"role":"user","content":"hi"}]}'`,
  ].join('\n')
})

function copyCurl() {
  copyText(curlExample.value)
  ElMessage.success('已复制调用示例')
}
</script>

<style scoped>
.head {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  margin-bottom: 12px;
}
.group-title {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 18px 0 8px;
  font-size: 14px;
  font-weight: 600;
  color: #1f2329;
}
.group-action {
  font-weight: 400;
}
.muted {
  color: #c0c4cc;
}
.hint {
  margin-top: 6px;
  color: #6b7280;
  font-size: 12px;
  line-height: 1.7;
}
.code {
  margin: 8px 0;
  padding: 10px 12px;
  background: #f7f8fa;
  border: 1px solid #e8eaee;
  border-radius: 6px;
  font-size: 12px;
  line-height: 1.6;
  white-space: pre-wrap;
  word-break: break-all;
}
.text-error {
  color: #f56c6c;
}
.text-warn {
  color: #e6a23c;
}
.cell-inline {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}
</style>
