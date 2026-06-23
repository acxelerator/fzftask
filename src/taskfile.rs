use indexmap::IndexMap;
use serde::Deserialize;
use std::{
    collections::HashSet,
    fmt,
    path::{Path, PathBuf},
};

/// Guard against pathological / cyclic include graphs.
const MAX_INCLUDE_DEPTH: usize = 16;

/// Candidate file names for a Taskfile, in the order `task` itself searches.
const TASKFILE_NAMES: &[&str] = &[
    "Taskfile.yml",
    "Taskfile.yaml",
    "taskfile.yml",
    "taskfile.yaml",
];

/// A parsed Taskfile. Only the fields fzftask cares about are modeled;
/// unknown keys are ignored.
#[derive(Debug, Deserialize)]
pub struct Taskfile {
    #[serde(default)]
    #[allow(dead_code)] // parsed for completeness; not yet shown in the UI
    pub version: Option<String>,
    /// Other Taskfiles pulled in under `includes`. Their tasks are namespaced
    /// by the include key (e.g. `docs:build`).
    #[serde(default)]
    pub includes: IndexMap<String, Include>,
    /// Task name -> definition. `IndexMap` preserves the order tasks appear
    /// in the file so the UI lists them the same way.
    #[serde(default)]
    pub tasks: IndexMap<String, TaskDef>,
}

/// An entry under `includes`: either a bare path string or a mapping with a
/// `taskfile`/`dir`/`optional`/`internal`/`flatten`/`excludes`.
#[derive(Debug, Clone)]
pub struct Include {
    /// Path to the included Taskfile (relative to the including file).
    pub taskfile: Option<String>,
    /// Directory to look in when no explicit `taskfile` is given.
    pub dir: Option<String>,
    /// A missing optional include is silently skipped instead of erroring.
    pub optional: bool,
    /// `internal: true` hides every task pulled in by this include.
    pub internal: bool,
    /// `flatten: true` merges the included tasks without a namespace prefix.
    pub flatten: bool,
    /// Task names to drop when flattening.
    pub excludes: Vec<String>,
}

impl<'de> Deserialize<'de> for Include {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            /// `docs: ./docs/Taskfile.yml`
            Path(String),
            /// `docs: { taskfile: ..., dir: ..., optional: true, ... }`
            Detailed {
                #[serde(default)]
                taskfile: Option<String>,
                #[serde(default)]
                dir: Option<String>,
                #[serde(default)]
                optional: bool,
                #[serde(default)]
                internal: bool,
                #[serde(default)]
                flatten: bool,
                #[serde(default)]
                excludes: Vec<String>,
            },
        }

        Ok(match Raw::deserialize(deserializer)? {
            Raw::Path(taskfile) => Include {
                taskfile: Some(taskfile),
                dir: None,
                optional: false,
                internal: false,
                flatten: false,
                excludes: Vec::new(),
            },
            Raw::Detailed {
                taskfile,
                dir,
                optional,
                internal,
                flatten,
                excludes,
            } => Include {
                taskfile,
                dir,
                optional,
                internal,
                flatten,
                excludes,
            },
        })
    }
}

/// A single task definition.
///
/// A task value may be either a full mapping (`desc`, `cmds`, ...) or a bare
/// list of commands. `TaskDef`'s custom `Deserialize` handles both forms.
#[derive(Debug, Default)]
pub struct TaskDef {
    pub desc: Option<String>,
    pub summary: Option<String>,
    pub cmds: Vec<String>,
    /// Variables the task declares under `requires.vars`.
    pub requires: Vec<RequiredVar>,
    /// Alternative names the task can be matched/selected by (`aliases`).
    pub aliases: Vec<String>,
    /// `internal: true` tasks are callable only by other tasks; hide them.
    pub internal: bool,
}

/// A variable a task requires before it can run (`requires.vars`).
#[derive(Debug, Default, Clone)]
pub struct RequiredVar {
    pub name: String,
    /// Allowed values when the variable is constrained by an `enum`.
    /// Empty means any value is accepted (free-form input).
    pub enum_values: Vec<String>,
}

