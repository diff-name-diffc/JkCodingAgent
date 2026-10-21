//! apply_patch 工具层测试：参数校验、路径沙箱、成功渲染。

use super::*;
use crate::test_util::TempDirGuard;

#[tokio::test]
async fn applies_patch_inside_workspace_and_reports_summary() {
    let dir = TempDirGuard::new("apply-patch-tool");
    std::fs::write(dir.path().join("target.md"), "# 旧标题\n\n正文\n").unwrap();
    let sandbox = FsSandbox {
        workspace: dir.path().to_path_buf(),
        restrict_to_workspace: true,
        extra_allowed_dirs: vec![],
    };
    let tool = apply_patch_tool(sandbox.clone());
    let output = tool
        .execute(serde_json::json!({
            "patch": "*** Begin Patch\n*** Update File: target.md\n@@\n # 旧标题\n \n-正文\n+新正文\n*** End Patch"
        }))
        .await
        .map_err(|error| error.message().to_string())
        .expect("补丁应应用成功");
    let text = output.render();
    assert!(
        text.contains("Success. Updated the following files:"),
        "{text}"
    );
    assert!(text.contains("M target.md"), "{text}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("target.md")).unwrap(),
        "# 旧标题\n\n新正文\n"
    );
}

#[tokio::test]
async fn path_outside_workspace_is_refused_before_any_write() {
    let dir = TempDirGuard::new("apply-patch-sandbox");
    let sandbox = FsSandbox {
        workspace: dir.path().to_path_buf(),
        restrict_to_workspace: true,
        extra_allowed_dirs: vec![],
    };
    let tool = apply_patch_tool(sandbox);
    let error = tool
        .execute(serde_json::json!({
            "patch": "*** Begin Patch\n*** Add File: ../escape.txt\n+内容\n*** End Patch"
        }))
        .await
        .err()
        .unwrap_or_else(|| panic!("越界路径必须被拒绝"));
    assert!(
        error.message().contains("工作区之外"),
        "{}",
        error.message()
    );
    // 绝对路径越界同样拒绝（不创建目标）。
    assert!(!dir.path().parent().unwrap().join("escape.txt").exists());
}

#[tokio::test]
async fn missing_patch_argument_is_invalid_args() {
    let dir = TempDirGuard::new("apply-patch-args");
    let sandbox = FsSandbox {
        workspace: dir.path().to_path_buf(),
        restrict_to_workspace: true,
        extra_allowed_dirs: vec![],
    };
    let tool = apply_patch_tool(sandbox);
    let error = tool.execute(serde_json::json!({})).await.unwrap_err();
    assert!(error.to_string().contains("缺少必填参数 patch"), "{error}");
}

#[tokio::test]
async fn malformed_patch_reports_parse_error_with_format_guidance() {
    let dir = TempDirGuard::new("apply-patch-parse");
    let sandbox = FsSandbox {
        workspace: dir.path().to_path_buf(),
        restrict_to_workspace: true,
        extra_allowed_dirs: vec![],
    };
    let tool = apply_patch_tool(sandbox);
    let error = tool
        .execute(serde_json::json!({ "patch": "不是补丁" }))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("补丁解析失败"), "{error}");
    assert!(error.to_string().contains("*** Begin Patch"), "{error}");
}

#[tokio::test]
async fn add_file_creates_missing_parent_directories() {
    let dir = TempDirGuard::new("apply-patch-parents");
    let sandbox = FsSandbox {
        workspace: dir.path().to_path_buf(),
        restrict_to_workspace: true,
        extra_allowed_dirs: vec![],
    };
    let tool = apply_patch_tool(sandbox);
    tool.execute(serde_json::json!({
        "patch": "*** Begin Patch\n*** Add File: nested/dir/file.txt\n+hello\n*** End Patch"
    }))
    .await
    .expect("嵌套路径应自动建目录");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("nested/dir/file.txt")).unwrap(),
        "hello\n"
    );
}
