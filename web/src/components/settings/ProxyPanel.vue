<template>
  <div class="proxy-panel">
    <el-alert type="info" :closable="false" show-icon class="info-banner">
      <template #title>
        <div class="info-text">
          系统默认代理在「运行参数 → 代理（全局）」的 <span class="mono">proxy.default_proxy_id</span> 配置（当前
          <span class="mono">{{ defaultProxyLabel }}</span>）。外联矩阵：供应商请求 / GitHub 更新检查 / FX 拉取 /
          价格 URL 导入 / 媒体下载 的各场景 <span class="mono">use_proxy</span> 与
          <span class="mono">proxy_id</span> 在「运行参数」对应分组内配置；代理自身
          <span class="mono">no_proxy</span> 与全局 <span class="mono">proxy.no_proxy</span> 取并集，命中即直连。
        </div>
      </template>
    </el-alert>

    <div class="toolbar">
      <el-button type="primary" :icon="Plus" @click="openCreate">新建代理</el-button>
      <el-input
        v-model="keyword"
        :prefix-icon="Search"
        placeholder="按名称 / host 过滤"
        clearable
        class="search-input"
      />
      <div class="spacer" />
      <el-button :icon="Refresh" :loading="loading" @click="load">刷新</el-button>
    </div>

    <el-card shadow="never">
      <el-table :data="filtered" v-loading="loading">
        <template #empty>
          <el-empty :description="keyword.trim() ? '无匹配的代理' : '暂无代理'" :image-size="60" />
        </template>
        <el-table-column prop="name" label="名称" min-width="160" show-overflow-tooltip>
          <template #default="{ row }">{{ row.name }}</template>
        </el-table-column>
        <el-table-column label="类型" width="100" align="center">
          <template #default="{ row }">
            <el-tag :type="kindType(row.kind)" size="small" effect="plain">{{ row.kind }}</el-tag>
          </template>
        </el-table-column>
        <el-table-column label="地址" min-width="170">
          <template #default="{ row }">
            <span class="mono">{{ row.host }}:{{ row.port }}</span>
          </template>
        </el-table-column>
        <el-table-column label="用户名" min-width="120">
          <template #default="{ row }">
            <span v-if="row.username" class="mono">{{ row.username }}</span>
            <span v-else class="muted">—</span>
          </template>
        </el-table-column>
        <el-table-column label="密码" width="110" align="center">
          <template #default="{ row }">
            <el-tag v-if="row.has_password" type="success" size="small" effect="light">已设密码</el-tag>
            <span v-else class="muted">—</span>
          </template>
        </el-table-column>
        <el-table-column label="直连名单" min-width="180">
          <template #default="{ row }">
            <template v-if="row.no_proxy.length">
              <el-tag v-for="(t, i) in showNoProxy(row)" :key="i" size="small" style="margin-right: 4px">{{ t }}</el-tag>
              <el-tooltip v-if="row.no_proxy.length > 3" placement="top">
                <template #content>
                  <div v-for="(t, i) in row.no_proxy" :key="i" class="tip-item">{{ t }}</div>
                </template>
                <el-tag size="small" type="info" effect="plain">+{{ row.no_proxy.length - 3 }}</el-tag>
              </el-tooltip>
            </template>
            <span v-else class="muted">—</span>
          </template>
        </el-table-column>
        <el-table-column label="启用" width="80" align="center">
          <template #default="{ row }">
            <el-switch
              :model-value="row.enabled"
              :loading="toggling.has(row.id)"
              @update:model-value="(v: boolean) => toggleEnabled(row, v)"
            />
          </template>
        </el-table-column>
        <el-table-column label="操作" width="200" fixed="right">
          <template #default="{ row }">
            <el-tooltip content="探测地址为运行参数 proxy.probe_url（系统设置-运行参数-代理）" placement="top">
              <el-button size="small" :loading="testing.has(row.id)" @click="test(row)">测试</el-button>
            </el-tooltip>
            <el-button size="small" @click="openEdit(row)">编辑</el-button>
            <el-button size="small" type="danger" :loading="deleting.has(row.id)" @click="remove(row)">删除</el-button>
          </template>
        </el-table-column>
      </el-table>
    </el-card>

    <el-dialog
      v-model="dlg.visible"
      :title="dlg.isEdit ? '编辑代理' : '新建代理'"
      class="dlg"
      :close-on-click-modal="false"
      destroy-on-close
    >
      <el-form ref="formRef" :model="dlg.form" :rules="rules" label-width="90px" @submit.prevent>
        <el-form-item label="名称" prop="name">
          <el-input v-model="dlg.form.name" placeholder="代理名称（唯一）" clearable />
        </el-form-item>
        <el-form-item label="类型" prop="kind">
          <el-select v-model="dlg.form.kind" style="width: 100%">
            <el-option v-for="k in PROXY_KINDS" :key="k.value" :value="k.value" :label="k.label" />
          </el-select>
        </el-form-item>
        <el-form-item label="主机" prop="host">
          <el-input v-model="dlg.form.host" placeholder="如 proxy.example.com" clearable />
        </el-form-item>
        <el-form-item label="端口" prop="port">
          <el-input-number v-model="dlg.form.port" :min="1" :max="65535" :controls-position="'right'" style="width: 200px" />
        </el-form-item>
        <el-form-item label="用户名">
          <el-input v-model="dlg.form.username" placeholder="（可选）" clearable />
        </el-form-item>
        <el-form-item label="密码">
          <div v-if="dlg.isEdit" class="pwd-block">
            <el-input
              v-model="dlg.form.password"
              type="password"
              show-password
              :placeholder="dlg.form.hasPassword ? '留空保持不变' : '（可选）'"
              clearable
            />
            <el-checkbox v-model="dlg.form.clearPassword">清除密码（提交空串）</el-checkbox>
          </div>
          <el-input
            v-else
            v-model="dlg.form.password"
            type="password"
            show-password
            placeholder="（可选）"
            clearable
          />
        </el-form-item>
        <el-form-item label="直连名单">
          <div class="tags-editor">
            <el-tag v-for="(t, i) in dlg.form.no_proxy" :key="i" closable size="small" @close="removeTag(i)">{{ t }}</el-tag>
            <el-input v-model="noProxyInput" placeholder="域名/CIDR/*后缀 回车添加" size="small" class="tag-input" @keyup.enter="addTag" />
          </div>
          <div class="hint">与全局 proxy.no_proxy 取并集命中即直连。</div>
        </el-form-item>
        <el-form-item label="启用">
          <el-switch v-model="dlg.form.enabled" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="dlg.visible = false">取消</el-button>
        <el-button type="primary" :loading="saving" @click="submit">保存</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue'
