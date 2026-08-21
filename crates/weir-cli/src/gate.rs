// SPDX-License-Identifier: Apache-2.0
//! Bound a tool result that already came back too large.
//!
//! `shape` prevents bloat where the size is predictable from the request. Gate
//! handles the rest: a shell command can return 50 tokens or 7,000 and there is
//! no way to know beforehand. Here we can look at the real output and decide.
//!
//! Measured on real logs, the 5–20k token band carries the largest share of
//! context pressure — bigger than the tiny results and bigger than the rare huge
//! ones. That band is what this targets.

/// Nothing that looks like a failure is ever trimmed. A truncated stack trace
/// costs far more than the tokens it saves, because the agent then re-runs the
/// command to see the part we cut.
const ERROR_MARKERS: &[&str] = &[
    "Traceback (most recent call last)",
    "panicked at",
    "error[E",
    "Segmentation fault",
    "FAILED",
    "AssertionError",
    "fatal:",
];

pub struct Trimmed {
    pub text: String,
    pub before_tokens: u64,
    pub after_tokens: u64,
}

fn tokens_of(s: &str) -> u64 {
    (s.len() / 4) as u64
}

/// True if this output should pass through untouched regardless of size.
fn must_pass_whole(s: &str) -> bool {
    ERROR_MARKERS.iter().any(|m| s.contains(m))
}

/// Keep the head and the tail, drop the middle, and say so.
///
/// The head matters because it carries the command, headers, or schema; the tail
/// because it carries the result or the last error. The middle is where bulk log
/// lines live. The marker is not decoration — it tells the agent the output is
/// incomplete so it can narrow the command instead of reasoning from a fragment
/// it believes is whole.
pub fn trim(
    text: &str,
    budget_tokens: u64,
    head_lines: usize,
    tail_lines: usize,
) -> Option<Trimmed> {
    let before = tokens_of(text);
    if before <= budget_tokens || must_pass_whole(text) {
        return None;
    }
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= head_lines + tail_lines + 1 {
        return None;
    }
    let cut = lines.len() - head_lines - tail_lines;
    let mut out = String::new();
    for l in &lines[..head_lines] {
        out.push_str(l);
        out.push('\n');
    }
    out.push_str(&format!(
        "\n[weir: {cut} lines cut from the middle, {before} tokens total. \
         Re-run with a narrower command or a filter to see them.]\n\n"
    ));
    for l in &lines[lines.len() - tail_lines..] {
        out.push_str(l);
        out.push('\n');
    }
    let after = tokens_of(&out);
    // Refuse to "save" nothing. Short-line output can trim to no gain at all.
    if after >= before {
        return None;
    }
    Some(Trimmed {
        text: out,
        before_tokens: before,
        after_tokens: after,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(n: usize) -> String {
        (0..n)
            .map(|i| format!("2026-08-21 line {i} some payload here"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_long_log_is_trimmed_and_says_so() {
        let t = trim(&log(2000), 500, 40, 40).unwrap();
        assert!(t.after_tokens < t.before_tokens / 4);
        assert!(t.text.contains("lines cut from the middle"));
        assert!(t.text.starts_with("2026-08-21 line 0"));
        assert!(t.text.trim_end().ends_with("line 1999 some payload here"));
    }

    #[test]
    fn output_under_budget_is_left_alone() {
        assert!(trim(&log(5), 500, 40, 40).is_none());
    }

    #[test]
    fn failures_pass_through_whole_however_long() {
        // The whole point: never cut the thing the agent needs to diagnose.
        for marker in [
            "Traceback (most recent call last)",
            "panicked at",
            "error[E0433]",
        ] {
            let text = format!("{}\n{}", log(2000), marker);
            assert!(trim(&text, 500, 40, 40).is_none(), "{marker} was trimmed");
        }
    }

    #[test]
    fn trimming_that_would_not_save_anything_is_refused() {
        // Few lines, each enormous: head+tail keeps nearly everything, so the
        // marker would make the result larger than the original.
        let text = vec!["x".repeat(20_000); 3].join("\n");
        assert!(trim(&text, 100, 40, 40).is_none());
    }
}
