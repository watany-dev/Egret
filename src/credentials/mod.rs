//! Credential provider mock for local development.
//!
//! Loads AWS credentials from the local environment (via `aws-config`)
//! and serves them in ECS credential provider compatible format.

use chrono::Utc;
use serde::Serialize;

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
#[derive(Debug, Clone, Serialize)]
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
        assert_eq!(
            json["RoleArn"],
            "arn:aws:iam::123456789012:role/my-role"
        );
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
    fn credential_error_display() {
        let err = CredentialError::Load("timeout".to_string());
        assert_eq!(err.to_string(), "failed to load AWS credentials: timeout");

        let err = CredentialError::NotAvailable;
        assert_eq!(err.to_string(), "no AWS credentials available");
    }
}