import { ElMessage, ElMessageBox, type FormInstance, type FormRules } from 'element-plus'
import { Plus, Refresh, Search } from '@element-plus/icons-vue'
import { proxyApi } from '@/api'
import { errMsg } from '@/api/http'
import { PROXY_KINDS } from '@/utils/consts'
import { confirmDanger } from '@/utils/confirm'
import type { ProxyIn, ProxyOut, SettingItem } from '@/api/types'

const props = defineProps<{ settings: SettingItem[] }>()

const proxies = ref<ProxyOut[]>([])
const loading = ref(false)
const saving = ref(false)
const toggling = reactive(new Set<string>())
const testing = reactive(new Set<string>())
const deleting = reactive(new Set<string>())
// 列表搜索关键字（按名称 / host 客户端过滤）
const keyword = ref('')

const formRef = ref<FormInstance>()

/** 按名称 / host 客户端过滤后的展示数据 */
const filtered = computed(() => {
  const kw = keyword.value.trim().toLowerCase()
  if (!kw) return proxies.value
  return proxies.value.filter(
    (p) => p.name.toLowerCase().includes(kw) || p.host.toLowerCase().includes(kw),
  )
})

function kindType(kind: string): 'primary' | 'success' | 'warning' | 'info' {
  if (kind === 'https') return 'success'
  if (kind === 'socks5') return 'warning'
  if (kind === 'http') return 'primary'
  return 'info'
}

const defaultProxyId = computed(() => {
  const it = props.settings.find((s) => s.key === 'proxy.default_proxy_id')
  return it ? String(it.value ?? '') : ''
})
const defaultProxyLabel = computed(() =>
  defaultProxyId.value ? defaultProxyId.value : '未设置（直连）',
)

async function load() {
  loading.value = true
  try {
    proxies.value = await proxyApi.list()
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    loading.value = false
  }
}

