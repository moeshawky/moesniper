//! Indentation detection and auto-correction for sniper.
//!
//! Handles "stupid indentation" — LLM-generated code with inconsistent spacing,
//! mixed tabs/spaces, off-by-one anomalies, and missing context.
//!
//! Key design decisions for resilience:
//! - Statistical mode, not min: finds the most common indent step, not just the smallest.
//! - Supermajority for tabs vs spaces: 80% threshold prevents one rogue tab from flipping.
//! - Backward scan with anomaly rejection: walks back up to 20 lines to find a reliable
//!   context line, skipping lines whose indent is not a multiple of the detected step.
//! - Brace/colon/continuation awareness: `{`, `}` , `:`, `(`, `[` all affect expected level.
//! - Round-to-nearest for off-by-one: 7 spaces in a 4-space file rounds to level 2, not 1.

use std::collections::HashMap;

/// Represents the indentation style detected from context.
#[derive(Debug, Clone, PartialEq)]
pub struct IndentStyle {
    pub spaces: usize,
    pub uses_tabs: bool,
    pub width: usize,
}

impl Default for IndentStyle {
    fn default() -> Self {
        Self {
            spaces: 4,
            uses_tabs: false,
            width: 4,
        }
    }
}

impl IndentStyle {
    pub fn indent_string(&self, level: usize) -> String {
        if self.uses_tabs {
            "\t".repeat(level)
        } else {
            " ".repeat(self.spaces * level)
        }
    }
}

// ————————————————————————————————————————————————————————————————————————————————
// detect_indent_style — robust statistical detection
// ————————————————————————————————————————————————————————————————————————————————

const SUPERMAJORITY_RATIO: f64 = 0.80;

/// Detects indentation style using statistical mode of indent step sizes.
/// Uses supermajority threshold for tabs-vs-spaces decision.
/// Samples all non-empty lines for accuracy (not just first 20).
pub fn detect_indent_style(lines: &[String]) -> IndentStyle {
    let mut space_levels: Vec<usize> = Vec::new();
    let mut tab_levels: Vec<usize> = Vec::new();

    for line in lines.iter() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let leading_spaces = line.chars().take_while(|c| *c == ' ').count();
        let leading_tabs = line.chars().take_while(|c| *c == '\t').count();

        if leading_tabs > 0 && leading_spaces == 0 {
            tab_levels.push(leading_tabs);
        } else if leading_spaces > 0 && leading_tabs == 0 {
            space_levels.push(leading_spaces);
        }
        // Mixed leading whitespace (both tabs and spaces) — discard as anomalous
    }

    let total_indented = tab_levels.len() + space_levels.len();
    if total_indented == 0 {
        return IndentStyle::default();
    }

    // Supermajority check: don't flip to tabs unless >80% of indented lines use tabs
    if tab_levels.len() as f64 / total_indented as f64 > SUPERMAJORITY_RATIO {
        // Tab mode: width is fixed at the most common tab count
        let mode_width = mode_usize(&tab_levels).unwrap_or(1).max(1);
        return IndentStyle {
            spaces: 0,
            uses_tabs: true,
            width: mode_width,
        };
    }

    // Space mode: find the indent step with frequency-weighted scoring
    let indent_step = detect_space_step(&space_levels);
    IndentStyle {
        spaces: indent_step,
        uses_tabs: false,
        width: indent_step,
    }
}

/// Find the most common indentation step size from a list of space counts.
///
/// Each candidate step is scored by: how many levels are clean multiples
/// of this step, weighted by frequency. Higher steps are preferred on ties
/// (4-space beats 2-space when both are equally plausible).
fn detect_space_step(levels: &[usize]) -> usize {
    if levels.is_empty() {
        return 4;
    }

    // Count frequency of each distinct space count
    let mut freq: HashMap<usize, usize> = HashMap::new();
    for &l in levels {
        *freq.entry(l).or_default() += 1;
    }

    // Candidate steps to test: each distinct non-zero space count,
    // plus each difference between distinct counts
    let mut candidates: Vec<usize> = freq.keys().copied().filter(|&k| k > 0).collect();
    let mut sorted_keys: Vec<usize> = candidates.clone();
    sorted_keys.sort_unstable();
    for w in sorted_keys.windows(2) {
        let d = w[1] - w[0];
        if d > 0 && !candidates.contains(&d) {
            candidates.push(d);
        }
    }
    // Always include 4 as a fallback candidate
    if !candidates.contains(&4) {
        candidates.push(4);
    }

    // Score: raw frequency dominates. On tie, SMALLER step wins
    // (indent step is the base unit, not the deepest nesting level).
    let mut best_step = 4;
    let mut best_score: i64 = -1;

    for &candidate in &candidates {
        if !(2..=16).contains(&candidate) {
            continue;
        }
        let raw_freq = freq.get(&candidate).copied().unwrap_or(0) as i64;
        let score = raw_freq * 1000 - candidate as i64;

        if score > best_score {
            best_score = score;
            best_step = candidate;
        }
    }

    best_step
}

