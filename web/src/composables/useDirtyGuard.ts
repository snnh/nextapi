// 脏表单防丢：长表单弹窗在关闭（Esc/X/遮罩/取消）前二次确认。
// 用法：
//   const { snapshot, isDirty, confirmClose } = useDirtyGuard(() => form)
//   打开弹窗回填后调用 snapshot()；el-dialog :before-close="(done) => confirmClose(done)"
import { ElMessageBox } from 'element-plus'

export function useDirtyGuard(serialize: () => unknown) {
  let baseline = ''
  let armed = false

  /** 记录基线（打开弹窗、回填完成后调用） */
  function snapshot() {
    try {
      baseline = JSON.stringify(serialize())
    } catch {
      baseline = ''
    }
    armed = true
  }

  /** 解除守卫（提交成功后调用，避免关闭时误报） */
  function disarm() {
    armed = false
  }

  function isDirty(): boolean {
    if (!armed) return false
    try {
      return JSON.stringify(serialize()) !== baseline
    } catch {
      return false
    }
  }

  /**
   * 适配 el-dialog 的 before-close：脏则弹确认，干净直接放行。
   * 注意：before-close 仅在用户点 X/遮罩/Esc 时触发；取消按钮请自行调用 confirmThen。
   */
  function confirmClose(done: () => void) {
    if (!isDirty()) return done()
    ElMessageBox.confirm('有未保存的修改，确定关闭并丢弃吗？', '未保存的修改', {
      type: 'warning',
      confirmButtonText: '丢弃修改',
      cancelButtonText: '继续编辑',
    })
      .then(() => done())
      .catch(() => {})
  }

  /** 取消按钮等自定义关闭路径：脏则确认后执行 close() */
  async function confirmThen(close: () => void) {
    if (!isDirty()) return close()
    try {
      await ElMessageBox.confirm('有未保存的修改，确定关闭并丢弃吗？', '未保存的修改', {
        type: 'warning',
        confirmButtonText: '丢弃修改',
        cancelButtonText: '继续编辑',
      })
      close()
    } catch {
      /* 继续编辑 */
    }
  }

  return { snapshot, disarm, isDirty, confirmClose, confirmThen }
}