function showNoProxy(row: ProxyOut): string[] {
  return row.no_proxy.slice(0, 3)
}

async function toggleEnabled(row: ProxyOut, val: boolean) {
  toggling.add(row.id)
  try {
    const body: ProxyIn = {
      name: row.name,
      host: row.host,
      kind: row.kind,
      port: row.port,
      username: row.username,
      no_proxy: [...row.no_proxy],
      enabled: val,
    }
    const updated = await proxyApi.update(row.id, body)
    Object.assign(row, updated)
    ElMessage.success(val ? '已启用' : '已禁用')
  } catch (e) {
    row.enabled = !val
    ElMessage.error(errMsg(e))
  } finally {
    toggling.delete(row.id)
  }
}

async function test(row: ProxyOut) {
  testing.add(row.id)
  try {
    const r = await proxyApi.test(row.id)
    if (r.ok) {
      const st = r.status != null ? `HTTP ${r.status}` : 'HTTP OK'
      const lat = r.latency_ms != null ? ` · ${r.latency_ms} ms` : ''
      ElMessage.success(`连通正常 ${st}${lat}`)
    } else {
      ElMessage.error(r.error || '测试失败')
    }
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    testing.delete(row.id)
  }
}

async function remove(row: ProxyOut) {
  if (deleting.has(row.id)) return // 防重复删除
  deleting.add(row.id)
  const ok = await confirmDanger({
    title: '删除代理',
    message: `将删除代理「${row.name}」。使用该代理的上游与更新检查会回退直连（或失去网络出口），该操作不可恢复。`,
  })
  if (!ok) {
    deleting.delete(row.id)
    return
  }
  try {
    await proxyApi.remove(row.id)
    ElMessage.success('已删除')
    proxies.value = proxies.value.filter((p) => p.id !== row.id)
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    deleting.delete(row.id)
  }
}

// —— 弹窗 ——
interface ProxyForm {
  name: string
  kind: string
  host: string
  port: number
  username: string
  password: string
  hasPassword: boolean
  clearPassword: boolean
  no_proxy: string[]
  enabled: boolean
}

function defaultForm(): ProxyForm {
  return {
    name: '',
    kind: 'http',
    host: '',
    port: 8080,
    username: '',
    password: '',
    hasPassword: false,
    clearPassword: false,
    no_proxy: [],
    enabled: true,
  }
}

const dlg = reactive({ visible: false, isEdit: false, id: '', form: defaultForm() })
const noProxyInput = ref('')

const rules: FormRules = {
  name: [{ required: true, message: '请输入代理名称', trigger: 'blur' }],
  host: [{ required: true, message: '请输入主机地址', trigger: 'blur' }],
  port: [
    { required: true, message: '请输入端口', trigger: 'blur' },
    {
      validator: (_r, v: number, cb) => {
        if (v == null || v < 1 || v > 65535) return cb(new Error('端口需在 1-65535 之间'))
        cb()
      },
      trigger: 'blur',
    },
  ],
}

function openCreate() {
  dlg.isEdit = false
  dlg.id = ''
  dlg.form = defaultForm()
  noProxyInput.value = ''
  dlg.visible = true
}

function openEdit(row: ProxyOut) {
  dlg.isEdit = true
  dlg.id = row.id
  dlg.form = {
    name: row.name,
    kind: row.kind,
    host: row.host,
    port: row.port,
    username: row.username ?? '',
    password: '',
    hasPassword: row.has_password,
    clearPassword: false,
    no_proxy: [...row.no_proxy],
    enabled: row.enabled,
  }
  noProxyInput.value = ''
  dlg.visible = true
}

const IPV4_RE = /^(?:\d{1,3}\.){3}\d{1,3}$/
// 域名 / host 名：段内字母数字开头结尾，可含 - 与内部 _；可选 *. 前缀通配
const HOST_RE = /^(?:\*\.)?(?:[A-Za-z0-9_](?:[A-Za-z0-9_-]{0,61}[A-Za-z0-9_])?\.)*[A-Za-z0-9_](?:[A-Za-z0-9_-]{0,61}[A-Za-z0-9_])?$/