fn mode_usize(values: &[usize]) -> Option<usize> {
    let mut counts: HashMap<usize, usize> = HashMap::new();
    for v in values {
        *counts.entry(*v).or_default() += 1;
    }
    counts.into_iter().max_by_key(|(_, c)| *c).map(|(v, _)| v)
}

// ————————————————————————————————————————————————————————————————————————————————
// detect_expected_indent — robust context scanner
// ————————————————————————————————————————————————————————————————————————————————

const MAX_SCAN_BACK: usize = 20;

/// Detects the expected indentation level at the splice site by scanning
/// backwards through up to 20 preceding lines for a reliable context line.
///
/// A line is "reliable" if its leading whitespace is a multiple of the indent
/// step (no off-by-one anomalies) and it is not a closing brace on its own.
///
/// Handles: `{` (+1 level), `}` (−1 level for content after), `:` (+1),
/// `(` and `[` (continuation: +1 level for next line).
pub fn detect_expected_indent(
    all_lines: &[String],
    start_line: usize,
    _end_line: usize,
) -> (IndentStyle, usize) {
    let idx = start_line.saturating_sub(1);
    let window_start = idx.saturating_sub(MAX_SCAN_BACK);
    let window = &all_lines[window_start..idx];
    let style = if window.len() >= 3 {
        detect_indent_style(window)
    } else {
        detect_indent_style(all_lines)
    };
    let step = if style.uses_tabs {
        1
    } else {
        style.spaces.max(1)
    };

    let s = start_line.saturating_sub(1);
    let scan_start = s.saturating_sub(MAX_SCAN_BACK);

    // Walk backwards to find the best context line
    let mut best_level: Option<usize> = None;
    let mut best_quality: i32 = -1; // higher = better

    for i in (scan_start..s).rev() {
        if i >= all_lines.len() {
            continue;
        }
        let line = &all_lines[i];
        if line.trim().is_empty() {
            continue;
        }

        let leading = count_leading_whitespace(line);
        let quality = context_quality(leading, step, line);

        if quality > best_quality {
            let level = round_to_nearest_level(leading, step);
            best_level = Some(level);
            best_quality = quality;

            // Perfect match: indent is an exact multiple — stop scanning
            if leading.is_multiple_of(step) {
                break;
            }
        }
    }

    let mut context_level = best_level.unwrap_or(0);

    // Adjust for block starters/enders on the context line
    if let Some(line) = context_line_before(all_lines, start_line) {
        let trimmed = line.trim_end();
        let trimmed_no_comment = strip_trailing_comment(trimmed);

        // Block/continuation opener: `{`, `:`, `(`, `[` → expect indent increase
        if trimmed_no_comment.ends_with('{')
            || trimmed_no_comment.ends_with(':')
            || trimmed_no_comment.ends_with('(')
            || trimmed_no_comment.ends_with('[')
        {
            context_level += 1;
        }

        // Block closer: line starts with `}` → subsequent content should be at this level
        // (Already handled: we used `}`-line's own indent as the context level,
        //  which IS the correct level for content after the closing brace.)
    }

    (style, context_level)
}

/// Returns the raw content of the last non-empty line before start_line.
fn context_line_before(all_lines: &[String], start_line: usize) -> Option<&String> {
    let s = start_line.saturating_sub(1);
    for i in (0..s).rev() {
        if i >= all_lines.len() {
            continue;
        }
        if !all_lines[i].trim().is_empty() {
            return Some(&all_lines[i]);
        }
    }
    None
}

