<template>
  <div class="page" v-loading="pageLoading">
    <div class="toolbar">
      <el-button type="primary" :icon="Plus" @click="openCreate">新建上游</el-button>
      <div class="spacer" />
      <el-button :icon="Refresh" @click="loadUpstreams">刷新</el-button>
    </div>

    <!-- 预设一键接入 -->
    <el-collapse v-model="presetOpen" class="page-card">
      <el-collapse-item name="presets" title="预设一键接入">
        <div class="hint" style="margin-bottom: 12px">
          从内置预设快速接入主流上游（阿里百炼 / OpenAI 等）。接入时仅需填写 API Key，其余配置自动填充，接入后可进入列表配置模型路由。
        </div>
        <el-row :gutter="16">
          <el-col v-for="p in presets" :key="p.name" :xs="24" :sm="12" :md="8" :lg="6" class="preset-col">
            <el-card shadow="hover" class="preset-card">
              <div class="preset-head">
                <span class="preset-name">{{ p.display_name }}</span>
                <el-tag size="small" type="info">{{ p.kind }}</el-tag>
              </div>
              <div class="preset-desc hint">{{ p.description }}</div>
              <div class="tag-group preset-protos">
                <el-tag v-for="proto in p.protocols" :key="proto" size="small" type="primary" effect="plain">
                  {{ PROTOCOL_LABEL(proto) }}
                </el-tag>
              </div>
              <div v-if="p.media_base_url" class="hint preset-media">
                媒体基础地址：{{ p.media_base_url }}
              </div>
              <el-button size="small" type="primary" @click="openProvision(p)">接入</el-button>
            </el-card>
          </el-col>
        </el-row>
      </el-collapse-item>
    </el-collapse>

    <!-- 上游表格 -->
    <el-card shadow="never">
      <template #header>
        <div class="table-head">
          <span>上游供应商</span>
          <el-button size="small" :icon="Refresh" @click="loadUpstreams">刷新</el-button>
        </div>
      </template>
      <el-table :data="upstreams" empty-text="暂无上游，可点击右上角「新建上游」或在上方预设一键接入">
        <el-table-column prop="name" label="名称" min-width="160" show-overflow-tooltip />

        <el-table-column label="类型" width="140">
          <template #default="{ row }">
            <el-tag size="small" type="info">{{ row.kind }}</el-tag>
          </template>
        </el-table-column>

        <el-table-column prop="base_url" label="Base URL" min-width="200" show-overflow-tooltip />

        <el-table-column label="协议" min-width="220">
          <template #default="{ row }">
            <div class="tag-group">
              <template v-for="(proto, i) in row.protocols" :key="proto">
                <el-tag
                  v-if="i < 3 || expandedRows.has(row.id)"
                  size="small"
                  type="primary"
                  effect="plain"
                >
                  {{ PROTOCOL_LABEL(proto) }}
                </el-tag>
              </template>
              <el-tag
                v-if="row.protocols.length > 3 && !expandedRows.has(row.id)"
                size="small"
                type="info"
                class="clickable"
                @click="toggleExpand(row.id)"
              >
                +{{ row.protocols.length - 3 }}
              </el-tag>
              <el-tag
                v-if="expandedRows.has(row.id)"
                size="small"
                type="info"
                class="clickable"
                @click="toggleExpand(row.id)"
              >
                收起
              </el-tag>
            </div>
          </template>
        </el-table-column>

        <el-table-column label="API Key" width="100" align="center">
          <template #default="{ row }">
            <el-tag v-if="row.has_api_key" type="success" size="small">已设置</el-tag>
            <el-tag v-else type="info" size="small">未设置</el-tag>
          </template>
        </el-table-column>

        <el-table-column label="代理" width="130">
          <template #default="{ row }">
            <span v-if="row.use_proxy">{{ proxyName(row.proxy_id) }}</span>
            <span v-else>直连</span>
          </template>
        </el-table-column>

        <el-table-column label="状态" min-width="200">
          <template #default="{ row }">
            <el-tag :type="statusMeta(row).type" size="small">{{ statusMeta(row).text }}</el-tag>
          </template>
        </el-table-column>

        <el-table-column label="启用" width="80" align="center">
          <template #default="{ row }">
            <el-switch
              v-model="row.enabled"
              :loading="togglingSet.has(row.id)"
              @change="(val: boolean) => toggleEnabled(row, val)"
            />
          </template>
        </el-table-column>

        <el-table-column label="操作" width="240" fixed="right">
          <template #default="{ row }">
            <el-button size="small" @click="openEdit(row)">编辑</el-button>
            <el-button size="small" :loading="testingId === row.id" @click="testUpstream(row)">
              连通测试
            </el-button>
            <el-button size="small" type="danger" @click="removeUpstream(row)">删除</el-button>
          </template>
        </el-table-column>
      </el-table>
    </el-card>

    <div class="hint page-card">
      说明：API Key 采用掩码存储，列表仅显示「已设置 / 未设置」；编辑时输入框「留空保持不变」，勾选「清除 Key」可移除。
      默认值：timeout_ms = 300000、breaker_threshold = 5；图片媒体地址留空时回退 base_url。启用/禁用直接切换行内开关（禁用后 disabled_by = manual）。
    </div>

    <!-- 预设接入弹窗 -->
    <el-dialog v-model="provisionVisible" title="预设一键接入" width="560px" :close-on-click-modal="false">
      <el-form label-width="90px" @submit.prevent>
        <el-form-item label="预设">
          <div class="hint">{{ provisionPreset?.display_name }}（{{ provisionPreset?.kind }}）</div>
        </el-form-item>
        <el-form-item label="上游名称" required>
          <el-input v-model="provisionName" placeholder="上游名称（预填预设名）" clearable />
        </el-form-item>
        <el-form-item label="API Key" required>
          <el-input
            v-model="provisionKey"
            type="password"
            show-password
            placeholder="必填，接入后用于网关调用"
            clearable
          />
        </el-form-item>
        <el-form-item label="Base URL">
          <el-input :model-value="provisionPreset?.base_url" readonly />
        </el-form-item>
      </el-form>

      <div v-if="provisionResult" class="provision-result">
        <el-alert
          v-if="provisionResult.test.ok"
          type="success"
          :closable="false"
          show-icon
          :title="`连通正常 HTTP ${provisionResult.test.status} · ${provisionResult.test.latency_ms}ms`"
        />
        <el-alert
          v-else
          type="error"
          :closable="false"
          show-icon
          :title="provisionResult.test.error || '连通测试失败'"
        />
        <div class="hint" style="margin-top: 8px">上游已创建，可在列表中配置模型路由。</div>
      </div>
      <div v-if="provisionConflict" class="hint" style="margin-top: 12px">
        名称「{{ provisionName }}」已存在，请修改名称后重试。
      </div>

      <template #footer>
        <el-button @click="provisionVisible = false">取消</el-button>
        <el-button type="primary" :loading="provisionSaving" @click="submitProvision">确认接入</el-button>
      </template>
    </el-dialog>

    <!-- 新建 / 编辑弹窗 -->
    <UpstreamFormDialog ref="formDialogRef" :proxies="proxies" @saved="loadUpstreams" />
  </div>
