//! Credential provider mock for local development.
//!
//! Loads AWS credentials from the local environment (via `aws-config`)
//! and serves them in ECS credential provider compatible format.

use std::time::Duration;

use chrono::Utc;
use serde::Serialize;

use crate::metadata::SharedState;

/// Default refresh interval when TTL cannot be determined (30 minutes).
#[allow(dead_code)]
pub const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// Minimum refresh interval to avoid excessive API calls (1 minute).
#[allow(dead_code)]
pub const MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// Maximum refresh interval ceiling (30 minutes).
#[allow(dead_code)]
pub const MAX_REFRESH_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// Credential loading errors.
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum CredentialError {
    /// Failed to load credentials from the environment.
    #[error("failed to load AWS credentials: {0}")]
    Load(String),

    /// No credentials available in the environment.
    #[error("no AWS credentials available")]
    NotAvailable,
}

/// AWS credentials response format (ECS credential provider compatible).
#[derive(Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
#[allow(dead_code)]
pub struct AwsCredentials {
    /// AWS access key ID.
    pub access_key_id: String,

    /// AWS secret access key.
    pub secret_access_key: String,

    /// Session token (optional, present for temporary credentials).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,

    /// Credential expiration timestamp (ISO 8601).
    pub expiration: String,

    /// Role ARN these credentials are associated with (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role_arn: Option<String>,
}

impl std::fmt::Debug for AwsCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsCredentials")
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"[REDACTED]")
            .field("token", &self.token.as_ref().map(|_| "[REDACTED]"))
            .field("expiration", &self.expiration)
            .field("role_arn", &self.role_arn)
            .finish()
    }
}

/// Compute the credential refresh interval from an ISO 8601 expiration timestamp.
///
/// Returns `min(TTL / 2, MAX_REFRESH_INTERVAL)`, clamped to at least
/// `MIN_REFRESH_INTERVAL`. If the timestamp cannot be parsed, returns
/// `DEFAULT_REFRESH_INTERVAL`.
#[must_use]
#[allow(dead_code)]
pub fn compute_refresh_interval(expiration: &str) -> Duration {
    let Ok(exp) = chrono::DateTime::parse_from_rfc3339(expiration) else {
        return DEFAULT_REFRESH_INTERVAL;
    };
    let now = Utc::now();
    let ttl = exp.signed_duration_since(now);
    let ttl_secs = ttl.num_seconds();
    if ttl_secs <= 0 {
        return MIN_REFRESH_INTERVAL;
    }
    let half_ttl = Duration::from_secs(ttl_secs.unsigned_abs() / 2);
    half_ttl.clamp(MIN_REFRESH_INTERVAL, MAX_REFRESH_INTERVAL)
}

/// Load AWS credentials from the local environment.
///
/// Uses the full AWS credential chain (env vars, SSO, profiles, IMDS, etc.)
/// via `aws-config`. If `role_arn` is provided, it is included in the response
/// but does **not** trigger an `AssumeRole` call — the local credentials are
/// used directly.
#[cfg(not(tarpaulin_include))]
#[allow(dead_code)]
pub async fn load_local_credentials(
    role_arn: Option<&str>,
) -> Result<AwsCredentials, CredentialError> {
    use aws_credential_types::provider::ProvideCredentials;

    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .load()
        .await;

    let provider = config
        .credentials_provider()
        .ok_or(CredentialError::NotAvailable)?;

    let creds = provider
        .provide_credentials()
        .await
        .map_err(|e| CredentialError::Load(e.to_string()))?;

    let expiration = creds.expiry().map_or_else(
        || {
            // Long-lived credentials: set expiration to now + 12 hours
            let future = Utc::now() + chrono::Duration::hours(12);
            future.format("%Y-%m-%dT%H:%M:%SZ").to_string()
        },
        |exp| {
            // Convert SystemTime to ISO 8601 via chrono
            let duration = exp
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            chrono::DateTime::from_timestamp(
                duration.as_secs().cast_signed(),
                duration.subsec_nanos(),
            )
            .map_or_else(
                || Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                |dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            )
        },
    );

    Ok(AwsCredentials {
        access_key_id: creds.access_key_id().to_string(),
        secret_access_key: creds.secret_access_key().to_string(),
        token: creds.session_token().map(ToString::to_string),
        expiration,
        role_arn: role_arn.map(ToString::to_string),
    })
}

/// Background credential refresher for service mode.
///
/// Periodically reloads AWS credentials from the local environment and
/// updates the shared metadata server state. The refresh interval is
/// computed from the credential expiration timestamp.
#[allow(dead_code)]
pub struct CredentialRefresher {
    state: SharedState,
    role_arn: Option<String>,
}

