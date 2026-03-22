//! `egret diff` command implementation.
//!
//! Provides semantic comparison of two ECS task definition files,
//! showing differences at the container, environment variable, and port level.

use std::collections::BTreeMap;
use std::fmt::Write;

use anyhow::Result;

use super::DiffArgs;
use crate::taskdef::TaskDefinition;

/// Execute the `diff` subcommand.
#[cfg(not(tarpaulin_include))]
#[allow(clippy::print_stdout)]
pub fn execute(args: &DiffArgs) -> Result<()> {
    let td1 = TaskDefinition::from_file(&args.file1)?;
    let td2 = TaskDefinition::from_file(&args.file2)?;
    let output = diff_task_definitions(&td1, &td2);
    println!("{output}");
    Ok(())
}

/// Compare two task definitions parsed from JSON strings (testable, no file I/O).
#[cfg(test)]
fn diff_from_json(json1: &str, json2: &str) -> Result<String> {
    let td1 = TaskDefinition::from_json(json1)?;
    let td2 = TaskDefinition::from_json(json2)?;
    Ok(diff_task_definitions(&td1, &td2))
}

/// Core semantic diff logic.
fn diff_task_definitions(td1: &TaskDefinition, td2: &TaskDefinition) -> String {
    let mut output = String::new();

    // Compare family
    if td1.family != td2.family {
        let _ = writeln!(output, "family: {} → {}", td1.family, td2.family);
    }

    // Build container maps keyed by name
    let containers1: BTreeMap<&str, _> = td1
        .container_definitions
        .iter()
        .map(|c| (c.name.as_str(), c))
        .collect();
    let containers2: BTreeMap<&str, _> = td2
        .container_definitions
        .iter()
        .map(|c| (c.name.as_str(), c))
        .collect();

    // Removed containers
    for name in containers1.keys() {
        if !containers2.contains_key(name) {
            let _ = writeln!(output, "\n=== Container: {name} (removed) ===");
        }
    }

    // Added containers
    for (name, c2) in &containers2 {
        if !containers1.contains_key(name) {
            let _ = writeln!(output, "\n=== Container: {name} (added) ===");
            format_container_summary(c2, &mut output);
        }
    }

    // Modified containers
    for (name, c1) in &containers1 {
        if let Some(c2) = containers2.get(name) {
            let container_diff = diff_container(c1, c2);
            if !container_diff.is_empty() {
                let _ = writeln!(output, "\n=== Container: {name} ===");
                output.push_str(&container_diff);
            }
        }
    }

    let result = output.trim().to_string();
    if result.is_empty() {
        "No differences found.".to_string()
    } else {
        result
    }
}

/// Format a summary of a container (for added containers).
fn format_container_summary(c: &crate::taskdef::ContainerDefinition, output: &mut String) {
    let _ = writeln!(output, "  image: {}", c.image);
    if !c.essential {
        let _ = writeln!(output, "  essential: false");
    }
    if !c.command.is_empty() {
        let _ = writeln!(output, "  command: {}", c.command.join(" "));
    }
    if !c.entry_point.is_empty() {
        let _ = writeln!(output, "  entryPoint: {}", c.entry_point.join(" "));
    }
    for env in &c.environment {
        let _ = writeln!(output, "  environment: {}={}", env.name, env.value);
    }
    for pm in &c.port_mappings {
        let _ = writeln!(output, "  portMappings: {}", format_port_mapping(pm));
    }
    for dep in &c.depends_on {
        let _ = writeln!(
            output,
            "  dependsOn: {} ({:?})",
            dep.container_name, dep.condition
        );
    }
}

