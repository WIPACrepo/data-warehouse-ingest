//! Kubernetes job executor implementation
//!
//! Creates and manages Kubernetes Job resources for file transfer operations.
//! Each job runs the `citadel-job` binary from the `citadel:latest` image.

use std::collections::BTreeMap;

use crate::config::CitadelConfig;
use crate::error::{AppError, Result};
use crate::jobs::executor::JobExecutor;
use crate::jobs::types::{JobExecutorInfo, JobStatus, JobStatusDetail};
use crate::models::JobDB;
use k8s_openapi::api::batch::v1::Job as K8sJob;
use k8s_openapi::api::core::v1::{
    Capabilities, Container, EmptyDirVolumeSource, EnvVar, PodSecurityContext, PodSpec,
    PodTemplateSpec, ResourceRequirements, SecretVolumeSource, SecurityContext, Volume,
    VolumeMount,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use tracing::{error, info, warn};
use uuid::Uuid;

pub struct KubernetesExecutor {
    config: CitadelConfig,
    client: kube::Client,
    namespace: String,
}

impl KubernetesExecutor {
    pub async fn new(config: CitadelConfig) -> Result<Self> {
        // Use the configured kubeconfig path when set, else the default
        // in-cluster / kubeconfig resolution (P4.5).
        let client = if let Some(path) = &config.kube_config_path {
            let kubeconfig = kube::config::Kubeconfig::read_from(path).map_err(|e| {
                AppError::KubernetesError(format!(
                    "Failed to load kubeconfig from {}: {}",
                    path.display(),
                    e
                ))
            })?;
            let kube_config = kube::Config::from_custom_kubeconfig(
                kubeconfig,
                &kube::config::KubeConfigOptions::default(),
            )
            .await
            .map_err(|e| AppError::KubernetesError(format!("Failed to load kubeconfig: {}", e)))?;
            kube::Client::try_from(kube_config).map_err(|e| {
                AppError::KubernetesError(format!("Failed to create K8s client: {}", e))
            })?
        } else {
            kube::Client::try_default().await.map_err(|e| {
                AppError::KubernetesError(format!("Failed to create K8s client: {}", e))
            })?
        };

        let namespace = config.kube_namespace.clone();
        Ok(Self {
            config,
            client,
            namespace,
        })
    }

    /// Create the Kubernetes Job spec for a given Citadel job
    fn create_job_spec(&self, job: &JobDB, nonce: &str, timestamp: &str) -> Result<K8sJob> {
        Self::create_job_spec_static(&self.config, &self.namespace, job, nonce, timestamp)
    }

    /// Static version for testing without a real K8s client
    pub(crate) fn create_job_spec_static(
        config: &CitadelConfig,
        namespace: &str,
        job: &JobDB,
        nonce: &str,
        timestamp: &str,
    ) -> Result<K8sJob> {
        let job_name = format!("citadel-job-{}", job.job_id);

        let job_type_str = match &job.job_type {
            crate::jobs::types::JobType::PublishInbox { .. } => "publish_inbox",
            crate::jobs::types::JobType::DeleteInbox { .. } => "delete_inbox",
        };

        let (cpu_limit, memory_limit) = match &job.job_type {
            crate::jobs::types::JobType::PublishInbox { .. } => (
                &config.queue_publish_cpu_limit,
                &config.queue_publish_memory_limit,
            ),
            crate::jobs::types::JobType::DeleteInbox { .. } => (
                &config.queue_delete_cpu_limit,
                &config.queue_delete_memory_limit,
            ),
        };

        // Build resource limits/requests using BTreeMap<Quantity>
        let mut limits = BTreeMap::new();
        limits.insert("cpu".to_string(), Quantity(cpu_limit.clone()));
        limits.insert("memory".to_string(), Quantity(memory_limit.clone()));

        let mut requests = BTreeMap::new();
        requests.insert("cpu".to_string(), Quantity("500m".to_string()));
        requests.insert("memory".to_string(), Quantity("512Mi".to_string()));

        // Build the job environment: static job coordinates plus the
        // warehouse base and inbox security settings forwarded from this
        // Citadel instance (so citadel-job validates and transfers with the
        // same configuration).
        let mut env = vec![
            EnvVar {
                name: "JOB_ID".to_string(),
                value: Some(job.job_id.clone()),
                value_from: None,
            },
            EnvVar {
                name: "JOB_TYPE".to_string(),
                value: Some(job_type_str.to_string()),
                value_from: None,
            },
            EnvVar {
                name: "INBOX_ID".to_string(),
                value: Some(job.inbox_id.to_string()),
                value_from: None,
            },
            EnvVar {
                name: "INBOX_PATH".to_string(),
                value: Some(format!("/data/inboxes/{}", job.inbox_id)),
                value_from: None,
            },
            EnvVar {
                name: "WAREHOUSE_PATH".to_string(),
                value: Some(format!("/data/protected/{}", job.inbox_id)),
                value_from: None,
            },
            EnvVar {
                name: "CITADEL_CALLBACK_URL".to_string(),
                value: Some(format!(
                    "http://citadel.citadel.svc.cluster.local:{}/api/v1/job/callback",
                    config.bind_address.split(':').nth(1).unwrap_or("8081")
                )),
                value_from: None,
            },
            EnvVar {
                name: "CITADEL_CALLBACK_SECRET_FILE".to_string(),
                value: Some("/etc/citadel/secrets/callback-secret".to_string()),
                value_from: None,
            },
            EnvVar {
                name: "CITADEL_CALLBACK_TIMESTAMP".to_string(),
                value: Some(timestamp.to_string()),
                value_from: None,
            },
            EnvVar {
                name: "CITADEL_CALLBACK_NONCE".to_string(),
                value: Some(nonce.to_string()),
                value_from: None,
            },
            EnvVar {
                name: "CITADEL_TRANSFER_PARALLELISM".to_string(),
                value: Some(config.transfer_parallelism.to_string()),
                value_from: None,
            },
            EnvVar {
                name: "CITADEL_TRANSFER_CHUNK_SIZE_MB".to_string(),
                value: Some(config.transfer_chunk_size_mb.to_string()),
                value_from: None,
            },
            EnvVar {
                name: "CITADEL_JOB_METADATA_POLL_INTERVAL_SECS".to_string(),
                value: Some(config.job_metadata_poll_interval_secs.to_string()),
                value_from: None,
            },
            EnvVar {
                name: "CITADEL_PROGRESS_UPDATE_INTERVAL_SECS".to_string(),
                value: Some(config.progress_update_interval_secs.to_string()),
                value_from: None,
            },
            EnvVar {
                name: "CITADEL_MONGODB_URI".to_string(),
                value: Some(config.mongodb_uri.clone()),
                value_from: None,
            },
            EnvVar {
                name: "CITADEL_MONGODB_DATABASE".to_string(),
                value: Some(config.mongodb_database.clone()),
                value_from: None,
            },
        ];
        for (name, value) in config.inbox_env_vars() {
            env.push(EnvVar {
                name,
                value: Some(value),
                value_from: None,
            });
        }

        let container = Container {
            name: "citadel-job".to_string(),
            image: Some(config.job_image.clone()),
            command: Some(vec!["citadel-job".to_string()]),
            env: Some(env),
            resources: Some(ResourceRequirements {
                limits: Some(limits),
                requests: Some(requests),
                ..Default::default()
            }),
            security_context: Some(SecurityContext {
                allow_privilege_escalation: Some(false),
                read_only_root_filesystem: Some(true),
                capabilities: Some(Capabilities {
                    drop: Some(vec!["ALL".to_string()]),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            volume_mounts: Some(vec![
                VolumeMount {
                    name: "callback-secret".to_string(),
                    mount_path: "/etc/citadel/secrets".to_string(),
                    read_only: Some(true),
                    ..Default::default()
                },
                VolumeMount {
                    name: "tmp".to_string(),
                    mount_path: "/tmp".to_string(),
                    ..Default::default()
                },
                VolumeMount {
                    name: "data".to_string(),
                    mount_path: "/data".to_string(),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };

        let pod_spec = PodSpec {
            containers: vec![container],
            restart_policy: Some("Never".to_string()),
            service_account_name: Some("citadel-job".to_string()),
            security_context: Some(PodSecurityContext {
                run_as_non_root: Some(true),
                run_as_user: Some(1000),
                run_as_group: Some(1000),
                fs_group: Some(1000),
                ..Default::default()
            }),
            volumes: Some(vec![
                Volume {
                    name: "callback-secret".to_string(),
                    secret: Some(SecretVolumeSource {
                        secret_name: Some("citadel-callback-secret".to_string()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                Volume {
                    name: "tmp".to_string(),
                    empty_dir: Some(EmptyDirVolumeSource::default()),
                    ..Default::default()
                },
                Volume {
                    name: "data".to_string(),
                    empty_dir: Some(EmptyDirVolumeSource::default()),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };

        let template = PodTemplateSpec {
            metadata: Some(ObjectMeta {
                name: Some(job_name.clone()),
                namespace: Some(namespace.to_string()),
                ..Default::default()
            }),
            spec: Some(pod_spec),
        };

        // Build annotations using BTreeMap<String, String>
        let mut annotations = BTreeMap::new();
        annotations.insert(
            "citadel/job-max-duration".to_string(),
            config.job_max_duration_secs.to_string(),
        );
        annotations.insert(
            "citadel/job-submitted-at".to_string(),
            timestamp.to_string(),
        );

        let k8s_job = K8sJob {
            metadata: ObjectMeta {
                name: Some(job_name),
                namespace: Some(namespace.to_string()),
                annotations: Some(annotations),
                ..Default::default()
            },
            spec: Some(k8s_openapi::api::batch::v1::JobSpec {
                active_deadline_seconds: Some(config.job_max_duration_secs as i64),
                ttl_seconds_after_finished: Some(3600),
                backoff_limit: Some(1),
                template,
                ..Default::default()
            }),
            ..Default::default()
        };

        Ok(k8s_job)
    }
}

#[async_trait::async_trait]
impl JobExecutor for KubernetesExecutor {
    async fn submit(&self, job: &JobDB) -> Result<String> {
        let jobs_api = kube::api::Api::<K8sJob>::namespaced(self.client.clone(), &self.namespace);

        let nonce = Uuid::new_v4().to_string();
        let timestamp = chrono::Utc::now().to_rfc3339();

        let k8s_job = self.create_job_spec(job, &nonce, &timestamp)?;

        match jobs_api
            .create(&kube::api::PostParams::default(), &k8s_job)
            .await
        {
            Ok(created) => {
                let job_name = created.metadata.name.unwrap_or_default();
                info!(
                    "Submitted Kubernetes job {} for citadel job {}",
                    job_name, job.job_id
                );
                Ok(job_name)
            }
            Err(e) => {
                error!("Failed to create Kubernetes job: {}", e);
                Err(AppError::KubernetesError(format!(
                    "Failed to create K8s job: {}",
                    e
                )))
            }
        }
    }

    async fn cancel(&self, job_id: &str) -> Result<()> {
        let jobs_api = kube::api::Api::<K8sJob>::namespaced(self.client.clone(), &self.namespace);
        let job_name = format!("citadel-job-{}", job_id);

        let delete_params = kube::api::DeleteParams::default();

        match jobs_api.delete(&job_name, &delete_params).await {
            Ok(_) => {
                info!("Deleted Kubernetes job {}", job_name);
                Ok(())
            }
            Err(kube::Error::Api(e)) if e.code == 404 => {
                warn!(
                    "Kubernetes job {} not found (may have already completed)",
                    job_name
                );
                Ok(())
            }
            Err(e) => {
                error!("Failed to delete Kubernetes job {}: {}", job_name, e);
                Err(AppError::KubernetesError(format!(
                    "Failed to cancel K8s job {}: {}",
                    job_name, e
                )))
            }
        }
    }

    async fn status(&self, job_id: &str) -> Result<JobStatusDetail> {
        let jobs_api = kube::api::Api::<K8sJob>::namespaced(self.client.clone(), &self.namespace);
        let job_name = format!("citadel-job-{}", job_id);

        match jobs_api.get(&job_name).await {
            Ok(k8s_job) => {
                let status = k8s_job_status_to_citadel(&k8s_job);
                Ok(JobStatusDetail {
                    job_id: job_id.to_string(),
                    status,
                    progress: None,
                    executor: JobExecutorInfo {
                        executor_type: "kubernetes".to_string(),
                        executor_id: Some(job_name),
                    },
                })
            }
            Err(kube::Error::Api(e)) if e.code == 404 => Ok(JobStatusDetail {
                job_id: job_id.to_string(),
                status: JobStatus::Completed,
                progress: None,
                executor: JobExecutorInfo {
                    executor_type: "kubernetes".to_string(),
                    executor_id: Some(format!("citadel-job-{}", job_id)),
                },
            }),
            Err(e) => {
                error!("Failed to get Kubernetes job status: {}", e);
                Err(AppError::KubernetesError(format!(
                    "Failed to get K8s job status: {}",
                    e
                )))
            }
        }
    }
}

/// Convert Kubernetes job status to Citadel job status
fn k8s_job_status_to_citadel(k8s_job: &K8sJob) -> JobStatus {
    if let Some(status) = &k8s_job.status {
        if status.succeeded.map_or(false, |s| s > 0) {
            return JobStatus::Completed;
        }
        if status.failed.map_or(false, |f| f > 0) {
            return JobStatus::Failed;
        }
        if status.active.map_or(false, |a| a > 0) {
            return JobStatus::Running;
        }
    }

    JobStatus::Running
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_job_spec() {
        let config = CitadelConfig {
            job_image: "citadel:latest".to_string(),
            bind_address: "0.0.0.0:8081".to_string(),
            transfer_parallelism: 4,
            transfer_chunk_size_mb: 16,
            job_max_duration_secs: 86400,
            job_metadata_poll_interval_secs: 15,
            queue_publish_cpu_limit: "2".to_string(),
            queue_publish_memory_limit: "4Gi".to_string(),
            queue_delete_cpu_limit: "1".to_string(),
            queue_delete_memory_limit: "2Gi".to_string(),
            ..CitadelConfig::default()
        };

        let job = JobDB::new(
            "test-job-123".to_string(),
            mongodb::bson::oid::ObjectId::new(),
            crate::jobs::types::JobType::PublishInbox {
                inbox_id: mongodb::bson::oid::ObjectId::new(),
                notify_catalog: true,
                notify_lta: true,
            },
            "kubernetes",
        );

        let result = KubernetesExecutor::create_job_spec_static(
            &config,
            "test",
            &job,
            "nonce-123",
            "2024-01-01T00:00:00Z",
        );
        assert!(result.is_ok());

        let k8s_job = result.unwrap();
        assert_eq!(
            k8s_job.metadata.name,
            Some("citadel-job-test-job-123".to_string())
        );
        assert_eq!(k8s_job.metadata.namespace, Some("test".to_string()));

        // Check container config
        if let Some(spec) = &k8s_job.spec {
            let template = &spec.template;
            if let Some(pod_spec) = &template.spec {
                let containers = pod_spec.containers.clone();
                assert_eq!(containers.len(), 1);
                assert_eq!(containers[0].name, "citadel-job");

                // Check security context
                if let Some(sec) = &containers[0].security_context {
                    assert_eq!(sec.allow_privilege_escalation, Some(false));
                    assert_eq!(sec.read_only_root_filesystem, Some(true));
                }

                // P4.1: the new job knobs (progress cadence + shared Mongo)
                // are forwarded to citadel-job so the metadata bail-out poll
                // and progress reporting work in-cluster.
                let env = containers[0]
                    .env
                    .clone()
                    .expect("container env must be set");
                let env_map: BTreeMap<String, String> = env
                    .into_iter()
                    .map(|e| (e.name, e.value.unwrap_or_default()))
                    .collect();
                assert_eq!(
                    env_map
                        .get("CITADEL_PROGRESS_UPDATE_INTERVAL_SECS")
                        .map(String::as_str),
                    Some("30")
                );
                assert_eq!(
                    env_map.get("CITADEL_MONGODB_URI").map(String::as_str),
                    Some("mongodb://localhost:27017")
                );
                assert_eq!(
                    env_map.get("CITADEL_MONGODB_DATABASE").map(String::as_str),
                    Some("data_warehouse")
                );
            }
        }
    }
}
#[cfg(test)]
mod namespace_tests {
    use super::*;
    use mongodb::bson::oid::ObjectId;

    /// P4.5: the configured CITADEL_KUBE_NAMESPACE flows into the K8s job
    /// spec instead of the hardcoded default.
    #[test]
    fn test_configured_namespace_flows_into_spec() {
        let config = CitadelConfig {
            kube_namespace: "my-jobs-ns".to_string(),
            job_image: "citadel:latest".to_string(),
            bind_address: "0.0.0.0:8081".to_string(),
            transfer_parallelism: 4,
            transfer_chunk_size_mb: 16,
            job_max_duration_secs: 86400,
            job_metadata_poll_interval_secs: 15,
            queue_publish_cpu_limit: "2".to_string(),
            queue_publish_memory_limit: "4Gi".to_string(),
            queue_delete_cpu_limit: "1".to_string(),
            queue_delete_memory_limit: "2Gi".to_string(),
            ..CitadelConfig::default()
        };

        let job = JobDB::new(
            "test-job-ns".to_string(),
            ObjectId::new(),
            crate::jobs::types::JobType::PublishInbox {
                inbox_id: ObjectId::new(),
                notify_catalog: true,
                notify_lta: true,
            },
            "kubernetes",
        );

        let k8s_job = KubernetesExecutor::create_job_spec_static(
            &config,
            &config.kube_namespace,
            &job,
            "nonce",
            "2024-01-01T00:00:00Z",
        )
        .unwrap();
        assert_eq!(k8s_job.metadata.namespace.as_deref(), Some("my-jobs-ns"));
    }
}
