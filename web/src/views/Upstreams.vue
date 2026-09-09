<template>
  <div class="page" v-loading="pageLoading">
    <!-- 页面标题与用途说明 -->
    <div class="page-intro">
      <h2>上游供应商</h2>
      <p>接入并管理实际提供模型的供应商（上游）。接入完成后，到「模型路由」页把这些上游绑定给对外模型。</p>
    </div>

    <div class="toolbar">
      <el-button type="primary" :icon="MagicStick" @click="openWizard(null)">接入上游</el-button>
      <el-button :icon="Plus" @click="openCreate">自定义上游</el-button>
      <div class="spacer" />
    </div>

    <!-- 上游表格 -->
    <el-card shadow="never">
      <template #header>
        <div class="table-head">
          <span>接入的上游列表</span>
          <el-button size="small" :icon="Refresh" @click="loadUpstreams">刷新</el-button>
        </div>
      </template>
      <el-table :data="upstreams">
        <template #empty>
          <div class="empty-guide">
            <el-empty :image-size="110">
              <template #description>
                <div class="empty-title">还没有接入任何上游</div>
                <div class="empty-sub hint">接入上游后，才能在「模型路由」页把模型请求转发到对应的上游。</div>
              </template>
              <div class="empty-actions">
                <el-button type="primary" :icon="MagicStick" @click="openWizard(null)">接入上游（预设）</el-button>
                <el-button :icon="Plus" @click="openCreate">自定义上游</el-button>
              </div>
            </el-empty>
          </div>
        </template>
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
              <el-tooltip
                v-if="row.protocols.length > 3 && !expandedRows.has(row.id)"
                content="展开全部协议"
                placement="top"
              >
                <el-tag
                  size="small"
                  type="info"
                  class="clickable"
                  role="button"
                  tabindex="0"
                  @click="toggleExpand(row.id)"
                  @keyup.enter="toggleExpand(row.id)"
                >
                  +{{ row.protocols.length - 3 }}
                </el-tag>
              </el-tooltip>
              <el-tooltip v-if="expandedRows.has(row.id)" content="收起" placement="top">
                <el-tag
                  size="small"
                  type="info"
                  class="clickable"
                  role="button"
                  tabindex="0"
                  @click="toggleExpand(row.id)"
                  @keyup.enter="toggleExpand(row.id)"
                >
                  收起
                </el-tag>
              </el-tooltip>
              <el-tag v-for="t in capsTags(row)" :key="t" size="small" type="warning" effect="plain">
                {{ t }}
              </el-tag>
            </div>
          </template>
        </el-table-column>

        <el-table-column label="凭证" width="100" align="center">
          <template #default="{ row }">
            <el-tooltip v-if="row.has_oauth" content="OAuth 凭证（到期自动刷新）" placement="top">
              <el-tag type="primary" size="small" effect="plain">OAuth</el-tag>
            </el-tooltip>
            <el-tag v-else-if="row.has_api_key" type="success" size="small">已设置</el-tag>
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

        <el-table-column label="操作" width="330" fixed="right">
          <template #default="{ row }">
            <el-button size="small" @click="openEdit(row)">编辑</el-button>
            <el-button v-if="row.has_api_key" size="small" @click="openReveal(row)">查看 Key</el-button>
            <el-button size="small" :loading="testingId === row.id" @click="testUpstream(row)">
              连通测试
            </el-button>
            <el-button
              size="small"
              type="danger"
              :loading="deletingSet.has(row.id)"
              :disabled="deletingSet.has(row.id)"
              @click="removeUpstream(row)"
            >
              删除
            </el-button>
          </template>
        </el-table-column>
      </el-table>
    </el-card>

    <div class="hint page-card">
      说明：API Key 采用掩码存储，列表仅显示「已设置 / 未设置」；编辑时输入框「留空保持不变」，勾选「清除 Key」可移除。
      完整 Key 默认不可再次查看；如需查看，点行内「查看 Key」并输入管理员密码二次验证（已启用 TOTP 时还需动态码），
      验证通过后明文仅弹窗内本次展示。
      默认值：timeout_ms = 300000、breaker_threshold = 5；图片媒体地址留空时回退 base_url。启用/禁用直接切换行内开关（禁用后 disabled_by = manual）。
    </div>

    <!-- 接入向导（选择供应商 → 填写凭证 → 连通与模型） -->
    <ProvisionWizard
      v-model="wizardVisible"
      :presets="presets"
      :initial-preset="wizardPreset"
      @saved="loadUpstreams"
      @goto-routes="goConfigureRoutes"
    />

    <!-- 查看明文 API Key（安全验证） -->
    <el-dialog
      v-model="revealVisible"
      :title="revealTitle"
      width="560px"
      :close-on-click-modal="false"
      :close-on-press-escape="!revealApiKey"
      :show-close="!revealApiKey"
      destroy-on-close
      @closed="resetReveal"
    >
      <!-- 验证通过 → 明文展示（仅本次；禁止 X / Esc 关闭） -->
      <template v-if="revealApiKey">
        <div class="secret-view">
          <el-alert
            type="error"
            :closable="false"
            show-icon
            title="明文仅本次展示"
            description="已通过安全验证并解密展示，关闭后需再次验证才能查看；请勿在公共场合留存或截图。"
          />
          <div class="secret-key mono">{{ revealApiKey }}</div>
          <el-button type="primary" :icon="CopyDocument" @click="copyRevealedKey">一键复制</el-button>
        </div>
      </template>

      <!-- 安全验证表单 -->
      <el-form v-else label-width="110px" @submit.prevent @keyup.enter="submitReveal">
        <div class="hint" style="margin-bottom: 14px">
          查看上游「{{ revealRow?.name }}」的完整 API Key 属于敏感操作，需要管理员二次验证；验证通过后明文仅本次展示。
        </div>
        <el-form-item label="管理员密码">
          <el-input
            v-model="revealPassword"
            type="password"
            show-password
            autocomplete="current-password"
            placeholder="输入当前登录的管理员密码"
            clearable
          />
        </el-form-item>
        <el-form-item v-if="revealNeedTotp" label="二次验证码">
          <el-input v-model="revealTotp" maxlength="6" placeholder="认证器 6 位数字" clearable />
        </el-form-item>
        <div v-if="revealNeedTotp" class="hint" style="padding-left: 110px">
          该账号已启用 TOTP，需同时填写认证器动态码。
        </div>
      </el-form>

      <template #footer>
        <template v-if="revealApiKey">
          <el-button type="primary" @click="closeRevealSecret">我已保存，关闭</el-button>
        </template>
        <template v-else>
          <el-button @click="revealVisible = false">取消</el-button>
          <el-button type="primary" :loading="revealChecking" @click="submitReveal">验证并查看</el-button>
        </template>
      </template>
    </el-dialog>

    <!-- 新建 / 编辑弹窗 -->
    <UpstreamFormDialog ref="formDialogRef" :proxies="proxies" @saved="loadUpstreams" />
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { ElMessage, ElMessageBox } from 'element-plus'
import { CopyDocument, MagicStick, Plus, Refresh } from '@element-plus/icons-vue'
import { useRouter } from 'vue-router'
import { authApi, presetApi, proxyApi, upstreamApi } from '@/api'
import { errMsg } from '@/api/http'
import { copyText } from '@/utils/clipboard'
import { PROTOCOL_LABEL } from '@/utils/consts'
import { fmtTime } from '@/utils/format'
import type { Preset, ProxyOut, UpstreamIn, UpstreamOut } from '@/api/types'
import UpstreamFormDialog from '@/components/upstreams/UpstreamFormDialog.vue'
import ProvisionWizard from '@/components/upstreams/ProvisionWizard.vue'