/// Compare two containers and return the diff string.
fn diff_container(
    c1: &crate::taskdef::ContainerDefinition,
    c2: &crate::taskdef::ContainerDefinition,
) -> String {
    let mut output = String::new();

    if c1.image != c2.image {
        let _ = writeln!(output, "  image: {} → {}", c1.image, c2.image);
    }

    if c1.essential != c2.essential {
        let _ = writeln!(output, "  essential: {} → {}", c1.essential, c2.essential);
    }

    if c1.command != c2.command {
        let _ = writeln!(
            output,
            "  command: [{}] → [{}]",
            c1.command.join(", "),
            c2.command.join(", ")
        );
    }

    if c1.entry_point != c2.entry_point {
        let _ = writeln!(
            output,
            "  entryPoint: [{}] → [{}]",
            c1.entry_point.join(", "),
            c2.entry_point.join(", ")
        );
    }

    diff_environment(&c1.environment, &c2.environment, &mut output);
    diff_port_mappings(&c1.port_mappings, &c2.port_mappings, &mut output);
    diff_depends_on(&c1.depends_on, &c2.depends_on, &mut output);
    diff_health_check(
        c1.health_check.as_ref(),
        c2.health_check.as_ref(),
        &mut output,
    );
    diff_mount_points(&c1.mount_points, &c2.mount_points, &mut output);

    if c1.cpu != c2.cpu {
        let old = format_optional_u32(c1.cpu);
        let new = format_optional_u32(c2.cpu);
        let _ = writeln!(output, "  cpu: {old} → {new}");
    }

    if c1.memory != c2.memory {
        let old = format_optional_u32(c1.memory);
        let new = format_optional_u32(c2.memory);
        let _ = writeln!(output, "  memory: {old} → {new}");
    }

    if c1.memory_reservation != c2.memory_reservation {
        let old = format_optional_u32(c1.memory_reservation);
        let new = format_optional_u32(c2.memory_reservation);
        let _ = writeln!(output, "  memoryReservation: {old} → {new}");
    }

    output
}

fn format_optional_u32(val: Option<u32>) -> String {
    val.map_or_else(|| "(none)".to_string(), |v| v.to_string())
}

fn diff_environment(
    env1: &[crate::taskdef::Environment],
    env2: &[crate::taskdef::Environment],
    output: &mut String,
) {
    let left: BTreeMap<&str, &str> = env1
        .iter()
        .map(|e| (e.name.as_str(), e.value.as_str()))
        .collect();
    let right: BTreeMap<&str, &str> = env2
        .iter()
        .map(|e| (e.name.as_str(), e.value.as_str()))
        .collect();

    for (name, val) in &left {
        if !right.contains_key(name) {
            let _ = writeln!(output, "  - environment: {name}={val}");
        }
    }

    for (name, val) in &right {
        if !left.contains_key(name) {
            let _ = writeln!(output, "  + environment: {name}={val}");
        }
    }

    for (name, val1) in &left {
        if let Some(val2) = right.get(name)
            && val1 != val2
        {
            let _ = writeln!(output, "  ~ environment: {name}: {val1} → {val2}");
        }
    }
}

fn format_port_mapping(pm: &crate::taskdef::PortMapping) -> String {
    let host = pm
        .host_port
        .map_or_else(|| pm.container_port.to_string(), |h| h.to_string());
    let cp = pm.container_port;
    let proto = &pm.protocol;
    format!("{host}:{cp}/{proto}")
}

fn diff_port_mappings(
    ports1: &[crate::taskdef::PortMapping],
    ports2: &[crate::taskdef::PortMapping],
    output: &mut String,
) {
    let left: BTreeMap<u16, &crate::taskdef::PortMapping> =
        ports1.iter().map(|p| (p.container_port, p)).collect();
    let right: BTreeMap<u16, &crate::taskdef::PortMapping> =
        ports2.iter().map(|p| (p.container_port, p)).collect();

    for cp in left.keys() {
        if !right.contains_key(cp) {
            let formatted = format_port_mapping(left[cp]);
            let _ = writeln!(output, "  - portMappings: {formatted}");
        }
    }

    for cp in right.keys() {
        if !left.contains_key(cp) {
            let formatted = format_port_mapping(right[cp]);
            let _ = writeln!(output, "  + portMappings: {formatted}");
        }
    }

    for (cp, p1) in &left {
        if let Some(p2) = right.get(cp) {
            let old = format_port_mapping(p1);
            let new = format_port_mapping(p2);
            if old != new {
                let _ = writeln!(output, "  ~ portMappings: {old} → {new}");
            }
        }
    }
}

