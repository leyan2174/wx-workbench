//! 统计仅保存客户端与专用后台探测的实测耗时，不保存消息或请求参数；分位数窗口固定。
use super::{transport::QueryFailure, RequestTiming};
use crate::daemon::cache::LatencyProbe;
use serde::Serialize;
use std::collections::{BTreeMap, VecDeque};

const RECENT_CAPACITY: usize = 1024;

#[derive(Default, Serialize)]
pub struct PhaseStatistics {
    pub observations: u64,
    pub min_ms: Option<f64>,
    pub mean_ms: Option<f64>,
    pub max_ms: Option<f64>,
}

impl PhaseStatistics {
    fn observe(&mut self, milliseconds: f64) {
        self.observations += 1;
        self.min_ms = Some(
            self.min_ms
                .map_or(milliseconds, |old| old.min(milliseconds)),
        );
        self.max_ms = Some(
            self.max_ms
                .map_or(milliseconds, |old| old.max(milliseconds)),
        );
        // 在线均值不累计总毫秒数；未观测时用 None，不把缺失阶段写成 0 ms。
        self.mean_ms = Some(self.mean_ms.map_or(milliseconds, |old| {
            old + (milliseconds - old) / self.observations as f64
        }));
    }
}

#[derive(Serialize)]
pub struct RecentQuantiles {
    pub scope: &'static str,
    pub capacity: usize,
    pub observations: usize,
    pub p50_ms: Option<f64>,
    pub p95_ms: Option<f64>,
}

#[derive(Serialize)]
pub struct RequestStatistics {
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub interrupted_requests: u64,
    pub failure_codes: BTreeMap<&'static str, u64>,
    pub interruption_codes: BTreeMap<&'static str, u64>,
    pub last_unsuccessful_phase: Option<&'static str>,
    pub phase_scope: &'static str,
    pub phases: BTreeMap<&'static str, PhaseStatistics>,
    pub recent_total: RecentQuantiles,
}

#[derive(Default)]
struct RequestAccumulator {
    successful_requests: u64,
    failed_requests: u64,
    interrupted_requests: u64,
    failure_codes: BTreeMap<&'static str, u64>,
    interruption_codes: BTreeMap<&'static str, u64>,
    last_unsuccessful_phase: Option<&'static str>,
    phases: BTreeMap<&'static str, PhaseStatistics>,
    recent_total: VecDeque<f64>,
}

impl RequestAccumulator {
    fn observe(&mut self, timing: &RequestTiming) {
        self.successful_requests += 1;
        for (name, value) in [
            ("serialize", timing.serialize_ms),
            ("connect", timing.connect_ms),
            ("write", timing.write_ms),
            ("wait_read", timing.wait_read_ms),
            ("parse", timing.parse_ms),
            ("authenticated_roundtrip", timing.authenticated_roundtrip_ms),
            ("total", Some(timing.total_ms)),
        ] {
            if let Some(value) = value {
                self.phases.entry(name).or_default().observe(value);
            }
        }
        if self.recent_total.len() == RECENT_CAPACITY {
            self.recent_total.pop_front();
        }
        self.recent_total.push_back(timing.total_ms);
    }

    fn unsuccessful(&mut self, error: &QueryFailure) {
        self.last_unsuccessful_phase = Some(error.phase);
        if error.is_stop() {
            self.interrupted_requests += 1;
            *self.interruption_codes.entry(error.code).or_default() += 1;
        } else {
            self.failed_requests += 1;
            *self.failure_codes.entry(error.code).or_default() += 1;
        }
    }

    fn observe_probe(&mut self, timing: &RequestTiming, probe: &LatencyProbe) {
        self.observe(timing);
        for (name, value) in [
            ("daemon_cache_resolve", Some(probe.daemon_cache_resolve_ms)),
            ("daemon_decrypt", probe.daemon_decrypt_ms),
            ("daemon_db_decrypt", probe.daemon_db_decrypt_ms),
            ("daemon_wal_apply", probe.daemon_wal_apply_ms),
            ("daemon_query", Some(probe.daemon_query_ms)),
        ] {
            if let Some(value) = value {
                self.phases.entry(name).or_default().observe(value);
            }
        }
    }

