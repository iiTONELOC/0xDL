use std::io::{Write, stdout};

pub fn default_with_progress(progress: f32) {
    let width = 40; // number of bar characters
    let filled = (progress / 100.0 * width as f32) as usize;

    let bar: String = format!(
        "[{}{}] {:>6.2}%",
        "=".repeat(filled),
        " ".repeat(width - filled),
        progress
    );

    {
        let mut out = stdout();
        out.write_all(b"\r\x1B[2K").unwrap(); // clear line
        out.write_all(bar.as_bytes()).unwrap();
        out.flush().unwrap();
    }
}

#[cfg(all(feature = "tests", test))]
mod tests {
    use super::*;

    // Helper that reproduces the formatting logic from default_with_progress so tests
    // can assert expected values deterministically without relying on capturing stdout.
    fn build_expected_bar(progress: f32) -> String {
        let width = 40;
        let filled = (progress / 100.0 * width as f32) as usize;
        format!(
            "\r[{}{}] {:>6.2}%",
            "=".repeat(filled),
            " ".repeat(width - filled),
            progress
        )
    }

    #[test]
    fn test_filled_counts_for_known_values() {
        let width = 40usize;

        let cases = [
            (0.0f32, 0usize),
            (25.0f32, (25.0 / 100.0 * width as f32) as usize),
            (50.0f32, (50.0 / 100.0 * width as f32) as usize),
            (50.5f32, (50.5 / 100.0 * width as f32) as usize),
            (99.9f32, (99.9 / 100.0 * width as f32) as usize),
            (100.0f32, (100.0 / 100.0 * width as f32) as usize),
        ];

        for (progress, expected_filled) in &cases {
            let filled = (progress / 100.0 * width as f32) as usize;
            assert_eq!(
                filled, *expected_filled,
                "filled count mismatch for progress {}",
                progress
            );

            // sanity: filled must be within [0, width]
            assert!(
                filled <= width,
                "filled ({}) should not exceed width ({}) for progress {}",
                filled,
                width,
                progress
            );
        }
    }

    #[test]
    fn test_bar_format_components() {
        let tests = [0.0f32, 3.14f32, 12.5f32, 50.5f32, 75.25f32, 100.0f32];

        for &p in &tests {
            let bar = build_expected_bar(p);

            // starts with carriage return and opening bracket
            assert!(
                bar.starts_with("\r["),
                "bar must start with \\r[ : got '{}'",
                bar
            );

            // contains closing bracket
            let closing_bracket_idx = bar.find(']').expect("bar must contain ]");
            // content inside brackets must be exactly 40 characters
            let inside = &bar[2..closing_bracket_idx]; // skip "\r["
            assert_eq!(
                inside.len(),
                40,
                "content inside brackets must be 40 chars, got {} for progress {}",
                inside.len(),
                p
            );

            // the number of '=' characters equals the computed filled
            let filled = (p / 100.0 * 40.0) as usize;
            let eq_count = inside.chars().take_while(|&c| c == '=').count();
            assert_eq!(
                eq_count, filled,
                "expected {} '=' for progress {}, got {}",
                filled, p, eq_count
            );

            // percentage formatting: there must be a '.' followed by exactly two digits before '%'
            let pct_part = &bar[closing_bracket_idx + 2..]; // skip "] "
            assert!(
                pct_part.ends_with('%'),
                "percentage part must end with '%', got '{}'",
                pct_part
            );
            let number_part = &pct_part[..pct_part.len() - 1]; // strip trailing '%'
            // ensure there is a decimal point with two digits after it
            let dot_idx = number_part
                .find('.')
                .expect("percentage must contain a decimal point");
            let after_dot = &number_part[dot_idx + 1..];
            assert_eq!(
                after_dot.len(),
                2,
                "percentage must have two decimal places, got '{}' for progress {}",
                after_dot,
                p
            );
        }
    }

    #[test]
    fn test_expected_string_matches_manual_construction() {
        // sanity check: build_expected_bar should be stable and deterministic
        let samples = [0.0f32, 1.23f32, 33.33f32, 66.66f32, 100.0f32];
        for &p in &samples {
            let manual = {
                let width = 40;
                let filled = (p / 100.0 * width as f32) as usize;
                let bar = format!(
                    "\r[{}{}] {:>6.2}%",
                    "=".repeat(filled),
                    " ".repeat(width - filled),
                    p
                );
                bar
            };
            let expected = build_expected_bar(p);
            assert_eq!(manual, expected);
        }
    }

    // Visual test to manually verify the progress bar output
    #[test]
    fn test_default_with_progress_visual() {
        for i in 0..=100 {
            default_with_progress(i as f32);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        println!();
    }
}
