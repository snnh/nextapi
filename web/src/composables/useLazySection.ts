// 区段懒加载：元素进入视口前 200px 时把 ready 置 true，只触发一次。
// 注意：全站滚动容器是 MainLayout 的 #main（el-main），不是 window。
import { onBeforeUnmount, onMounted, ref, type Ref } from 'vue'

function scrollHost(): HTMLElement | null {
  return document.getElementById('main')
}

export function useLazySection(target: Ref<HTMLElement | null>) {
  const ready = ref(false)
  let observer: IntersectionObserver | null = null

  onMounted(() => {
    if (typeof IntersectionObserver === 'undefined' || !target.value) {
      ready.value = true
      return
    }
    observer = new IntersectionObserver(
      (entries) => {
        if (!entries.some((e) => e.isIntersecting)) return
        ready.value = true
        observer?.disconnect()
        observer = null
      },
      { root: scrollHost(), rootMargin: '200px 0px' },
    )
    observer.observe(target.value)
  })

  onBeforeUnmount(() => {
    observer?.disconnect()
    observer = null
  })

  return { ready }
}