fn diff_depends_on(
    dep1: &[crate::taskdef::DependsOn],
    dep2: &[crate::taskdef::DependsOn],
    output: &mut String,
) {
    let left: BTreeMap<&str, crate::taskdef::DependencyCondition> = dep1
        .iter()
        .map(|d| (d.container_name.as_str(), d.condition))
        .collect();
    let right: BTreeMap<&str, crate::taskdef::DependencyCondition> = dep2
        .iter()
        .map(|d| (d.container_name.as_str(), d.condition))
        .collect();

    for (name, cond) in &left {
        if !right.contains_key(name) {
            let _ = writeln!(output, "  - dependsOn: {name} ({cond:?})");
        }
    }

    for (name, cond) in &right {
        if !left.contains_key(name) {
            let _ = writeln!(output, "  + dependsOn: {name} ({cond:?})");
        }
    }

    for (name, cond1) in &left {
        if let Some(cond2) = right.get(name)
            && cond1 != cond2
        {
            let _ = writeln!(output, "  ~ dependsOn: {name}: {cond1:?} → {cond2:?}");
        }
    }
}

fn diff_health_check(
    hc1: Option<&crate::taskdef::HealthCheck>,
    hc2: Option<&crate::taskdef::HealthCheck>,
    output: &mut String,
) {
    match (hc1, hc2) {
        (None, None) => {}
        (Some(_), None) => {
            let _ = writeln!(output, "  - healthCheck: (removed)");
        }
        (None, Some(hc)) => {
            let cmd = hc.command.join(", ");
            let _ = writeln!(output, "  + healthCheck: command=[{cmd}]");
        }
        (Some(h1), Some(h2)) => {
            if h1.command != h2.command {
                let old = h1.command.join(", ");
                let new = h2.command.join(", ");
                let _ = writeln!(output, "  ~ healthCheck.command: [{old}] → [{new}]");
            }
            if h1.interval != h2.interval {
                let _ = writeln!(
                    output,
                    "  ~ healthCheck.interval: {}s → {}s",
                    h1.interval, h2.interval
                );
            }
            if h1.timeout != h2.timeout {
                let _ = writeln!(
                    output,
                    "  ~ healthCheck.timeout: {}s → {}s",
                    h1.timeout, h2.timeout
                );
            }
            if h1.retries != h2.retries {
                let _ = writeln!(
                    output,
                    "  ~ healthCheck.retries: {} → {}",
                    h1.retries, h2.retries
                );
            }
            if h1.start_period != h2.start_period {
                let _ = writeln!(
                    output,
                    "  ~ healthCheck.startPeriod: {}s → {}s",
                    h1.start_period, h2.start_period
                );
            }
        }
    }
}

