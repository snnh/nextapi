//! 进程堆内存回收辅助。
//!
//! glibc malloc 会按线程创建 arena；对象释放后，内存通常只回到 arena 的空闲
//! 链表，不会主动归还 OS，长期运行后 RSS 会停在历史高水位。这里调用
//! `malloc_trim(0)` 让 glibc 把**空闲页**通过 `madvise(MADV_DONTNEED)` 归还
//! 内核；仍存活的分配不受影响，也不需要改任何配置项。
//!
//! 非 Linux/glibc 平台为空操作，保持可移植性。

/// 周期回收间隔（固定值，不引入配置项）。
pub const TRIM_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// 归还 glibc 堆中的空闲页；返回是否实际释放了内存。
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub fn trim_allocator() -> bool {
    // SAFETY: `malloc_trim` 是 glibc 提供的线程安全 API；`pad = 0` 表示尽量
    // 归还全部空闲页。它只处理空闲块，不会移动或释放仍存活的分配。
    unsafe { libc::malloc_trim(0) != 0 }
}

/// 非 glibc 平台：没有等价的通用 API，保持空操作。
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub fn trim_allocator() -> bool {
    false
}

/// 在后台阻塞线程池触发一次回收，避免阻塞 async worker；不等待结果。
pub fn trim_allocator_soon() {
    if tokio::runtime::Handle::try_current().is_ok() {
        // spawn_blocking 启动后即脱离 JoinHandle 执行；显式 drop 表示不等待。
        drop(tokio::task::spawn_blocking(trim_allocator));
    } else {
        // 同步上下文（如测试/启动早期）直接执行。
        trim_allocator();
    }
}
