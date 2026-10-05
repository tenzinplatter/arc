use textwrap::wrap;

pub fn count_wrapped_lines(text: &str, max_width: usize) -> usize {
    if text.is_empty() || max_width == 0 {
        return 1;
    }
    wrap(text, max_width).len().max(1)
}

pub fn truncate_to_lines(text: &str, max_width: usize, max_lines: usize) -> String {
    if max_width == 0 || max_lines == 0 {
        return String::new();
    }

    let lines = wrap(text, max_width);

    if lines.len() <= max_lines {
        return text.to_string();
    }

    let mut result: String = lines[..max_lines]
        .iter()
        .map(|l| l.as_ref())
        .collect::<Vec<_>>()
        .join(" ");

    while !result.is_empty() && wrap(&format!("{}...", result), max_width).len() > max_lines {
        result.pop();
    }
    result.push_str("...");

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    mod count_wrapped_lines {
        use super::*;

        #[test]
        fn empty_text_returns_one() {
            assert_eq!(count_wrapped_lines("", 10), 1);
        }

        #[test]
        fn zero_width_returns_one() {
            assert_eq!(count_wrapped_lines("hello", 0), 1);
        }

        #[test]
        fn single_word_fits_in_width() {
            assert_eq!(count_wrapped_lines("hello", 10), 1);
        }

        #[test]
        fn text_wraps_to_multiple_lines() {
            assert_eq!(count_wrapped_lines("hello world", 6), 2);
        }

        #[test]
        fn long_text_wraps_to_many_lines() {
            assert_eq!(count_wrapped_lines("the quick brown fox", 10), 2);
        }
    }

    mod truncate_to_lines {
        use super::*;

        #[test]
        fn zero_width_returns_empty() {
            assert_eq!(truncate_to_lines("hello", 0, 3), "");
        }

        #[test]
        fn zero_max_lines_returns_empty() {
            assert_eq!(truncate_to_lines("hello", 10, 0), "");
        }

        #[test]
        fn text_fits_returns_original() {
            assert_eq!(truncate_to_lines("hello", 10, 3), "hello");
        }

        #[test]
        fn text_wrapping_within_limit_returns_original() {
            assert_eq!(truncate_to_lines("hello world", 6, 3), "hello world");
        }

        #[test]
        fn text_exceeding_limit_is_truncated_with_ellipsis() {
            let result = truncate_to_lines("one two three four five", 10, 1);
            assert!(
                result.ends_with("..."),
                "Expected ellipsis, got: {}",
                result
            );
            assert!(
                count_wrapped_lines(&result, 10) <= 1,
                "Result should fit in 1 line"
            );
        }

        #[test]
        fn truncation_respects_max_lines() {
            let result = truncate_to_lines(
                "this is a very long text that should wrap to many lines",
                10,
                2,
            );
            assert!(
                result.ends_with("..."),
                "Expected ellipsis, got: {}",
                result
            );
            assert!(
                count_wrapped_lines(&result, 10) <= 2,
                "Result should fit in 2 lines"
            );
        }
    }
}
