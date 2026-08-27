//! Prometheus metrics for Citadel

use prometheus::{Counter, CounterVec, Histogram, HistogramOpts, Opts, Registry, IntGauge};

/// Citadel metrics
pub struct CitadelMetrics {
    pub registry: Registry,
    pub jobs_total: Counter,
    pub jobs_duration: Histogram,
    pub transfer_bytes_total: Counter,
    pub transfer_files_total: Counter,
    pub transfer_errors_total: CounterVec,
    pub transfer_atomic_rollbacks_total: Counter,
    pub security_events_total: CounterVec,
    pub barbican_api_errors_total: Counter,
    pub callback_errors_total: Counter,
    pub circuit_breaker_state: IntGauge,
}

impl CitadelMetrics {
    pub fn new() -> Result<Self, prometheus::Error> {
        let registry = Registry::new();
        
        let jobs_total = Counter::with_opts(Opts::new(
            "citadel_jobs_total",
            "Total jobs processed by status",
        ))?;
        registry.register(Box::new(jobs_total.clone()))?;
        
        let jobs_duration = Histogram::with_opts(HistogramOpts::new(
            "citadel_jobs_duration_seconds",
            "Job execution duration",
        ))?;
        registry.register(Box::new(jobs_duration.clone()))?;
        
        let transfer_bytes_total = Counter::with_opts(Opts::new(
            "citadel_transfer_bytes_total",
            "Total bytes transferred",
        ))?;
        registry.register(Box::new(transfer_bytes_total.clone()))?;
        
        let transfer_files_total = Counter::with_opts(Opts::new(
            "citadel_transfer_files_total",
            "Total files transferred",
        ))?;
        registry.register(Box::new(transfer_files_total.clone()))?;
        
        let transfer_errors_total = CounterVec::new(
            Opts::new(
                "citadel_transfer_errors_total",
                "Transfer errors by type (no paths)",
            ),
            &["error_type"],
        )?;
        registry.register(Box::new(transfer_errors_total.clone()))?;
        
        let transfer_atomic_rollbacks_total = Counter::with_opts(Opts::new(
            "citadel_transfer_atomic_rollbacks_total",
            "Total atomic rollback operations",
        ))?;
        registry.register(Box::new(transfer_atomic_rollbacks_total.clone()))?;
        
        let security_events_total = CounterVec::new(
            Opts::new(
                "citadel_security_events_total",
                "Security audit events by type",
            ),
            &["event_type"],
        )?;
        registry.register(Box::new(security_events_total.clone()))?;
        
        let barbican_api_errors_total = Counter::with_opts(Opts::new(
            "citadel_barbican_api_errors_total",
            "Barbican API errors",
        ))?;
        registry.register(Box::new(barbican_api_errors_total.clone()))?;
        
        let callback_errors_total = Counter::with_opts(Opts::new(
            "citadel_callback_errors_total",
            "Callback authentication failures",
        ))?;
        registry.register(Box::new(callback_errors_total.clone()))?;
        
        let circuit_breaker_state = IntGauge::with_opts(Opts::new(
            "citadel_circuit_breaker_state",
            "Circuit breaker state (0=closed, 1=open)",
        ))?;
        registry.register(Box::new(circuit_breaker_state.clone()))?;
        
        Ok(Self {
            registry,
            jobs_total,
            jobs_duration,
            transfer_bytes_total,
            transfer_files_total,
            transfer_errors_total,
            transfer_atomic_rollbacks_total,
            security_events_total,
            barbican_api_errors_total,
            callback_errors_total,
            circuit_breaker_state,
        })
    }
}