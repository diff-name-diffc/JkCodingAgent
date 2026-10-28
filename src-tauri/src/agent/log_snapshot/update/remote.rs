use super::super::model::{FileFingerprint, FINGERPRINT_BEGIN, FINGERPRINT_END};
use super::UpdateFailure;

pub(super) fn parse_fingerprints(
    stderr: &str,
) -> Result<(FileFingerprint, FileFingerprint), UpdateFailure> {
    let mut before = None;
    let mut after = None;
    for line in stderr.lines() {
        if let Some(raw) = line.strip_prefix(FINGERPRINT_BEGIN) {
            before = Some(parse_fingerprint(raw)?);
        } else if let Some(raw) = line.strip_prefix(FINGERPRINT_END) {
            after = Some(parse_fingerprint(raw)?);
        }
    }
    match (before, after) {
        (Some(before), Some(after)) => Ok((before, after)),
        _ => Err(UpdateFailure::failed(
            "远程日志读取成功但未取得完整的源文件前后指纹",
        )),
    }
}

fn parse_fingerprint(raw: &str) -> Result<FileFingerprint, UpdateFailure> {
    let fields = raw.split(':').collect::<Vec<_>>();
    if fields.len() != 4 {
        return Err(UpdateFailure::failed(format!("源文件指纹格式无效：{raw}")));
    }
    let parse_u64 = |value: &str| {
        value
            .parse::<u64>()
            .map_err(|_| UpdateFailure::failed(format!("源文件指纹数值无效：{raw}")))
    };
    Ok(FileFingerprint {
        device: Some(parse_u64(fields[0])?),
        inode: Some(parse_u64(fields[1])?),
        size_bytes: parse_u64(fields[2])?,
        mtime_secs: Some(
            fields[3]
                .parse::<i64>()
                .map_err(|_| UpdateFailure::failed(format!("源文件指纹时间无效：{raw}")))?,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::log_snapshot::model::{SourceEvidence, SourceMetrics};
    use chrono::Utc;
    use std::time::Instant;

    #[test]
    fn parses_before_and_after_fingerprints() {
        let stderr = "noise\nAHA_LOG_FINGERPRINT_BEGIN=25:42:100:1700000000\nAHA_LOG_FINGERPRINT_END=25:42:120:1700000001\n";
        let (before, after) = parse_fingerprints(stderr).unwrap();
        assert_eq!(before.size_bytes, 100);
        assert_eq!(after.size_bytes, 120);
        let evidence = SourceEvidence::file(
            before,
            after,
            120,
            SourceMetrics::new(Utc::now(), Instant::now(), 120),
        );
        assert_eq!(evidence.source_changed, Some(true));
    }

    #[test]
    fn missing_fingerprint_is_an_error() {
        assert!(parse_fingerprints("cat: permission denied").is_err());
    }
}
