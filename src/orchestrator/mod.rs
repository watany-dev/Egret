//! Container lifecycle orchestration and `dependsOn` DAG.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

use crate::container::ContainerRuntime;
use crate::taskdef::{DependencyCondition, DependsOn, HealthCheck};

/// Orchestrator errors.
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum OrchestratorError {
    #[error("cyclic dependency detected: {0}")]
    CyclicDependency(String),

    #[error("container runtime error: {0}")]
    Runtime(#[from] crate::container::ContainerError),

    #[error("condition not met for container '{0}': {1}")]
    ConditionNotMet(String, String),

    #[error("essential container '{0}' exited with code {1}")]
    EssentialContainerFailed(String, i64),

    #[error("health check timed out for container '{0}'")]
    HealthCheckTimeout(String),
}

/// Lightweight dependency information for DAG resolution.
#[allow(dead_code)]
pub struct DependencyInfo {
    pub name: String,
    pub depends_on: Vec<DependsOn>,
}

/// Resolve the startup order of containers using Kahn's algorithm.
///
/// Returns layers of container names that can be started concurrently within each layer.
/// Layer N must complete (according to its dependsOn conditions) before layer N+1 starts.
#[allow(dead_code)]
pub fn resolve_start_order(
    deps: &[DependencyInfo],
) -> Result<Vec<Vec<String>>, OrchestratorError> {
    if deps.is_empty() {
        return Ok(vec![]);
    }

    // Build adjacency and in-degree map
    let names: HashSet<&str> = deps.iter().map(|d| d.name.as_str()).collect();
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();

    for dep in deps {
        in_degree.entry(dep.name.as_str()).or_insert(0);
        for d in &dep.depends_on {
            if names.contains(d.container_name.as_str()) {
                *in_degree.entry(dep.name.as_str()).or_insert(0) += 1;
                dependents
                    .entry(d.container_name.as_str())
                    .or_default()
                    .push(dep.name.as_str());
            }
        }
    }

    // Kahn's algorithm with layers
    let mut queue: VecDeque<&str> = VecDeque::new();
    let mut initial: Vec<&str> = in_degree
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(name, _)| *name)
        .collect();
    initial.sort_unstable();
    queue.extend(initial);

    let mut layers: Vec<Vec<String>> = Vec::new();
    let mut processed = 0usize;

    while !queue.is_empty() {
        let layer: Vec<&str> = std::mem::take(&mut queue).into_iter().collect();
        processed += layer.len();

        // Update in-degrees and collect next layer
        let mut next: Vec<&str> = Vec::new();
        for &name in &layer {
            if let Some(children) = dependents.get(name) {
                for &child in children {
                    if let Some(deg) = in_degree.get_mut(child) {
                        *deg -= 1;
                        if *deg == 0 {
                            next.push(child);
                        }
                    }
                }
            }
        }
        next.sort_unstable();
        queue.extend(next);

        layers.push(layer.into_iter().map(String::from).collect());
    }

    if processed != deps.len() {
        let cycle_path = find_cycle(deps);
        return Err(OrchestratorError::CyclicDependency(cycle_path));
    }

    Ok(layers)
}

/// Find a cycle in the dependency graph using DFS and return a descriptive path string.
fn find_cycle(deps: &[DependencyInfo]) -> String {
    let adj: HashMap<&str, Vec<&str>> = deps
        .iter()
        .map(|d| {
            (
                d.name.as_str(),
                d.depends_on
                    .iter()
                    .map(|dep| dep.container_name.as_str())
                    .collect(),
            )
        })
        .collect();

    let mut visited: HashSet<&str> = HashSet::new();
    let mut in_stack: HashSet<&str> = HashSet::new();
    let mut path: Vec<&str> = Vec::new();

    for dep in deps {
        if !visited.contains(dep.name.as_str())
            && dfs_cycle(
                dep.name.as_str(),
                &adj,
                &mut visited,
                &mut in_stack,
                &mut path,
            )
        {
            return format_cycle_path(&path);
        }
    }

    "unknown cycle".to_string()
}

