//! The lines that differ between two versions of config.toml, as `-` / `+` hunks with a little context, for `config set|unset|remove --dry-run`.

/// Lines around a change kept on each side of it.
const CONTEXT: usize = 2;

/// The lines that differ between two texts, `-` and `+` with up to
/// [`CONTEXT`] unchanged lines around them (` `), each hunk after an
/// `@@ -line,count +line,count @@` line.
pub fn line_diff(before: &str, after: &str) -> Vec<String> {
    let old: Vec<&str> = before.lines().collect();
    let new: Vec<&str> = after.lines().collect();
    let (n, m) = (old.len(), new.len());
    // common[i][j]: the longest common subsequence of old[i..] and new[j..].
    let mut common = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            common[i][j] = if old[i] == new[j] {
                common[i + 1][j + 1] + 1
            } else {
                common[i + 1][j].max(common[i][j + 1])
            };
        }
    }
    let mut ops: Vec<(char, &str)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && old[i] == new[j] {
            ops.push((' ', old[i]));
            i += 1;
            j += 1;
        } else if i < n && (j == m || common[i + 1][j] >= common[i][j + 1]) {
            ops.push(('-', old[i]));
            i += 1;
        } else {
            ops.push(('+', new[j]));
            j += 1;
        }
    }
    let changed: Vec<usize> = (0..ops.len()).filter(|&k| ops[k].0 != ' ').collect();
    let near = |k: usize| changed.iter().any(|&c| c.abs_diff(k) <= CONTEXT);
    let mut out = Vec::new();
    let (mut old_line, mut new_line) = (1, 1);
    let mut k = 0;
    while k < ops.len() {
        if !near(k) {
            old_line += 1;
            new_line += 1;
            k += 1;
            continue;
        }
        let mut end = k;
        while end < ops.len() && near(end) {
            end += 1;
        }
        let hunk = &ops[k..end];
        let removed = hunk.iter().filter(|op| op.0 != '+').count();
        let added = hunk.iter().filter(|op| op.0 != '-').count();
        out.push(format!("@@ -{old_line},{removed} +{new_line},{added} @@"));
        out.extend(hunk.iter().map(|(sign, line)| format!("{sign}{line}")));
        old_line += removed;
        new_line += added;
        k = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diff(before: &[&str], after: &[&str]) -> Vec<String> {
        line_diff(&before.join("\n"), &after.join("\n"))
    }

    #[test]
    fn identical_texts_have_no_hunk() {
        assert!(diff(&["a", "b"], &["a", "b"]).is_empty());
    }

    /// Changes more than twice the context apart are two hunks, each with
    /// its own line numbers; lines added at the end count from the end.
    #[test]
    fn distant_changes_are_separate_hunks_and_an_addition_at_the_end_is_one() {
        let before = ["1", "2", "3", "4", "5", "6", "7", "8", "9"];
        let after = ["1", "two", "3", "4", "5", "6", "7", "8", "9", "10", "11"];
        assert_eq!(
            diff(&before, &after),
            [
                "@@ -1,4 +1,4 @@",
                " 1",
                "-2",
                "+two",
                " 3",
                " 4",
                "@@ -8,2 +8,4 @@",
                " 8",
                " 9",
                "+10",
                "+11",
            ]
        );
    }

    #[test]
    fn a_removal_at_the_start_keeps_the_context_after_it() {
        assert_eq!(
            diff(&["x", "a", "b", "c", "d"], &["a", "b", "c", "d"]),
            ["@@ -1,3 +1,2 @@", "-x", " a", " b"]
        );
    }
}
