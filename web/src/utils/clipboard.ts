// 复制到剪贴板：优先 Clipboard API，http/LAN（非安全上下文）降级 textarea + execCommand。
// 自托管场景常见 http 访问，navigator.clipboard 不可用，必须有降级。
import { ElMessage } from 'element-plus'

async function fallbackCopy(text: string): Promise<void> {
  const ta = document.createElement('textarea')
  ta.value = text
  // 防止滚动与可见闪烁
  ta.style.position = 'fixed'
  ta.style.top = '-9999px'
  ta.style.opacity = '0'
  document.body.appendChild(ta)
  ta.focus()
  ta.select()
  try {
    const ok = document.execCommand('copy')
    if (!ok) throw new Error('execCommand copy failed')
  } finally {
    document.body.removeChild(ta)
  }
}

/**
 * 复制文本并给出统一反馈。
 * @param text 待复制文本
 * @param label 成功提示文案（默认「已复制」）
 */
export async function copyText(text: string, label = '已复制'): Promise<boolean> {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text)
    } else {
      await fallbackCopy(text)
    }
    ElMessage.success(label)
    return true
  } catch {
    ElMessage.error('复制失败，请手动选择文本复制')
    return false
  }
}
