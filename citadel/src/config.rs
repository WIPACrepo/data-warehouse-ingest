//! Citadel configuration from environment variables

use std::path::PathBuf;
use thiserror::Error;

/// Inbox-specific transfer configuration with security-hardened defaults.
#[derive(Debug, Clone)]
pub struct InboxConfig {
    pub allow_external_symlinks: bool,
    pub max_recursion_depth: u32,
    pub max_errors_before_fail: u32,
    pub max_path_length: Option<usize>,
    pub max_total_files: Option<u64>,
    pub max_total_symlinks: Option<u32>,
    pub max_symlink_depth: Option<u32>,
    pub max_symlink_references: Option<u32>,
    pub atomic_transfer: bool,
    pub pre_transfer_integrity_scan: bool,
}

impl Default for InboxConfig {
    fn default() -> Self {
        Self {
            allow_external_symlinks: false,
            max_recursion_depth: 25,
            max_errors_before_fail: 1,
            max_path_length: Some(4096),
            max_total_files: Some(100_000),
            max_total_symlinks: Some(1_000),
            max_symlink_depth: Some(10),
            max_symlink_references: Some(10),
            atomic_transfer: true,
            pre_transfer_integrity_scan: true,
        }
    }
}

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Missing required environment variable: {0}")]
    MissingEnvVar(String),
    #[error("Invalid configuration value for {0}: {1}")]
    InvalidValue(String, String),
}

/// Job executor type
#[derive(Debug, Clone, PartialEq)]
pub enum JobExecutorType {
    Kubernetes,
    Mock,
}

impl std::fmt::Display for JobExecutorType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobExecutorType::Kubernetes => write!(f, "kubernetes"),
            JobExecutorType::Mock => write!(f, "mock"),
        }
    }
}

/// Citadel service configuration
#[derive(Debug, Clone)]
pub struct CitadelConfig {
    // HTTP Server
    pub bind_address: String,
    pub prometheus_port: u16,

    // Storage paths
    pub inbox_base_path: PathBuf,

    // MongoDB
    pub mongodb_uri: String,
    pub mongodb_database: String,

    // Keycloak / JWT
    pub keycloak_url: String,
    pub keycloak_realm: String,
    pub keycloak_client_id: String,
    pub keycloak_client_secret: String,
    pub required_roles: Vec<String>,
    pub allowed_algorithms: Vec<String>,
    pub jwks_refresh_secs: u64,

    // Job execution
    pub job_executor: JobExecutorType,
    pub kube_config_path: Option<PathBuf>,
    pub kube_namespace: String,
    pub job_image: String,
    pub job_binary: String,
    pub job_max_duration_secs: u64,

    // Transfer settings
    pub transfer_parallelism: usize,
    pub transfer_chunk_size_mb: usize,

    // Inbox security settings (nested)
    pub inbox: InboxConfig,

    // Callback authentication
    pub job_callback_secret_file: PathBuf,

    // Warehouse protection
    pub protected_base_path: PathBuf,

    // Barbican API coordination
    pub barbican_api_url: String,
    pub barbican_api_retry_max: u32,
    pub barbican_api_retry_backoff_ms: u64,
    pub barbican_api_timeout_secs: u64,

    // Polling intervals
    pub job_metadata_poll_interval_secs: u64,
    pub progress_update_interval_secs: u64,

    // Queue rate limits
    pub queue_publish_rate_limit: usize,
    pub queue_delete_rate_limit: usize,
    pub queue_publish_cpu_limit: String,
    pub queue_publish_memory_limit: String,
    pub queue_delete_cpu_limit: String,
    pub queue_delete_memory_limit: String,

    // Integration retries
    pub catalog_notify_retry_max: u32,
    pub catalog_notify_retry_backoff_ms: u64,
    pub catalog_notify_timeout_secs: u64,
    pub lta_notify_retry_max: u32,
    pub lta_notify_retry_backoff_ms: u64,
    pub lta_notify_timeout_secs: u64,
}