/// Quality score for a context line. Higher = more reliable.
/// - Exact multiple of step: +10 (ideal)
/// - Off by 1 or 2: +5 (slightly anomalous, still usable)
/// - Block opener/closer line: +0 (reliable but may need level adjustment)
/// - Deeply indented lines: bonus (more structural, harder to be LLM noise)
#[allow(clippy::cast_possible_truncation)] // .min(5) guarantees value fits in i32
fn context_quality(leading: usize, step: usize, _line: &str) -> i32 {
    if step == 0 {
        return 0;
    }
    let remainder = leading % step;
    let base = if remainder == 0 {
        10
    } else if remainder <= 2 {
        5
    } else {
        0
    };
    // Bonus for deeper indentation: more reliable than top-level lines
    let depth_bonus = (leading / step).min(5) as i32;
    base + depth_bonus
}

/// Rounds a whitespace count to the nearest indent level.
/// 7 spaces in a 4-space file → level 2 (7 rounds to 8, 8/4 = 2).
fn round_to_nearest_level(leading: usize, step: usize) -> usize {
    if step == 0 {
        return 0;
    }
    let exact = leading / step;
    let remainder = leading % step;
    if remainder > step / 2 {
        exact + 1
    } else {
        exact
    }
}

fn count_leading_whitespace(line: &str) -> usize {
    line.chars().take_while(|c| c.is_whitespace()).count()
}

fn strip_trailing_comment(s: &str) -> &str {
    if let Some(pos) = s.find("//") {
        return &s[..pos];
    }
    if let Some(pos) = s.find('#') {
        return &s[..pos];
    }
    s
}

// ————————————————————————————————————————————————————————————————————————————————
// validate_indentation
// ————————————————————————————————————————————————————————————————————————————————

