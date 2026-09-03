// 下载工具：blob / 文本保存为文件
export function downloadBlob(blob: Blob, filename: string) {
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = filename
  document.body.appendChild(a)
  a.click()
  document.body.removeChild(a)
  URL.revokeObjectURL(url)
}

export function downloadText(text: string, filename: string, mime = 'text/plain;charset=utf-8') {
  downloadBlob(new Blob([text], { type: mime }), filename)
}

export function downloadJsonText(obj: unknown, filename: string) {
  downloadText(JSON.stringify(obj, null, 2), filename, 'application/json;charset=utf-8')
}