impl Default for CitadelConfig {
    fn default() -> Self {
        Self {
            bind_address: "127.0.0.1:8081".to_string(),
            prometheus_port: 9090,
            inbox_base_path: PathBuf::from("./data/inboxes"),
            protected_base_path: PathBuf::from("./data/protected"),
            mongodb_uri: "mongodb://localhost:27017".to_string(),
            mongodb_database: "data_warehouse".to_string(),
            keycloak_url: "http://localhost:8080".to_string(),
            keycloak_realm: "data-warehouse".to_string(),
            keycloak_client_id: "citadel".to_string(),
            keycloak_client_secret: String::new(),
            required_roles: vec!["read".to_string(), "write".to_string()],
            allowed_algorithms: vec!["RS256".to_string()],
            jwks_refresh_secs: 60,
            job_executor: JobExecutorType::Kubernetes,
            kube_config_path: None,
            kube_namespace: "citadel-jobs".to_string(),
            job_image: "citadel:latest".to_string(),
            job_binary: "citadel-job".to_string(),
            job_max_duration_secs: 86400,
            transfer_parallelism: 4,
            transfer_chunk_size_mb: 16,
            inbox: InboxConfig::default(),
            job_callback_secret_file: PathBuf::from("/etc/citadel/secrets/callback-secret"),
            barbican_api_url: "http://barbican:8080".to_string(),
            barbican_api_retry_max: 5,
            barbican_api_retry_backoff_ms: 1000,
            barbican_api_timeout_secs: 10,
            job_metadata_poll_interval_secs: 15,
            progress_update_interval_secs: 30,
            queue_publish_rate_limit: 10,
            queue_delete_rate_limit: 20,
            queue_publish_cpu_limit: "2".to_string(),
            queue_publish_memory_limit: "4Gi".to_string(),
            queue_delete_cpu_limit: "1".to_string(),
            queue_delete_memory_limit: "2Gi".to_string(),
            catalog_notify_retry_max: 3,
            catalog_notify_retry_backoff_ms: 5000,
            catalog_notify_timeout_secs: 30,
            lta_notify_retry_max: 3,
            lta_notify_retry_backoff_ms: 5000,
            lta_notify_timeout_secs: 30,
        }
    }
}

