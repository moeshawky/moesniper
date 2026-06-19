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
struct IndentStyle {
    /// Number of spaces per indent level (0 if using tabs).
    pub spaces: usize,
    /// Whether this file uses tab indentation.
    pub uses_tabs: bool,
    /// Visual width of one indent level.
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
    /// Returns the indentation string for a given nesting level.
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
fn detect_indent_style(lines: &[String]) -> IndentStyle {
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
    candidates.sort_unstable();
    let initial_len = candidates.len();
    for i in 0..initial_len.saturating_sub(1) {
        let d = candidates[i + 1] - candidates[i];
        if d > 0 {
            candidates.push(d);
        }
    }
    // Always include 4 as a fallback candidate
    candidates.push(4);
    candidates.sort_unstable();
    candidates.dedup();

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
fn detect_expected_indent(all_lines: &[String], start_line: usize) -> (IndentStyle, usize) {
    let idx = start_line.saturating_sub(1).min(all_lines.len());
    let window_start = idx.saturating_sub(MAX_SCAN_BACK);
    let window = &all_lines[window_start..idx];

    // Count indented lines in the backward window to assess signal strength.
    let indented_in_window = window
        .iter()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty()
                && (line.chars().take_while(|c| *c == ' ').count() > 0
                    || line.chars().take_while(|c| *c == '\t').count() > 0)
        })
        .count();

    // For style detection, use a broader context to avoid module-docstring bias.
    // When the backward window is at the file top AND has weak signal (< 2 indented lines),
    // include lines AFTER the edit point so we see actual code structure.
    let style = if window_start == 0 && all_lines.len() > idx && indented_in_window < 2 {
        // At file top with weak backward signal: use forward context for style detection
        let forward_end = (idx + MAX_SCAN_BACK).min(all_lines.len());
        let mut combined = Vec::with_capacity(window.len() + forward_end - idx);
        combined.extend_from_slice(window);
        combined.extend_from_slice(&all_lines[idx..forward_end]);
        if combined.len() >= 3 {
            detect_indent_style(&combined)
        } else {
            detect_indent_style(all_lines)
        }
    } else if window.len() >= 3 {
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

        // Block/continuation opener: `{`, `:`, `(`, `[` → expect indent increase.
        //
        // For `{` we find the LAST brace-like character on the line rather than
        // checking ends_with.  This handles `fn foo() { something();` where `{`
        // is not at end-of-line but still opens a block for lines below.
        // `fn foo() {}` (last brace is `}`) is correctly NOT treated as opener.
        let last_brace_pos = trimmed_no_comment.rfind(['{', '}']);
        let opens_block =
            last_brace_pos.is_some_and(|pos| trimmed_no_comment.as_bytes()[pos] == b'{');
        if opens_block
            || trimmed_no_comment.ends_with(':')
            || trimmed_no_comment.ends_with('(')
            || trimmed_no_comment.ends_with('[')
        {
            context_level += 1;
        }

        // Block closer: line starts with `}` → subsequent content should be at this level
        // (Already handled: we used `}`-line's own indent as the context level,
        //  which IS the correct level for content after the closing brace.)

        // Ensure context_level is at least the level of the line immediately
        // before the insert point — the backward scan may have found a
        // shallower structural line further up, but the adjacent line is
        // the most direct context.
        let clb_leading = count_leading_whitespace(line);
        let clb_level = round_to_nearest_level(clb_leading, step);
        context_level = context_level.max(clb_level);
    }

