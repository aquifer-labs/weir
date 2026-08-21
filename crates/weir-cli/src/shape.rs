// SPDX-License-Identifier: Apache-2.0
//! Deterministic rules that bound a tool call's output at the source.
//!
//! Every rule is conservative by construction: it either returns a strictly
//! narrower request, or it returns nothing. A rule that is unsure does nothing —
//! a missed saving is cheap, a broken tool call is not.

use serde_json::{json, Map, Value};

/// What a rule decided, and why. The reason is written into the shadow log and,
/// once gating is live, into the receipt.
pub struct Shaped {
    pub input: Value,
    pub rule: &'static str,
    pub note: String,
}

/// Does this SQL already bound its own result?
///
/// Deliberately crude: we only look for the keyword. A query that mentions LIMIT
/// anywhere — including inside a CTE or a comment — is left alone. Being fooled
/// into *not* acting is harmless; being fooled into rewriting a query that was
/// already bounded is not.
fn sql_is_bounded(sql: &str) -> bool {
    let upper = sql.to_uppercase();
    upper.contains("LIMIT") || upper.contains("FETCH FIRST") || upper.contains("TOP ")
}

/// Only plain SELECTs are safe to append a LIMIT to. Anything that writes, or
/// that we cannot confidently classify, is out of scope.
fn sql_is_plain_select(sql: &str) -> bool {
    let t = sql.trim_start().to_uppercase();
    (t.starts_with("SELECT") || t.starts_with("WITH"))
        && !t.contains("INSERT ")
        && !t.contains("UPDATE ")
        && !t.contains("DELETE ")
        && !t.contains("CREATE ")
        && !t.contains("DROP ")
        && !t.contains("ALTER ")
        && !t.contains("TRUNCATE ")
}

fn obj(v: &Value) -> Option<&Map<String, Value>> {
    v.as_object()
}

/// Append a LIMIT to an unbounded SELECT.
fn rule_sql_limit(tool: &str, input: &Value, limit: u64) -> Option<Shaped> {
    if !tool.contains("execute_sql") && !tool.contains("execute_query") && !tool.contains("query") {
        return None;
    }
    let map = obj(input)?;
    // The parameter is spelled differently across MCP servers.
    let key = ["sql", "query", "statement"]
        .into_iter()
        .find(|k| map.get(*k).and_then(Value::as_str).is_some())?;
    let sql = map.get(key)?.as_str()?;

    if sql_is_bounded(sql) || !sql_is_plain_select(sql) {
        return None;
    }
    let trimmed = sql.trim_end().trim_end_matches(';');
    let mut out = map.clone();
    out.insert(key.to_string(), json!(format!("{trimmed}\nLIMIT {limit}")));
    Some(Shaped {
        input: Value::Object(out),
        rule: "sql_limit",
        note: format!("appended LIMIT {limit} to an unbounded SELECT"),
    })
}

/// Cap how many memories a recall returns. Large recall payloads are re-read on
/// every later turn, so the default is usually far more than the task needs.
fn rule_recall_limit(tool: &str, input: &Value, limit: u64) -> Option<Shaped> {
    if !tool.contains("memory_find") && !tool.contains("memory_context") {
        return None;
    }
    let map = obj(input)?;
    match map.get("limit").and_then(Value::as_u64) {
        Some(n) if n <= limit => None,
        _ => {
            let mut out = map.clone();
            let was = map
                .get("limit")
                .and_then(Value::as_u64)
                .map(|n| n.to_string())
                .unwrap_or_else(|| "default".into());
            out.insert("limit".into(), json!(limit));
            Some(Shaped {
                input: Value::Object(out),
                rule: "recall_limit",
                note: format!("limit {was} -> {limit}"),
            })
        }
    }
}