impl CitadelConfig {
    /// Load configuration from environment variables
    pub fn from_env() -> Result<Self, ConfigError> {
        let mut config = Self::default();

        // Override with environment variables
        if let Ok(val) = std::env::var("CITADEL_BIND_ADDRESS") {
            config.bind_address = val;
        }
        if let Ok(val) = std::env::var("CITADEL_PROMETHEUS_PORT") {
            config.prometheus_port = val.parse().map_err(|_| {
                ConfigError::InvalidValue("CITADEL_PROMETHEUS_PORT".to_string(), val)
            })?;
        }
        if let Ok(val) = std::env::var("CITADEL_INBOX_BASE_PATH") {
            config.inbox_base_path = PathBuf::from(val);
        }
        if let Ok(val) = std::env::var("CITADEL_PROTECTED_BASE_PATH") {
            config.protected_base_path = PathBuf::from(val);
        }
        if let Ok(val) = std::env::var("CITADEL_MONGODB_URI") {
            config.mongodb_uri = val;
        }
        if let Ok(val) = std::env::var("CITADEL_MONGODB_DATABASE") {
            config.mongodb_database = val;
        }
        if let Ok(val) = std::env::var("CITADEL_KEYCLOAK_URL") {
            config.keycloak_url = val;
        }
        if let Ok(val) = std::env::var("CITADEL_KEYCLOAK_REALM") {
            config.keycloak_realm = val;
        }
        if let Ok(val) = std::env::var("CITADEL_KEYCLOAK_CLIENT_ID") {
            config.keycloak_client_id = val;
        }
        if let Ok(val) = std::env::var("CITADEL_KEYCLOAK_CLIENT_SECRET") {
            config.keycloak_client_secret = val;
        }
        if let Ok(val) = std::env::var("CITADEL_REQUIRED_ROLES") {
            config.required_roles = val.split(',').map(|s| s.trim().to_string()).collect();
        }
        if let Ok(val) = std::env::var("CITADEL_ALLOWED_ALGORITHMS") {
            config.allowed_algorithms = val.split(',').map(|s| s.trim().to_string()).collect();
        }
        if let Ok(val) = std::env::var("CITADEL_JWKS_REFRESH_SECS") {
            config.jwks_refresh_secs = val.parse().map_err(|_| {
                ConfigError::InvalidValue("CITADEL_JWKS_REFRESH_SECS".to_string(), val)
            })?;
        }
        if let Ok(val) = std::env::var("CITADEL_JOB_EXECUTOR") {
            config.job_executor = match val.to_lowercase().as_str() {
                "mock" => JobExecutorType::Mock,
                _ => JobExecutorType::Kubernetes,
            };
        }
        if let Ok(val) = std::env::var("CITADEL_KUBE_CONFIG_PATH") {
            config.kube_config_path = Some(PathBuf::from(val));
        }
        if let Ok(val) = std::env::var("CITADEL_KUBE_NAMESPACE") {
            config.kube_namespace = val;
        }
        if let Ok(val) = std::env::var("CITADEL_JOB_IMAGE") {
            config.job_image = val;
        }
        if let Ok(val) = std::env::var("CITADEL_JOB_BINARY") {
            config.job_binary = val;
        }
        if let Ok(val) = std::env::var("CITADEL_JOB_MAX_DURATION_SECS") {
            config.job_max_duration_secs = val.parse().map_err(|_| {
                ConfigError::InvalidValue("CITADEL_JOB_MAX_DURATION_SECS".to_string(), val)
            })?;
        }
        if let Ok(val) = std::env::var("CITADEL_TRANSFER_PARALLELISM") {
            config.transfer_parallelism = val.parse().map_err(|_| {
                ConfigError::InvalidValue("CITADEL_TRANSFER_PARALLELISM".to_string(), val)
            })?;
        }
        if let Ok(val) = std::env::var("CITADEL_TRANSFER_CHUNK_SIZE_MB") {
            config.transfer_chunk_size_mb = val.parse().map_err(|_| {
                ConfigError::InvalidValue("CITADEL_TRANSFER_CHUNK_SIZE_MB".to_string(), val)
            })?;
        }
        if let Ok(val) = std::env::var("CITADEL_JOB_CALLBACK_SECRET_FILE") {
            config.job_callback_secret_file = PathBuf::from(val);
        }
        if let Ok(val) = std::env::var("CITADEL_BARBICAN_API_URL") {
            config.barbican_api_url = val;
        }
        if let Ok(val) = std::env::var("CITADEL_BARBICAN_API_RETRY_MAX") {
            config.barbican_api_retry_max = val.parse().map_err(|_| {
                ConfigError::InvalidValue("CITADEL_BARBICAN_API_RETRY_MAX".to_string(), val)
            })?;
        }
        if let Ok(val) = std::env::var("CITADEL_BARBICAN_API_RETRY_BACKOFF_MS") {
            config.barbican_api_retry_backoff_ms = val.parse().map_err(|_| {
                ConfigError::InvalidValue("CITADEL_BARBICAN_API_RETRY_BACKOFF_MS".to_string(), val)
            })?;
        }
        if let Ok(val) = std::env::var("CITADEL_BARBICAN_API_TIMEOUT_SECS") {
            config.barbican_api_timeout_secs = val.parse().map_err(|_| {
                ConfigError::InvalidValue("CITADEL_BARBICAN_API_TIMEOUT_SECS".to_string(), val)
            })?;
        }
        if let Ok(val) = std::env::var("CITADEL_JOB_METADATA_POLL_INTERVAL_SECS") {
            config.job_metadata_poll_interval_secs = val.parse().map_err(|_| {
                ConfigError::InvalidValue(
                    "CITADEL_JOB_METADATA_POLL_INTERVAL_SECS".to_string(),
                    val,
                )
            })?;
        }
        if let Ok(val) = std::env::var("CITADEL_PROGRESS_UPDATE_INTERVAL_SECS") {
            config.progress_update_interval_secs = val.parse().map_err(|_| {
                ConfigError::InvalidValue("CITADEL_PROGRESS_UPDATE_INTERVAL_SECS".to_string(), val)
            })?;
        }
        if let Ok(val) = std::env::var("CITADEL_QUEUE_PUBLISH_RATE_LIMIT") {
            config.queue_publish_rate_limit = val.parse().map_err(|_| {
                ConfigError::InvalidValue("CITADEL_QUEUE_PUBLISH_RATE_LIMIT".to_string(), val)
            })?;
        }
        if let Ok(val) = std::env::var("CITADEL_QUEUE_DELETE_RATE_LIMIT") {
            config.queue_delete_rate_limit = val.parse().map_err(|_| {
                ConfigError::InvalidValue("CITADEL_QUEUE_DELETE_RATE_LIMIT".to_string(), val)
            })?;
        }
        if let Ok(val) = std::env::var("CITADEL_QUEUE_PUBLISH_CPU_LIMIT") {
            config.queue_publish_cpu_limit = val;
        }
        if let Ok(val) = std::env::var("CITADEL_QUEUE_PUBLISH_MEMORY_LIMIT") {
            config.queue_publish_memory_limit = val;
        }
        if let Ok(val) = std::env::var("CITADEL_QUEUE_DELETE_CPU_LIMIT") {
            config.queue_delete_cpu_limit = val;
        }
        if let Ok(val) = std::env::var("CITADEL_QUEUE_DELETE_MEMORY_LIMIT") {
            config.queue_delete_memory_limit = val;
        }
        if let Ok(val) = std::env::var("CITADEL_CATALOG_NOTIFY_RETRY_MAX") {
            config.catalog_notify_retry_max = val.parse().map_err(|_| {
                ConfigError::InvalidValue("CITADEL_CATALOG_NOTIFY_RETRY_MAX".to_string(), val)
            })?;
        }
        if let Ok(val) = std::env::var("CITADEL_CATALOG_NOTIFY_RETRY_BACKOFF_MS") {
            config.catalog_notify_retry_backoff_ms = val.parse().map_err(|_| {
                ConfigError::InvalidValue(
                    "CITADEL_CATALOG_NOTIFY_RETRY_BACKOFF_MS".to_string(),
                    val,
                )
            })?;
        }
        if let Ok(val) = std::env::var("CITADEL_CATALOG_NOTIFY_TIMEOUT_SECS") {
            config.catalog_notify_timeout_secs = val.parse().map_err(|_| {
                ConfigError::InvalidValue("CITADEL_CATALOG_NOTIFY_TIMEOUT_SECS".to_string(), val)
            })?;
        }
        if let Ok(val) = std::env::var("CITADEL_LTA_NOTIFY_RETRY_MAX") {
            config.lta_notify_retry_max = val.parse().map_err(|_| {
                ConfigError::InvalidValue("CITADEL_LTA_NOTIFY_RETRY_MAX".to_string(), val)
            })?;
        }
        if let Ok(val) = std::env::var("CITADEL_LTA_NOTIFY_RETRY_BACKOFF_MS") {
            config.lta_notify_retry_backoff_ms = val.parse().map_err(|_| {
                ConfigError::InvalidValue("CITADEL_LTA_NOTIFY_RETRY_BACKOFF_MS".to_string(), val)
            })?;
        }
        if let Ok(val) = std::env::var("CITADEL_LTA_NOTIFY_TIMEOUT_SECS") {
            config.lta_notify_timeout_secs = val.parse().map_err(|_| {
                ConfigError::InvalidValue("CITADEL_LTA_NOTIFY_TIMEOUT_SECS".to_string(), val)
            })?;
        }

        // Inbox security settings
        if let Ok(val) = std::env::var("CITADEL_INBOX_ALLOW_EXTERNAL_SYMLINKS") {
            config.inbox.allow_external_symlinks = parse_bool(&val)?;
        }
        if let Ok(val) = std::env::var("CITADEL_INBOX_MAX_RECURSION_DEPTH") {
            config.inbox.max_recursion_depth = val.parse().map_err(|_| {
                ConfigError::InvalidValue("CITADEL_INBOX_MAX_RECURSION_DEPTH".to_string(), val)
            })?;
        }
        if let Ok(val) = std::env::var("CITADEL_INBOX_MAX_ERRORS_BEFORE_FAIL") {
            config.inbox.max_errors_before_fail = val.parse().map_err(|_| {
                ConfigError::InvalidValue("CITADEL_INBOX_MAX_ERRORS_BEFORE_FAIL".to_string(), val)
            })?;
        }
        if let Ok(val) = std::env::var("CITADEL_INBOX_MAX_PATH_LENGTH") {
            let len: usize = val.parse().map_err(|_| {
                ConfigError::InvalidValue("CITADEL_INBOX_MAX_PATH_LENGTH".to_string(), val)
            })?;
            config.inbox.max_path_length = if len == 0 { None } else { Some(len) };
        }
        if let Ok(val) = std::env::var("CITADEL_INBOX_MAX_TOTAL_FILES") {
            let count: u64 = val.parse().map_err(|_| {
                ConfigError::InvalidValue("CITADEL_INBOX_MAX_TOTAL_FILES".to_string(), val)
            })?;
            config.inbox.max_total_files = if count == 0 { None } else { Some(count) };
        }
        if let Ok(val) = std::env::var("CITADEL_INBOX_MAX_TOTAL_SYMLINKS") {
            let count: u32 = val.parse().map_err(|_| {
                ConfigError::InvalidValue("CITADEL_INBOX_MAX_TOTAL_SYMLINKS".to_string(), val)
            })?;
            config.inbox.max_total_symlinks = if count == 0 { None } else { Some(count) };
        }
        if let Ok(val) = std::env::var("CITADEL_INBOX_MAX_SYMLINK_DEPTH") {
            let depth: u32 = val.parse().map_err(|_| {
                ConfigError::InvalidValue("CITADEL_INBOX_MAX_SYMLINK_DEPTH".to_string(), val)
            })?;
            config.inbox.max_symlink_depth = if depth == 0 { None } else { Some(depth) };
        }
        if let Ok(val) = std::env::var("CITADEL_INBOX_MAX_SYMLINK_REFERENCES") {
            let refs: u32 = val.parse().map_err(|_| {
                ConfigError::InvalidValue("CITADEL_INBOX_MAX_SYMLINK_REFERENCES".to_string(), val)
            })?;
            config.inbox.max_symlink_references = if refs == 0 { None } else { Some(refs) };
        }
        if let Ok(val) = std::env::var("CITADEL_INBOX_ATOMIC_TRANSFER") {
            config.inbox.atomic_transfer = parse_bool(&val)?;
        }
        if let Ok(val) = std::env::var("CITADEL_INBOX_PRE_TRANSFER_INTEGRITY_SCAN") {
            config.inbox.pre_transfer_integrity_scan = parse_bool(&val)?;
        }

        Ok(config)
    }

