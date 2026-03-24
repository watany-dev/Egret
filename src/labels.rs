//! Lecs container label keys.
//!
//! Centralises the Docker/Podman label strings that Lecs uses to tag and
//! discover managed containers.  Every module that reads or writes these
//! labels should reference these constants instead of hard-coding strings.

/// Marks a container as managed by Lecs (`"true"`).
pub const MANAGED: &str = "lecs.managed";

/// The task-definition family name.
pub const TASK: &str = "lecs.task";

/// The individual container name inside the task.
pub const CONTAINER: &str = "lecs.container";

/// Comma-separated secret environment variable names (for inspect masking).
pub const SECRETS: &str = "lecs.secrets";

/// `stopTimeout` value stored for cleanup.
pub const STOP_TIMEOUT: &str = "lecs.stop_timeout";

/// Comma-separated `dependsOn` entries (`name:CONDITION`).
pub const DEPENDS_ON: &str = "lecs.depends_on";
