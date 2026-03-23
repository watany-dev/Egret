//! Shared formatting utilities for CLI table output.

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
}
