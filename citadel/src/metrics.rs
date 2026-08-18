//! Prometheus metrics for Citadel

use prometheus::{Counter, Histogram, HistogramOpts, Opts, Registry, IntGauge};

/// Citadel metrics
pub struct CitadelMetrics {
    pub registry: Registry,
    pub jobs_total: Counter,
    pub jobs_duration: Histogram,
    pub transfer_bytes_total: Counter,
    pub transfer_files_total: Counter,
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
            barbican_api_errors_total,
            callback_errors_total,
            circuit_breaker_state,
        })
    }
}