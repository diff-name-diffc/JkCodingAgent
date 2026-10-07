pub(crate) mod fs;
pub(crate) mod rope;

pub(crate) use rope::RopeManager;

use std::path::{Path, PathBuf};

use crate::shared::io_error::{io_error, PathIoError};

/// 工作区路径校验错误：fs 与 rope 两条命令路径共用的判定结果，
/// 各自经 `From` 映射进领域错误枚举（文案保持一致）。
#[derive(Debug, thiserror::Error)]
pub(crate) enum PathValidationError {
    #[error("路径必须是绝对路径")]
    NotAbsolute,
    #[error("路径不在允许目录内")]
    OutsideAllowed,
    #[error("{action} 失败（{path}）：{source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl PathIoError for PathValidationError {
    fn path_io(action: &'static str, path: PathBuf, source: std::io::Error) -> Self {
        Self::Io {
            action,
            path,
            source,
        }
    }
}

/// 校验 `target` 是位于 `allowed_root` 内的绝对路径（canonicalize 后前缀判定，
/// 防目录遍历）。fs 与 rope 的唯一实现；要求目标路径已存在（canonicalize）。
pub(crate) fn validate_path_within(
    target: &str,
    allowed_root: &str,
) -> Result<PathBuf, PathValidationError> {
    let target = Path::new(target);
    let root = Path::new(allowed_root);

    if !target.is_absolute() {
        return Err(PathValidationError::NotAbsolute);
    }

    let canonical_target = target
        .canonicalize()
        .map_err(io_error("解析目标路径", target))?;
    let canonical_root = root
        .canonicalize()
        .map_err(io_error("解析项目根目录", root))?;

    if !canonical_target.starts_with(&canonical_root) {
        return Err(PathValidationError::OutsideAllowed);
    }

    Ok(canonical_target)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (路径, 守卫)：`.0` 保持路径语义，`.1` Drop 时整目录回收。
    struct Temp(PathBuf, #[allow(dead_code)] crate::test_util::TempDirGuard);
    impl Temp {
        fn new() -> Self {
            let guard = crate::test_util::TempDirGuard::new("ws-path-test");
            Self(guard.path().to_path_buf(), guard)
        }
    }

    #[test]
    fn accepts_absolute_path_inside_root() {
        let tmp = Temp::new();
        let file = tmp.0.join("a.txt");
        std::fs::write(&file, "x").unwrap();
        let validated =
            validate_path_within(file.to_str().unwrap(), tmp.0.to_str().unwrap()).unwrap();
        assert!(validated.starts_with(tmp.0.canonicalize().unwrap()));
    }

    #[test]
    fn rejects_relative_and_outside_paths() {
        let tmp = Temp::new();
        let file = tmp.0.join("a.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(matches!(
            validate_path_within("a.txt", tmp.0.to_str().unwrap()),
            Err(PathValidationError::NotAbsolute)
        ));
        let outside_dir = crate::test_util::TempDirGuard::new("ws-out");
        let outside = outside_dir.path().join("outside.txt");
        std::fs::write(&outside, "x").unwrap();
        assert!(matches!(
            validate_path_within(outside.to_str().unwrap(), tmp.0.to_str().unwrap()),
            Err(PathValidationError::OutsideAllowed)
        ));
    }
}
