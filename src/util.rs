//! Shared utility functions.

/// Format seconds as `MM:SS` or `HH:MM:SS`.
pub fn format_seconds(seconds: f64) -> String {
    let total_seconds = seconds as u32;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let secs = total_seconds % 60;

    if hours > 0 {
        format!("{:02}:{:02}:{:02}", hours, minutes, secs)
    } else {
        format!("{:02}:{:02}", minutes, secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_seconds() {
        assert_eq!(format_seconds(0.0), "00:00");
        assert_eq!(format_seconds(65.0), "01:05");
        assert_eq!(format_seconds(3665.0), "01:01:05");
        assert_eq!(format_seconds(125.5), "02:05");
    }
}
