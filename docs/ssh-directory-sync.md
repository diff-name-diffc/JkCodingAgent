# SSH 目录同步

Agent 工具名为 **sync_directory**，在独立聊天工具集（plain_chat）中注册，
子智能体经显式工具白名单继承；项目编排器（orchestrator）注册表不含此工具。
使用全局 SSH 配置；ssh_profile 是 ssh_list_servers 返回的服务器 **id**，不是显示名称。

~~~json
{
  "ssh_profile": "customer-beijing-01",
  "source": "/releases/v1.5.0",
  "destination": "/opt/product/releases/v1.5.0",
  "delete": false,
  "dry_run": false
}
~~~

## 同步语义

- 上传 source **目录内容**，不在 destination 内再嵌套一层源目录名。
- source 必须存在，经过既有工作区／额外允许目录校验。工作区外的 /releases 需要已配置的访问授权。
- 运行时向模型注入当前文件工作区、额外授权路径和 local_zsh 的实际执行目录。文件工具相对路径以工作区为基准；local_zsh 命令相对路径以它自己的执行目录为基准，上传其产物应使用绝对路径。
- 位于 .jkcodingagent 下的合法会话工作区及 local_zsh 产物目录可以作为 source；应用配置根目录、SSH 敏感目录及 Git 元数据仍禁止同步。
- destination 为远端绝对 POSIX 路径，拒绝根目录、点路径和控制字符。目标父目录须存在，末级目录交由 rsync 创建。
- delete=false 保留远端多余文件；delete=true 使用 --delete-delay 删除目标目录中源端不存在的文件。
- dry_run=true 不改动远端文件，但仍需 SSH 认证及 rsync 通信。本地可产生正常审计记录和首次主机指纹记录。
- 递归传输，保留权限和修改时间；不保留 owner/group，不传输设备节点。符号链接不解引用，--safe-links 忽略指向源目录树外的链接。
- .git/ 与 .jkcodingagent/ 始终排除，远端同名排除目录也不删除。
- 不提供任意 rsync 参数、远端命令或提权入口。传输不自动重试；非零退出（包括 23/24）、取消、超时均不能视为同步成功，远端可能已经有部分变更。

## 依赖与认证

本机要求 macOS/Linux、rsync **3.1+** 和支持 SSH_ASKPASS_REQUIRE=force 的 OpenSSH **8.4+**；
远端要求 SSH 服务及 rsync **3.0+**（支持 --protect-args）。
macOS 自带的 openrsync/rsync 2.6.9 不满足要求。工具按应用 PATH 顺序寻找符合版本要求的 rsync，跳过旧版；macOS 还检查 /opt/homebrew/bin 和 /usr/local/bin，兼容从桌面启动时未继承终端 PATH 的情况。最终使用已验证的绝对路径执行，不自动安装或降低参数保护。全部候选不满足要求时，报错列出检查路径和原因。

已有密码认证和私钥认证（含私钥口令）均复用。先由现有 russh 连接完成认证和 TOFU 指纹验证，
再将该次已验证公钥写入专用临时 known_hosts，OpenSSH 使用 StrictHostKeyChecking=yes。
不读取用户 SSH config、不使用 SSH agent、不使用系统 known_hosts 替代应用信任记录。

口令仅短暂保存在随机命名的 0700 临时目录内的 0600 文件中，由专用 askpass 读取；
不会进入 argv、环境变量值或结果结构。调用结束清理临时目录。
异常退出应用／操作系统崩溃可能留下临时目录，需要按系统临时文件生命周期清理。

## 权限、生命周期与结果

工具按 ReviewRequired 和外部副作用声明。与 ssh_exec 保持相同审查语义：
未配置审查模型时拒绝；已配置模型时尊重服务器的审查开关；审查异常或拒绝则阻断并记录 SSH 审计。
送审内容含规范化源目录、目标目录、删除／预演标记和服务器信息。

阻塞文件／进程操作在 spawn_blocking 执行。传输超时复用服务器 defaultTimeoutSecs（1–300 秒）；
认证阶段单独受同一超时值约束。取消或超时终止整个本地 rsync/OpenSSH 进程组并回收。

结果通过 ToolResult.data 返回 SshSyncResult（定义由 src/types.ts 导出），包括实际退出码、取消／超时、
耗时、stdout/stderr、截断标记、末次进度和独立 auditError。
审计写入失败不掩盖已发生的远端同步结果。输出按服务器配置限制，并设 1 MiB 硬上限。

ssh-sync-progress 事件每 250 ms 至多一次，并在结束时发送末次进度；
载荷为 SshSyncProgressEvent，含 workspaceId、toolCallId、sshProfile 和 progress（transferredBytes、percent）。
目前提供事件与类型供消费，未新增独立进度面板。进度百分比来自 rsync 的动态文件列表，不代表成功；
最终以退出码和错误状态为准。

参考：[rsync 官方手册](https://download.samba.org/pub/rsync/rsync.1)。