/** no_proxy 单项宽松校验：host/域名（可 *. 通配）、IPv4/CIDR、IPv6（含 [] 与 CIDR） */
function isValidNoProxyEntry(raw: string): boolean {
  const v = raw.trim()
  if (!v || v.length > 253 || /\s/.test(v)) return false
  const slashIdx = v.indexOf('/')
  if (slashIdx !== -1) {
    // CIDR：前缀须为数字，网段须为 IPv4 或含冒号的 IPv6
    const net = v.slice(0, slashIdx)
    const prefix = v.slice(slashIdx + 1)
    if (!/^\d+$/.test(prefix)) return false
    const p = Number(prefix)
    if (net.includes(':')) return p <= 128 && /^[0-9A-Fa-f:.]+$/.test(net.replace(/[[\]]/g, ''))
    return p <= 32 && IPV4_RE.test(net)
  }
  if (v.includes(':')) {
    // 宽松 IPv6（可带 []），仅允许十六进制、冒号与点
    return /^\[?[0-9A-Fa-f:.]+\]?$/.test(v)
  }
  return HOST_RE.test(v)
}

function addTag() {
  const v = noProxyInput.value.trim()
  if (!v) return
  if (!isValidNoProxyEntry(v)) {
    // 非法格式即时提示：不添加也不清空输入，便于就地修改
    ElMessage.warning('格式不合法：仅支持 host/域名（可带 *. 通配）、IPv4/CIDR、IPv6')
    return
  }
  if (!dlg.form.no_proxy.includes(v)) dlg.form.no_proxy.push(v)
  noProxyInput.value = ''
}

function removeTag(idx: number) {
  dlg.form.no_proxy.splice(idx, 1)
}

function buildBody(): ProxyIn {
  const body: ProxyIn = {
    name: dlg.form.name.trim(),
    kind: dlg.form.kind,
    host: dlg.form.host.trim(),
    port: dlg.form.port,
    username: dlg.form.username.trim() || null,
    no_proxy: [...dlg.form.no_proxy],
    enabled: dlg.form.enabled,
  }
  if (dlg.isEdit) {
    if (dlg.form.clearPassword) body.password = ''
    else if (dlg.form.password && dlg.form.password !== '***') body.password = dlg.form.password
  } else if (dlg.form.password) {
    body.password = dlg.form.password
  }
  return body
}

async function submit() {
  if (!formRef.value || saving.value) return
  try {
    await formRef.value.validate()
  } catch {
    return
  }
  // 编辑态勾选「清除密码」属破坏性操作：提交前确认一次
  if (dlg.isEdit && dlg.form.clearPassword) {
    try {
      await ElMessageBox.confirm(
        '将清除该代理已设置的密码（以空串提交覆盖）。确认清除？',
        '清除密码',
        { type: 'warning', confirmButtonText: '确认清除', cancelButtonText: '取消' },
      )
    } catch {
      return
    }
  }
  saving.value = true
  try {
    const body = buildBody()
    if (dlg.isEdit) {
      await proxyApi.update(dlg.id, body)
      ElMessage.success('代理已更新')
    } else {
      await proxyApi.create(body)
      ElMessage.success('代理已创建')
    }
    dlg.visible = false
    dlg.form.password = '' // 敏感字段用完即清（发布审阅前端 M3）
    load()
  } catch (e) {
    ElMessage.error(errMsg(e))
    dlg.form.password = ''
  } finally {
    saving.value = false
  }
}

onMounted(load)
</script>

<style scoped>
.info-banner {
  margin-bottom: 14px;
}
.info-text {
  line-height: 1.7;
}
.info-text .mono,
.proxy-panel .mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-size: 12px;
  background: #f5f7fa;
  border-radius: 4px;
  padding: 1px 6px;
}
.toolbar {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 14px;
}
.toolbar .spacer {
  flex: 1;
}
.search-input {
  width: 220px;
}
.muted {
  color: #c0c4cc;
}
.tip-item {
  font-size: 12px;
  line-height: 1.6;
  white-space: nowrap;
}
.pwd-block {
  width: 100%;
}
.pwd-block .el-checkbox {
  margin-top: 6px;
}
.tags-editor {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 6px;
  width: 100%;
}
.tag-input {
  width: 220px;
}
.hint {
  font-size: 12px;
  color: #6b7280;
  margin-top: 6px;
}
</style>
