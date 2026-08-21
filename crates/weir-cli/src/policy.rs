// SPDX-License-Identifier: Apache-2.0
//! Refuse calls that a domain has decided are never acceptable.
//!
//! This is safety, not saving. It does not reduce context and is not counted
//! towards any token figure.
//!
//! **A hook is not a security boundary.** It sees what the agent asked for
//! literally; the same command wrapped in a script, a variable, or base64 sails
//! straight past. Read-only database credentials and filesystem permissions are
//! what actually protect data. Treat these rules as guard rails that catch the
//! obvious slip, and never as a sandbox.

use serde_json::Value;

pub struct Denial {
    pub rule: &'static str,
    pub reason: String,
}

/// First leading keyword of a statement, uppercased, comments skipped.
fn leading_keyword(sql: &str) -> Option<String> {
    for line in sql.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with("--") {
            continue;
        }
        return l
            .split_whitespace()
            .next()
            .map(|w| w.trim_matches(|c: char| !c.is_alphabetic()).to_uppercase());
    }
    None
}

pub fn check(
    tool: &str,
    input: &Value,
    deny_bash: &[String],
    deny_sql: &[String],
) -> Option<Denial> {
    let map = input.as_object()?;

    if matches!(tool, "Bash" | "shell" | "exec_command") {
        let cmd = ["command", "cmd"]
            .into_iter()
            .find_map(|k| map.get(k).and_then(Value::as_str))?;
        if let Some(hit) = deny_bash.iter().find(|p| cmd.contains(p.as_str())) {
            return Some(Denial {
                rule: "deny_bash",
                reason: format!("`{hit}` is denied by the active weir profile"),
            });
        }
        return None;
    }

    if tool.contains("execute_sql") || tool.contains("execute_query") || tool.contains("query") {
        let sql = ["sql", "query", "statement"]
            .into_iter()
            .find_map(|k| map.get(k).and_then(Value::as_str))?;
        let kw = leading_keyword(sql)?;
        if deny_sql.iter().any(|d| d.eq_ignore_ascii_case(&kw)) {
            return Some(Denial {
                rule: "deny_sql",
                reason: format!(
                    "{kw} is denied by the active weir profile. \
                     This is a guard rail, not a boundary - use a read-only role for real protection."
                ),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sqls() -> Vec<String> {
        ["DROP", "TRUNCATE", "DELETE", "UPDATE", "INSERT", "ALTER"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn destructive_sql_is_refused() {
        for sql in ["DELETE FROM orders", "  drop table x", "TRUNCATE t"] {
            let d = check(
                "mcp__metabase__execute_sql",
                &json!({ "query": sql }),
                &[],
                &sqls(),
            );
            assert!(d.is_some(), "{sql} slipped through");
        }
    }

    #[test]
    fn reads_are_allowed() {
        assert!(
            check(
                "mcp__metabase__execute_sql",
                &json!({"query": "SELECT 1"}),
                &[],
                &sqls()
            )
            .is_none()
        );
    }

    #[test]
    fn a_leading_comment_does_not_hide_the_verb() {
        let sql = "-- housekeeping\n-- approved by nobody\nDELETE FROM orders";
        assert!(
            check(
                "mcp__metabase__execute_sql",
                &json!({ "query": sql }),
                &[],
                &sqls()
            )
            .is_some()
        );
    }

    #[test]
    fn a_table_named_like_a_verb_is_not_mistaken_for_one() {
        // `deleted_orders` starts with the letters of DELETE; keyword matching
        // must be on the whole word, not a prefix.
        let sql = "SELECT * FROM deleted_orders";
        assert!(
            check(
                "mcp__metabase__execute_sql",
                &json!({ "query": sql }),
                &[],
                &sqls()
            )
            .is_none()
        );
    }

    #[test]
    fn denied_shell_substrings_are_refused() {
        let deny = vec!["rm -rf".to_string(), "mkfs".to_string()];
        assert!(check("Bash", &json!({"command": "rm -rf /tmp/x"}), &deny, &[]).is_some());
        assert!(check("Bash", &json!({"command": "ls -la"}), &deny, &[]).is_none());
    }

    #[test]
    fn an_empty_deny_list_denies_nothing() {
        assert!(check("Bash", &json!({"command": "rm -rf /"}), &[], &[]).is_none());
    }
}
