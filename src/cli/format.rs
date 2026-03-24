//! Shared formatting and lookup utilities for CLI commands.

use crate::container::ContainerInfo;

/// Calculate column width: max(data widths, header width).
pub fn col_width(data_widths: impl Iterator<Item = usize>, header_width: usize) -> usize {
    data_widths.max().unwrap_or(0).max(header_width)
}

/// Format bytes as human-readable size.
#[allow(clippy::cast_precision_loss)]
pub fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;

    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Errors returned by [`find_container`].
#[derive(Debug)]
pub enum FindContainerError {
    NotFound,
    Ambiguous(String),
}

/// Find a container by name (exact match → unambiguous partial match).
///
/// Returns an error when multiple containers partially match the query.
pub fn find_container<'a>(
    containers: &'a [ContainerInfo],
    query: &str,
) -> Result<&'a ContainerInfo, FindContainerError> {
    // 1. Exact match
    if let Some(c) = containers.iter().find(|c| c.name == query) {
        return Ok(c);
    }

    // 2. Partial match — reject if ambiguous
    let matches: Vec<&ContainerInfo> = containers
        .iter()
        .filter(|c| c.name.contains(query))
        .collect();

    match matches.len() {
        0 => Err(FindContainerError::NotFound),
        1 => Ok(matches[0]),
        _ => {
            let names: Vec<&str> = matches.iter().map(|c| c.name.as_str()).collect();
            Err(FindContainerError::Ambiguous(names.join(", ")))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn col_width_uses_max_of_data_and_header() {
        assert_eq!(col_width([3, 5, 2].into_iter(), 4), 5);
    }

    #[test]
    fn col_width_uses_header_when_data_smaller() {
        assert_eq!(col_width([1, 2].into_iter(), 10), 10);
    }

    #[test]
    fn col_width_empty_data_uses_header() {
        assert_eq!(col_width(std::iter::empty(), 6), 6);
    }

    #[test]
    fn format_bytes_values() {
        assert_eq!(format_bytes(500), "500 B");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(1_572_864), "1.5 MiB");
        assert_eq!(format_bytes(1_610_612_736), "1.5 GiB");
    }

    fn container_info(name: &str) -> ContainerInfo {
        ContainerInfo {
            id: format!("{name}-id"),
            name: name.to_string(),
            image: "alpine:latest".to_string(),
            family: "test".to_string(),
            state: "running".to_string(),
            health_status: None,
            ports: vec![],
            started_at: None,
        }
    }

    #[test]
    fn find_container_exact_match() {
        let containers = vec![
            container_info("my-app-web"),
            container_info("my-app-sidecar"),
        ];
        let result = find_container(&containers, "my-app-web");
        assert_eq!(result.unwrap().name, "my-app-web");
    }

    #[test]
    fn find_container_partial_match() {
        let containers = vec![
            container_info("my-app-web"),
            container_info("my-app-sidecar"),
        ];
        let result = find_container(&containers, "web");
        assert_eq!(result.unwrap().name, "my-app-web");
    }

    #[test]
    fn find_container_not_found() {
        let containers = vec![container_info("my-app-web")];
        let result = find_container(&containers, "nonexistent");
        assert!(matches!(result, Err(FindContainerError::NotFound)));
    }

    #[test]
    fn find_container_exact_match_preferred() {
        let containers = vec![container_info("app"), container_info("my-app-app")];
        let result = find_container(&containers, "app");
        assert_eq!(result.unwrap().name, "app");
    }

    #[test]
    fn find_container_ambiguous_partial_match() {
        let containers = vec![
            container_info("my-app-web"),
            container_info("my-app-worker"),
        ];
        let result = find_container(&containers, "my-app");
        assert!(matches!(result, Err(FindContainerError::Ambiguous(_))));
    }
}
