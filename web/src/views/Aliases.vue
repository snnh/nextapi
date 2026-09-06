<template>
  <div class="page" v-loading="pageLoading">
    <div class="toolbar">
      <el-button type="primary" :icon="Plus" @click="openCreate">新建别名</el-button>
      <el-button :icon="Refresh" @click="load">刷新</el-button>
      <div class="spacer" />
      <div class="hint" style="max-width: 620px">
        客户端可用别名请求模型，网关按实际模型路由、计费。白名单按实际模型判定；日志保留入口别名与实际模型。
      </div>
    </div>

    <el-card shadow="never">
      <el-table :data="items" :row-key="(row: ModelAliasRow) => row.id" border>
        <template #empty>
          <el-empty description="暂无别名，点击左上角「新建别名」创建" />
        </template>

        <el-table-column label="别名" min-width="180">
          <template #default="{ row }"><span class="mono">{{ row.alias }}</span></template>
        </el-table-column>
        <el-table-column label="实际模型" min-width="180">
          <template #default="{ row }"><span class="mono">{{ row.model }}</span></template>
        </el-table-column>
        <el-table-column label="启用" width="90" align="center">
          <template #default="{ row }">
            <el-switch
              v-model="row.enabled"
              :loading="togglingSet.has(row.id)"
              @change="(val: boolean) => toggleEnabled(row, val)"
            />
          </template>
        </el-table-column>
        <el-table-column label="创建时间" width="170">
          <template #default="{ row }">{{ fmtTime(row.created_at) }}</template>
        </el-table-column>
        <el-table-column label="操作" width="150" fixed="right">
          <template #default="{ row }">
            <el-button link type="primary" @click="openEdit(row)">编辑</el-button>
            <el-button link type="danger" @click="remove(row)">删除</el-button>
          </template>
        </el-table-column>
      </el-table>
    </el-card>

    <el-dialog v-model="dlg.visible" :title="dlg.isEdit ? '编辑别名' : '新建别名'" width="480px">
      <el-form ref="formRef" :model="dlg.form" :rules="rules" label-width="90px" @submit.prevent>
        <el-form-item label="别名" prop="alias">
          <el-input v-model="dlg.form.alias" placeholder="客户端请求使用的模型名，如 fast-gpt" />
        </el-form-item>
        <el-form-item label="实际模型" prop="model">
          <el-input v-model="dlg.form.model" placeholder="路由匹配使用的真实模型 ID，如 gpt-4o-mini" />
        </el-form-item>
        <el-form-item label="启用">
          <el-switch v-model="dlg.form.enabled" />
        </el-form-item>
        <div class="hint">
          别名不能与非通配路由的模型名同名，也不能指向另一个别名（不支持链式）。
        </div>
      </el-form>
      <template #footer>
        <el-button @click="dlg.visible = false">取消</el-button>
        <el-button type="primary" :loading="saving" @click="save">保存</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { onMounted, reactive, ref } from 'vue'
import { ElMessage, ElMessageBox, type FormInstance, type FormRules } from 'element-plus'
import { Plus, Refresh } from '@element-plus/icons-vue'
import { aliasApi } from '@/api'
import { errMsg } from '@/api/http'
import type { AliasIn, ModelAliasRow } from '@/api/types'
import { fmtTime } from '@/utils/format'

const items = ref<ModelAliasRow[]>([])
const pageLoading = ref(false)
const saving = ref(false)
const togglingSet = reactive(new Set<string>())

const formRef = ref<FormInstance>()
const dlg = reactive({
  visible: false,
  isEdit: false,
  id: '',
  form: { alias: '', model: '', enabled: true } as Required<AliasIn>,
})

const rules: FormRules = {
  alias: [{ required: true, message: '请输入别名', trigger: 'blur' }],
  model: [{ required: true, message: '请输入实际模型', trigger: 'blur' }],
}

async function load() {
  pageLoading.value = true
  try {
    items.value = await aliasApi.list()
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    pageLoading.value = false
  }
}

function openCreate() {
  dlg.isEdit = false
  dlg.id = ''
  dlg.form = { alias: '', model: '', enabled: true }
  dlg.visible = true
}

function openEdit(row: ModelAliasRow) {
  dlg.isEdit = true
  dlg.id = row.id
  dlg.form = { alias: row.alias, model: row.model, enabled: row.enabled }
  dlg.visible = true
}

async function save() {
  if (saving.value || !formRef.value) return
  try {
    await formRef.value.validate()
  } catch {
    return
  }
  saving.value = true
  try {
    if (dlg.isEdit) {
      await aliasApi.update(dlg.id, dlg.form)
    } else {
      await aliasApi.create(dlg.form)
    }
    ElMessage.success('已保存')
    dlg.visible = false
    await load()
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    saving.value = false
  }
}

async function toggleEnabled(row: ModelAliasRow, val: boolean) {
  togglingSet.add(row.id)
  try {
    await aliasApi.update(row.id, { alias: row.alias, model: row.model, enabled: val })
    ElMessage.success(val ? '已启用' : '已停用')
  } catch (e) {
    row.enabled = !val
    ElMessage.error(errMsg(e))
  } finally {
    togglingSet.delete(row.id)
  }
}

async function remove(row: ModelAliasRow) {
  try {
    await ElMessageBox.confirm(`确认删除别名「${row.alias}」？`, '提示', { type: 'warning' })
  } catch {
    return
  }
  try {
    await aliasApi.remove(row.id)
    ElMessage.success('已删除')
    await load()
  } catch (e) {
    ElMessage.error(errMsg(e))
  }
}

onMounted(load)
</script>
