//! 通用目录提示与普通聊天的提示词辅助。
use std::path::PathBuf;

/// SSH 运维备忘录纪律（仅在备忘录工具可用时注入）。
pub(super) const SSH_MEMO_GUIDANCE: &str = "\n\n## SSH 运维备忘录\n\n\
- 备忘录是下一次运维的「环境索引 + 高频易踩坑提醒」，不是排障日志或工作报告。每台 SSH 服务器一份，用 ssh_memo_read / ssh_memo_upsert / ssh_memo_delete 读取与更新。\n\
- 对不熟悉的服务器执行运维操作前，先调用 ssh_memo_read，优先复用已有部署/目录索引与注意事项，避免重复探索和再次踩坑。\n\
- 写入默认克制，宁可不写：只有信息已在目标服务器核实、长期稳定、下一次会话大概率能直接复用，且不写就会导致重复探索或再次踩坑时才写；说不清未来用途和复用场景就不要写。不要因为执行过命令、完成排查或解决故障而例行写入。\n\
- 值得写入的内容：①部署与目录索引——部署程序、服务、容器的名称与用途，以及安装/工作目录和配置、日志、数据、脚本等关键目录，必要时附稳定端口与依赖；②非通用且会复用的启停、发布、备份等操作方式；③注意事项——仅记录环境特有、下次仍很容易再犯且会导致返工或风险的坑，浓缩为「触发条件或误区 → 正确做法」，必要时补一句根因。\n\
- 禁止写入：冗长报错或堆栈全文、原始日志与命令输出、JAR 反编译等排障步骤和次要命令、一次性探索/安装/修复过程与临时状态、通用软件知识、未验证猜测、密码/密钥/令牌等凭据。错误信息只有揭示上述高频坑时才可浓缩为一行提醒，不保留报错全文和排障过程。\n\
- 写入前必须 ssh_memo_read 并合并去重；优先补充既有索引，避免为每次任务新建段落。修正单行或增删个别条目用局部替换（title+old_text+new_text，new_text 留空即删除），较大改动才用 title+content 整段重写；主动删除重复、过时和琐碎条目，写入被上限拒绝时先精简再重试。段落正文不要写 `## ` 开头的行（子标题用 `### `）。";

pub(super) const LOG_SEARCH_GUIDANCE: &str = "\n\n## 大日志调查\n\n\
- 多轮排障优先使用日志快照：ssh_log_discover 先确认容器/文件和大小，ssh_log_snapshot_update 同步并自动建索引，ssh_log_snapshot_status 等待 ready，再 ssh_log_search 与 ssh_log_read。不要用全量 docker logs/cat 代替快照流程。\n\
- 先发现后精读：搜索先用 files/count 收敛来源，再用 matches 获取 file_id、line_number、byte_offset，最后 read 读取有限上下文；不要把整份日志放入上下文。\n\
- 同一快照可重复 update 获取最新日志，省略 sources 会复用上一代来源。更新后 generation 改变，旧 cursor 作废，必须重新搜索。\n\
- scan_complete 表示是否完成扫描，results_truncated 表示结果是否被限量；不完整时不得断言没有错误或只有这些错误。需要完整性证明时使用 engine=scan 并缩小范围直至完成。\n\
- 超长行按 byte_offset 和 next_byte_offset 精读；普通上下文按 next_line 分页。远程或本地日志都可能含敏感信息，只提取与问题相关的证据。";

/// 会话图片目录（构造失败按空白名单收紧，fail-closed）。
pub(super) fn chat_image_dir(workspace_id: &str) -> Vec<PathBuf> {
    match crate::chat_images::workspace_image_dir(workspace_id) {
        Ok(dir) => vec![dir],
        Err(error) => {
            eprintln!(
                "构造会话图片目录失败（workspace_id={workspace_id}），按空白名单收紧：{error}"
            );
            Vec::new()
        }
    }
}

/// 运行工作目录与路径权限块（迁移自 `prompt/runtime_workspace.rs::render`）。
pub(crate) fn render_runtime_workspace(
    workspace: &std::path::Path,
    restricted: bool,
    extra: &[PathBuf],
    local_zsh: bool,
) -> String {
    let mut prompt = format!(
        "\n\n## 本次运行的工作目录与路径权限\n\n\
         - 当前文件工作区（绝对路径）：{}\n\
         - 文件工具与 sync_directory.source 的相对路径均以该工作区为基准；exec 默认在该工作区执行。\n",
        workspace.display()
    );
    if restricted {
        prompt.push_str("- 文件路径限制：只允许当前工作区及下列额外授权路径；上级目录、其他项目目录不会自动获得授权。\n");
    } else {
        prompt.push_str("- 工作区边界限制已关闭；应用敏感路径保护与命令安全审查仍生效。\n");
    }
    if extra.is_empty() {
        prompt.push_str("- 额外授权路径：无。\n");
    } else {
        prompt.push_str("- 额外授权路径（目录内或精确文件）：\n");
        for path in extra {
            prompt.push_str(&format!("  - {}\n", path.display()));
        }
    }
    if local_zsh {
        if let Ok(directory) = crate::agent::rig_ext::tools::exec::local_zsh_dir(workspace) {
            prompt.push_str(&format!(
                "- local_zsh 实际命令工作目录：{}。它与文件工作区不同，且不能使用 cd。命令内的相对路径以此目录为基准。\n\
                 - 上传 local_zsh 生成的文件时，使用该执行目录下产物子目录的绝对路径作为 source，不要将相对路径误当成文件工作区下的路径。\n",
                directory.display()
            ));
        }
    }
    prompt.push_str(
        "- sync_directory.destination 是远端目录，不受本地工作区前缀约束；source 必须是本地已有的产物目录。\n\
         - 不要上传整个 .jkcodingagent 配置根目录或 .git 元数据目录；会话工作区和 local_zsh 的产物子目录可按现有路径权限使用。\n\
         - 不要猜测 /Users、用户主目录、/releases 等目录。路径越界时说明当前工作区和所需路径，让用户在对应项目会话操作或将文件放入允许目录；不要换工具绕过限制或重复尝试同一个被拒绝路径。\n"
    );
    prompt
}