const DISABLED_BY_LABELS: Record<string, string> = {
  manual: '手动禁用',
  breaker: '熔断',
  cooldown: '冷却中',
}

const presets = ref<Preset[]>([])
const upstreams = ref<UpstreamOut[]>([])
const proxies = ref<ProxyOut[]>([])
const pageLoading = ref(false)
const router = useRouter()
/** 接入向导可见性 + 预选供应商（null = 从「选择供应商」开始） */
const wizardVisible = ref(false)
const wizardPreset = ref<Preset | null>(null)
const expandedRows = reactive(new Set<string>())
const togglingSet = reactive(new Set<string>())
const deletingSet = reactive(new Set<string>())
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

/** 能力矩阵限制标签（M10.3）：显式「不支持」的能力 + 最大上下文。 */
function capsTags(row: UpstreamOut): string[] {
  const caps = row.extra?.capabilities
  if (!caps) return []
  const tags: string[] = []
  if (caps.stream === false) tags.push('禁流式')
  if (caps.tools === false) tags.push('禁工具')
  if (caps.vision === false) tags.push('禁视觉')
  if (caps.max_context) tags.push(`上下文 ${Math.round(caps.max_context / 1024)}k`)
  return tags
}

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
  if (deletingSet.has(row.id)) return
  deletingSet.add(row.id)
  try {
    try {
      await ElMessageBox.confirm(
        `确定删除上游「${row.name}」？关联的模型路由可能受影响。`,
        '删除上游',
        { type: 'warning' },
      )
    } catch {
      return
    }
    await upstreamApi.remove(row.id)
    ElMessage.success('已删除')
    loadUpstreams()
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    deletingSet.delete(row.id)
  }
}

