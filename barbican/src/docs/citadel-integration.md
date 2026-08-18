# Citadel Integration

Documentation for the integration between Barbican and Citadel, including automatic triggers, progress reporting, and service-to-service communication.

## Table of Contents

- [Overview](#overview)
- [Architecture](#architecture)
- [Automatic Triggers](#automatic-triggers)
- [Progress Reporting](#progress-reporting)
- [Service-to-Service Communication](#service-to-service-communication)
- [Error Handling](#error-handling)
- [Configuration](#configuration)

---

## Overview

Barbican and Citadel work together to manage the complete inbox lifecycle:

- **Barbican**: Frontend service managing inbox metadata and API
- **Citadel**: Backend service handling physical file operations

**Key Integration Points:**
1. Barbican automatically triggers Citadel operations
2. Citadel reports progress back to Barbican
3. Citadel notifies Barbican of operation completion/failure
4. Both services share MongoDB inbox collection

---

## Architecture

### Integration Flow

```
┌─────────────┐      ┌─────────────┐      ┌─────────────┐
│   Client    │      │   Barbican  │      │   Citadel   │
└──────┬──────┘      └──────┬──────┘      └──────┬──────┘
       │                    │                    │
       │  1. Create Inbox   │                    │
       │───────────────────>│                    │
       │                    │                    │
       │  2. Accept (202)   │                    │
       │<───────────────────│                    │
       │                    │                    │
       │                    │  3. Trigger Create │
       │                    │───────────────────>│
       │                    │                    │
       │                    │  4. Create Directory
       │                    │                    │
       │                    │  5. Progress Update│
       │                    │<───────────────────│
       │                    │                    │
       │                    │  6. Complete       │
       │                    │<───────────────────│
       │                    │                    │
       │  7. Query Status   │                    │
       │───────────────────>│                    │
       │                    │                    │
       │  8. Return Status  │                    │
       │<───────────────────│                    │
       │                    │                    │
```

### Shared Data Model

Both services use the same MongoDB inbox collection:

```rust
pub struct InboxDB {
    pub _id: ObjectId,
    pub name: String,
    pub size: u64,
    pub status: InboxStatus,
    pub warehouse_path: String,
    pub inbox_path: Option<String>,
    pub lifetime: Option<u64>,
    pub archive_enabled: bool,
    pub archive_sites: Vec<String>,
    pub version: u32,  // Optimistic locking
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    
    // Progress tracking (updated by Citadel via Barbican)
    pub current_job_id: Option<String>,
    pub progress: Option<InboxProgress>,
    pub error: Option<InboxError>,
}
```

**Important:** Barbican owns all writes to the inbox collection. Citadel reads inbox metadata directly but must call Barbican's API for any state changes.

---

## Automatic Triggers

### Trigger Pattern

Barbican automatically triggers Citadel operations when external clients perform certain actions:

| Client Action | Barbican Response | Citadel Trigger |
|---------------|-------------------|-----------------|
| `POST /api/v1/inbox` | 202 Accepted | `POST /create/start` |
| `POST /api/v1/inbox/{id}/publish` | 202 Accepted | `POST /publish/start` |
| `DELETE /api/v1/inbox/{id}` | 202 Accepted | `POST /delete/start` |

### Implementation

```rust
// In handlers/create_inbox.rs
pub async fn create_inbox(
    State(state): State<Arc<AppState>>,
    Json(input): Json<CreateInboxRequest>,
) -> Result<AppJson<CreateInboxResponse>, AppError> {
    // 1. Create inbox metadata
    let inbox = state.store.insert(InboxDB {
        status: InboxStatus::Active,
        // ... other fields
    }).await?;
    
    // 2. Trigger Citadel asynchronously
    if let Some(citadel) = &state.citadel_client {
        tokio::spawn({
            let citadel = citadel.clone();
            let inbox_id = inbox._id;
            async move {
                let _ = citadel.trigger_create(inbox_id).await;
            }
        });
    }
    
    // 3. Return acceptance to client
    Ok(AppJson::new(CreateInboxResponse {
        inbox_id: inbox._id,
        status: "creating",
        message: "Inbox creation initiated",
    }))
}
```

### Trigger Configuration

```bash
# Citadel API endpoint
BARBICAN_CITADEL_API_URL=http://citadel:8081

# Timeout for trigger calls
BARBICAN_CITADEL_API_TIMEOUT_SECS=30

# Retry configuration
BARBICAN_CITADEL_API_RETRY_MAX=3
BARBICAN_CITADEL_TRIGGER_RETRY_MS=1000
```

### Retry Logic

Citadel triggers use exponential backoff:

```rust
pub async fn trigger_with_retry(
    &self,
    endpoint: &str,
    payload: &serde_json::Value,
) -> Result<(), CitadelError> {
    let mut attempts = 0;
    let mut delay = self.config.retry_initial_ms;
    
    loop {
        match self.http_post(endpoint, payload).await {
            Ok(_) => return Ok(()),
            Err(e) if attempts < self.config.retry_max => {
                attempts += 1;
                tokio::time::sleep(Duration::from_millis(delay)).await;
                delay *= 2; // Exponential backoff
            }
            Err(e) => return Err(e),
        }
    }
}
```

---

## Progress Reporting

### Progress Update Pattern

Citadel periodically reports progress to Barbican during long-running operations:

**Endpoint Pattern:**
```
POST /api/v1/inbox/{id}/{operation}/progress
```

**Request Body:**
```json
{
  "job_id": "uuid-string",
  "phase": "copying",
  "total_files": 100,
  "processed_files": 45,
  "total_bytes": 1073741824,
  "transferred_bytes": 483183820
}
```

**Response:**
```json
{
  "inbox_id": "507f1f77bcf86cd799439011",
  "status": "publishing",
  "progress": {
    "phase": "copying",
    "percent_complete": 45
  }
}
```

### Progress Calculation

Barbican calculates percentage complete:

```rust
fn calculate_percent_complete(progress: &InboxProgress) -> u8 {
    if progress.total_bytes == 0 {
        return 0;
    }
    ((progress.transferred_bytes as f64 / progress.total_bytes as f64) * 100.0) as u8
}
```

### Recommended Update Frequency

- **Progress Updates**: Every 30 seconds during active operations
- **Final Update**: Always send complete/failure notification
- **Idle Periods**: No updates needed when operation is paused

### Progress Phases

| Phase | Description | Operation |
|-------|-------------|-----------|
| `preparing` | Setting up directories, validating paths | create |
| `copying` | Transferring files to destination | publish |
| `verifying` | Checking file integrity after transfer | publish |
| `cleaning` | Removing source files after successful transfer | delete |
| `finalizing` | Updating metadata, completing operation | all |

---

## Service-to-Service Communication

### Authentication

Citadel authenticates to Barbican using a JWT service account:

**Configuration:**
```bash
# On Citadel side
CITADEL_BARBICAN_API_URL=http://barbican:8080
CITADEL_BARBICAN_SERVICE_TOKEN_FILE=/etc/citadel/barbican.jwt

# On Barbican side
BARBICAN_CITADEL_SERVICE_TOKEN_FILE=/etc/barbican/citadel.jwt
BARBICAN_INTERNAL_ROLES=internal
```

**Token Requirements:**
- Must have `internal` or `admin` role
- Must be valid (not expired)
- Must be signed by trusted Keycloak instance

### HTTP Client

Barbican's Citadel client implementation:

```rust
pub struct CitadelClient {
    http_client: reqwest::Client,
    base_url: String,
    service_token: String,
    timeout: Duration,
    retry_max: u32,
}

impl CitadelClient {
    pub fn new(config: &Config) -> Result<Self, CitadelClientError> {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.citadel_api_timeout_secs))
            .build()?;
        
        let service_token = load_service_token(&config.citadel_service_token_file)?;
        
        Ok(Self {
            http_client,
            base_url: config.citadel_api_url.clone(),
            service_token,
            timeout: Duration::from_secs(config.citadel_api_timeout_secs),
            retry_max: config.citadel_api_retry_max,
        })
    }
    
    pub async fn trigger_create(&self, inbox_id: ObjectId) -> Result<(), CitadelError> {
        let url = format!("{}/api/v1/inbox/{}/create/start", self.base_url, inbox_id);
        
        let payload = json!({
            "job_id": Uuid::new_v4().to_string()
        });
        
        let response = self.http_client
            .post(&url)
            .bearer_auth(&self.service_token)
            .json(&payload)
            .send()
            .await?;
        
        if !response.status().is_success() {
            return Err(CitadelError::ApiError(response.status()));
        }
        
        Ok(())
    }
}
```

### Network Considerations

- **Service Discovery**: Use Kubernetes service names or DNS
- **TLS**: Enable mutual TLS in production environments
- **Timeouts**: Configure appropriate timeouts (default: 30 seconds)
- **Retries**: Implement exponential backoff for transient failures
- **Circuit Breaker**: Consider circuit breaker pattern for resilience

---

## Error Handling

### Citadel Error Responses

When Citadel operations fail, it notifies Barbican:

**Request:**
```http
POST /api/v1/inbox/{id}/create/failure
Authorization: Bearer <citadel_jwt>
Content-Type: application/json
```

**Body:**
```json
{
  "job_id": "uuid-string",
  "error_code": "DIRECTORY_CREATE_FAILED",
  "error_message": "Permission denied creating /data/inboxes/abc123",
  "total_bytes": 0,
  "transferred_bytes": 0
}
```

**Response:**
```json
{
  "inbox_id": "507f1f77bcf86cd799439011",
  "status": "failed",
  "error": "DIRECTORY_CREATE_FAILED"
}
```

### Error Code Categories

| Category | Error Codes | Description |
|----------|-------------|-------------|
| Directory | `DIRECTORY_CREATE_FAILED`, `DIRECTORY_NOT_FOUND` | File system errors |
| Transfer | `TRANSFER_FAILED`, `FILE_NOT_FOUND`, `INSUFFICIENT_SPACE` | File operation errors |
| Permission | `PERMISSION_DENIED`, `ACCESS_FORBIDDEN` | Authorization errors |
| Validation | `INVALID_PATH`, `CHECKSUM_MISMATCH` | Data integrity errors |
| System | `INTERNAL_ERROR`, `TIMEOUT`, `SERVICE_UNAVAILABLE` | Infrastructure errors |

### Error Handling Strategy

1. **Log Error**: Citadel logs full error details
2. **Notify Barbican**: Call appropriate `/failure` endpoint
3. **Update State**: Barbican transitions inbox to `Failed`
4. **Alert Operators**: Monitoring system triggers alerts
5. **Manual Intervention**: Operators investigate and resolve

### Timeout Handling

**Citadel Responsibility:**
- Detect operation timeouts
- Report timeout as failure
- Clean up partial work

**Barbican Responsibility:**
- Track operation start time
- Detect stale progress updates
- Alert on operations exceeding SLA

```rust
// Detect stale progress (no update in 5 minutes)
fn is_progress_stale(progress: &InboxProgress) -> bool {
    let age = Utc::now() - progress.updated_at;
    age > chrono::Duration::minutes(5)
}
```

---

## Configuration

### Barbican Configuration

```bash
# Citadel API endpoint
BARBICAN_CITADEL_API_URL=http://citadel:8081

# HTTP client timeout
BARBICAN_CITADEL_API_TIMEOUT_SECS=30

# Retry configuration
BARBICAN_CITADEL_API_RETRY_MAX=3
BARBICAN_CITADEL_TRIGGER_RETRY_MS=1000

# Service account token
BARBICAN_CITADEL_SERVICE_TOKEN_FILE=/etc/barbican/citadel.jwt

# Inbox base path (for validation)
BARBICAN_INBOX_BASE_PATH=/data/inboxes
```

### Citadel Configuration

```bash
# Barbican API endpoint
CITADEL_BARBICAN_API_URL=http://barbican:8080

# Service account token
CITADEL_BARBICAN_SERVICE_TOKEN_FILE=/etc/citadel/barbican.jwt

# Progress update interval
CITADEL_PROGRESS_UPDATE_INTERVAL_SECS=30

# Operation timeouts
CITADEL_CREATE_TIMEOUT_SECS=300
CITADEL_PUBLISH_TIMEOUT_SECS=600
CITADEL_DELETE_TIMEOUT_SECS=300
```

### Environment-Specific Settings

#### Development

```bash
BARBICAN_CITADEL_API_URL=http://localhost:8081
BARBICAN_CITADEL_API_TIMEOUT_SECS=60
BARBICAN_CITADEL_API_RETRY_MAX=5
```

#### Production

```bash
BARBICAN_CITADEL_API_URL=http://citadel.data-warehouse.svc.cluster.local:8081
BARBICAN_CITADEL_API_TIMEOUT_SECS=30
BARBICAN_CITADEL_API_RETRY_MAX=3
BARBICAN_CITADEL_SERVICE_TOKEN_FILE=/run/secrets/citadel-jwt
```

---

## Monitoring and Observability

### Metrics to Track

**Barbican Side:**
- Citadel trigger success/failure rate
- Progress update frequency
- Operation completion time (P50, P95, P99)
- Error code distribution

**Citadel Side:**
- API response time from Barbican
- Token refresh frequency
- Retry attempt count
- Operation throughput

### Health Checks

**Barbican `/ready` endpoint should verify:**
- MongoDB connectivity
- Citadel API reachability
- JWT validation working

**Example:**
```json
{
  "status": "ready",
  "checks": {
    "mongodb": "ok",
    "citadel_api": "ok",
    "jwt_validation": "ok"
  }
}
```

### Logging

**Structured JSON logging recommended:**

```json
{
  "timestamp": "2024-01-01T00:00:00Z",
  "level": "INFO",
  "service": "barbican",
  "event": "citadel_trigger_success",
  "inbox_id": "507f1f77bcf86cd799439011",
  "operation": "create",
  "citadel_url": "http://citadel:8081",
  "duration_ms": 45
}
```

---

## Testing

### Integration Test Strategy

1. **Mock Citadel**: Use mock server for unit tests
2. **Test Environment**: Deploy both services for integration tests
3. **Chaos Testing**: Simulate Citadel failures and timeouts
4. **Load Testing**: Verify performance under high trigger volume

### Example Test

```rust
#[tokio::test]
async fn test_citadel_trigger_on_create() {
    // Setup mock Citadel server
    let mock_citadel = MockServer::start().await;
    
    mock_citadel.mock(|when, then| {
        when.method(POST)
            .path("/api/v1/inbox/abc123/create/start");
        then.status(200)
            .json_body(json!({"status": "started"}));
    });
    
    // Create test Barbican app with mock Citadel URL
    let config = Config {
        citadel_api_url: mock_citadel.uri(),
        ..Default::default()
    };
    
    let app = create_test_app(config).await;
    
    // Trigger inbox creation
    let response = app
        .post("/api/v1/inbox")
        .json(&CreateInboxRequest { /* ... */ })
        .send()
        .await;
    
    assert_eq!(response.status(), 202);
    
    // Verify Citadel was triggered
    mock_citadel.assert_mocked();
}
```

---

*See also:*
- [Architecture](architecture.md) - System design overview
- [State Machine](state-machine.md) - Inbox lifecycle
- [API Reference](api-reference.md) - Endpoint documentation
- [Configuration](configuration.md) - Environment variables