/// Bound the output of a shell command that has no bound of its own.
///
/// Only applies to a small allowlist of read-only commands whose output is
/// unbounded by nature. Anything with a pipe, redirect, or shell operator is
/// left alone: rewriting a compound command is how you silently corrupt it.
fn rule_shell_cap(tool: &str, input: &Value, bytes: u64) -> Option<Shaped> {
    if tool != "Bash" && tool != "shell" && tool != "exec_command" {
        return None;
    }
    let map = obj(input)?;
    let key = ["command", "cmd"]
        .into_iter()
        .find(|k| map.get(*k).and_then(Value::as_str).is_some())?;
    let cmd = map.get(key)?.as_str()?.trim();

    if cmd.contains('|') || cmd.contains('>') || cmd.contains(';') || cmd.contains("&&") {
        return None;
    }
    let head = cmd.split_whitespace().next()?;
    if !matches!(
        head,
        "cat" | "curl" | "env" | "printenv" | "dmesg" | "journalctl"
    ) {
        return None;
    }
    let mut out = map.clone();
    out.insert(key.to_string(), json!(format!("{cmd} | head -c {bytes}")));
    Some(Shaped {
        input: Value::Object(out),
        rule: "shell_cap",
        note: format!("bounded `{head}` output to {bytes} bytes"),
    })
}

pub struct Limits {
    pub sql_limit: u64,
    pub recall_limit: u64,
    pub shell_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            sql_limit: 1000,
            recall_limit: 5,
            shell_bytes: 4000,
        }
    }
}

/// Run every rule; the first that fires wins. Returns None when nothing applies,
/// which is the common and desired case.
pub fn shape(tool: &str, input: &Value, l: &Limits) -> Option<Shaped> {
    rule_sql_limit(tool, input, l.sql_limit)
        .or_else(|| rule_recall_limit(tool, input, l.recall_limit))
        .or_else(|| rule_shell_cap(tool, input, l.shell_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lim() -> Limits {
        Limits::default()
    }

    #[test]
    fn unbounded_select_gets_a_limit() {
        let i = json!({"query": "SELECT * FROM orders"});
        let s = shape("mcp__metabase__execute_sql", &i, &lim()).unwrap();
        assert_eq!(s.rule, "sql_limit");
        assert!(s.input["query"].as_str().unwrap().contains("LIMIT 1000"));
    }

    #[test]
    fn a_query_that_already_limits_is_left_alone() {
        let i = json!({"query": "SELECT * FROM orders LIMIT 10"});
        assert!(shape("mcp__metabase__execute_sql", &i, &lim()).is_none());
    }

    #[test]
    fn writes_are_never_rewritten() {
        for sql in [
            "DELETE FROM orders",
            "UPDATE orders SET x=1",
            "DROP TABLE orders",
        ] {
            let i = json!({ "query": sql });
            assert!(
                shape("mcp__metabase__execute_sql", &i, &lim()).is_none(),
                "{sql}"
            );
        }
    }

    #[test]
    fn recall_is_capped_but_a_smaller_request_is_respected() {
        let big = json!({"query": "x", "limit": 25});
        assert_eq!(
            shape("mcp__artesian__memory_find", &big, &lim())
                .unwrap()
                .rule,
            "recall_limit"
        );
        let small = json!({"query": "x", "limit": 3});
        assert!(shape("mcp__artesian__memory_find", &small, &lim()).is_none());
    }

    #[test]
    fn compound_shell_commands_are_never_touched() {
        for cmd in [
            "cat a | grep b",
            "cat a > b",
            "cat a && cat b",
            "cat a; cat b",
        ] {
            let i = json!({ "command": cmd });
            assert!(shape("Bash", &i, &lim()).is_none(), "{cmd}");
        }
    }

    #[test]
    fn a_bare_cat_is_bounded() {
        let i = json!({"command": "cat /var/log/system.log"});
        let s = shape("Bash", &i, &lim()).unwrap();
        assert!(s.input["command"]
            .as_str()
            .unwrap()
            .ends_with("| head -c 4000"));
    }

    #[test]
    fn commands_outside_the_allowlist_are_left_alone() {
        let i = json!({"command": "ls -la /tmp"});
        assert!(shape("Bash", &i, &lim()).is_none());
    }
}