fn diff_mount_points(
    mounts1: &[crate::taskdef::MountPoint],
    mounts2: &[crate::taskdef::MountPoint],
    output: &mut String,
) {
    let left: BTreeMap<&str, &crate::taskdef::MountPoint> = mounts1
        .iter()
        .map(|m| (m.source_volume.as_str(), m))
        .collect();
    let right: BTreeMap<&str, &crate::taskdef::MountPoint> = mounts2
        .iter()
        .map(|m| (m.source_volume.as_str(), m))
        .collect();

    for (name, mount) in &left {
        if !right.contains_key(name) {
            let _ = writeln!(
                output,
                "  - mountPoints: {} → {}",
                mount.source_volume, mount.container_path
            );
        }
    }

    for (name, mount) in &right {
        if !left.contains_key(name) {
            let _ = writeln!(
                output,
                "  + mountPoints: {} → {}",
                mount.source_volume, mount.container_path
            );
        }
    }

    for (name, mount1) in &left {
        if let Some(mount2) = right.get(name) {
            if mount1.container_path != mount2.container_path {
                let _ = writeln!(
                    output,
                    "  ~ mountPoints: {name}: {} → {}",
                    mount1.container_path, mount2.container_path
                );
            }
            if mount1.read_only != mount2.read_only {
                let _ = writeln!(
                    output,
                    "  ~ mountPoints: {name}: readOnly {} → {}",
                    mount1.read_only, mount2.read_only
                );
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn minimal_json(family: &str) -> String {
        format!(
            r#"{{
                "family": "{family}",
                "containerDefinitions": [
                    {{
                        "name": "app",
                        "image": "nginx:latest",
                        "portMappings": [{{ "containerPort": 80, "hostPort": 8080 }}]
                    }}
                ]
            }}"#
        )
    }

    #[test]
    fn identical_definitions_no_diff() {
        let json = minimal_json("test");
        let result = diff_from_json(&json, &json).expect("should succeed");
        assert_eq!(result, "No differences found.");
    }

    #[test]
    fn family_change() {
        let json1 = minimal_json("app-v1");
        let json2 = minimal_json("app-v2");
        let result = diff_from_json(&json1, &json2).expect("should succeed");
        assert!(result.contains("family: app-v1 → app-v2"));
    }

    #[test]
    fn container_added() {
        let json1 = minimal_json("test");
        let json2 = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "app", "image": "nginx:latest", "portMappings": [{ "containerPort": 80, "hostPort": 8080 }] },
                { "name": "redis", "image": "redis:7", "portMappings": [{ "containerPort": 6379 }] }
            ]
        }"#;
        let result = diff_from_json(&json1, json2).expect("should succeed");
        assert!(result.contains("Container: redis (added)"));
        assert!(result.contains("image: redis:7"));
    }

    #[test]
    fn container_removed() {
        let json1 = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "app", "image": "nginx:latest", "portMappings": [{ "containerPort": 80, "hostPort": 8080 }] },
                { "name": "sidecar", "image": "busybox:latest" }
            ]
        }"#;
        let json2 = minimal_json("test");
        let result = diff_from_json(json1, &json2).expect("should succeed");
        assert!(result.contains("Container: sidecar (removed)"));
    }

    #[test]
    fn image_change() {
        let json1 = minimal_json("test");
        let json2 = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "app", "image": "nginx:1.25", "portMappings": [{ "containerPort": 80, "hostPort": 8080 }] }
            ]
        }"#;
        let result = diff_from_json(&json1, json2).expect("should succeed");
        assert!(result.contains("image: nginx:latest → nginx:1.25"));
    }

    #[test]
    fn environment_added_removed_changed() {
        let json1 = r#"{
            "family": "test",
            "containerDefinitions": [{
                "name": "app",
                "image": "nginx:latest",
                "environment": [
                    { "name": "OLD_VAR", "value": "old" },
                    { "name": "SHARED", "value": "v1" }
                ],
                "portMappings": [{ "containerPort": 80 }]
            }]
        }"#;
        let json2 = r#"{
            "family": "test",
            "containerDefinitions": [{
                "name": "app",
                "image": "nginx:latest",
                "environment": [
                    { "name": "NEW_VAR", "value": "new" },
                    { "name": "SHARED", "value": "v2" }
                ],
                "portMappings": [{ "containerPort": 80 }]
            }]
        }"#;
        let result = diff_from_json(json1, json2).expect("should succeed");
        assert!(result.contains("- environment: OLD_VAR=old"));
        assert!(result.contains("+ environment: NEW_VAR=new"));
        assert!(result.contains("~ environment: SHARED: v1 → v2"));
    }

    #[test]
    fn port_mapping_change() {
        let json1 = minimal_json("test");
        let json2 = r#"{
            "family": "test",
            "containerDefinitions": [{
                "name": "app",
                "image": "nginx:latest",
                "portMappings": [{ "containerPort": 80, "hostPort": 9090 }]
            }]
        }"#;
        let result = diff_from_json(&json1, json2).expect("should succeed");
        assert!(result.contains("~ portMappings:"));
    }

    #[test]
    fn depends_on_change() {
        let json1 = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "db", "image": "postgres:15", "essential": true },
                {
                    "name": "app",
                    "image": "nginx:latest",
                    "dependsOn": [{ "containerName": "db", "condition": "START" }],
                    "portMappings": [{ "containerPort": 80 }]
                }
            ]
        }"#;
        let json2 = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "db", "image": "postgres:15", "essential": true,
                  "healthCheck": { "command": ["CMD-SHELL", "pg_isready"] } },
                {
                    "name": "app",
                    "image": "nginx:latest",
                    "dependsOn": [{ "containerName": "db", "condition": "HEALTHY" }],
                    "portMappings": [{ "containerPort": 80 }]
                }
            ]
        }"#;
        let result = diff_from_json(json1, json2).expect("should succeed");
        assert!(result.contains("~ dependsOn: db: Start → Healthy"));
    }

    #[test]
    fn health_check_change() {
        let json1 = r#"{
            "family": "test",
            "containerDefinitions": [{
                "name": "app",
                "image": "nginx:latest",
                "healthCheck": {
                    "command": ["CMD-SHELL", "curl -f http://localhost/"],
                    "interval": 30,
                    "timeout": 5,
                    "retries": 3,
                    "startPeriod": 0
                },
                "portMappings": [{ "containerPort": 80 }]
            }]
        }"#;
        let json2 = r#"{
            "family": "test",
            "containerDefinitions": [{
                "name": "app",
                "image": "nginx:latest",
                "healthCheck": {
                    "command": ["CMD-SHELL", "wget -q --spider http://localhost/"],
                    "interval": 10,
                    "timeout": 5,
                    "retries": 5,
                    "startPeriod": 15
                },
                "portMappings": [{ "containerPort": 80 }]
            }]
        }"#;
        let result = diff_from_json(json1, json2).expect("should succeed");
        assert!(result.contains("~ healthCheck.command:"));
        assert!(result.contains("~ healthCheck.interval: 30s → 10s"));
        assert!(result.contains("~ healthCheck.retries: 3 → 5"));
        assert!(result.contains("~ healthCheck.startPeriod: 0s → 15s"));
    }

    #[test]
    fn essential_change() {
        let json1 = minimal_json("test");
        let json2 = r#"{
            "family": "test",
            "containerDefinitions": [{
                "name": "app",
                "image": "nginx:latest",
                "essential": false,
                "portMappings": [{ "containerPort": 80, "hostPort": 8080 }]
            }]
        }"#;
        let result = diff_from_json(&json1, json2).expect("should succeed");
        assert!(result.contains("essential: true → false"));
    }

    #[test]
    fn cpu_memory_change() {
        let json1 = r#"{
            "family": "test",
            "containerDefinitions": [{
                "name": "app",
                "image": "nginx:latest",
                "cpu": 256,
                "memory": 512,
                "portMappings": [{ "containerPort": 80 }]
            }]
        }"#;
        let json2 = r#"{
            "family": "test",
            "containerDefinitions": [{
                "name": "app",
                "image": "nginx:latest",
                "cpu": 512,
                "memory": 1024,
                "portMappings": [{ "containerPort": 80 }]
            }]
        }"#;
        let result = diff_from_json(json1, json2).expect("should succeed");
        assert!(result.contains("cpu: 256 → 512"));
        assert!(result.contains("memory: 512 → 1024"));
    }

    #[test]
    fn health_check_added() {
        let json1 = minimal_json("test");
        let json2 = r#"{
            "family": "test",
            "containerDefinitions": [{
                "name": "app",
                "image": "nginx:latest",
                "healthCheck": { "command": ["CMD-SHELL", "curl -f http://localhost/"] },
                "portMappings": [{ "containerPort": 80, "hostPort": 8080 }]
            }]
        }"#;
        let result = diff_from_json(&json1, json2).expect("should succeed");
        assert!(result.contains("+ healthCheck:"));
    }

    #[test]
    fn health_check_removed() {
        let json1 = r#"{
            "family": "test",
            "containerDefinitions": [{
                "name": "app",
                "image": "nginx:latest",
                "healthCheck": { "command": ["CMD-SHELL", "curl -f http://localhost/"] },
                "portMappings": [{ "containerPort": 80, "hostPort": 8080 }]
            }]
        }"#;
        let json2 = minimal_json("test");
        let result = diff_from_json(json1, &json2).expect("should succeed");
        assert!(result.contains("- healthCheck: (removed)"));
    }

    #[test]
    fn container_added_with_details() {
        let json1 = minimal_json("test");
        let json2 = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "app", "image": "nginx:latest", "portMappings": [{ "containerPort": 80, "hostPort": 8080 }] },
                {
                    "name": "worker",
                    "image": "worker:latest",
                    "essential": false,
                    "command": ["python", "worker.py"],
                    "entryPoint": ["/bin/sh", "-c"],
                    "environment": [{ "name": "MODE", "value": "prod" }],
                    "portMappings": [{ "containerPort": 3000 }],
                    "dependsOn": [{ "containerName": "app", "condition": "START" }]
                }
            ]
        }"#;
        let result = diff_from_json(&json1, json2).expect("should succeed");
        assert!(result.contains("Container: worker (added)"));
        assert!(result.contains("essential: false"));
        assert!(result.contains("command: python worker.py"));
        assert!(result.contains("entryPoint: /bin/sh -c"));
        assert!(result.contains("environment: MODE=prod"));
        assert!(result.contains("portMappings:"));
        assert!(result.contains("dependsOn: app"));
    }

    #[test]
    fn command_and_entrypoint_change() {
        let json1 = r#"{
            "family": "test",
            "containerDefinitions": [{
                "name": "app",
                "image": "nginx:latest",
                "command": ["old-cmd"],
                "entryPoint": ["/bin/sh"],
                "portMappings": [{ "containerPort": 80 }]
            }]
        }"#;
        let json2 = r#"{
            "family": "test",
            "containerDefinitions": [{
                "name": "app",
                "image": "nginx:latest",
                "command": ["new-cmd", "--flag"],
                "entryPoint": ["/bin/bash"],
                "portMappings": [{ "containerPort": 80 }]
            }]
        }"#;
        let result = diff_from_json(json1, json2).expect("should succeed");
        assert!(result.contains("command:"));
        assert!(result.contains("entryPoint:"));
    }

    #[test]
    fn port_mapping_added_and_removed() {
        let json1 = r#"{
            "family": "test",
            "containerDefinitions": [{
                "name": "app",
                "image": "nginx:latest",
                "portMappings": [{ "containerPort": 80 }]
            }]
        }"#;
        let json2 = r#"{
            "family": "test",
            "containerDefinitions": [{
                "name": "app",
                "image": "nginx:latest",
                "portMappings": [{ "containerPort": 443 }]
            }]
        }"#;
        let result = diff_from_json(json1, json2).expect("should succeed");
        assert!(result.contains("- portMappings:"));
        assert!(result.contains("+ portMappings:"));
    }

    #[test]
    fn depends_on_added_and_removed() {
        let json1 = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "db", "image": "postgres:15", "essential": true },
                { "name": "cache", "image": "redis:7", "essential": true },
                {
                    "name": "app",
                    "image": "nginx:latest",
                    "dependsOn": [{ "containerName": "db", "condition": "START" }],
                    "portMappings": [{ "containerPort": 80 }]
                }
            ]
        }"#;
        let json2 = r#"{
            "family": "test",
            "containerDefinitions": [
                { "name": "db", "image": "postgres:15", "essential": true },
                { "name": "cache", "image": "redis:7", "essential": true },
                {
                    "name": "app",
                    "image": "nginx:latest",
                    "dependsOn": [{ "containerName": "cache", "condition": "START" }],
                    "portMappings": [{ "containerPort": 80 }]
                }
            ]
        }"#;
        let result = diff_from_json(json1, json2).expect("should succeed");
        assert!(result.contains("- dependsOn: db"));
        assert!(result.contains("+ dependsOn: cache"));
    }

    #[test]
    fn mount_points_added_removed_changed() {
        let json1 = r#"{
            "family": "test",
            "containerDefinitions": [{
                "name": "app",
                "image": "nginx:latest",
                "mountPoints": [
                    { "sourceVolume": "logs", "containerPath": "/var/log", "readOnly": false },
                    { "sourceVolume": "config", "containerPath": "/etc/old", "readOnly": false }
                ],
                "portMappings": [{ "containerPort": 80 }]
            }],
            "volumes": [
                { "name": "logs", "host": { "sourcePath": "/tmp/logs" } },
                { "name": "config", "host": { "sourcePath": "/tmp/config" } },
                { "name": "data", "host": { "sourcePath": "/tmp/data" } }
            ]
        }"#;
        let json2 = r#"{
            "family": "test",
            "containerDefinitions": [{
                "name": "app",
                "image": "nginx:latest",
                "mountPoints": [
                    { "sourceVolume": "config", "containerPath": "/etc/new", "readOnly": true },
                    { "sourceVolume": "data", "containerPath": "/data", "readOnly": false }
                ],
                "portMappings": [{ "containerPort": 80 }]
            }],
            "volumes": [
                { "name": "logs", "host": { "sourcePath": "/tmp/logs" } },
                { "name": "config", "host": { "sourcePath": "/tmp/config" } },
                { "name": "data", "host": { "sourcePath": "/tmp/data" } }
            ]
        }"#;
        let result = diff_from_json(json1, json2).expect("should succeed");
        assert!(result.contains("- mountPoints: logs"));
        assert!(result.contains("+ mountPoints: data"));
        assert!(result.contains("~ mountPoints: config: /etc/old → /etc/new"));
        assert!(result.contains("~ mountPoints: config: readOnly false → true"));
    }

    #[test]
    fn memory_reservation_change() {
        let json1 = r#"{
            "family": "test",
            "containerDefinitions": [{
                "name": "app",
                "image": "nginx:latest",
                "memoryReservation": 256,
                "portMappings": [{ "containerPort": 80 }]
            }]
        }"#;
        let json2 = r#"{
            "family": "test",
            "containerDefinitions": [{
                "name": "app",
                "image": "nginx:latest",
                "memoryReservation": 512,
                "portMappings": [{ "containerPort": 80 }]
            }]
        }"#;
        let result = diff_from_json(json1, json2).expect("should succeed");
        assert!(result.contains("memoryReservation: 256 → 512"));
    }

    #[test]
    fn health_check_timeout_change() {
        let json1 = r#"{
            "family": "test",
            "containerDefinitions": [{
                "name": "app",
                "image": "nginx:latest",
                "healthCheck": {
                    "command": ["CMD-SHELL", "curl -f http://localhost/"],
                    "interval": 30,
                    "timeout": 5,
                    "retries": 3,
                    "startPeriod": 0
                },
                "portMappings": [{ "containerPort": 80 }]
            }]
        }"#;
        let json2 = r#"{
            "family": "test",
            "containerDefinitions": [{
                "name": "app",
                "image": "nginx:latest",
                "healthCheck": {
                    "command": ["CMD-SHELL", "curl -f http://localhost/"],
                    "interval": 30,
                    "timeout": 10,
                    "retries": 3,
                    "startPeriod": 0
                },
                "portMappings": [{ "containerPort": 80 }]
            }]
        }"#;
        let result = diff_from_json(json1, json2).expect("should succeed");
        assert!(result.contains("~ healthCheck.timeout: 5s → 10s"));
    }
}
