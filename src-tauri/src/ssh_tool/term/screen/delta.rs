//! 备用屏可见行差分：最长公共子序列保留滚动前后的旧行，仅返回新增/改写行。
//! 行删除与位置移动由完整快照表达；此差分不声称恢复两次读取之间的完整日志。

pub(super) fn added_lines(previous: &[String], current: &[String]) -> Vec<String> {
    // PTY 尺寸上限为 60 行，二维表最多 61 × 61，不扫描历史缓冲区。
    let mut shared = vec![vec![0usize; current.len() + 1]; previous.len() + 1];
    for old in (0..previous.len()).rev() {
        for new in (0..current.len()).rev() {
            shared[old][new] = if previous[old] == current[new] {
                shared[old + 1][new + 1] + 1
            } else {
                shared[old + 1][new].max(shared[old][new + 1])
            };
        }
    }
    let (mut old, mut new) = (0, 0);
    let mut added = Vec::new();
    while new < current.len() {
        if old < previous.len() && previous[old] == current[new] {
            old += 1;
            new += 1;
        } else if old < previous.len() && shared[old + 1][new] >= shared[old][new + 1] {
            old += 1;
        } else {
            added.push(current[new].clone());
            new += 1;
        }
    }
    added
}
