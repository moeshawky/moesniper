//! Indentation detection and auto-correction for sniper.
//!
//! Detects expected indentation from surrounding context and can:
//! - Validate that replacement content matches expected indentation
//! - Auto-correct missing indentation by prepending detected indent

/// Represents the indentation style detected from context.
#[derive(Debug, Clone, PartialEq)]
pub struct IndentStyle {
    pub spaces: usize,
    pub uses_tabs: bool,
    pub width: usize, // Tab width (typically 4 or 8)
}

impl Default for IndentStyle {
    fn default() -> Self {
        Self {
            spaces: 0,
            uses_tabs: false,
            width: 4,
        }
    }
}

impl IndentStyle {
    /// Returns the indentation string for a given level.
    pub fn indent_string(&self, level: usize) -> String {
        if self.uses_tabs {
            "\t".repeat(level)
        } else {
            " ".repeat(self.spaces * level)
        }
    }
}

/// Detects indentation style from a slice of lines.
/// Looks at non-empty lines to determine the prevailing style.
pub fn detect_indent_style(lines: &[String]) -> IndentStyle {
    let mut space_counts: Vec<usize> = Vec::new();
    let mut tab_count = 0;
    let mut sample_lines = 0;

 for line in lines.iter().filter(|l| !l.trim().is_empty()) {
 let spaces = line.chars().take_while(|c| *c == ' ').count();
        let tabs = line.chars().take_while(|c| *c == '\t').count();

        if tabs > 0 {
            tab_count += 1;
        } else if spaces > 0 {
            space_counts.push(spaces);
        }

        sample_lines += 1;
        if sample_lines >= 20 {
            break; // Sample first 20 non-empty lines
        }
    }

    if tab_count > space_counts.len() {
        // More tabs than spaces - use tabs
        IndentStyle {
            spaces: 0,
            uses_tabs: true,
            width: 4,
        }
    } else {
        // Use spaces - find most common indent
        let indent = if space_counts.is_empty() {
            4 // Default to 4 spaces
        } else {
            // Find GCD of space counts to infer indent width
            let min_spaces = *space_counts.iter().min().unwrap_or(&4);
            if (2..=8).contains(&min_spaces) {
                min_spaces
            } else {
                4
            }
        };

        IndentStyle {
            spaces: indent,
            uses_tabs: false,
            width: indent,
        }
    }
}

/// Detects expected indentation for a specific line range.
/// Looks at lines immediately before the range to determine context.
pub fn detect_expected_indent(
    all_lines: &[String],
    start_line: usize,
    _end_line: usize,
) -> (IndentStyle, usize) {
    let style = detect_indent_style(all_lines);

    // Find the indentation level from context (lines before start)
    let mut context_level = 0;
    let s = start_line.saturating_sub(1);

    // Look at line before the splice for context
    if s > 0 && s <= all_lines.len() {
        let prev_line = &all_lines[s - 1];
        let leading = prev_line.chars().take_while(|c| c.is_whitespace()).count();
        context_level = leading / style.width.max(1);

        // Check if previous line ends with a colon (Python block starter)
        let trimmed = prev_line.trim_end();
        if trimmed.ends_with(':') {
            context_level += 1; // Expect one more level of indent
        }
    }

    // Check if first line in range already has indentation
    if s < all_lines.len() && !all_lines[s].trim().is_empty() {
        let first_leading = all_lines[s]
            .chars()
            .take_while(|c| c.is_whitespace())
            .count();
        let first_level = first_leading / style.width.max(1);
        if first_level > context_level {
            context_level = first_level;
        }
    }

    (style, context_level)
}