impl Taskfile {
    /// Find the nearest Taskfile in `dir`, parse it, and recursively merge any
    /// `includes`. Tasks from an include are namespaced by the include key
    /// (e.g. `docs:build`), matching `task`'s own behaviour.
    pub fn load_from_dir(dir: &Path) -> Result<Self, LoadError> {
        let path = find_taskfile(dir).ok_or(LoadError::NotFound)?;

        let mut tasks = IndexMap::new();
        let mut visited = HashSet::new();
        let version = merge_file(&path, "", &[], &mut tasks, &mut visited, 0)?;

        Ok(Taskfile {
            version,
            includes: IndexMap::new(),
            tasks,
        })
    }
}

/// Locate a Taskfile inside `dir` by trying the known file names in order.
fn find_taskfile(dir: &Path) -> Option<PathBuf> {
    TASKFILE_NAMES
        .iter()
        .map(|name| dir.join(name))
        .find(|p| p.is_file())
}

/// Parse the Taskfile at `path`, add its tasks (prefixed with `prefix`, minus
/// any in `excludes`) to `tasks`, then recurse into its includes. Returns the
/// file's own `version`.
fn merge_file(
    path: &Path,
    prefix: &str,
    excludes: &[String],
    tasks: &mut IndexMap<String, TaskDef>,
    visited: &mut HashSet<PathBuf>,
    depth: usize,
) -> Result<Option<String>, LoadError> {
    if depth > MAX_INCLUDE_DEPTH {
        return Ok(None);
    }
    // Skip files already processed to break include cycles.
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !visited.insert(canonical) {
        return Ok(None);
    }

    let contents = std::fs::read_to_string(path).map_err(LoadError::Io)?;
    let parsed: Taskfile = serde_yaml_ng::from_str(&contents).map_err(LoadError::Parse)?;

    for (name, mut def) in parsed.tasks {
        // `internal: true` tasks are not meant to be invoked directly; hide them.
        if def.internal || excludes.iter().any(|e| e == &name) {
            continue;
        }
        let key = if prefix.is_empty() {
            name
        } else {
            // Namespace the task and its aliases (e.g. `docs:build`, `docs:b`).
            def.aliases = def.aliases.iter().map(|a| format!("{prefix}{a}")).collect();
            format!("{prefix}{name}")
        };
        tasks.insert(key, def);
    }

    let base = path.parent().unwrap_or_else(|| Path::new("."));
    for (namespace, include) in &parsed.includes {
        // An internal include hides all of its tasks, so skip it entirely.
        if include.internal {
            continue;
        }
        let Some(include_path) = resolve_include(base, include) else {
            continue; // remote (http) or unresolved include
        };
        // `flatten` merges without adding a namespace segment; `excludes` then
        // drops the named tasks.
        let (nested_prefix, nested_excludes): (String, &[String]) = if include.flatten {
            (prefix.to_string(), &include.excludes)
        } else {
            (format!("{prefix}{namespace}:"), &[])
        };
        match merge_file(
            &include_path,
            &nested_prefix,
            nested_excludes,
            tasks,
            visited,
            depth + 1,
        ) {
            Ok(_) => {}
            // Missing includes are tolerated (optional or not) so one broken
            // include does not make the whole TUI fail to load.
            Err(LoadError::NotFound | LoadError::Io(_)) => {}
            // Propagate genuine parse errors only for required includes.
            Err(e) => {
                if !include.optional {
                    return Err(e);
                }
            }
        }
    }

    Ok(parsed.version)
}

/// Resolve the path to an included Taskfile, or `None` for remote/unresolvable
/// includes. A directory resolves to the Taskfile it contains.
fn resolve_include(base: &Path, include: &Include) -> Option<PathBuf> {
    let rel = include.taskfile.as_deref().or(include.dir.as_deref())?;

    // fzftask is a local tool; skip remote includes.
    if rel.starts_with("http://") || rel.starts_with("https://") {
        return None;
    }

    let joined = base.join(rel);
    if joined.is_dir() {
        find_taskfile(&joined)
    } else if joined.is_file() {
        Some(joined)
    } else {
        None
    }
}

