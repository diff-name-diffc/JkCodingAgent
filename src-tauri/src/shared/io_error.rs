use std::path::PathBuf;

/// 带路径上下文的 io 错误构造约定：错误枚举实现本 trait 后即可使用共享的
/// [`io_error`] 构造闭包（`{action} 失败（{path}）：{source}` 家族）。
///
/// 领域模块保留一个五行类型钉住适配器（`io_error(...) -> impl FnOnce(..) -> FsError`
/// 委托本函数）：`?` 经 `From` 转换的调用点无法从多个候选 impl 中唯一推断
/// 泛型 E，适配器把错误类型固定回模块错误枚举，全部既有调用点零改动。
pub(crate) trait PathIoError: Sized {
    /// io 失败，带动作与路径上下文。
    fn path_io(action: &'static str, path: PathBuf, source: std::io::Error) -> Self;
}

/// 构造 `FnOnce(io::Error) -> E` 闭包：把 io 错误包装为带动作与路径上下文的
/// 目标错误（各领域错误枚举实现 [`PathIoError`]）。
pub(crate) fn io_error<E: PathIoError>(
    action: &'static str,
    path: impl Into<PathBuf>,
) -> impl FnOnce(std::io::Error) -> E {
    move |source| E::path_io(action, path.into(), source)
}
