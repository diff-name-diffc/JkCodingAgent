//! 测试专用基础设施（仅 `cfg(test)` 编译，不进生产二进制）。

use std::path::{Path, PathBuf};

/// 测试临时目录守卫：创建唯一临时目录，Drop 时整目录回收。
///
/// 取代旧模式（`env::temp_dir().join(format!("x-{}", Uuid::new_v4()))` +
/// `create_dir_all`，用例结束不清理）——频繁跑测试会在系统临时目录无限
/// 累积 sqlite/产物文件。约定：
/// - 测试自建的目录与其中的产物文件（sqlite、下载物、导出件）一律放进
///   守卫目录，随 Drop 一并回收；
/// - 仅把 `env::temp_dir()` 当作「工作区外路径」引用、不创建内容的场景
///   （越界拒绝类用例）不适用本守卫；
/// - 持有需要存活整场用例的资源（如打开的 `DispatcherDb`）时，守卫必须
///   绑定在用例作用域（`let _dir = ...`），不能在工厂函数内创建后丢弃。
pub(crate) struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    pub(crate) fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("创建测试临时目录失败");
        Self { path }
    }

    /// 守卫目录路径；sqlite 等文件产物放目录内，随 Drop 一并回收。
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        // best-effort 回收：失败不影响用例判定（macOS/Linux 允许删除已打开
        // 文件的目录项，Windows 上句柄未关时可能残留，可接受）。
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