fn dfs_cycle<'a>(
    node: &'a str,
    adj: &HashMap<&str, Vec<&'a str>>,
    visited: &mut HashSet<&'a str>,
    in_stack: &mut HashSet<&'a str>,
    path: &mut Vec<&'a str>,
) -> bool {
    visited.insert(node);
    in_stack.insert(node);
    path.push(node);

    if let Some(neighbors) = adj.get(node) {
        for &neighbor in neighbors {
            if !visited.contains(neighbor) {
                if dfs_cycle(neighbor, adj, visited, in_stack, path) {
                    return true;
                }
            } else if in_stack.contains(neighbor) {
                path.push(neighbor);
                return true;
            }
        }
    }

    path.pop();
    in_stack.remove(node);
    false
}

fn format_cycle_path(path: &[&str]) -> String {
    if path.len() < 2 {
        return "unknown cycle".to_string();
    }

    let cycle_start = path[path.len() - 1];
    path[..path.len() - 1]
        .iter()
        .position(|&n| n == cycle_start)
        .map_or_else(
            || path.join(" -> "),
            |start_idx| path[start_idx..].join(" -> "),
        )
}

/// Wait for a dependency condition to be met.
#[allow(dead_code)]
pub async fn wait_for_condition(
    client: &(impl ContainerRuntime + ?Sized),
    id: &str,
    name: &str,
    condition: DependencyCondition,
    health_check: Option<&HealthCheck>,
) -> Result<(), OrchestratorError> {
    match condition {
        DependencyCondition::Start => {
            // Container was already started, condition is met immediately.
            Ok(())
        }
        DependencyCondition::Complete => {
            let result = client.wait_container(id).await?;
            tracing::info!(container = %name, exit_code = result.status_code, "Container completed");
            Ok(())
        }
        DependencyCondition::Success => {
            let result = client.wait_container(id).await?;
            if result.status_code == 0 {
                tracing::info!(container = %name, "Container completed successfully");
                Ok(())
            } else {
                Err(OrchestratorError::ConditionNotMet(
                    name.to_string(),
                    format!("expected exit code 0, got {}", result.status_code),
                ))
            }
        }
        DependencyCondition::Healthy => {
            let hc = health_check.ok_or_else(|| {
                OrchestratorError::ConditionNotMet(
                    name.to_string(),
                    "HEALTHY condition requires a healthCheck".to_string(),
                )
            })?;
            wait_for_healthy(client, id, name, hc).await
        }
    }
}