</template>

<script setup lang="ts">
import { onMounted, reactive, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { Plus, Refresh } from '@element-plus/icons-vue'
import { presetApi, proxyApi, upstreamApi } from '@/api'
import { errMsg } from '@/api/http'
import { PROTOCOL_LABEL } from '@/utils/consts'
import { fmtTime } from '@/utils/format'
import type { Preset, ProvisionResp, ProxyOut, UpstreamIn, UpstreamOut } from '@/api/types'
import UpstreamFormDialog from '@/components/upstreams/UpstreamFormDialog.vue'

const DISABLED_BY_LABELS: Record<string, string> = {
  manual: '手动禁用',
  breaker: '熔断',
  cooldown: '冷却中',
}

const presets = ref<Preset[]>([])
const upstreams = ref<UpstreamOut[]>([])
const proxies = ref<ProxyOut[]>([])
const pageLoading = ref(false)
const presetOpen = ref<string[]>(['presets'])
const expandedRows = reactive(new Set<string>())
const togglingSet = reactive(new Set<string>())
const testingId = ref('')
const formDialogRef = ref<InstanceType<typeof UpstreamFormDialog>>()

async function loadUpstreams() {
  pageLoading.value = true
  try {
    upstreams.value = await upstreamApi.list()
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    pageLoading.value = false
  }
}

async function loadProxies() {
  try {
    proxies.value = await proxyApi.list()
  } catch (e) {
    ElMessage.error(errMsg(e))
  }
}

async function loadPresets() {
  try {
    presets.value = await presetApi.list()
  } catch (e) {
    ElMessage.error(errMsg(e))
  }
}

onMounted(() => {
  loadUpstreams()
  loadProxies()
  loadPresets()
})

function toggleExpand(id: string) {
  if (expandedRows.has(id)) expandedRows.delete(id)
  else expandedRows.add(id)
}

function proxyName(proxyId: string | null): string {
  if (!proxyId) return '跟随默认'
  const p = proxies.value.find((x) => x.id === proxyId)
  return p ? p.name : proxyId
}

function statusMeta(row: UpstreamOut): { type: 'success' | 'warning' | 'danger'; text: string } {
  if (!row.enabled || row.disabled_by || row.cooldown_until) {
    let reason: string
    if (row.disabled_by) reason = DISABLED_BY_LABELS[row.disabled_by] ?? row.disabled_by
    else if (row.cooldown_until) reason = '冷却中'
    else reason = '已禁用'
    let text = reason
    if (row.cooldown_until) text += ` · 冷却至 ${fmtTime(row.cooldown_until)}`
    return { type: 'danger', text }
  }
  if (row.consecutive_failures > 0) {
    return { type: 'warning', text: `启用 · 连续失败 ${row.consecutive_failures}` }
  }
  return { type: 'success', text: '启用' }
}

function rowToIn(row: UpstreamOut): UpstreamIn {
  return {
    name: row.name,
    base_url: row.base_url,
    kind: row.kind || null,
    protocols: row.protocols.length ? [...row.protocols] : null,
    enabled: row.enabled,
    timeout_ms: row.timeout_ms,
    breaker_threshold: row.breaker_threshold,
    probe_model: row.probe_model,
    use_proxy: row.use_proxy,
    proxy_id: row.use_proxy ? row.proxy_id : null,
    extra: row.extra,
  }
}

async function toggleEnabled(row: UpstreamOut, val: boolean) {
  togglingSet.add(row.id)
  try {
    const body = rowToIn(row)
    body.enabled = val
    const updated = await upstreamApi.update(row.id, body)
    Object.assign(row, updated)
    ElMessage.success(val ? '已启用' : '已禁用')
  } catch (e) {
    row.enabled = !val
    ElMessage.error(errMsg(e))
  } finally {
    togglingSet.delete(row.id)
  }
}

async function testUpstream(row: UpstreamOut) {
  testingId.value = row.id
  try {
    const r = await upstreamApi.test(row.id)
    if (r.ok) {
      ElMessage.success(`连通正常 HTTP ${r.status} · ${r.latency_ms}ms`)
    } else {
      ElMessage.error(r.error || '连通测试失败')
    }
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    testingId.value = ''
  }
}

async function removeUpstream(row: UpstreamOut) {
  try {
    await ElMessageBox.confirm(
      `确定删除上游「${row.name}」？关联的模型路由可能受影响。`,
      '删除上游',
      { type: 'warning' },
    )
  } catch {
    return
  }
  try {
    await upstreamApi.remove(row.id)
    ElMessage.success('已删除')
    loadUpstreams()
  } catch (e) {
    ElMessage.error(errMsg(e))
  }
}

function openCreate() {
  formDialogRef.value?.open()
}

function openEdit(row: UpstreamOut) {
  formDialogRef.value?.open(row)
}

// —— 预设一键接入 ——
const provisionVisible = ref(false)
const provisionSaving = ref(false)
const provisionPreset = ref<Preset | null>(null)
const provisionName = ref('')
const provisionKey = ref('')
const provisionResult = ref<ProvisionResp | null>(null)
const provisionConflict = ref(false)

function openProvision(p: Preset) {
  provisionPreset.value = p
  provisionName.value = p.name
  provisionKey.value = ''
  provisionResult.value = null
  provisionConflict.value = false
  provisionVisible.value = true
}

async function submitProvision() {
  if (!provisionPreset.value) return
  if (!provisionName.value.trim()) {
    ElMessage.warning('请输入上游名称')
    return
  }
  if (!provisionKey.value.trim()) {
    ElMessage.warning('请填写 API Key')
    return
  }
  provisionSaving.value = true
  provisionResult.value = null
  provisionConflict.value = false
  try {
    const resp = await presetApi.provision(provisionPreset.value.name, {
      api_key: provisionKey.value,
      name: provisionName.value.trim(),
    })
    provisionResult.value = resp
    ElMessage.success('上游已创建，可在列表中配置模型路由')
    loadUpstreams()
  } catch (e) {
    const msg = errMsg(e)
    if (msg.includes('已存在')) {
      provisionConflict.value = true
      ElMessage.warning('名称已存在，请修改名称后重试')
    } else {
      ElMessage.error(msg)
    }
  } finally {
    provisionSaving.value = false
  }
}
</script>

<style scoped>
.table-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
}
.preset-col {
  margin-bottom: 16px;
}
.preset-card :deep(.el-card__body) {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.preset-head {
  display: flex;
  align-items: center;
  gap: 8px;
}
.preset-name {
  font-size: 15px;
  font-weight: 600;
  color: #303133;
}
.preset-desc {
  min-height: 36px;
}
.preset-protos {
  margin: 4px 0;
}
.preset-media {
  margin-bottom: 4px;
}
.clickable {
  cursor: pointer;
}
.provision-result {
  margin-top: 12px;
}
</style>
