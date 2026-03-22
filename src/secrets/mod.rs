//! Secrets local resolver.
//!
//! Resolves ECS Secrets Manager ARN references to local plaintext values
//! using a mapping file (`secrets.local.json`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::taskdef::Secret;

/// Secrets resolution errors.
#[derive(Debug, thiserror::Error)]
pub enum SecretsError {
    #[error("failed to read secrets file from {path}: {source}")]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("failed to parse secrets JSON: {0}")]
    ParseJson(#[from] serde_json::Error),

    #[error("secret ARN not found in local mapping: {arn}")]
    ArnNotFound { arn: String },
}

/// Resolves Secrets Manager ARN references to local values.
#[derive(Debug)]
pub struct SecretsResolver {
    mapping: HashMap<String, String>,
}

impl SecretsResolver {
    /// Load a secrets mapping from a file path.
    pub fn from_file(path: &Path) -> Result<Self, SecretsError> {
        let content = std::fs::read_to_string(path).map_err(|source| SecretsError::ReadFile {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_json(&content)
    }

    /// Parse a secrets mapping from a JSON string.
    pub fn from_json(json: &str) -> Result<Self, SecretsError> {
        let mapping: HashMap<String, String> = serde_json::from_str(json)?;
        Ok(Self { mapping })
    }

    /// Resolve a list of secrets to `(name, value)` pairs.
    ///
    /// Returns an error if any ARN is not found in the mapping.
    pub fn resolve(&self, secrets: &[Secret]) -> Result<Vec<(String, String)>, SecretsError> {
        secrets
            .iter()
            .map(|secret| {
                let value = self.mapping.get(&secret.value_from).ok_or_else(|| {
                    SecretsError::ArnNotFound {
                        arn: secret.value_from.clone(),
                    }
                })?;
                Ok((secret.name.clone(), value.clone()))
            })
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::similar_names)]
mod tests {
    use super::*;

    #[test]
    fn parse_secrets_mapping() {
        let json = r#"{
            "arn:aws:secretsmanager:us-east-1:123:secret:db-pass": "local-db-password",
            "arn:aws:secretsmanager:us-east-1:123:secret:api-key": "local-api-key"
        }"#;
        let resolver = SecretsResolver::from_json(json).expect("should parse");
        assert_eq!(resolver.mapping.len(), 2);
    }

    #[test]
    fn resolve_all_found() {
        let json = r#"{
            "arn:aws:secretsmanager:us-east-1:123:secret:db-pass": "local-db-password",
            "arn:aws:secretsmanager:us-east-1:123:secret:api-key": "local-api-key"
        }"#;
        let resolver = SecretsResolver::from_json(json).expect("should parse");

        let secrets = vec![
            Secret {
                name: "DB_PASSWORD".to_string(),
                value_from: "arn:aws:secretsmanager:us-east-1:123:secret:db-pass".to_string(),
            },
            Secret {
                name: "API_KEY".to_string(),
                value_from: "arn:aws:secretsmanager:us-east-1:123:secret:api-key".to_string(),
            },
        ];

        let resolved = resolver.resolve(&secrets).expect("should resolve");
        assert_eq!(resolved.len(), 2);
        assert_eq!(
            resolved[0],
            ("DB_PASSWORD".to_string(), "local-db-password".to_string())
        );
        assert_eq!(
            resolved[1],
            ("API_KEY".to_string(), "local-api-key".to_string())
        );
    }

    #[test]
    fn resolve_missing_arn() {
        let json = r#"{
            "arn:aws:secretsmanager:us-east-1:123:secret:db-pass": "local-db-password"
        }"#;
        let resolver = SecretsResolver::from_json(json).expect("should parse");

        let secrets = vec![Secret {
            name: "MISSING".to_string(),
            value_from: "arn:aws:secretsmanager:us-east-1:123:secret:unknown".to_string(),
        }];

        let err = resolver.resolve(&secrets).unwrap_err();
        assert!(
            matches!(err, SecretsError::ArnNotFound { ref arn } if arn.contains("unknown")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn resolve_empty_secrets() {
        let resolver = SecretsResolver::from_json("{}").expect("should parse");
        let resolved = resolver.resolve(&[]).expect("should resolve");
        assert!(resolved.is_empty());
    }

    #[test]
    fn error_invalid_json() {
        let err = SecretsResolver::from_json("not json").unwrap_err();
        assert!(matches!(err, SecretsError::ParseJson(_)));
    }

    #[test]
    fn error_file_not_found() {
        let err = SecretsResolver::from_file(Path::new("/nonexistent/secrets.json")).unwrap_err();
        assert!(
            matches!(err, SecretsError::ReadFile { .. }),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn error_display_messages() {
        let err = SecretsResolver::from_json("bad").unwrap_err();
        assert!(err.to_string().contains("failed to parse secrets JSON"));

        let err = SecretsResolver::from_file(Path::new("/no/such")).unwrap_err();
        assert!(err.to_string().contains("failed to read secrets file"));

        let err = SecretsError::ArnNotFound {
            arn: "arn:test".to_string(),
        };
        assert!(err.to_string().contains("arn:test"));
    }
}