impl CredentialRefresher {
    /// Create a new credential refresher.
    #[must_use]
    #[allow(dead_code)]
    pub const fn new(state: SharedState, role_arn: Option<String>) -> Self {
        Self { state, role_arn }
    }

    /// Replace the credentials in the shared state.
    #[allow(dead_code)]
    pub async fn update_state(state: &SharedState, creds: AwsCredentials) {
        let mut guard = state.write().await;
        guard.credentials = Some(creds);
    }

    /// Start the background refresh loop.
    ///
    /// Returns a `JoinHandle` that can be aborted to stop the loop.
    /// On each iteration the refresher attempts to load credentials:
    /// on success, the shared state is updated and the loop sleeps for an
    /// interval derived from the new credential's TTL;
    /// on failure, it logs a warning and sleeps for 60s before retrying.
    #[cfg(not(tarpaulin_include))]
    #[allow(dead_code)]
    pub fn start(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                match load_local_credentials(self.role_arn.as_deref()).await {
                    Ok(creds) => {
                        let interval = compute_refresh_interval(&creds.expiration);
                        tracing::debug!(
                            expiration = %creds.expiration,
                            "refreshed AWS credentials"
                        );
                        Self::update_state(&self.state, creds).await;
                        tokio::time::sleep(interval).await;
                    }
                    Err(e) => {
                        tracing::warn!("failed to refresh AWS credentials: {e}");
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                }
            }
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn aws_credentials_serialization() {
        let creds = AwsCredentials {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
            token: Some("FwoGZXIv...".to_string()),
            expiration: "2026-03-21T01:00:00Z".to_string(),
            role_arn: Some("arn:aws:iam::123456789012:role/my-role".to_string()),
        };

        let json = serde_json::to_value(&creds).expect("should serialize");
        assert_eq!(json["AccessKeyId"], "AKIAIOSFODNN7EXAMPLE");
        assert_eq!(
            json["SecretAccessKey"],
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
        );
        assert_eq!(json["Token"], "FwoGZXIv...");
        assert_eq!(json["Expiration"], "2026-03-21T01:00:00Z");
        assert_eq!(json["RoleArn"], "arn:aws:iam::123456789012:role/my-role");
    }

    #[test]
    fn aws_credentials_without_token() {
        let creds = AwsCredentials {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".to_string(),
            secret_access_key: "secret".to_string(),
            token: None,
            expiration: "2026-03-21T01:00:00Z".to_string(),
            role_arn: None,
        };

        let json = serde_json::to_value(&creds).expect("should serialize");
        assert!(json.get("Token").is_none(), "Token should be omitted");
        assert!(json.get("RoleArn").is_none(), "RoleArn should be omitted");
    }

    #[test]
    fn aws_credentials_with_role_arn() {
        let creds = AwsCredentials {
            access_key_id: "AKID".to_string(),
            secret_access_key: "secret".to_string(),
            token: None,
            expiration: "2026-03-21T01:00:00Z".to_string(),
            role_arn: Some("arn:aws:iam::111:role/test".to_string()),
        };

        let json = serde_json::to_value(&creds).expect("should serialize");
        assert_eq!(json["RoleArn"], "arn:aws:iam::111:role/test");
        assert!(json.get("Token").is_none());
    }

    #[test]
    fn debug_redacts_sensitive_fields() {
        let creds = AwsCredentials {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
            token: Some("session-token-value".to_string()),
            expiration: "2026-03-21T01:00:00Z".to_string(),
            role_arn: None,
        };

        let debug_output = format!("{creds:?}");
        assert!(
            !debug_output.contains("wJalrXUtnFEMI"),
            "secret_access_key must be redacted in Debug output"
        );
        assert!(
            !debug_output.contains("session-token-value"),
            "token must be redacted in Debug output"
        );
        assert!(
            debug_output.contains("AKIAIOSFODNN7EXAMPLE"),
            "access_key_id should be visible in Debug output"
        );
        assert!(
            debug_output.contains("[REDACTED]"),
            "redacted placeholder should appear"
        );
    }

    #[test]
    fn compute_refresh_interval_with_future_expiration() {
        // 2 hours from now → half = 1 hour → clamped to 30 min max.
        let future = Utc::now() + chrono::Duration::hours(2);
        let iso = future.to_rfc3339();
        let interval = compute_refresh_interval(&iso);
        assert_eq!(interval, MAX_REFRESH_INTERVAL);
    }

    #[test]
    fn compute_refresh_interval_with_short_expiration() {
        // 10 min from now → half = 5 min → returned as-is (>= 1 min, < 30 min).
        let future = Utc::now() + chrono::Duration::minutes(10);
        let iso = future.to_rfc3339();
        let interval = compute_refresh_interval(&iso);
        // Allow a small tolerance for elapsed time during the test.
        assert!(interval >= Duration::from_secs(270)); // ~4.5 min
        assert!(interval <= Duration::from_secs(300)); // 5 min
    }

    #[test]
    fn compute_refresh_interval_with_past_expiration() {
        let past = Utc::now() - chrono::Duration::hours(1);
        let iso = past.to_rfc3339();
        let interval = compute_refresh_interval(&iso);
        assert_eq!(interval, MIN_REFRESH_INTERVAL);
    }

    #[test]
    fn compute_refresh_interval_with_very_short_ttl() {
        // 30 seconds → half = 15s → clamped up to MIN_REFRESH_INTERVAL (60s).
        let future = Utc::now() + chrono::Duration::seconds(30);
        let iso = future.to_rfc3339();
        let interval = compute_refresh_interval(&iso);
        assert_eq!(interval, MIN_REFRESH_INTERVAL);
    }

    #[test]
    fn compute_refresh_interval_with_unparseable_expiration() {
        let interval = compute_refresh_interval("not-a-timestamp");
        assert_eq!(interval, DEFAULT_REFRESH_INTERVAL);
    }

    #[test]
    fn compute_refresh_interval_with_empty_string() {
        let interval = compute_refresh_interval("");
        assert_eq!(interval, DEFAULT_REFRESH_INTERVAL);
    }

    #[test]
    fn credential_error_display() {
        let err = CredentialError::Load("timeout".to_string());
        assert_eq!(err.to_string(), "failed to load AWS credentials: timeout");

        let err = CredentialError::NotAvailable;
        assert_eq!(err.to_string(), "no AWS credentials available");
    }

    fn build_empty_state() -> SharedState {
        use std::collections::HashMap;
        use std::sync::Arc;

        use tokio::sync::RwLock;

        use crate::metadata::{ServerState, TaskMetadata};

        Arc::new(RwLock::new(ServerState {
            task_metadata: TaskMetadata {
                cluster: "lecs-local".to_string(),
                task_arn: "arn:aws:ecs:local:000000000000:task/lecs/test".to_string(),
                family: "test".to_string(),
                revision: "0".to_string(),
                desired_status: "RUNNING".to_string(),
                known_status: "RUNNING".to_string(),
                containers: vec![],
                launch_type: "EC2".to_string(),
                availability_zone: "local".to_string(),
                task_role_arn: None,
            },
            container_metadata: HashMap::new(),
            credentials: None,
            container_ids: HashMap::new(),
            auth_token: "test-token".to_string(),
        }))
    }

    #[tokio::test]
    async fn credential_refresher_new_stores_fields() {
        let state = build_empty_state();
        let refresher =
            CredentialRefresher::new(state, Some("arn:aws:iam::111:role/test".to_string()));
        assert_eq!(
            refresher.role_arn.as_deref(),
            Some("arn:aws:iam::111:role/test")
        );
    }

    #[tokio::test]
    async fn credential_refresher_new_without_role_arn() {
        let state = build_empty_state();
        let refresher = CredentialRefresher::new(state, None);
        assert!(refresher.role_arn.is_none());
    }

    #[tokio::test]
    async fn credential_refresher_update_state_sets_credentials() {
        let state = build_empty_state();
        assert!(state.read().await.credentials.is_none());

        let creds = AwsCredentials {
            access_key_id: "AKIA-NEW".to_string(),
            secret_access_key: "secret".to_string(),
            token: None,
            expiration: "2099-01-01T00:00:00Z".to_string(),
            role_arn: None,
        };
        CredentialRefresher::update_state(&state, creds).await;

        let updated = state
            .read()
            .await
            .credentials
            .clone()
            .expect("should be Some");
        assert_eq!(updated.access_key_id, "AKIA-NEW");
        assert_eq!(updated.expiration, "2099-01-01T00:00:00Z");
    }

    #[tokio::test]
    async fn credential_refresher_update_state_replaces_existing() {
        let state = build_empty_state();
        let old = AwsCredentials {
            access_key_id: "AKIA-OLD".to_string(),
            secret_access_key: "secret".to_string(),
            token: None,
            expiration: "2026-01-01T00:00:00Z".to_string(),
            role_arn: None,
        };
        CredentialRefresher::update_state(&state, old).await;

        let new = AwsCredentials {
            access_key_id: "AKIA-NEW".to_string(),
            secret_access_key: "secret".to_string(),
            token: Some("token".to_string()),
            expiration: "2099-01-01T00:00:00Z".to_string(),
            role_arn: None,
        };
        CredentialRefresher::update_state(&state, new).await;

        let updated = state
            .read()
            .await
            .credentials
            .clone()
            .expect("should be Some");
        assert_eq!(updated.access_key_id, "AKIA-NEW");
        assert_eq!(updated.token.as_deref(), Some("token"));
    }
}