function openCreate() {
  formDialogRef.value?.open()
}

function openEdit(row: UpstreamOut) {
  formDialogRef.value?.open(row)
}

/** 空态引导 / 工具栏主操作：打开接入向导（可预选供应商） */
function openWizard(p: Preset | null) {
  wizardPreset.value = p
  wizardVisible.value = true
}

/** 向导「下一步：配置模型路由」：跳转模型页 */
function goConfigureRoutes() {
  router.push({ name: 'routes' })
}

// —— 查看明文 API Key（安全验证） ——
const revealVisible = ref(false)
const revealRow = ref<UpstreamOut | null>(null)
const revealPassword = ref('')
const revealTotp = ref('')
/** 账号是否已启用 TOTP（打开弹窗时预取，决定是否展示动态码输入框） */
const revealNeedTotp = ref(false)
const revealChecking = ref(false)
const revealApiKey = ref('')

const revealTitle = computed(() =>
  revealApiKey.value ? '完整 API Key' : '安全验证 · 查看 API Key',
)

async function openReveal(row: UpstreamOut) {
  revealRow.value = row
  revealPassword.value = ''
  revealTotp.value = ''
  revealApiKey.value = ''
  revealChecking.value = false
  revealNeedTotp.value = false
  revealVisible.value = true
  try {
    revealNeedTotp.value = (await authApi.totpStatus()).enabled
  } catch {
    // 状态预取失败不阻断：若后端要求动态码会返回 400 提示，届时补填重试
    revealNeedTotp.value = false
  }
}

function resetReveal() {
  revealRow.value = null
  revealPassword.value = ''
  revealTotp.value = ''
  revealApiKey.value = ''
  revealChecking.value = false
  revealNeedTotp.value = false
}

async function submitReveal() {
  if (!revealRow.value || revealChecking.value) return
  if (!revealPassword.value.trim()) {
    ElMessage.warning('请输入管理员密码')
    return
  }
  if (revealNeedTotp.value && !revealTotp.value.trim()) {
    ElMessage.warning('请输入二次验证码')
    return
  }
  revealChecking.value = true
  try {
    const resp = await upstreamApi.revealKey(revealRow.value.id, {
      password: revealPassword.value,
      totp_code: revealNeedTotp.value ? revealTotp.value.trim() : undefined,
    })
    revealApiKey.value = resp.api_key
  } catch (e) {
    const msg = errMsg(e)
    // 后端提示需要 TOTP（本地预取过期/状态变化）时补出动态码输入框
    if (msg.includes('TOTP') || msg.includes('验证码')) revealNeedTotp.value = true
    ElMessage.error(msg)
  } finally {
    revealChecking.value = false
  }
}

function copyRevealedKey() {
  copyText(revealApiKey.value, '已复制 API Key')
}

function closeRevealSecret() {
  revealVisible.value = false
}

</script>

<style scoped>
.page-intro {
  margin-bottom: 16px;
}
.page-intro h2 {
  margin: 0;
  color: #1f2329;
  font-size: 20px;
}
.page-intro p {
  margin: 6px 0 0;
  color: #6b7280;
  font-size: 13px;
  line-height: 1.6;
}
.empty-guide {
  display: flex;
  justify-content: center;
  width: 100%;
  padding: 6px 0 12px;
}
.empty-title {
  font-size: 14px;
  font-weight: 500;
  color: #1f2329;
}
.empty-sub {
  margin-top: 4px;
}
.empty-actions {
  margin-top: 14px;
}
.table-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
}
.clickable {
  cursor: pointer;
}
.secret-view {
  display: flex;
  flex-direction: column;
  gap: 16px;
  align-items: flex-start;
}
.secret-key {
  font-size: 15px;
  line-height: 1.5;
  background: #f5f7fa;
  border: 1px solid #e4e7ed;
  border-radius: 6px;
  padding: 12px 16px;
  width: 100%;
  box-sizing: border-box;
  color: #1f2329;
  word-break: break-all;
}

</style>
