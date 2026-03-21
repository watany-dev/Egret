//! Container lifecycle orchestration and `dependsOn` DAG.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::taskdef::DependsOn;

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
}
