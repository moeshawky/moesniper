//! Diff generation for sniper dry-run mode.
//!
//! Generates unified diff-style output showing actual changes.

/// Generates a preview showing before/after for dry-run.
pub fn generate_preview(
    old_lines: &[String],
    new_lines: &[String],
    start: usize,
    end: usize,
) -> Vec<String> {
    let mut preview = Vec::new();

    // Show the range being edited
    preview.push(format!("--- original (lines {}-{})", start, end));
    preview.push(format!("+++ modified ({} lines)", new_lines.len()));
    preview.push(String::new());

    // Context before (up to 3 lines)
    let ctx_before = start.saturating_sub(3);
    for line in old_lines.iter().take(start - 1).skip(ctx_before) {
        preview.push(format!(" {} | {}", ctx_before + 1, line.trim_end_matches('\n')));
    }

    // Separator
    if ctx_before < start - 1 {
        preview.push(" ...".to_string());
    }

    // Old content (marked for removal)
    let splice_start = start - 1;
    let splice_end = end.min(old_lines.len());
    for (idx, line) in old_lines.iter().enumerate().take(splice_end).skip(splice_start) {
        preview.push(format!("-{}| {}", idx + 1, line.trim_end_matches('\n')));
    }

    // New content (marked for addition)
    for (i, line) in new_lines.iter().enumerate() {
        let trimmed = line.trim_end_matches('\n');
        preview.push(format!("+{}| {}", start + i, trimmed));
    }

    // Context after (up to 3 lines)
    if splice_end < old_lines.len() {
        let ctx_after = (splice_end + 3).min(old_lines.len());
        if splice_end < ctx_after {
            preview.push(" ...".to_string());
        }
        for line in old_lines.iter().take(ctx_after).skip(splice_end) {
            preview.push(format!(" {} | {}", splice_end + 1, line.trim_end_matches('\n')));
        }
    }

    preview
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_lines(text: &str) -> Vec<String> {
        text.split_inclusive('\n').map(String::from).collect()
    }

    #[test]
    fn test_generate_preview_simple() {
        let old = make_lines("line1\nline2\nline3\nline4\nline5\n");
        let new = vec!["new2\n".to_string()];

        let preview = generate_preview(&old, &new, 2, 2);

        assert!(preview.iter().any(|l| l.contains("--- original")));
        assert!(preview.iter().any(|l| l.contains("+++ modified")));
        assert!(preview.iter().any(|l| l.starts_with('-') && l.contains("line2")));
        assert!(preview.iter().any(|l| l.starts_with('+') && l.contains("new2")));
    }
}