/// Validates that replacement content matches expected indentation.
/// Returns (is_valid, warning_message, suggested_fix).
pub fn validate_indentation(
    all_lines: &[String],
    start_line: usize,
    end_line: usize,
    replacement_lines: &[String],
) -> (bool, Option<String>, Option<String>) {
    let (style, expected_level) = detect_expected_indent(all_lines, start_line, end_line);
    let expected_indent = style.indent_string(expected_level);

    // Check each non-empty replacement line
    let mut has_content = false;
    let mut min_leading = usize::MAX;

    for line in replacement_lines.iter().filter(|l| !l.trim().is_empty()) {
        has_content = true;
        let leading = line.chars().take_while(|c| c.is_whitespace()).count();
        min_leading = min_leading.min(leading);
    }

    if !has_content {
        return (true, None, None); // Empty replacement is valid
    }

    let expected_spaces = expected_indent.len();
    let style_desc = if style.uses_tabs {
        format!("{} tab(s)", expected_level)
    } else {
        format!("{} space(s)", expected_spaces)
    };

    if min_leading < expected_spaces {
        let diff = expected_spaces - min_leading;
        let warning = format!(
            "INDENTATION WARNING: Replacement has {} leading {}, expected {} (diff: {})",
            min_leading,
            if style.uses_tabs { "tab(s)" } else { "space(s)" },
            style_desc,
            diff
        );

        // Suggest the fix
let fix = replacement_lines
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                line.clone()
            } else {
                format!("{}{}", expected_indent, line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

        (false, Some(warning), Some(fix))
    } else {
        (true, None, None)
    }
}

/// Auto-fixes indentation by prepending the expected indent to each line.
pub fn auto_indent_content(
    all_lines: &[String],
    start_line: usize,
    end_line: usize,
    content: &str,
) -> String {
    let (style, expected_level) = detect_expected_indent(all_lines, start_line, end_line);
    let expected_indent = style.indent_string(expected_level);

    if expected_indent.is_empty() {
        return content.to_string();
    }

    content
        .split_inclusive('\n')
        .map(|line| {
            if line.trim().is_empty() {
                line.to_string()
            } else {
                format!("{}{}", expected_indent, line)
            }
        })
        .collect()
}

/// Checks if content needs indentation fix.
pub fn needs_indent_fix(
    all_lines: &[String],
    start_line: usize,
    end_line: usize,
    content: &str,
) -> bool {
    let (style, expected_level) = detect_expected_indent(all_lines, start_line, end_line);
    let expected_spaces = style.width.max(1) * expected_level;

    for line in content.lines() {
        if !line.trim().is_empty() {
            let leading = line.chars().take_while(|c| c.is_whitespace()).count();
            if leading < expected_spaces {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_spaces_indent() {
        let lines = vec![
            "    def foo():".to_string(),
            "        pass".to_string(),
            "    def bar():".to_string(),
        ];
        let style = detect_indent_style(&lines);
        assert_eq!(style.spaces, 4);
        assert!(!style.uses_tabs);
    }

    #[test]
    fn test_detect_tabs_indent() {
        let lines = vec![
            "\tdef foo():".to_string(),
            "\t\tpass".to_string(),
            "\tdef bar():".to_string(),
        ];
        let style = detect_indent_style(&lines);
        assert!(style.uses_tabs);
    }

    #[test]
    fn test_auto_indent() {
        let all_lines = vec![
            "def foo():\n".to_string(),
            "    pass\n".to_string(),
        ];
        let content = "print('hello')";
        let fixed = auto_indent_content(&all_lines, 2, 2, content);
        assert_eq!(fixed, "    print('hello')");
    }

    #[test]
    fn test_validate_indentation_missing() {
        let all_lines = vec![
            "def foo():\n".to_string(),
            "    pass\n".to_string(),
        ];
        let replacement = vec!["print('hello')".to_string()];
        let (valid, warning, fix) = validate_indentation(&all_lines, 2, 2, &replacement);

        assert!(!valid);
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("4 space"));
        assert!(fix.is_some());
        assert_eq!(fix.unwrap(), "    print('hello')");
    }

    #[test]
    fn test_validate_indentation_correct() {
        let all_lines = vec![
            "def foo():\n".to_string(),
            "    pass\n".to_string(),
        ];
        let replacement = vec!["    print('hello')".to_string()];
        let (valid, warning, _fix) = validate_indentation(&all_lines, 2, 2, &replacement);

        assert!(valid);
        assert!(warning.is_none());
    }

    #[test]
    fn test_detect_expected_indent_after_colon() {
        let all_lines = vec![
            "def foo():\n".to_string(),
            "    pass\n".to_string(),
        ];
        let (style, level) = detect_expected_indent(&all_lines, 2, 2);
        assert_eq!(level, 1); // After colon, expect 1 level
        assert_eq!(style.indent_string(level), "    ");
    }
}