    fn finish(self) -> RequestStatistics {
        let mut recent: Vec<_> = self.recent_total.into_iter().collect();
        recent.sort_by(f64::total_cmp);
        let percentile = |percent: usize| -> Option<f64> {
            if recent.is_empty() {
                return None;
            }
            let rank = (recent.len() * percent).div_ceil(100);
            recent.get(rank.saturating_sub(1)).copied()
        };
        RequestStatistics {
            successful_requests: self.successful_requests,
            failed_requests: self.failed_requests,
            interrupted_requests: self.interrupted_requests,
            failure_codes: self.failure_codes,
            interruption_codes: self.interruption_codes,
            last_unsuccessful_phase: self.last_unsuccessful_phase,
            phase_scope: if self.phases.contains_key("daemon_query") {
                "successful_client_roundtrips_and_measured_fixed_session_probe_phases"
            } else {
                "all_successful_client_roundtrips_including_cold_and_warm_cache"
            },
            phases: self.phases,
            recent_total: RecentQuantiles {
                scope: "most_recent_successful_requests_nearest_rank",
                capacity: RECENT_CAPACITY,
                observations: recent.len(),
                p50_ms: percentile(50),
                p95_ms: percentile(95),
            },
        }
    }
}

#[derive(Default)]
pub(super) struct Statistics {
    requests: BTreeMap<&'static str, RequestAccumulator>,
}
impl Statistics {
    pub fn success(&mut self, operation: &'static str, timing: &RequestTiming) {
        self.requests.entry(operation).or_default().observe(timing);
    }
    pub fn success_probe(&mut self, timing: &RequestTiming, probe: &LatencyProbe) {
        self.requests
            .entry("daemon_probe")
            .or_default()
            .observe_probe(timing, probe);
    }
    pub fn unsuccessful(&mut self, operation: &'static str, error: &QueryFailure) {
        self.requests
            .entry(operation)
            .or_default()
            .unsuccessful(error);
    }
    pub fn finish(self) -> BTreeMap<&'static str, RequestStatistics> {
        self.requests
            .into_iter()
            .map(|(name, accumulator)| (name, accumulator.finish()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::cache::LATENCY_PROBE_KIND;

    #[test]
    fn daemon_statistics_skip_unexecuted_crypto_and_keep_client_quantiles() {
        let mut statistics = Statistics::default();
        let mut probe = LatencyProbe {
            version: 1,
            query_kind: LATENCY_PROBE_KIND.into(),
            cache_mode: "full_decrypt".into(),
            daemon_cache_resolve_ms: 6.0,
            daemon_decrypt_ms: Some(3.0),
            daemon_db_decrypt_ms: Some(2.0),
            daemon_wal_apply_ms: Some(1.0),
            daemon_query_ms: 4.0,
            decrypt_skipped_reason: None,
            rows_read: 2,
            row_limit: 10,
            possible_truncation: false,
            latest_timestamp: Some(200),
        };
        probe.validate(10).unwrap();
        statistics.success_probe(
            &RequestTiming {
                total_ms: 12.0,
                ..Default::default()
            },
            &probe,
        );
        probe.cache_mode = "cache_hit".into();
        probe.daemon_cache_resolve_ms = 1.0;
        probe.daemon_decrypt_ms = None;
        probe.daemon_db_decrypt_ms = None;
        probe.daemon_wal_apply_ms = None;
        probe.daemon_query_ms = 2.0;
        probe.decrypt_skipped_reason = Some("cache_hit_no_decryption".into());
        probe.validate(10).unwrap();
        statistics.success_probe(
            &RequestTiming {
                total_ms: 8.0,
                ..Default::default()
            },
            &probe,
        );
        let reports = statistics.finish();
        let report = &reports["daemon_probe"];
        assert_eq!(report.successful_requests, 2);
        assert_eq!(report.phases["daemon_query"].observations, 2);
        assert_eq!(report.phases["daemon_query"].mean_ms, Some(3.0));
        assert_eq!(report.phases["daemon_cache_resolve"].mean_ms, Some(3.5));
        for (phase, mean) in [
            ("daemon_decrypt", 3.0),
            ("daemon_db_decrypt", 2.0),
            ("daemon_wal_apply", 1.0),
        ] {
            assert_eq!(report.phases[phase].observations, 1);
            assert_eq!(report.phases[phase].mean_ms, Some(mean));
        }
        assert_eq!(report.recent_total.p50_ms, Some(8.0));
        assert_eq!(report.recent_total.p95_ms, Some(12.0));
    }
}
