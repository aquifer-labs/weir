// SPDX-License-Identifier: Apache-2.0
//! Install Weir's hooks into whichever harnesses are present.
//!
//! Idempotent and merge-preserving: existing hooks are never dropped, and running
//! `init` twice changes nothing. Every file is backed up before it is touched.

use anyhow::{Context, Result};
use clap::Args;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Args)]
pub struct InitArgs {
    /// Show what would change without writing anything
    #[arg(long)]
    dry_run: bool,
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn weir_bin() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "weir".into())
}

fn backup(path: &Path) -> Result<()> {
    if path.exists() {
        let bak = path.with_extension(format!(
            "pre-weir.{}.bak",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        ));
        std::fs::copy(path, &bak).with_context(|| format!("back up {}", path.display()))?;
        println!("  backed up -> {}", bak.display());
    }
    Ok(())
}

/// True if any hook in this event's list already runs the weir binary.
fn already_installed(events: &Value, event: &str) -> bool {
    events
        .get(event)
        .and_then(Value::as_array)
        .map(|matchers| {
            matchers.iter().any(|m| {
                m.get("hooks")
                    .and_then(Value::as_array)
                    .map(|hs| {
                        hs.iter().any(|h| {
                            h.get("command")
                                .and_then(Value::as_str)
                                .is_some_and(|c| c.contains("weir"))
                        })
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn install_claude(dry: bool) -> Result<bool> {
    let path = home().join(".claude/settings.json");
    if !path.exists() {
        return Ok(false);
    }
    println!("Claude Code: {}", path.display());

    let mut root: Value =
        serde_json::from_str(&std::fs::read_to_string(&path)?).unwrap_or_else(|_| json!({}));
    let hooks = root
        .as_object_mut()
        .context("settings.json is not an object")?
        .entry("hooks")
        .or_insert_with(|| json!({}));

    let bin = weir_bin();
    let mut changed = false;
    for event in ["PreToolUse", "PostToolUse"] {
        if already_installed(hooks, event) {
            println!("  {event}: already installed");
            continue;
        }
        let entry = json!({
            "hooks": [{
                "type": "command",
                "command": format!("{bin} hook"),
                "timeout": 5
            }]
        });
        hooks
            .as_object_mut()
            .context("hooks is not an object")?
            .entry(event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .context("hook event is not an array")?
            .push(entry);
        println!("  {event}: will install (shadow mode)");
        changed = true;
    }

    if changed && !dry {
        backup(&path)?;
        std::fs::write(&path, serde_json::to_string_pretty(&root)?)?;
        println!("  written");
    }
    Ok(changed)
}

/// Codex takes hooks only through a plugin, never from `config.toml`.
///
/// Hook tables written into `config.toml` parse cleanly and then do nothing —
/// verified by running a tool call and watching no hook fire. Codex loads hooks
/// from a plugin manifest and gates them behind persisted trust, which is what
/// the `trusted_hash` entries under `[hooks.state]` are for.
///
/// So we report status and hand over the two commands rather than editing
/// anything: installing a plugin is Codex's job, and the trust prompt has to be
/// answered by a human on the first interactive run.
fn install_codex(_dry: bool) -> Result<bool> {
    let cfg = home().join(".codex/config.toml");
    if !cfg.exists() {
        return Ok(false);
    }
    println!("Codex: {}", cfg.display());

    let installed = home().join(".codex/plugins/cache/weir").exists();
    if installed {
        println!("  plugin installed");
        // A trust entry is keyed like `[hooks.state."weir@weir:hooks/hooks.json:pre_tool_use:0:0"]`.
        // Looking for the two words separately gives a false positive, because
        // `hooks.state` and `weir` both appear in an untrusted config already.
        let trusted = std::fs::read_to_string(&cfg)
            .map(|c| {
                c.lines()
                    .any(|l| l.starts_with("[hooks.state.") && l.contains("weir@"))
            })
            .unwrap_or(false);
        if trusted {
            println!("  hooks trusted");
        } else {
            println!("  hooks NOT trusted yet — start `codex` once interactively and approve them");
            println!("  (until then Codex silently skips them)");
        }
    } else {
        println!("  plugin not installed. Run:");
        println!("    codex plugin marketplace add aquifer-labs/weir");
        println!("    codex plugin add weir --marketplace weir");
        println!("  then start `codex` once interactively to approve the hooks.");
    }
    Ok(false)
}

pub fn run(args: InitArgs) -> Result<()> {
    if args.dry_run {
        println!("(dry run — nothing will be written)\n");
    }
    let c = install_claude(args.dry_run)?;
    let x = install_codex(args.dry_run)?;
    if !c && !x {
        println!("\nNothing to do.");
    } else {
        println!(
            "\nHooks are in SHADOW mode: they record what they would change and \
             alter nothing.\nRun `weir shadow` after some real work to see the effect."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_an_existing_weir_hook_so_init_is_idempotent() {
        let events = json!({
            "PreToolUse": [{"hooks": [{"type": "command", "command": "/usr/local/bin/weir hook"}]}]
        });
        assert!(already_installed(&events, "PreToolUse"));
        assert!(!already_installed(&events, "PostToolUse"));
    }

    #[test]
    fn a_trust_entry_needs_our_plugin_key_not_just_the_word_weir() {
        let untrusted = "[hooks.state.\"ponytail@ponytail:hooks/h.json:session_start:0:0\"]\nmodel = \"weir-local\"\n";
        let trusted = "[hooks.state.\"weir@weir:hooks/hooks.json:pre_tool_use:0:0\"]\ntrusted_hash = \"sha256:x\"\n";
        let looks_trusted = |c: &str| {
            c.lines()
                .any(|l| l.starts_with("[hooks.state.") && l.contains("weir@"))
        };
        assert!(
            !looks_trusted(untrusted),
            "the word weir alone must not count"
        );
        assert!(looks_trusted(trusted));
    }

    #[test]
    fn other_peoples_hooks_are_not_mistaken_for_ours() {
        let events = json!({
            "PreToolUse": [{"hooks": [{"type": "command", "command": "node other.js"}]}]
        });
        assert!(!already_installed(&events, "PreToolUse"));
    }
}
