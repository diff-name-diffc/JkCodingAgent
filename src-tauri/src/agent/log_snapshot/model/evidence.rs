use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Instant;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SnapshotMetrics {
    pub total_duration_ms: u64,
    pub review_duration_ms: u64,
    pub download_duration_ms: u64,
    pub index_duration_ms: u64,
    pub publish_duration_ms: u64,
    pub indexed_lines: u64,
    pub index_bytes: u64,
    pub download_bytes_per_second: Option<f64>,
    pub index_lines_per_second: Option<f64>,
    pub index_ratio: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceMetrics {
    pub started_at: String,
    pub ended_at: String,
    pub duration_ms: u64,
    pub bytes_per_second: Option<f64>,
}

impl SourceMetrics {
    pub(crate) fn new(started_at: DateTime<Utc>, started: Instant, bytes: u64) -> Self {
        let elapsed = started.elapsed();
        let seconds = elapsed.as_secs_f64();
        Self {
            started_at: started_at.to_rfc3339(),
            ended_at: Utc::now().to_rfc3339(),
            duration_ms: duration_ms(elapsed),
            bytes_per_second: (seconds > 0.0).then_some(bytes as f64 / seconds),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileFingerprint {
    pub device: Option<u64>,
    pub inode: Option<u64>,
    pub size_bytes: u64,
    pub mtime_secs: Option<i64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceEvidence {
    pub fingerprint_before: Option<FileFingerprint>,
    pub fingerprint_after: Option<FileFingerprint>,
    pub source_changed: Option<bool>,
    pub collection: Option<SourceMetrics>,
}

impl SourceEvidence {
    pub(crate) fn file(
        before: FileFingerprint,
        after: FileFingerprint,
        collected_bytes: u64,
        collection: SourceMetrics,
    ) -> Self {
        let changed = before != after || after.size_bytes != collected_bytes;
        Self {
            fingerprint_before: Some(before),
            fingerprint_after: Some(after),
            source_changed: Some(changed),
            collection: Some(collection),
        }
    }

    pub(crate) fn docker(collection: SourceMetrics) -> Self {
        Self {
            collection: Some(collection),
            ..Self::default()
        }
    }
}

pub(crate) fn duration_ms(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::log_snapshot::model::{SnapshotFile, UpdateRecord, UpdateState};

    fn fingerprint(size: u64) -> FileFingerprint {
        FileFingerprint {
            device: Some(1),
            inode: Some(2),
            size_bytes: size,
            mtime_secs: Some(1),
        }
    }

    #[test]
    fn unchanged_metadata_does_not_mark_source_changed() {
        let evidence = SourceEvidence::file(
            fingerprint(100),
            fingerprint(100),
            100,
            SourceMetrics::new(Utc::now(), Instant::now(), 100),
        );
        assert_eq!(evidence.source_changed, Some(false));
        let changed = SourceEvidence::file(
            fingerprint(100),
            fingerprint(100),
            90,
            SourceMetrics::new(Utc::now(), Instant::now(), 90),
        );
        assert_eq!(changed.source_changed, Some(true));
    }

    #[test]
    fn old_records_keep_unknown_evidence() {
        let file: SnapshotFile = serde_json::from_value(serde_json::json!({
            "fileId":1,
            "source":{"type":"local_file","path":"/tmp/a.log"},
            "sourceLabel":"local:/tmp/a.log",
            "rawPath":"raw/1.log",
            "sizeBytes":100,
            "lineCount":1,
            "longLines":0
        }))
        .unwrap();
        assert_eq!(file.evidence.source_changed, None);
        let update: UpdateRecord = serde_json::from_value(serde_json::json!({
            "jobId":"old",
            "snapshotId":"old",
            "generation":1,
            "state":"ready",
            "createdAt":"2026-01-01T00:00:00Z",
            "updatedAt":"2026-01-01T00:00:00Z",
            "rawBytes":100,
            "fileCount":1,
            "error":null
        }))
        .unwrap();
        assert!(update.metrics.is_none());
        assert_eq!(update.state, UpdateState::Ready);
    }
}