/// Errors that can occur while locating and parsing a Taskfile.
#[derive(Debug)]
pub enum LoadError {
    NotFound,
    Io(std::io::Error),
    Parse(serde_yaml_ng::Error),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::NotFound => write!(f, "no Taskfile.yml found in the current directory"),
            LoadError::Io(e) => write!(f, "failed to read Taskfile: {e}"),
            LoadError::Parse(e) => write!(f, "failed to parse Taskfile: {e}"),
        }
    }
}

impl std::error::Error for LoadError {}

// --- custom deserialization for the flexible `TaskDef` shape ---

impl<'de> Deserialize<'de> for TaskDef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            /// `taskname: [cmd1, cmd2]`
            Cmds(Vec<Cmd>),
            /// `taskname: { desc: ..., cmds: [...] }`
            Full {
                #[serde(default)]
                desc: Option<String>,
                #[serde(default)]
                summary: Option<String>,
                #[serde(default)]
                cmds: Vec<Cmd>,
                #[serde(default)]
                requires: Option<Requires>,
                #[serde(default)]
                aliases: Vec<String>,
                #[serde(default)]
                internal: bool,
            },
        }

        /// The `requires:` block of a task.
        #[derive(Deserialize)]
        struct Requires {
            #[serde(default)]
            vars: Vec<VarSpec>,
        }

        /// An entry under `requires.vars`: either a bare name or a mapping with
        /// an optional `enum` constraint.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum VarSpec {
            Name(String),
            Detailed {
                name: String,
                #[serde(default, rename = "enum")]
                enum_values: Vec<String>,
            },
        }

        impl VarSpec {
            fn into_required(self) -> RequiredVar {
                match self {
                    VarSpec::Name(name) => RequiredVar {
                        name,
                        enum_values: Vec::new(),
                    },
                    VarSpec::Detailed { name, enum_values } => RequiredVar { name, enum_values },
                }
            }
        }

        /// A command entry: a plain string, a mapping with a `cmd`/`task` key,
        /// or anything else (e.g. a `null` produced by a comment-only list item
        /// such as `- # note`, or a `defer:`/`for:` form we don't display).
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Cmd {
            Str(String),
            Map {
                #[serde(default)]
                cmd: Option<String>,
                #[serde(default)]
                task: Option<String>,
            },
            // Catch-all so an unexpected shape skips the entry instead of
            // failing the whole task. Must be last (it matches anything).
            Other(serde::de::IgnoredAny),
        }

        impl Cmd {
            fn into_string(self) -> Option<String> {
                match self {
                    Cmd::Str(s) => Some(s),
                    Cmd::Map { cmd: Some(c), .. } => Some(c),
                    Cmd::Map { task: Some(t), .. } => Some(format!("task: {t}")),
                    Cmd::Map { .. } | Cmd::Other(_) => None,
                }
            }
        }

        let raw = Raw::deserialize(deserializer)?;
        Ok(match raw {
            Raw::Cmds(cmds) => TaskDef {
                cmds: cmds.into_iter().filter_map(Cmd::into_string).collect(),
                ..Default::default()
            },
            Raw::Full {
                desc,
                summary,
                cmds,
                requires,
                aliases,
                internal,
            } => TaskDef {
                desc,
                summary,
                cmds: cmds.into_iter().filter_map(Cmd::into_string).collect(),
                requires: requires
                    .map(|r| r.vars.into_iter().map(VarSpec::into_required).collect())
                    .unwrap_or_default(),
                aliases,
                internal,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Regression test modelled on real-world Taskfiles (e.g. paak-develop/raund):
    // a comment-only command list item like `- # note` is parsed by YAML as a
    // `null`, and tasks carry many fields fzftask does not model (dir, silent,
    // internal, status, vars/sh, ...). None of these should fail the parse.
    #[test]
    fn tolerates_null_commands_and_unmodelled_fields() {
        let yaml = r#"
version: '3'
vars:
  ENV:
    sh: echo dev
tasks:
  ci-all:
    desc: run all CI checks
    dir: frontend/src-v2
    silent: true
    internal: false
    status:
      - test -f marker
    cmds:
      - # FORMAT
      - npm run format-check
      - # LINT
      - npm run lint
      - task: setup
      - cmd: echo done
"#;
        let tf: Taskfile = serde_yaml_ng::from_str(yaml).unwrap();
        let ci = &tf.tasks["ci-all"];

        assert_eq!(ci.desc.as_deref(), Some("run all CI checks"));
        // null (comment-only) entries are dropped; real commands keep order.
        assert_eq!(
            ci.cmds,
            ["npm run format-check", "npm run lint", "task: setup", "echo done"]
        );
    }

    #[test]
    fn parses_full_and_shorthand_tasks() {
        let yaml = r#"
version: '3'
tasks:
  build:
    desc: Build it
    cmds:
      - cargo build
  quick: [echo hi, echo bye]
  composite:
    summary: Runs other tasks
    cmds:
      - cmd: echo start
      - task: build
"#;
        let tf: Taskfile = serde_yaml_ng::from_str(yaml).unwrap();

        assert_eq!(tf.version.as_deref(), Some("3"));
        // Order is preserved.
        let names: Vec<_> = tf.tasks.keys().cloned().collect();
        assert_eq!(names, ["build", "quick", "composite"]);

        let build = &tf.tasks["build"];
        assert_eq!(build.desc.as_deref(), Some("Build it"));
        assert_eq!(build.cmds, ["cargo build"]);

        assert_eq!(tf.tasks["quick"].cmds, ["echo hi", "echo bye"]);

        let composite = &tf.tasks["composite"];
        assert_eq!(composite.summary.as_deref(), Some("Runs other tasks"));
        assert_eq!(composite.cmds, ["echo start", "task: build"]);
    }

    #[test]
    fn parses_requires_with_and_without_enum() {
        let yaml = r#"
version: '3'
tasks:
  deploy:
    desc: Deploy the app
    requires:
      vars:
        - NAME
        - name: ENV
          enum: [dev, staging, prod]
    cmds:
      - echo deploy
"#;
        let tf: Taskfile = serde_yaml_ng::from_str(yaml).unwrap();
        let deploy = &tf.tasks["deploy"];

        assert_eq!(deploy.requires.len(), 2);

        // Bare name -> free-form (no enum).
        assert_eq!(deploy.requires[0].name, "NAME");
        assert!(deploy.requires[0].enum_values.is_empty());

        // Detailed name with enum candidates.
        assert_eq!(deploy.requires[1].name, "ENV");
        assert_eq!(deploy.requires[1].enum_values, ["dev", "staging", "prod"]);
    }

    #[test]
    fn merges_includes_with_namespaced_tasks() {
        // Build a throwaway directory tree:
        //   root/Taskfile.yml          (build; includes docs + sub)
        //   root/docs/Taskfile.yml     (serve)
        //   root/nested/Taskfile.yml   (deploy; includes inner)
        //   root/nested/inner.yml      (run)
        let root = std::env::temp_dir().join(format!("fzftask-inc-{}", std::process::id()));
        let docs = root.join("docs");
        let nested = root.join("nested");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::create_dir_all(&nested).unwrap();

        std::fs::write(
            root.join("Taskfile.yml"),
            "version: '3'\n\
             includes:\n  \
               docs: ./docs\n  \
               sub:\n    taskfile: ./nested/Taskfile.yml\n\
             tasks:\n  build:\n    cmds: [cargo build]\n",
        )
        .unwrap();
        std::fs::write(
            docs.join("Taskfile.yml"),
            "version: '3'\ntasks:\n  serve:\n    cmds: [mkdocs serve]\n",
        )
        .unwrap();
        std::fs::write(
            nested.join("Taskfile.yml"),
            "version: '3'\n\
             includes:\n  inner: ./inner.yml\n\
             tasks:\n  deploy:\n    cmds: [echo deploy]\n",
        )
        .unwrap();
        std::fs::write(
            nested.join("inner.yml"),
            "version: '3'\ntasks:\n  run:\n    cmds: [echo run]\n",
        )
        .unwrap();

        let tf = Taskfile::load_from_dir(&root).unwrap();
        let names: Vec<&str> = tf.tasks.keys().map(|s| s.as_str()).collect();

        // Root task, directory include, file include, and a nested include.
        assert!(names.contains(&"build"));
        assert!(names.contains(&"docs:serve"));
        assert!(names.contains(&"sub:deploy"));
        assert!(names.contains(&"sub:inner:run"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn optional_missing_include_is_skipped() {
        let root = std::env::temp_dir().join(format!("fzftask-opt-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("Taskfile.yml"),
            "version: '3'\n\
             includes:\n  \
               missing:\n    taskfile: ./nope/Taskfile.yml\n    optional: true\n\
             tasks:\n  build:\n    cmds: [cargo build]\n",
        )
        .unwrap();

        let tf = Taskfile::load_from_dir(&root).unwrap();
        assert!(tf.tasks.contains_key("build"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn internal_tasks_are_hidden() {
        let root = std::env::temp_dir().join(format!("fzftask-int-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("Taskfile.yml"),
            "version: '3'\n\
             tasks:\n  \
               build:\n    cmds: [cargo build]\n  \
               _setup:\n    internal: true\n    cmds: [echo setup]\n",
        )
        .unwrap();

        let tf = Taskfile::load_from_dir(&root).unwrap();
        assert!(tf.tasks.contains_key("build"));
        assert!(!tf.tasks.contains_key("_setup"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn parses_task_aliases_and_namespaces_them() {
        let root = std::env::temp_dir().join(format!("fzftask-alias-{}", std::process::id()));
        let docs = root.join("docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(
            root.join("Taskfile.yml"),
            "version: '3'\n\
             includes:\n  docs: ./docs\n\
             tasks:\n  \
               build:\n    aliases: [b, bld]\n    cmds: [cargo build]\n",
        )
        .unwrap();
        std::fs::write(
            docs.join("Taskfile.yml"),
            "version: '3'\ntasks:\n  serve:\n    aliases: [s]\n    cmds: [mkdocs serve]\n",
        )
        .unwrap();

        let tf = Taskfile::load_from_dir(&root).unwrap();
        assert_eq!(tf.tasks["build"].aliases, ["b", "bld"]);
        // Included task aliases get the namespace prefix too.
        assert_eq!(tf.tasks["docs:serve"].aliases, ["docs:s"]);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn internal_include_is_hidden_and_flatten_excludes_work() {
        let root = std::env::temp_dir().join(format!("fzftask-incadv-{}", std::process::id()));
        let secret = root.join("secret");
        let lib = root.join("lib");
        std::fs::create_dir_all(&secret).unwrap();
        std::fs::create_dir_all(&lib).unwrap();

        std::fs::write(
            root.join("Taskfile.yml"),
            "version: '3'\n\
             includes:\n  \
               secret:\n    taskfile: ./secret/Taskfile.yml\n    internal: true\n  \
               lib:\n    taskfile: ./lib/Taskfile.yml\n    flatten: true\n    excludes: [helper]\n\
             tasks:\n  build:\n    cmds: [cargo build]\n",
        )
        .unwrap();
        std::fs::write(
            secret.join("Taskfile.yml"),
            "version: '3'\ntasks:\n  deploy:\n    cmds: [echo deploy]\n",
        )
        .unwrap();
        std::fs::write(
            lib.join("Taskfile.yml"),
            "version: '3'\ntasks:\n  \
               format:\n    cmds: [echo fmt]\n  \
               helper:\n    cmds: [echo helper]\n",
        )
        .unwrap();

        let tf = Taskfile::load_from_dir(&root).unwrap();
        let names: Vec<&str> = tf.tasks.keys().map(|s| s.as_str()).collect();

        assert!(names.contains(&"build"));
        // internal include: none of its tasks appear (no `secret:deploy`).
        assert!(!names.iter().any(|n| n.starts_with("secret:")));
        // flatten: no namespace prefix...
        assert!(names.contains(&"format"));
        assert!(!names.contains(&"lib:format"));
        // ...and excluded tasks are dropped.
        assert!(!names.contains(&"helper"));

        std::fs::remove_dir_all(&root).ok();
    }
}
