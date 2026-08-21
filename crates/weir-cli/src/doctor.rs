// SPDX-License-Identifier: Apache-2.0
//! Report whether the pieces are in step.
//!
//! weir is delivered as two halves that update independently: the plugin (hook
//! manifests, pulled by each harness from the marketplace) and the binary
//! (installed with cargo or brew). Upgrading one and not the other is the
//! ordinary mistake, not an exotic one, and it fails quietly — the hooks keep
//! firing, they just do the wrong thing or nothing. This is where you find out.

use anyhow::Result;
use clap::Args;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Args)]
pub struct DoctorArgs {}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn mark(ok: bool) -> &'static str {
    if ok { "ok  " } else { "WARN" }
}

/// Version recorded in an installed plugin manifest, if one is there.
fn installed_plugin_version(root: &Path) -> Option<String> {
    for name in [
        ".claude-plugin/plugin.json",
        ".codex-plugin/plugin.json",
        "plugin.json",
    ] {
        if let Ok(text) = std::fs::read_to_string(root.join(name))
            && let Ok(v) = serde_json::from_str::<Value>(&text)
            && let Some(s) = v.get("version").and_then(Value::as_str)
        {
            return Some(s.to_string());
        }
    }
    None
}

/// Newest versioned directory under a plugin cache root.
fn newest_child(dir: &Path) -> Option<PathBuf> {
    let mut kids: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    kids.sort();
    kids.pop()
}

pub fn run(_args: DoctorArgs) -> Result<()> {
    let binary = env!("CARGO_PKG_VERSION");
    println!("\nweir {binary}\n");

    println!("Plugin");
    let mut any_plugin = false;
    for (label, root) in [
        (
            "Claude Code",
            home().join(".claude/plugins/cache/weir/weir"),
        ),
        ("Codex", home().join(".codex/plugins/cache/weir/weir")),
    ] {
        match newest_child(&root)
            .as_deref()
            .and_then(installed_plugin_version)
        {
            Some(v) => {
                any_plugin = true;
                let same = v == binary;
                println!("  [{}] {label}: plugin {v}", mark(same));
                if !same {
                    println!(
                        "         binary is {binary}. Update both: `cargo install --path crates/weir-cli` \
                         and re-run the marketplace upgrade for this harness."
                    );
                }
            }
            None => println!("  [    ] {label}: plugin not installed"),
        }
    }
    if !any_plugin {
        println!("         Install with: codex plugin marketplace add aquifer-labs/weir");
    }

    println!("\nHooks");
    let cc = home().join(".claude/settings.json");
    let cc_wired = std::fs::read_to_string(&cc)
        .map(|s| s.contains("weir"))
        .unwrap_or(false);
    println!(
        "  [{}] Claude Code: {}",
        mark(cc_wired),
        if cc_wired {
            "registered"
        } else {
            "not registered - run `weir init`"
        }
    );

    let cx = home().join(".codex/config.toml");
    let cx_text = std::fs::read_to_string(&cx).unwrap_or_default();
    let cx_trusted = cx_text
        .lines()
        .any(|l| l.starts_with("[hooks.state.") && l.contains("weir@"));
    println!(
        "  [{}] Codex: {}",
        mark(cx_trusted),
        if cx_trusted {
            "trusted"
        } else {
            "not trusted - start `codex` once interactively and approve them"
        }
    );

    println!("\nConfig");
    let cfg = crate::config::load(std::env::current_dir().ok().as_deref());
    let user_cfg = home().join(".config/weir/weir.toml");
    println!(
        "  [    ] user config: {}",
        if user_cfg.exists() {
            user_cfg.display().to_string()
        } else {
            "none (using defaults)".into()
        }
    );
    println!(
        "  [    ] effective: sql_limit={} recall_limit={} gate_budget={} deny_sql={} deny_bash={}",
        cfg.shape.sql_limit,
        cfg.shape.recall_limit,
        cfg.gate.budget_tokens,
        cfg.policy.deny_sql.len(),
        cfg.policy.deny_bash.len()
    );

    println!("\nShadow log");
    let log = home().join(".weir/shadow.jsonl");
    match std::fs::read_to_string(&log) {
        Ok(s) => {
            let n = s.lines().filter(|l| !l.trim().is_empty()).count();
            println!("  [ok  ] {n} entries — run `weir shadow` to see what would have changed");
        }
        Err(_) => println!("  [    ] empty — nothing recorded yet"),
    }
    println!();
    Ok(())
}
