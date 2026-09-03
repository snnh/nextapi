import { createApp } from 'vue'
import { createPinia } from 'pinia'
import ElementPlus from 'element-plus'
import zhCn from 'element-plus/es/locale/lang/zh-cn'
import * as Icons from '@element-plus/icons-vue'
import 'element-plus/dist/index.css'
import App from './App.vue'
import router from './router'
import './styles/index.css'

const app = createApp(App)
app.use(createPinia())
app.use(router)
app.use(ElementPlus, { locale: zhCn })

// 全量注册 Element Plus 图标（按名使用 <el-icon><component :is="..."/>）
for (const [name, comp] of Object.entries(Icons)) {
  app.component(name, comp as never)
}

app.mount('#app')