/// Poll `inspect_container` until health status becomes "healthy" or timeout.
#[allow(dead_code)]
async fn wait_for_healthy(
    client: &(impl ContainerRuntime + ?Sized),
    id: &str,
    name: &str,
    health_check: &HealthCheck,
) -> Result<(), OrchestratorError> {
    let timeout_secs = u64::from(health_check.start_period)
        + u64::from(health_check.interval) * (u64::from(health_check.retries) + 1)
        + 30;
    let timeout = Duration::from_secs(timeout_secs);
    let poll_interval = Duration::from_secs(u64::from(health_check.interval).max(1));

    let poll_future = async {
        loop {
            let inspection = client.inspect_container(id).await?;
            match inspection.state.health_status.as_deref() {
                Some("healthy") => {
                    tracing::info!(container = %name, "Container is healthy");
                    return Ok(());
                }
                Some("unhealthy") => {
                    return Err(OrchestratorError::HealthCheckTimeout(name.to_string()));
                }
                _ => {
                    tokio::time::sleep(poll_interval).await;
                }
            }
        }
    };

    tokio::time::timeout(timeout, poll_future)
        .await
        .map_err(|_| OrchestratorError::HealthCheckTimeout(name.to_string()))?
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::taskdef::DependencyCondition;

    fn dep(name: &str, depends: &[(&str, DependencyCondition)]) -> DependencyInfo {
        DependencyInfo {
            name: name.to_string(),
            depends_on: depends
                .iter()
                .map(|(n, c)| DependsOn {
                    container_name: n.to_string(),
                    condition: *c,
                })
                .collect(),
        }
    }

    #[test]
    fn resolve_no_dependencies() {
        let deps = vec![dep("a", &[]), dep("b", &[]), dep("c", &[])];
        let layers = resolve_start_order(&deps).expect("should resolve");
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0], vec!["a", "b", "c"]);
    }

    #[test]
    fn resolve_linear_chain() {
        let deps = vec![
            dep("a", &[]),
            dep("b", &[("a", DependencyCondition::Start)]),
            dep("c", &[("b", DependencyCondition::Healthy)]),
        ];
        let layers = resolve_start_order(&deps).expect("should resolve");
        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0], vec!["a"]);
        assert_eq!(layers[1], vec!["b"]);
        assert_eq!(layers[2], vec!["c"]);
    }

    #[test]
    fn resolve_diamond_dependency() {
        let deps = vec![
            dep("a", &[]),
            dep("b", &[("a", DependencyCondition::Start)]),
            dep("c", &[("a", DependencyCondition::Start)]),
            dep(
                "d",
                &[
                    ("b", DependencyCondition::Complete),
                    ("c", DependencyCondition::Complete),
                ],
            ),
        ];
        let layers = resolve_start_order(&deps).expect("should resolve");
        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0], vec!["a"]);
        assert_eq!(layers[1], vec!["b", "c"]);
        assert_eq!(layers[2], vec!["d"]);
    }

    #[test]
    fn resolve_single_container() {
        let deps = vec![dep("only", &[])];
        let layers = resolve_start_order(&deps).expect("should resolve");
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0], vec!["only"]);
    }

    #[test]
    fn resolve_circular_two_nodes() {
        let deps = vec![
            dep("a", &[("b", DependencyCondition::Start)]),
            dep("b", &[("a", DependencyCondition::Start)]),
        ];
        let err = resolve_start_order(&deps).unwrap_err();
        assert!(
            matches!(err, OrchestratorError::CyclicDependency(ref msg) if msg.contains('a') && msg.contains('b')),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn resolve_circular_three_nodes() {
        let deps = vec![
            dep("a", &[("c", DependencyCondition::Start)]),
            dep("b", &[("a", DependencyCondition::Start)]),
            dep("c", &[("b", DependencyCondition::Start)]),
        ];
        let err = resolve_start_order(&deps).unwrap_err();
        assert!(
            matches!(err, OrchestratorError::CyclicDependency(_)),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn resolve_partial_dependencies() {
        let deps = vec![
            dep("a", &[]),
            dep("b", &[("a", DependencyCondition::Success)]),
            dep("c", &[]),
        ];
        let layers = resolve_start_order(&deps).expect("should resolve");
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0], vec!["a", "c"]);
        assert_eq!(layers[1], vec!["b"]);
    }

    #[test]
    fn resolve_multiple_roots() {
        let deps = vec![
            dep("a", &[]),
            dep("b", &[]),
            dep(
                "c",
                &[
                    ("a", DependencyCondition::Start),
                    ("b", DependencyCondition::Start),
                ],
            ),
        ];
        let layers = resolve_start_order(&deps).expect("should resolve");
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0], vec!["a", "b"]);
        assert_eq!(layers[1], vec!["c"]);
    }

    #[test]
    fn resolve_empty_input() {
        let layers = resolve_start_order(&[]).expect("should resolve");
        assert!(layers.is_empty());
    }

    // --- Condition waiting tests ---

    use crate::container::test_support::MockContainerClient;
    use crate::container::{ContainerInspection, ContainerState, WaitResult};
    use crate::taskdef::HealthCheck;

    #[tokio::test]
    async fn wait_for_condition_start_returns_immediately() {
        let mock = MockContainerClient::new();
        let result =
            wait_for_condition(&mock, "id1", "app", DependencyCondition::Start, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn wait_for_condition_complete_waits_for_exit() {
        let mock = MockContainerClient::new();
        mock.wait_container_results
            .lock()
            .unwrap()
            .push_back(Ok(WaitResult { status_code: 1 }));

        let result =
            wait_for_condition(&mock, "id1", "app", DependencyCondition::Complete, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn wait_for_condition_success_exits_zero() {
        let mock = MockContainerClient::new();
        mock.wait_container_results
            .lock()
            .unwrap()
            .push_back(Ok(WaitResult { status_code: 0 }));

        let result =
            wait_for_condition(&mock, "id1", "app", DependencyCondition::Success, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn wait_for_condition_success_exits_nonzero_fails() {
        let mock = MockContainerClient::new();
        mock.wait_container_results
            .lock()
            .unwrap()
            .push_back(Ok(WaitResult { status_code: 1 }));

        let result =
            wait_for_condition(&mock, "id1", "app", DependencyCondition::Success, None).await;
        assert!(
            matches!(result, Err(OrchestratorError::ConditionNotMet(ref name, _)) if name == "app"),
            "unexpected: {result:?}"
        );
    }

    #[tokio::test]
    async fn wait_for_healthy_becomes_healthy() {
        let mock = MockContainerClient::new();
        // First poll: starting, second poll: healthy
        {
            let mut q = mock.inspect_container_results.lock().unwrap();
            q.push_back(Ok(ContainerInspection {
                id: "id1".into(),
                state: ContainerState {
                    status: "running".into(),
                    running: true,
                    exit_code: None,
                    health_status: Some("starting".into()),
                },
            }));
            q.push_back(Ok(ContainerInspection {
                id: "id1".into(),
                state: ContainerState {
                    status: "running".into(),
                    running: true,
                    exit_code: None,
                    health_status: Some("healthy".into()),
                },
            }));
        }

        let hc = HealthCheck {
            command: vec!["CMD-SHELL".into(), "true".into()],
            interval: 1,
            timeout: 1,
            retries: 3,
            start_period: 0,
        };
        let result = wait_for_condition(
            &mock,
            "id1",
            "db",
            DependencyCondition::Healthy,
            Some(&hc),
        )
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn wait_for_healthy_becomes_unhealthy_fails() {
        let mock = MockContainerClient::new();
        mock.inspect_container_results
            .lock()
            .unwrap()
            .push_back(Ok(ContainerInspection {
                id: "id1".into(),
                state: ContainerState {
                    status: "running".into(),
                    running: true,
                    exit_code: None,
                    health_status: Some("unhealthy".into()),
                },
            }));

        let hc = HealthCheck {
            command: vec!["CMD-SHELL".into(), "false".into()],
            interval: 1,
            timeout: 1,
            retries: 1,
            start_period: 0,
        };
        let result = wait_for_condition(
            &mock,
            "id1",
            "db",
            DependencyCondition::Healthy,
            Some(&hc),
        )
        .await;
        assert!(
            matches!(result, Err(OrchestratorError::HealthCheckTimeout(ref name)) if name == "db"),
            "unexpected: {result:?}"
        );
    }

    #[tokio::test]
    async fn wait_for_healthy_timeout_fails() {
        let mock = MockContainerClient::new();
        // Return "starting" indefinitely — each call pops from queue, then RuntimeNotRunning
        for _ in 0..100 {
            mock.inspect_container_results
                .lock()
                .unwrap()
                .push_back(Ok(ContainerInspection {
                    id: "id1".into(),
                    state: ContainerState {
                        status: "running".into(),
                        running: true,
                        exit_code: None,
                        health_status: Some("starting".into()),
                    },
                }));
        }

        let hc = HealthCheck {
            command: vec!["CMD-SHELL".into(), "true".into()],
            interval: 1,
            timeout: 1,
            retries: 1,
            start_period: 0,
        };

        // Use tokio::time::pause for deterministic time control
        tokio::time::pause();
        let result = wait_for_condition(
            &mock,
            "id1",
            "db",
            DependencyCondition::Healthy,
            Some(&hc),
        )
        .await;
        assert!(
            matches!(result, Err(OrchestratorError::HealthCheckTimeout(ref name)) if name == "db"),
            "unexpected: {result:?}"
        );
    }
}
