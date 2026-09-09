// ============================================================================
// 危险操作统一确认（M16）
//
// 统一规范：标题 = 动作名（如「删除上游」）；正文 = 影响范围 + 不可恢复说明；
// 确认按钮文案 = 动作名（红色 danger），取消按钮固定「取消」，禁用点击遮罩关闭。
// 返回 true 表示用户确认，false 表示取消（调用方直接 return）。
// ============================================================================
import { ElMessageBox } from 'element-plus'

export interface ConfirmDangerOptions {
  /** 标题：动作名（同时作为确认按钮文案，除非显式指定 confirmText） */
  title: string
  /** 影响范围与不可恢复说明（建议包含「不可恢复」等明确措辞） */
  message: string
  /** 确认按钮文案，默认取 title */
  confirmText?: string
  /** 非破坏性但需二次确认的场景可设为 'warning'（按钮不高亮为红色） */
  level?: 'danger' | 'warning'
}

/** 危险操作统一确认；用户取消或关闭时返回 false */
export async function confirmDanger(opts: ConfirmDangerOptions): Promise<boolean> {
  try {
    await ElMessageBox.confirm(opts.message, opts.title, {
      type: 'warning',
      confirmButtonText: opts.confirmText ?? opts.title,
      confirmButtonClass: opts.level === 'warning' ? undefined : 'el-button--danger',
      cancelButtonText: '取消',
      closeOnClickModal: false,
    })
    return true
  } catch {
    return false
  }
}
