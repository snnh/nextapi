<template>
  <div class="page" v-loading="settingsLoading">
    <div class="page-card tabs-card">
      <el-tabs v-model="activeTab">
      <el-tab-pane label="运行参数" name="runtime">
        <runtime-params-panel :settings="settings" @saved="loadSettings" />
      </el-tab-pane>
      <el-tab-pane label="代理管理" name="proxy" lazy>
        <proxy-panel :settings="settings" />
      </el-tab-pane>
      <el-tab-pane label="更新检查" name="update" lazy>
        <update-check-panel :settings="settings" />
      </el-tab-pane>
      <el-tab-pane label="审计日志" name="audit" lazy>
        <audit-panel />
      </el-tab-pane>
      <el-tab-pane label="安全" name="security" lazy>
        <totp-panel />
      </el-tab-pane>
      <el-tab-pane label="系统" name="system" lazy>
        <system-info-panel />
      </el-tab-pane>
      </el-tabs>
    </div>
  </div>
</template>

<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { settingsApi } from '@/api'
import { errMsg } from '@/api/http'
import type { SettingItem } from '@/api/types'
import RuntimeParamsPanel from '@/components/settings/RuntimeParamsPanel.vue'
import ProxyPanel from '@/components/settings/ProxyPanel.vue'
import UpdateCheckPanel from '@/components/settings/UpdateCheckPanel.vue'
import AuditPanel from '@/components/settings/AuditPanel.vue'
import SystemInfoPanel from '@/components/settings/SystemInfoPanel.vue'
import TotpPanel from '@/components/settings/TotpPanel.vue'

const activeTab = ref('runtime')
const settings = ref<SettingItem[]>([])
const settingsLoading = ref(false)

async function loadSettings() {
  settingsLoading.value = true
  try {
    settings.value = await settingsApi.view()
  } catch (e) {
    ElMessage.error(errMsg(e))
  } finally {
    settingsLoading.value = false
  }
}

onMounted(loadSettings)
</script>
