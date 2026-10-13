//! Windows 的 taskkill 在独立线程执行；成功必须来自退出码，不能只确认 spawn。
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// taskkill 对不存在的进程返回的固定退出码（"The process ... not found."）。
const TASKKILL_PROCESS_NOT_FOUND: i32 = 128;

#[cfg(windows)]
pub(super) fn taskkill(pid: u32) -> Result<(), String> {
    let mut command = Command::new("taskkill");
    command.args(["/PID", &pid.to_string(), "/T", "/F"]);
    run_command(command, Duration::from_secs(3))
}

pub(super) fn run_command(mut command: Command, timeout: Duration) -> Result<(), String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("启动浏览器终止命令失败：{error}"))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    Ok(())
                } else if status.code() == Some(TASKKILL_PROCESS_NOT_FOUND) {
                    // 进程已不存在（自然退出后 taskkill 补杀的常态，等价 unix 的
                    // ESRCH）：父进程已回收即视为终止成功，否则 has_exited 恒为
                    // false，浏览器永远无法重启。
                    Ok(())
                } else {
                    Err(format!("浏览器终止命令执行失败：{status}"))
                };
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => return stop_command(&mut child, "浏览器终止命令超时"),
            Err(error) => {
                return stop_command(&mut child, &format!("读取浏览器终止命令状态失败：{error}"));
            }
        }
    }
}

fn stop_command(child: &mut Child, reason: &str) -> Result<(), String> {
    if let Err(error) = child.kill() {
        return Err(format!("{reason}；终止命令回收失败：{error}"));
    }
    // 不调用无界 wait；连终止命令自身也必须在有界宽限内确认收敛。
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Err(reason.to_string()),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => return Err(format!("{reason}；仍未确认终止命令退出")),
            Err(error) => return Err(format!("{reason}；回收终止命令失败：{error}")),
        }
    }
}