    /// Get the JWKS endpoint URL
    pub fn jwks_url(&self) -> String {
        format!(
            "{}/auth/realms/{}/protocol/openid-connect/certs",
            self.keycloak_url, self.keycloak_realm
        )
    }

    /// Get the issuer for JWT validation
    pub fn jwt_issuer(&self) -> String {
        format!("{}/auth/realms/{}", self.keycloak_url, self.keycloak_realm)
    }

    /// Environment variables that carry the warehouse base path and inbox
    /// security settings to job executors (citadel-job).
    ///
    /// Executors should forward these to spawned jobs so that the transfer
    /// runs against the same protected base and security limits as the
    /// Citadel service itself.
    pub fn inbox_env_vars(&self) -> Vec<(String, String)> {
        let inbox = &self.inbox;
        vec![
            (
                "CITADEL_PROTECTED_BASE_PATH".to_string(),
                self.protected_base_path.to_string_lossy().to_string(),
            ),
            (
                "CITADEL_INBOX_ALLOW_EXTERNAL_SYMLINKS".to_string(),
                inbox.allow_external_symlinks.to_string(),
            ),
            (
                "CITADEL_INBOX_MAX_RECURSION_DEPTH".to_string(),
                inbox.max_recursion_depth.to_string(),
            ),
            (
                "CITADEL_INBOX_MAX_ERRORS_BEFORE_FAIL".to_string(),
                inbox.max_errors_before_fail.to_string(),
            ),
            (
                "CITADEL_INBOX_MAX_PATH_LENGTH".to_string(),
                inbox
                    .max_path_length
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "0".to_string()),
            ),
            (
                "CITADEL_INBOX_MAX_TOTAL_FILES".to_string(),
                inbox
                    .max_total_files
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "0".to_string()),
            ),
            (
                "CITADEL_INBOX_MAX_TOTAL_SYMLINKS".to_string(),
                inbox
                    .max_total_symlinks
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "0".to_string()),
            ),
            (
                "CITADEL_INBOX_MAX_SYMLINK_DEPTH".to_string(),
                inbox
                    .max_symlink_depth
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "0".to_string()),
            ),
            (
                "CITADEL_INBOX_MAX_SYMLINK_REFERENCES".to_string(),
                inbox
                    .max_symlink_references
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "0".to_string()),
            ),
            (
                "CITADEL_INBOX_ATOMIC_TRANSFER".to_string(),
                inbox.atomic_transfer.to_string(),
            ),
            (
                "CITADEL_INBOX_PRE_TRANSFER_INTEGRITY_SCAN".to_string(),
                inbox.pre_transfer_integrity_scan.to_string(),
            ),
        ]
    }
}

fn parse_bool(val: &str) -> Result<bool, ConfigError> {
    match val.to_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(ConfigError::InvalidValue(
            "boolean".to_string(),
            val.to_string(),
        )),
    }
}
