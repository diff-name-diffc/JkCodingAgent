use std::time::Instant;

use super::super::model::{duration_ms, SnapshotMetrics, UpdateState};

pub(super) struct StageClock {
    state: UpdateState,
    started: Instant,
    total_started: Instant,
    pub metrics: SnapshotMetrics,
    raw_bytes: u64,
    indexed_lines: u64,
    index_bytes: u64,
}

impl StageClock {
    pub(super) fn new() -> Self {
        let now = Instant::now();
        Self {
            state: UpdateState::Reviewing,
            started: now,
            total_started: now,
            metrics: SnapshotMetrics::default(),
            raw_bytes: 0,
            indexed_lines: 0,
            index_bytes: 0,
        }
    }

    pub(super) fn transition(&mut self, state: UpdateState) {
        if self.state == state {
            return;
        }
        self.finish_stage();
        self.state = state;
        self.started = Instant::now();
    }

    pub(super) fn set_raw_bytes(&mut self, bytes: u64) {
        self.raw_bytes = bytes;
    }

    pub(super) fn set_index(&mut self, lines: u64, bytes: u64) {
        self.indexed_lines = lines;
        self.index_bytes = bytes;
    }

    pub(super) fn finish(&mut self) -> SnapshotMetrics {
        self.finish_stage();
        self.metrics.total_duration_ms = duration_ms(self.total_started.elapsed());
        self.metrics.indexed_lines = self.indexed_lines;
        self.metrics.index_bytes = self.index_bytes;
        self.metrics.download_bytes_per_second =
            rate(self.raw_bytes, self.metrics.download_duration_ms);
        self.metrics.index_lines_per_second =
            rate(self.indexed_lines, self.metrics.index_duration_ms);
        self.metrics.index_ratio = (self.raw_bytes > 0 && self.index_bytes > 0)
            .then_some(self.index_bytes as f64 / self.raw_bytes as f64);
        self.metrics.clone()
    }

    fn finish_stage(&mut self) {
        let elapsed = self.started.elapsed();
        let elapsed_ms = duration_ms(elapsed);
        match self.state {
            UpdateState::Reviewing => self.metrics.review_duration_ms += elapsed_ms,
            UpdateState::Syncing => self.metrics.download_duration_ms += elapsed_ms,
            UpdateState::Indexing => self.metrics.index_duration_ms += elapsed_ms,
            UpdateState::Publishing => self.metrics.publish_duration_ms += elapsed_ms,
            UpdateState::Queued
            | UpdateState::Ready
            | UpdateState::Failed
            | UpdateState::Cancelled => {}
        }
    }
}

fn rate(value: u64, duration_ms: u64) -> Option<f64> {
    if value == 0 || duration_ms == 0 {
        return None;
    }
    Some(value as f64 / (duration_ms as f64 / 1000.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_stage_durations_and_rates() {
        let mut clock = StageClock::new();
        std::thread::sleep(std::time::Duration::from_millis(2));
        clock.transition(UpdateState::Syncing);
        clock.set_raw_bytes(100);
        std::thread::sleep(std::time::Duration::from_millis(2));
        clock.transition(UpdateState::Indexing);
        clock.set_index(10, 400);
        std::thread::sleep(std::time::Duration::from_millis(2));
        clock.transition(UpdateState::Publishing);
        let metrics = clock.finish();
        let stages = metrics.review_duration_ms
            + metrics.download_duration_ms
            + metrics.index_duration_ms
            + metrics.publish_duration_ms;
        assert!(metrics.total_duration_ms >= stages);
        assert_eq!(metrics.indexed_lines, 10);
        assert_eq!(metrics.index_bytes, 400);
        assert_eq!(metrics.index_ratio, Some(4.0));
        assert!(metrics.download_bytes_per_second.is_some());
        assert!(metrics.index_lines_per_second.is_some());
    }
}