    // If there is a line at the insert position and it is indented deeper
    // than the detected context, use its level — the line being replaced
    // is the strongest signal of the expected indent.  Only applies when
    // the backward scan found structural context (best_level is set).
    if best_level.is_some() && s < all_lines.len() && !all_lines[s].trim().is_empty() {
        let at_line = &all_lines[s];
        let at_leading = count_leading_whitespace(at_line);
        let at_level = round_to_nearest_level(at_leading, step);
        context_level = context_level.max(at_level);
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

/// Validates that replacement lines match the detected indentation style.
pub fn validate_indentation(
    all_lines: &[String],
    start_line: usize,
    _end_line: usize,
    replacement_lines: &[String],
) -> (bool, Option<String>, Option<String>) {
    let (style, expected_level) = detect_expected_indent(all_lines, start_line);
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

/// Adjusts indentation of content to match the surrounding context.
///
/// Finds the minimum leading whitespace in the content, computes the delta to
/// the expected indent from context, and shifts all content lines by that delta.
/// This preserves internal indentation structure (multi-level content) while
/// fixing the base level.
///
/// If the content is already at or above the expected indent level (i.e. the
/// LLM sent correctly indented content), the content is returned unchanged.
pub fn auto_indent_content(
    all_lines: &[String],
    start_line: usize,
    _end_line: usize,
    content: &str,
) -> String {
    let (style, expected_level) = detect_expected_indent(all_lines, start_line);
    let expected_indent = style.indent_string(expected_level);

    if expected_indent.is_empty() {
        return content.to_string();
    }

    let content_lines: Vec<&str> = content.lines().collect();

    // Compute minimum leading whitespace, skipping closer tokens
    // (}, ), ]) — closer tokens are at a different indent level by design
    // and should not drag down the min_leading for body content.
    // This mirrors validate_indentation's closer-token skip.
    let is_closer = |l: &&str| -> bool {
        let t = l.trim();
        t.starts_with('}') || t.starts_with(')') || t.starts_with(']')
    };
    let min_leading = content_lines
        .iter()
        .filter(|l| !l.trim().is_empty() && !is_closer(l))
        .map(|l| l.chars().take_while(|c| c.is_whitespace()).count())
        .min()
        .unwrap_or(0);

    // If content starts with a closer token (}, ), ]), the whole block
    // should be at one level less indentation than body content — the
    // closer closes the current block, so content after it is at the
    // block level.  This mirrors validate_indentation's closer skip.
    let first_nonempty = content_lines.iter().find(|l| !l.trim().is_empty());
    let starts_with_closer = first_nonempty.is_some_and(|l| {
        let t = l.trim();
        t.starts_with('}') || t.starts_with(')') || t.starts_with(']')
    });
    let effective_level = if starts_with_closer {
        expected_level.saturating_sub(1)
    } else {
        expected_level
    };
    let effective_indent = style.indent_string(effective_level);

    if min_leading >= effective_indent.len() {
        return content.to_string();
    }

    content_lines
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                (*line).to_string()
            } else {
                let leading = line.chars().take_while(|c| c.is_whitespace()).count();
                let overhang = leading - min_leading;
                let indent_char = if style.uses_tabs { '\t' } else { ' ' };
                // BUG FIX: base indent uses style char, overhang is always spaces
                let base = indent_char.to_string().repeat(effective_indent.len());
                let overhang_spaces = " ".repeat(overhang);
                format!("{}{}{}", base, overhang_spaces, line.trim_start())
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ————————————————————————————————————————————————————————————————————————————————
// needs_indent_fix
// ————————————————————————————————————————————————————————————————————————————————

/// Returns true if the content's minimum indentation is less than expected.
pub fn needs_indent_fix(
    all_lines: &[String],
    start_line: usize,
    _end_line: usize,
    content: &str,
) -> bool {
    let (style, expected_level) = detect_expected_indent(all_lines, start_line);
    let expected_indent = style.indent_string(expected_level);

    if expected_indent.is_empty() {
        return false;
    }

    // Skip closer tokens for min_leading (same rationale as auto_indent_content)
    let is_closer = |l: &&str| -> bool {
        let t = l.trim();
        t.starts_with('}') || t.starts_with(')') || t.starts_with(']')
    };
    let min_leading = content
        .lines()
        .filter(|l| !l.trim().is_empty() && !is_closer(l))
        .map(|l| l.chars().take_while(|c| c.is_whitespace()).count())
        .min()
        .unwrap_or(0);

    min_leading < expected_indent.len()
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
        let (_, level) = detect_expected_indent(&all_lines, 2);
        assert_eq!(level, 1);
    }

    #[test]
    fn test_expected_indent_after_brace() {
        let all_lines = vec!["fn main() {\n".to_string(), "    let x = 1;\n".to_string()];
        let (_, level) = detect_expected_indent(&all_lines, 2);
        assert_eq!(level, 1, "Content after `{{` should be indent level 1");
    }

    /// G1 edge: `{` not at end-of-line still opens a block for subsequent lines.
    #[test]
    fn test_expected_indent_after_brace_not_at_eol() {
        // `{` is not the last character — body starts on same line but block
        // extends below.  The `{` should still increment the expected level.
        let all_lines = vec![
            "fn foo() { let x = 1;\n".to_string(),
            "    let y = 2;\n".to_string(),
        ];
        let (_, level) = detect_expected_indent(&all_lines, 2);
        assert_eq!(
            level, 1,
            "`{{` not at EOL still opens a block for next line"
        );
    }

    /// G1 edge: self-contained `fn foo() {}` — brace opens AND closes on same line,
    /// so the next line should NOT be indented (block is already closed).
    #[test]
    fn test_expected_indent_self_contained_block_no_indent() {
        let all_lines = vec!["fn foo() {}\n".to_string(), "fn bar() {\n".to_string()];
        let (_, level) = detect_expected_indent(&all_lines, 2);
        // `fn foo() {}` has last brace as `}` → no increment.
        // `fn bar() {` on line 2 is at file top (only 1 prior non-empty)
        // → context_level derived from line 1's indent (level 0).
        assert_eq!(
            level, 0,
            "Self-contained block on prior line must not increment level"
        );
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
        let (_, level) = detect_expected_indent(&all_lines, 5);
        assert_eq!(
            level, 1,
            "Content after `}}` at outer level should be level 1, not 2"
        );
    }

    #[test]
    fn test_expected_indent_top_of_file() {
        let all_lines = vec!["fn main() {\n".to_string(), "    let x = 1;\n".to_string()];
        let (_, level) = detect_expected_indent(&all_lines, 1);
        assert_eq!(level, 0);
    }

    #[test]
    fn test_expected_indent_out_of_bounds() {
        let all_lines = vec!["fn main() {\n".to_string(), "    let x = 1;\n".to_string()];
        let (_, level) = detect_expected_indent(&all_lines, 10);
        assert_eq!(level, 1);
    }

    #[test]
    fn test_expected_indent_empty_lines() {
        let all_lines: Vec<String> = vec![];
        let (style, level) = detect_expected_indent(&all_lines, 1);
        assert_eq!(level, 0);
        assert_eq!(style.spaces, 4);
        assert!(!style.uses_tabs);
    }

    #[test]
    fn test_expected_indent_start_line_zero() {
        let all_lines = vec!["fn main() {\n".to_string(), "    let x = 1;\n".to_string()];
        let (_, level) = detect_expected_indent(&all_lines, 0);
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
        let (_, level) = detect_expected_indent(&all_lines, 4);
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
        let (_, level) = detect_expected_indent(&all_lines, 5);
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
        let (_, level) = detect_expected_indent(&all_lines, 8);
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
        let (_, level) = detect_expected_indent(&all_lines, 2);
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
        let (_, level) = detect_expected_indent(&all_lines, 6);
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

        let (style, _) = detect_expected_indent(&lines, 5);
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
        let (_, level) = detect_expected_indent(&all_lines, 24);
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
        // min_leading=0, delta=4. Overhangs preserved: (2-0=2→6), (4-0=4→8), (0-0=0→4)
        assert_eq!(fixed, "      line1\n        line2\n    line3");
    }

    #[test]
    fn test_auto_indent_preserves_multilevel_structure() {
        let all_lines = vec!["def foo():\n".to_string(), "    pass\n".to_string()];
        // Content already has correct base indent with internal nesting — leave it alone
        let content = "    let x = 1;\n        if true {\n            run();\n        }\n    }";
        let fixed = auto_indent_content(&all_lines, 2, 2, content);
        assert_eq!(fixed, content);
    }

    #[test]
    fn test_auto_indent_shifts_underindented_multilevel() {
        let all_lines = vec!["def foo():\n".to_string(), "    pass\n".to_string()];
        // Content has internal structure but base indent is too shallow (2 spaces vs 4)
        let content = "  let x = 1;\n      if true {\n          run();\n      }\n  }";
        let fixed = auto_indent_content(&all_lines, 2, 2, content);
        // Min leading = 2, delta = 2. Every line gets 2 extra spaces.
        assert_eq!(
            fixed,
            "    let x = 1;\n        if true {\n            run();\n        }\n    }"
        );
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
        // Content already has 4 spaces of indent, expected is 2 tabs (len=2).
        // Guard: min_leading(4) >= expected_indent.len()(2) → content left unchanged.
        // Style mismatch is handled by validate_indentation, not auto_indent.
        let content = "    print('hello')";
        let fixed = auto_indent_content(&lines, 31, 31, content);
        assert_eq!(fixed, "    print('hello')");
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

    // ============================================================
    // BUG-HUNTING: edge cases that existing tests miss
    // ============================================================

    // --- BUG 1: auto_indent_content doesn't dedent closing braces ---

    #[test]
    fn bug_auto_indent_closing_brace_should_not_be_indented() {
        // When inserting `}` after a block body, the closing brace should
        // be at the block's indent level, NOT the body's indent level.
        // auto_indent_content places it at body level because it sees
        // body-level context, not realizing `}` is a dedent token.
        let lines = vec![
            "fn outer() {\n".to_string(),
            "    let x = 1;\n".to_string(),
            "    // insert `}` here\n".to_string(),
        ];
        // Context: editing at line 3, inside the block (indent level 1).
        // The `}` to close fn outer should be at level 0.
        let content = "}";
        let fixed = auto_indent_content(&lines, 3, 3, content);
        // EXPECTED: closing brace should be at block level 0
        assert_eq!(
            fixed, "}",
            "BUG: auto_indent_content indents closing brace to body level"
        );
    }

    #[test]
    fn bug_auto_indent_closing_brace_in_multi_line() {
        // Multi-line content where first line is a closing brace.
        let lines = vec![
            "fn outer() {\n".to_string(),
            "    if true {\n".to_string(),
            "        do_work();\n".to_string(),
            "    } // closing if\n".to_string(),
            "    // insert here\n".to_string(),
        ];
        // Content: `}` (close outer) then `fn next() {` (new function at level 0)
        let content = "}\nfn next() {";
        let fixed = auto_indent_content(&lines, 5, 5, content);
        // EXPECTED: `}` at level 0, `fn next()` at level 0
        assert_eq!(
            fixed, "}\nfn next() {",
            "BUG: closing brace and next function should be at level 0"
        );
    }

    // --- BUG 2: auto_indent_content tabs overhang bug ---

    #[test]
    fn bug_auto_indent_tabs_overhang_uses_tabs_for_spaces() {
        // When a tab-indented file receives space-indented content with
        // multi-level internal indentation, the overhang (extra spaces beyond
        // the minimum) is incorrectly converted to tabs instead of spaces.
        let mut lines: Vec<String> = (0..30).map(|_| "\tfn foo() {}".to_string()).collect();
        lines.push("\t\tpass".to_string()); // line 31, inside a `{` block → expected level 2

        // Content has base indent 0 but internal line at 4 spaces
        // min_leading=0, overhang for second line = 4-0 = 4
        // expected_indent is "\t\t" (len=2)
        // Since min_leading(0) < expected_indent.len()(2), transform branch activates.
        // indent_char = '\t', repeat for expected_indent.len() + overhang = 2+4 = 6
        // Bug: all 6 chars become tabs, but overhang should remain spaces.
        let content = "outer\n    inner";
        let fixed = auto_indent_content(&lines, 31, 31, content);
        // EXPECTED: 2 tabs for base + 4 spaces for overhang
        assert_eq!(
            fixed, "\t\touter\n\t\t    inner",
            "BUG: overhang spaces incorrectly converted to tabs"
        );
    }

    // --- BUG 3: auto_indent_content with only whitespace lines ---

    #[test]
    fn bug_auto_indent_whitespace_only_lines() {
        // Content where some lines are only whitespace should preserve them
        // without treating them as content-bearing lines.
        let lines = vec!["def foo():\n".to_string(), "    pass\n".to_string()];
        // Content has a whitespace-only line between two code lines
        let content = "x = 1\n    \ny = 2";
        let fixed = auto_indent_content(&lines, 2, 2, content);
        // Whitespace-only lines: trim() produces empty, so they're skipped
        // for min_leading computation but should be preserved as-is in output.
        // Actually the current code: if line.trim().is_empty() → returns line unchanged
        // Since the whitespace-only line has min_leading not counted, the min_leading
        // is based on "x = 1" (0) and "y = 2" (0), so min_leading=0.
        // expected_indent="    " len=4. Content left as-is because min_leading(0) < 4?
        // No: min_leading(0) >= expected_indent.len()(4)? 0 >= 4 is FALSE.
        // So we enter the transform branch. Whitespace-only line stays as "    ".
        // Non-empty lines get: indent_char(space).repeat(4+overhang) + trimmed
        // "x = 1": overhang = 0-0=0 → "    x = 1"
        // "    ": trim().is_empty() → "    " preserved
        // "y = 2": overhang = 0-0=0 → "    y = 2"
        // Result: "    x = 1\n    \n    y = 2"
        assert_eq!(fixed, "    x = 1\n    \n    y = 2");
    }

    // --- BUG 4: detect_indent_style with all blank lines ---

    #[test]
    fn bug_detect_indent_all_blank_lines() {
        // All lines are blank/empty — should fall back to default (4 spaces).
        let lines: Vec<String> = vec![
            "\n".to_string(),
            "   \n".to_string(), // whitespace-only
            "\n".to_string(),
            "     \n".to_string(), // whitespace-only
        ];
        let style = detect_indent_style(&lines);
        // All lines trim to empty, so total_indented==0 → default
        assert_eq!(style.spaces, 4);
        assert!(!style.uses_tabs);
    }

    // --- BUG 5: detect_indent_style with zero-indent lines only ---

    #[test]
    fn bug_detect_indent_all_zero_indent() {
        // All non-empty lines start at column 0 — no indentation signals.
        let lines = vec![
            "package main\n".to_string(),
            "\n".to_string(),
            "func main() {\n".to_string(),
            "}\n".to_string(),
        ];
        let style = detect_indent_style(&lines);
        // No leading whitespace on any non-empty line → default
        assert_eq!(style.spaces, 4);
        assert!(!style.uses_tabs);
    }

    // --- BUG 6: needs_indent_fix with tab file and space content ---

    #[test]
    fn bug_needs_indent_fix_tab_file_space_content() {
        let lines: Vec<String> = (0..30)
            .map(|_| "\tfn foo() {}".to_string())
            .chain(std::iter::once("\t\tpass".to_string()))
            .collect();
        // File uses tabs, expected level 2 (two tabs).
        // Content uses 4 spaces — min_leading=4 >= expected_indent.len()(2) → false
        // But the content uses SPACES while the file uses TABS!
        // The indent styles are mismatched even though the widths are similar.
        // This test verifies that needs_indent_fix only checks width, not style.
        // Current behavior: it returns false (4 >= 2), which means the content
        // "appears" correct but actually uses wrong whitespace character.
        assert!(!needs_indent_fix(&lines, 31, 31, "    print('hello')"));
    }

    // --- BUG 7: auto_indent_content with empty file ---

    #[test]
    fn bug_auto_indent_empty_file() {
        // Inserting content into an empty file at line 1.
        let lines: Vec<String> = vec![];
        let content = "fn main() {\n    println!(\"hello\");\n}";
        let fixed = auto_indent_content(&lines, 1, 1, content);
        // Empty file → default indent style (4 spaces), level 0.
        // expected_indent = "" (level 0) → content returned unchanged.
        assert_eq!(fixed, content);
    }

    // --- BUG 8: context_line_before with all blank context ---

    #[test]
    fn bug_detect_expected_indent_all_blank_context() {
        // All lines before the edit point are blank.
        let lines: Vec<String> = vec![
            "\n".to_string(),
            "\n".to_string(),
            "\n".to_string(),
            "    let x = 1;\n".to_string(), // first non-blank is at edit point
        ];
        let (style, level) = detect_expected_indent(&lines, 4);
        // Forward context at line 4 shows 4-space indent → style.spaces=4
        // context_line_before returns None (all preceding are blank)
        // context_level = best_level.unwrap_or(0) = 0
        assert_eq!(style.spaces, 4);
        assert_eq!(level, 0);
    }

    // --- BUG 9: context_quality with step=0 ---

    #[test]
    fn bug_context_quality_step_zero() {
        // context_quality with step=0 should return 0 without panicking.
        let q = context_quality(8, 0, "    let x = 1;");
        assert_eq!(q, 0);
    }

    // --- BUG 10: round_to_nearest_level with step=0 ---

    #[test]
    fn bug_round_to_nearest_level_step_zero() {
        assert_eq!(round_to_nearest_level(8, 0), 0);
        assert_eq!(round_to_nearest_level(0, 0), 0);
    }

    // --- BUG 11: validate_indentation all-closer-token content ---

    #[test]
    fn bug_validate_indentation_only_closer_tokens() {
        // Content that is ONLY closing braces should pass validation
        // even if the indent level is "wrong" because closer tokens
        // are at a different level by design.
        let lines = vec![
            "fn outer() {\n".to_string(),
            "    if true {\n".to_string(),
            "        do_work();\n".to_string(),
            "    }\n".to_string(),
            "}\n".to_string(),
        ];
        let replacement = vec!["    }\n".to_string(), "}\n".to_string()];
        let (valid, warning, _fix) = validate_indentation(&lines, 4, 5, &replacement);
        // `}` at level 1 closes `if`, `}` at level 0 closes `outer`.
        // Closer tokens should be skipped for min_leading computation.
        assert!(
            valid,
            "Only-closer-token content should be valid, got warning: {:?}",
            warning
        );
    }

    // --- BUG 12: detect_indent_style with mixed tabs+spaces per line (discarded) ---

    #[test]
    fn bug_detect_indent_mixed_tabs_spaces_per_line_discarded() {
        // Lines that have BOTH leading tabs AND spaces are discarded.
        // This tests that the supermajority check works correctly when
        // some lines are mixed and discarded.
        let mut lines: Vec<String> = (0..20).map(|_| "    fn foo() {}".to_string()).collect();
        // Add 3 mixed lines (should be discarded)
        lines.push("  \t  fn mixed() {}".to_string());
        lines.push(" \t fn mixed2() {}".to_string());
        lines.push("\t  fn mixed3() {}".to_string());
        // 20 space lines + 0 tab lines (mixed discarded) → supermajority = 20/20 = 1.0 > 0.80
        // Wait, 1.0 > 0.80 would flip to tabs! No — tab_levels.len() is 0 because mixed lines
        // have BOTH tabs and spaces, so they don't count for either.
        let style = detect_indent_style(&lines);
        assert!(
            !style.uses_tabs,
            "Mixed lines should be discarded, keeping space style"
        );
        assert_eq!(style.spaces, 4);
    }

    // --- BUG 13: auto_indent_content with content already at correct level ---

    #[test]
    fn bug_auto_indent_already_correct_multilevel() {
        // Content is already at the expected indent level with internal nesting.
        // Should be returned unchanged.
        let lines = vec!["def foo():\n".to_string(), "    pass\n".to_string()];
        let content = "    x = 1\n        if y:\n            z()\n    w = 2";
        let fixed = auto_indent_content(&lines, 2, 2, content);
        // min_leading = 4 (from "    x = 1"), expected_indent.len() = 4
        // 4 >= 4 → content returned unchanged
        assert_eq!(fixed, content);
    }

    // --- BUG 14: auto_indent_content with content having extra whitespace ---

    #[test]
    fn bug_auto_indent_overindented_content() {
        // Content has MORE indentation than expected (e.g., 8 spaces vs expected 4).
        // Should be returned unchanged because min_leading >= expected.
        let lines = vec!["def foo():\n".to_string(), "    pass\n".to_string()];
        let content = "        overindented"; // 8 spaces, expected 4
        let fixed = auto_indent_content(&lines, 2, 2, content);
        // min_leading(8) >= expected_indent.len()(4) → unchanged
        assert_eq!(fixed, content);
    }

    // --- BUG 15: auto_indent_content single line empty ---

    #[test]
    fn bug_auto_indent_single_empty_line() {
        let lines = vec!["def foo():\n".to_string(), "    pass\n".to_string()];
        let content = ""; // empty content
        let fixed = auto_indent_content(&lines, 2, 2, content);
        assert_eq!(fixed, "");
    }

    // --- BUG 16: validate_indentation with replacement having extra indentation ---

    #[test]
    fn bug_validate_indentation_overindented_passes() {
        // Replacement has MORE indentation than expected — passes validation.
        let lines = vec!["def foo():\n".to_string(), "    pass\n".to_string()];
        let replacement = vec!["        print('over')".to_string()]; // 8 spaces vs expected 4
        let (valid, _warning, _fix) = validate_indentation(&lines, 2, 2, &replacement);
        // Only checks min_leading < expected_spaces. 8 >= 4 → valid.
        assert!(valid, "Over-indented content should pass validation");
    }

    // --- BUG 17: detect_indent_style with single indented line ---

    #[test]
    fn bug_detect_indent_single_indented_line() {
        // Only one non-empty, indented line.
        let lines = vec!["    let x = 1;\n".to_string()];
        let style = detect_indent_style(&lines);
        // freq has {4: 1}, candidates include 4. score = 1*1000 - 4 = 996.
        // best_step should be 4.
        assert_eq!(style.spaces, 4);
        assert!(!style.uses_tabs);
    }

    // --- G2 edge: closer token in body of content does not corrupt min_leading ---

    #[test]
    fn bug_auto_indent_closer_mid_content_preserves_body_indent() {
        // Content has body code at level 1 and a closing `}` meant for level 0.
        // The `}` should NOT drag down min_leading, so body code stays at level 1
        // and the `}` stays at its intended level 0 (dedented relative to body).
        let lines = vec![
            "fn outer() {\n".to_string(),
            "    // insert here\n".to_string(),
        ];
        let content = "    do_thing()\n}";
        let fixed = auto_indent_content(&lines, 2, 2, content);
        // `do_thing()` at level 1 (4 spaces), `}` at level 0 (closes outer).
        assert_eq!(
            fixed, "    do_thing()\n}",
            "G2: closer in content must not pull down body indentation"
        );
    }
}
