// SPDX-License-Identifier: Apache-2.0
//! Configuration, resolved from four layers.
//!
//! `builtin defaults -> named profile -> user config -> project config`
//!
//! Later layers override earlier ones field by field, so a project that only
//! wants a different SQL limit says just that and inherits the rest. This is the
//! portability story: a team shares a profile file, each person keeps their own
//! global preferences, and a repository can still tighten one knob.

use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub shape: Shape,
    pub gate: Gate,
    pub policy: Policy,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub sql_limit: u64,
    pub recall_limit: u64,
    pub shell_cap_bytes: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Gate {
    pub budget_tokens: u64,
    pub head_lines: usize,
    pub tail_lines: usize,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Policy {
    /// Substrings that make a shell command refuse to run.
    pub deny_bash: Vec<String>,
    /// SQL leading keywords that are refused outright.
    pub deny_sql: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            shape: Shape {
                sql_limit: 1000,
                recall_limit: 5,
                shell_cap_bytes: 4000,
            },
            gate: Gate {
                budget_tokens: 5000,
                head_lines: 40,
                tail_lines: 40,
            },
            policy: Policy::default(),
        }
    }
}

/// The on-disk shape. Everything optional, because a layer that mentions one
/// field must not silently reset the others to their defaults.
#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    profile: Option<String>,
    shape: Option<ShapeFile>,
    gate: Option<GateFile>,
    policy: Option<PolicyFile>,
}

#[derive(Debug, Default, Deserialize)]
struct ShapeFile {
    sql_limit: Option<u64>,
    recall_limit: Option<u64>,
    shell_cap_bytes: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct GateFile {
    budget_tokens: Option<u64>,
    head_lines: Option<usize>,
    tail_lines: Option<usize>,
}

#[derive(Debug, Default, Deserialize)]
struct PolicyFile {
    deny_bash: Option<Vec<String>>,
    deny_sql: Option<Vec<String>>,
}

impl FileConfig {
    fn apply_to(&self, c: &mut Config) {
        if let Some(s) = &self.shape {
            if let Some(v) = s.sql_limit {
                c.shape.sql_limit = v;
            }
            if let Some(v) = s.recall_limit {
                c.shape.recall_limit = v;
            }
            if let Some(v) = s.shell_cap_bytes {
                c.shape.shell_cap_bytes = v;
            }
        }
        if let Some(g) = &self.gate {
            if let Some(v) = g.budget_tokens {
                c.gate.budget_tokens = v;
            }
            if let Some(v) = g.head_lines {
                c.gate.head_lines = v;
            }
            if let Some(v) = g.tail_lines {
                c.gate.tail_lines = v;
            }
        }
        if let Some(p) = &self.policy {
            if let Some(v) = &p.deny_bash {
                c.policy.deny_bash = v.clone();
            }
            if let Some(v) = &p.deny_sql {
                c.policy.deny_sql = v.clone();
            }
        }
    }
}

fn parse(path: &Path) -> Option<FileConfig> {
    // A malformed config must not take the agent down with it. Weir's whole
    // contract is that a broken rule degrades to no effect.
    let text = std::fs::read_to_string(path).ok()?;
    match toml::from_str::<FileConfig>(&text) {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("weir: ignoring {}: {e}", path.display());
            None
        }
    }
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// Where a named profile lives. Profiles ship with the repo and can also be
/// dropped into the user's config directory.
fn profile_path(name: &str) -> Option<PathBuf> {
    let candidates = [
        home().join(format!(".config/weir/profiles/{name}.toml")),
        std::env::current_exe()
            .ok()
            .and_then(|p| {
                p.parent()
                    .map(|d| d.join(format!("../profiles/{name}.toml")))
            })
            .unwrap_or_default(),
    ];
    candidates.into_iter().find(|p| p.exists())
}

/// Resolve the four layers. `project_dir` is normally the agent's working
/// directory; `None` skips the project layer.
pub fn load(project_dir: Option<&Path>) -> Config {
    let mut cfg = Config::default();
    let user = home().join(".config/weir/weir.toml");
    let project = project_dir.map(|d| d.join("weir.toml"));

    // The profile named by either layer is applied first, so an explicit value
    // in the file that named it still wins.
    let named = [Some(user.clone()), project.clone()]
        .into_iter()
        .flatten()
        .filter(|p| p.exists())
        .filter_map(|p| parse(&p))
        .find_map(|f| f.profile);

    if let Some(name) = named
        && let Some(p) = profile_path(&name)
        && let Some(f) = parse(&p)
    {
        f.apply_to(&mut cfg);
    }
    for layer in [Some(user), project].into_iter().flatten() {
        if layer.exists()
            && let Some(f) = parse(&layer)
        {
            f.apply_to(&mut cfg);
        }
    }
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        p
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("weir-cfg-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_layer_overrides_only_what_it_mentions() {
        let d = tmp("partial");
        let p = write(&d, "weir.toml", "[shape]\nsql_limit = 50\n");
        let mut c = Config::default();
        parse(&p).unwrap().apply_to(&mut c);
        assert_eq!(c.shape.sql_limit, 50);
        // untouched fields keep their defaults - the whole point of Option
        assert_eq!(c.shape.recall_limit, Config::default().shape.recall_limit);
        assert_eq!(c.gate.budget_tokens, Config::default().gate.budget_tokens);
    }

    #[test]
    fn a_malformed_config_is_ignored_not_fatal() {
        let d = tmp("broken");
        let p = write(&d, "weir.toml", "[shape]\nsql_limit = \"not a number\"\n");
        assert!(parse(&p).is_none());
    }

    #[test]
    fn a_missing_file_is_simply_absent() {
        assert!(parse(&tmp("empty").join("nope.toml")).is_none());
    }

    #[test]
    fn policy_lists_replace_rather_than_append() {
        // Appending would make it impossible to loosen an inherited profile,
        // and a rule you cannot remove is a rule you will work around.
        let d = tmp("policy");
        let mut c = Config::default();
        c.policy.deny_bash = vec!["inherited".into()];
        let p = write(&d, "weir.toml", "[policy]\ndeny_bash = [\"mine\"]\n");
        parse(&p).unwrap().apply_to(&mut c);
        assert_eq!(c.policy.deny_bash, vec!["mine".to_string()]);
    }
}