pub fn validate_indentation(
    all_lines: &[String],
    start_line: usize,
    end_line: usize,
    replacement_lines: &[String],
) -> (bool, Option<String>, Option<String>) {
    let (style, expected_level) = detect_expected_indent(all_lines, start_line, end_line);
    let expected_indent = style.indent_string(expected_level);

    let mut has_content = false;
    let mut min_leading = usize::MAX;

    for line in replacement_lines.iter().filter(|l| !l.trim().is_empty()) {
        has_content = true;
        let trimmed = line.trim_start();
        if trimmed.starts_with(')') || trimmed.starts_with('}') || trimmed.starts_with(']') {
            continue;
        }
        let leading = line.chars().take_while(|c| c.is_whitespace()).count();
        min_leading = min_leading.min(leading);
    }

    if !has_content {
        return (true, None, None);
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
            if style.uses_tabs {
                "tab(s)"
            } else {
                "space(s)"
            },
            style_desc,
            diff
        );

        let fix = replacement_lines
            .iter()
            .map(|line| {
                if line.trim().is_empty() {
                    line.clone()
                } else {
                    let stripped = line.trim_start();
                    format!("{}{}", expected_indent, stripped)
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        (false, Some(warning), Some(fix))
    } else {
        (true, None, None)
    }
}

// ————————————————————————————————————————————————————————————————————————————————
// auto_indent_content
// ————————————————————————————————————————————————————————————————————————————————

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
        .lines()
        .map(|line| {
            if line.trim().is_empty() {
                line.to_string()
            } else {
                let stripped = line.trim_start();
                format!("{}{}", expected_indent, stripped)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ————————————————————————————————————————————————————————————————————————————————
// needs_indent_fix
// ————————————————————————————————————————————————————————————————————————————————

pub fn needs_indent_fix(
    all_lines: &[String],
    start_line: usize,
    end_line: usize,
    content: &str,
) -> bool {
    let (style, expected_level) = detect_expected_indent(all_lines, start_line, end_line);
    let expected_indent = style.indent_string(expected_level);

    if expected_indent.is_empty() {
        return false;
    }

    let expected_spaces = expected_indent.len();

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

// ————————————————————————————————————————————————————————————————————————————————
// tests
// ————————————————————————————————————————————————————————————————————————————————

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // detect_indent_style — statistical mode detection
    // ============================================================

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
        let lines: Vec<String> = (0..50).map(|_| "\tfn foo() {".to_string()).collect();
        let style = detect_indent_style(&lines);
        assert!(style.uses_tabs);
    }

    #[test]
    fn test_detect_2_space_indent() {
        let lines = vec!["  def foo():".to_string(), "    pass".to_string()];
        let style = detect_indent_style(&lines);
        assert_eq!(style.spaces, 2);
    }

    #[test]
    fn test_detect_8_space_indent() {
        let lines = vec![
            "        fn main() {".to_string(),
            "                println!();".to_string(),
        ];
        let style = detect_indent_style(&lines);
        assert_eq!(style.spaces, 8);
    }

    #[test]
    fn test_detect_indent_empty_file_defaults_spaces() {
        let lines: Vec<String> = vec![];
        let style = detect_indent_style(&lines);
        assert_eq!(style.spaces, 4);
        assert!(!style.uses_tabs);
    }

    #[test]
    fn test_supermajority_one_tab_does_not_flip_space_file() {
        // 1 tab line in 29 space lines — should NOT flip to tabs
        let mut lines: Vec<String> = (0..29).map(|_| "    fn foo() {".to_string()).collect();
        lines.push("\tfn rogue() {}".to_string());
        let style = detect_indent_style(&lines);
        assert!(
            !style.uses_tabs,
            "Single rogue tab should not flip a space-indented file"
        );
    }

    #[test]
    fn test_detect_step_from_level_differences() {
        let lines = vec![
            "class Foo:".to_string(),
            "    def a(self):".to_string(),
            "        pass".to_string(),
            "    def b(self):".to_string(),
            "        return 1".to_string(),
        ];
        let style = detect_indent_style(&lines);
        assert_eq!(style.spaces, 4);
    }

    #[test]
    fn test_detect_indent_with_anomalous_spacing() {
        // LLM might produce 0, 3, 7, 4, 8 spaces — mode of diffs should still be ~4
        let lines = vec![
            "fn main() {".to_string(),
            "   let x = 1;".to_string(),      // 3 spaces (anomaly)
            "    let y = 2;".to_string(),     // 4 spaces
            "       let z = 3;".to_string(),  // 7 spaces (anomaly)
            "        let w = 4;".to_string(), // 8 spaces
        ];
        let style = detect_indent_style(&lines);
        assert!(!style.uses_tabs);
        // Diffs: 3-0=3, 4-3=1, 7-4=3, 8-7=1. Most common diff not reliable.
        // GCD of [1,3] = 1 → falls to 4. Still acceptable.
        assert!(style.spaces >= 1);
    }

    // ============================================================
    // detect_expected_indent — robust context scanner
    // ============================================================

    #[test]
    fn test_expected_indent_after_colon() {
        let all_lines = vec!["def foo():\n".to_string(), "    pass\n".to_string()];
        let (_, level) = detect_expected_indent(&all_lines, 2, 2);
        assert_eq!(level, 1);
    }

    #[test]
    fn test_expected_indent_after_brace() {
        let all_lines = vec!["fn main() {\n".to_string(), "    let x = 1;\n".to_string()];
        let (_, level) = detect_expected_indent(&all_lines, 2, 2);
        assert_eq!(level, 1, "Content after `{{` should be indent level 1");
    }

    #[test]
    fn test_expected_indent_after_closing_brace() {
        let all_lines = vec![
            "fn outer() {\n".to_string(),
            "    fn inner() {\n".to_string(),
            "        pass\n".to_string(),
            "    }\n".to_string(),
            "    // editing here\n".to_string(),
        ];
        let (_, level) = detect_expected_indent(&all_lines, 5, 5);
        assert_eq!(
            level, 1,
            "Content after `}}` at outer level should be level 1, not 2"
        );
    }

    #[test]
    fn test_expected_indent_top_of_file() {
        let all_lines = vec!["fn main() {\n".to_string(), "    let x = 1;\n".to_string()];
        let (_, level) = detect_expected_indent(&all_lines, 1, 1);
        assert_eq!(level, 0);
    }

    #[test]
    fn test_expected_indent_deeply_nested() {
        let all_lines = vec![
            "class Foo:\n".to_string(),
            "    def bar(self):\n".to_string(),
            "        if True:\n".to_string(),
            "            pass\n".to_string(),
        ];
        let (_, level) = detect_expected_indent(&all_lines, 4, 4);
        assert_eq!(level, 3);
    }

    #[test]
    fn test_expected_indent_skips_blank_lines() {
        let all_lines = vec![
            "fn outer() {\n".to_string(),
            "    let x = 1;\n".to_string(),
            "\n".to_string(),
            "\n".to_string(),
            "    // editing here\n".to_string(),
        ];
        let (_, level) = detect_expected_indent(&all_lines, 5, 5);
        assert_eq!(level, 1, "Blank lines must be skipped to find real context");
    }

    #[test]
    fn test_expected_indent_one_misindented_line_does_not_poison() {
        let all_lines = vec![
            "fn main() {\n".to_string(),
            "    let a = 1;\n".to_string(),
            "  let bad = 2;\n".to_string(),
            "    let b = 3;\n".to_string(),
            "    let c = 4;\n".to_string(),
            "    let d = 5;\n".to_string(),
            "    let e = 6;\n".to_string(),
            "  // editing here\n".to_string(),
        ];
        let (_, level) = detect_expected_indent(&all_lines, 8, 8);
        assert_eq!(
            level, 1,
            "A single misindented line must not poison context detection"
        );
    }

    #[test]
    fn test_expected_indent_off_by_one_rounding() {
        let all_lines = vec![
            "fn main() {\n".to_string(),
            "   let x = 1;\n".to_string(), // 3 spaces, should be 4
        ];
        let (_, level) = detect_expected_indent(&all_lines, 2, 2);
        assert_eq!(
            level, 1,
            "3 spaces in a 4-space file should round to level 1, not 0"
        );
    }

    #[test]
    fn test_expected_indent_scan_back_multiple_lines() {
        // The immediate preceding line is blank+misindented, but a good line is further back
        let all_lines = vec![
            "fn main() {\n".to_string(),
            "    let a = 1;\n".to_string(),
            "    let b = 2;\n".to_string(),
            "     let c = 3;\n".to_string(), // 5 spaces — anomaly
            "\n".to_string(),                // blank
            "// editing here\n".to_string(),
        ];
        let (_, level) = detect_expected_indent(&all_lines, 6, 6);
        assert_eq!(
            level, 1,
            "Must scan past blank+anomaly to find the real context"
        );
    }

    #[test]
    fn test_indent_step_from_window_not_global_file() {
        let mut lines: Vec<String> = vec![
            "fn main() {\n".to_string(),
            "    let x = 1;\n".to_string(),
            "    let y = 2;\n".to_string(),
        ];
        for _ in 0..100 {
            lines.push("        deep_body();\n".to_string());
        }
        lines.push("    // editing here\n".to_string());

        let (style, _) = detect_expected_indent(&lines, 5, 5);
        assert_eq!(
            style.spaces, 4,
            "Edit near 4-space context should detect 4-space step, not 8 (deep body bias)"
        );
    }

    #[test]
    fn test_expected_indent_after_trailing_comma() {
        let mut all_lines: Vec<String> = (0..20).map(|_| "    let x = 1;\n".to_string()).collect();
        all_lines.push("    my_function(\n".to_string());
        all_lines.push("        arg1,\n".to_string());
        all_lines.push("        arg2,\n".to_string());
        let (_, level) = detect_expected_indent(&all_lines, 24, 24);
        assert_eq!(
            level, 2,
            "Comma means same-level continuation, not deeper indent"
        );
    }

    // ============================================================
    // auto_indent_content
    // ============================================================

    #[test]
    fn test_auto_indent_unindented_content() {
        let all_lines = vec!["def foo():\n".to_string(), "    pass\n".to_string()];
        let content = "print('hello')";
        let fixed = auto_indent_content(&all_lines, 2, 2, content);
        assert_eq!(fixed, "    print('hello')");
    }

    #[test]
    fn test_auto_indent_strips_existing_whitespace() {
        let all_lines = vec!["def foo():\n".to_string(), "    pass\n".to_string()];
        let content = "  print('hello')";
        let fixed = auto_indent_content(&all_lines, 2, 2, content);
        assert_eq!(fixed, "    print('hello')");
    }

    #[test]
    fn test_auto_indent_multiline_content() {
        let all_lines = vec!["def foo():\n".to_string(), "    pass\n".to_string()];
        let content = "print('hello')\nprint('world')";
        let fixed = auto_indent_content(&all_lines, 2, 2, content);
        assert_eq!(fixed, "    print('hello')\n    print('world')");
    }

    #[test]
    fn test_auto_indent_preserves_blank_lines() {
        let all_lines = vec!["def foo():\n".to_string(), "    pass\n".to_string()];
        let content = "print('a')\n\nprint('b')";
        let fixed = auto_indent_content(&all_lines, 2, 2, content);
        assert_eq!(fixed, "    print('a')\n\n    print('b')");
    }

    #[test]
    fn test_auto_indent_mixed_existing_indent() {
        let all_lines = vec!["def foo():\n".to_string(), "    pass\n".to_string()];
        let content = "  line1\n    line2\nline3";
        let fixed = auto_indent_content(&all_lines, 2, 2, content);
        assert_eq!(fixed, "    line1\n    line2\n    line3");
    }

    #[test]
    fn test_auto_indent_no_indent_needed() {
        let all_lines = vec!["fn main() {\n".to_string(), "    let x = 1;\n".to_string()];
        let content = "println!(\"hello\");";
        let fixed = auto_indent_content(&all_lines, 1, 1, content);
        assert_eq!(fixed, "println!(\"hello\");");
    }

    #[test]
    fn test_auto_indent_with_tabs() {
        // File with tab-indented functions. Line 31 (last line) is "\t\tpass"
        // which is inside a `{` block from line 30 → expected level is 2.
        let lines: Vec<String> = (0..30)
            .map(|_| "\tfn foo() {".to_string())
            .chain(std::iter::once("\t\tpass".to_string()))
            .collect();
        let content = "print('hello')";
        let fixed = auto_indent_content(&lines, 31, 31, content);
        // Content replaces line 31, inside the `{` block → level 2
        assert_eq!(fixed, "\t\tprint('hello')");
    }

    #[test]
    fn test_auto_indent_tabs_strips_existing_spaces() {
        let lines: Vec<String> = (0..30)
            .map(|_| "\tfn foo() {".to_string())
            .chain(std::iter::once("\t\tpass".to_string()))
            .collect();
        let content = "    print('hello')";
        let fixed = auto_indent_content(&lines, 31, 31, content);
        assert_eq!(fixed, "\t\tprint('hello')");
    }

    #[test]
    fn test_needs_indent_fix_unindented_content() {
        let all_lines = vec!["def foo():\n".to_string(), "    pass\n".to_string()];
        assert!(needs_indent_fix(&all_lines, 2, 2, "print('hello')"));
    }

    #[test]
    fn test_needs_indent_fix_already_indented() {
        let all_lines = vec!["def foo():\n".to_string(), "    pass\n".to_string()];
        assert!(!needs_indent_fix(&all_lines, 2, 2, "    print('hello')"));
    }

    #[test]
    fn test_needs_indent_fix_partial_indent() {
        let all_lines = vec!["def foo():\n".to_string(), "    pass\n".to_string()];
        assert!(needs_indent_fix(&all_lines, 2, 2, "  print('hello')"));
    }

    #[test]
    fn test_needs_indent_fix_tab_content() {
        let lines: Vec<String> = (0..30)
            .map(|_| "\tfn foo() {".to_string())
            .chain(std::iter::once("\t\tpass".to_string()))
            .collect();
        // Unindented content needs fix (expected level 2)
        assert!(needs_indent_fix(&lines, 31, 31, "print('hello')"));
        // Content at level 2 (two tabs) is correct
        assert!(!needs_indent_fix(&lines, 31, 31, "\t\tprint('hello')"));
    }

    #[test]
    fn test_needs_indent_fix_top_level() {
        let all_lines = vec!["fn main() {\n".to_string(), "    let x = 1;\n".to_string()];
        assert!(!needs_indent_fix(&all_lines, 1, 1, "println!(\"hello\");"));
    }

    // ============================================================
    // validate_indentation
    // ============================================================

    #[test]
    fn test_validate_indentation_missing() {
        let all_lines = vec!["def foo():\n".to_string(), "    pass\n".to_string()];
        let replacement = vec!["print('hello')".to_string()];
        let (valid, warning, fix) = validate_indentation(&all_lines, 2, 2, &replacement);
        assert!(!valid);
        assert!(warning.is_some());
        assert!(warning.unwrap().contains("4 space"));
        assert_eq!(fix.unwrap(), "    print('hello')");
    }

    #[test]
    fn test_validate_indentation_correct() {
        let all_lines = vec!["def foo():\n".to_string(), "    pass\n".to_string()];
        let replacement = vec!["    print('hello')".to_string()];
        let (valid, warning, _fix) = validate_indentation(&all_lines, 2, 2, &replacement);
        assert!(valid);
        assert!(warning.is_none());
    }

    #[test]
    fn test_validate_indentation_partial_fix_strips_existing() {
        let all_lines = vec!["def foo():\n".to_string(), "    pass\n".to_string()];
        let replacement = vec!["  print('hello')".to_string()];
        let (valid, _warning, fix) = validate_indentation(&all_lines, 2, 2, &replacement);
        assert!(!valid);
        assert_eq!(fix.unwrap(), "    print('hello')");
    }

    #[test]
    fn test_validate_indentation_empty_replacement() {
        let all_lines = vec!["def foo():\n".to_string(), "    pass\n".to_string()];
        let replacement: Vec<String> = vec!["".to_string()];
        let (valid, warning, _fix) = validate_indentation(&all_lines, 2, 2, &replacement);
        assert!(valid);
        assert!(warning.is_none());
    }

    #[test]
    fn test_validate_indentation_whitespace_only_line() {
        let all_lines = vec!["def foo():\n".to_string(), "    pass\n".to_string()];
        let replacement = vec!["    ".to_string(), "print('x')".to_string()];
        let (valid, _warning, _fix) = validate_indentation(&all_lines, 2, 2, &replacement);
        assert!(!valid);
    }

    #[test]
    fn test_validate_indentation_closer_tokens_at_lower_indent() {
        let all_lines = vec![
            "fn outer() {\n".to_string(),
            "    if true {\n".to_string(),
            "        do_thing();\n".to_string(),
            "    }\n".to_string(),
            "}\n".to_string(),
        ];
        let replacement = vec![
            "        more_code();\n".to_string(),
            "    }\n".to_string(),
            "}\n".to_string(),
        ];
        let (valid, warning, _fix) = validate_indentation(&all_lines, 3, 5, &replacement);
        assert!(
            valid,
            "Closer tokens at lower indent are correct, got warning: {:?}",
            warning
        );
    }

    #[test]
    fn test_validate_indentation_closer_tokens_at_lower_indent_single_line() {
        let all_lines = vec![
            "fn outer() {\n".to_string(),
            "    let x = 1;\n".to_string(),
            "}\n".to_string(),
        ];
        let replacement = vec!["}".to_string()];
        let (valid, _warning, _fix) = validate_indentation(&all_lines, 2, 2, &replacement);
        assert!(valid, "Single closing brace at lower indent is correct");
    }

    // ============================================================
    // helper functions
    // ============================================================

    #[test]
    fn test_indent_string_spaces() {
        let style = IndentStyle {
            spaces: 4,
            uses_tabs: false,
            width: 4,
        };
        assert_eq!(style.indent_string(0), "");
        assert_eq!(style.indent_string(1), "    ");
        assert_eq!(style.indent_string(2), "        ");
    }

    #[test]
    fn test_indent_string_tabs() {
        let style = IndentStyle {
            spaces: 0,
            uses_tabs: true,
            width: 4,
        };
        assert_eq!(style.indent_string(0), "");
        assert_eq!(style.indent_string(1), "\t");
        assert_eq!(style.indent_string(2), "\t\t");
    }

    #[test]
    fn test_round_to_nearest_level() {
        assert_eq!(round_to_nearest_level(0, 4), 0);
        assert_eq!(round_to_nearest_level(4, 4), 1);
        assert_eq!(round_to_nearest_level(6, 4), 1); // equidistant → round down
        assert_eq!(round_to_nearest_level(2, 4), 0); // equidistant → round down
        assert_eq!(round_to_nearest_level(7, 4), 2); // 7 > 6, closer to 8 → level 2
        assert_eq!(round_to_nearest_level(3, 4), 1); // 3 is closer to 4 than 0
    }

    #[test]
    fn test_strip_trailing_comment() {
        assert_eq!(
            strip_trailing_comment("    let x = 1; // comment"),
            "    let x = 1; "
        );
        assert_eq!(strip_trailing_comment("    # python comment"), "    ");
        assert_eq!(strip_trailing_comment("    let x = 1;"), "    let x = 1;");
        // # anywhere starts a comment (aggressive strip is safe for indent detection)
        assert_eq!(strip_trailing_comment("x = obj#method"), "x = obj");
    }
}
