use crate::template;
use indexmap::IndexMap;
use serde::Deserialize;
use serde_yaml_ng::Value;
use std::{
    collections::{HashMap, HashSet},
    fmt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Guard against pathological / cyclic include graphs.
const MAX_INCLUDE_DEPTH: usize = 16;

/// Guard against cyclic `ref` variables (`A: {ref: .B}`, `B: {ref: .A}`).
const MAX_REF_DEPTH: usize = 16;

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
    /// Global `vars`, kept raw; only used to resolve `enum: { ref: ... }`.
    #[serde(default)]
    pub vars: IndexMap<String, Value>,
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
    /// Task-level `vars`, kept raw; only used to resolve `enum: { ref: ... }`.
    pub vars: IndexMap<String, Value>,
}

/// A variable a task requires before it can run (`requires.vars`).
#[derive(Debug, Default, Clone)]
pub struct RequiredVar {
    pub name: String,
    /// Allowed values when the variable is constrained by an `enum`.
    /// Empty means any value is accepted (free-form input).
    pub enum_values: Vec<String>,
    /// Template expression from `enum: { ref: ... }`. Resolved into
    /// `enum_values` while loading; left empty (free-form) if it can't be.
    pub enum_ref: Option<String>,
}

impl Taskfile {
    /// Find the nearest Taskfile at or above `dir`, parse it, and recursively
    /// merge any `includes`. Tasks from an include are namespaced by the
    /// include key (e.g. `docs:build`), matching `task`'s own behaviour.
    ///
    /// Like `task`, this walks up parent directories, so fzftask works from any
    /// subdirectory of a project whose Taskfile lives at the repo root.
    pub fn load_from_dir(dir: &Path) -> Result<Self, LoadError> {
        let path = find_taskfile_upwards(dir).ok_or(LoadError::NotFound)?;

        let mut tasks = IndexMap::new();
        let mut visited = HashSet::new();
        // Like `task`, commands (and so `sh` vars) run from the root
        // Taskfile's directory unless an include sets its own `dir`.
        let work_dir = path.parent().unwrap_or_else(|| Path::new("."));
        let version = merge_file(&path, work_dir, "", &[], &mut tasks, &mut visited, 0)?;

        Ok(Taskfile {
            version,
            includes: IndexMap::new(),
            vars: IndexMap::new(),
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

/// Search `start` and each of its ancestor directories for a Taskfile, using
/// the first one found (nearest wins).
fn find_taskfile_upwards(start: &Path) -> Option<PathBuf> {
    start.ancestors().find_map(find_taskfile)
}

/// Parse the Taskfile at `path`, add its tasks (prefixed with `prefix`, minus
/// any in `excludes`) to `tasks`, then recurse into its includes. Returns the
/// file's own `version`.
fn merge_file(
    path: &Path,
    work_dir: &Path,
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

    let mut resolver = VarResolver {
        globals: &parsed.vars,
        work_dir,
        cache: HashMap::new(),
    };
    for (name, mut def) in parsed.tasks {
        // `internal: true` tasks are not meant to be invoked directly; hide them.
        if def.internal || excludes.iter().any(|e| e == &name) {
            continue;
        }
        resolver.resolve_enum_refs(&mut def);
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
        let nested_work_dir = match &include.dir {
            Some(dir) => base.join(dir),
            None => work_dir.to_path_buf(),
        };
        match merge_file(
            &include_path,
            &nested_work_dir,
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

/// Resolves `enum: { ref: ... }` against one Taskfile's variables.
struct VarResolver<'a> {
    globals: &'a IndexMap<String, Value>,
    /// Directory `sh` vars run in.
    work_dir: &'a Path,
    /// Evaluated global vars, so each `sh` runs at most once per file.
    cache: HashMap<String, Option<Value>>,
}

impl VarResolver<'_> {
    fn resolve_enum_refs(&mut self, def: &mut TaskDef) {
        for var in &mut def.requires {
            if let Some(expr) = &var.enum_ref {
                var.enum_values = self
                    .eval(expr, &def.vars, 0)
                    .and_then(template::into_strings)
                    .unwrap_or_default();
            }
        }
    }

    fn eval(
        &mut self,
        expr: &str,
        task_vars: &IndexMap<String, Value>,
        depth: usize,
    ) -> Option<Value> {
        if depth > MAX_REF_DEPTH {
            return None;
        }
        template::eval(expr, &mut |name| self.lookup(name, task_vars, depth))
    }

    /// Task vars shadow global vars, which shadow the environment.
    fn lookup(
        &mut self,
        name: &str,
        task_vars: &IndexMap<String, Value>,
        depth: usize,
    ) -> Option<Value> {
        if let Some(raw) = task_vars.get(name) {
            return self.var_value(raw, task_vars, depth);
        }
        if let Some(raw) = self.globals.get(name) {
            if let Some(cached) = self.cache.get(name) {
                return cached.clone();
            }
            // Global vars can't see task vars.
            let value = self.var_value(raw, &IndexMap::new(), depth);
            self.cache.insert(name.to_string(), value.clone());
            return value;
        }
        std::env::var(name).ok().map(Value::String)
    }

    /// The value of a var definition: a literal, or `{sh|ref|value|map: ...}`.
    fn var_value(
        &mut self,
        raw: &Value,
        task_vars: &IndexMap<String, Value>,
        depth: usize,
    ) -> Option<Value> {
        let Value::Mapping(map) = raw else {
            return Some(raw.clone());
        };
        if let Some(cmd) = map.get("sh").and_then(Value::as_str) {
            run_sh(cmd, self.work_dir).map(Value::String)
        } else if let Some(expr) = map.get("ref").and_then(Value::as_str) {
            self.eval(expr, task_vars, depth + 1)
        } else {
            map.get("value").or_else(|| map.get("map")).cloned()
        }
    }
}

/// Run a `sh` var like `task` does: stdout with the trailing newline trimmed.
fn run_sh(cmd: &str, dir: &Path) -> Option<String> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let trimmed = stdout.strip_suffix('\n').unwrap_or(&stdout);
    Some(trimmed.strip_suffix('\r').unwrap_or(trimmed).to_string())
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
            LoadError::NotFound => {
                write!(f, "no Taskfile found in this directory or any parent")
            }
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
                #[serde(default)]
                vars: IndexMap<String, Value>,
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
                enum_spec: Option<EnumSpec>,
            },
        }

        /// An `enum`: a literal list, or `{ ref: <template expression> }`.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum EnumSpec {
            List(Vec<String>),
            Ref {
                #[serde(rename = "ref")]
                expr: String,
            },
            // Catch-all so an unexpected shape falls back to free-form input
            // instead of failing the whole task. Must be last.
            Other(serde::de::IgnoredAny),
        }

        impl VarSpec {
            fn into_required(self) -> RequiredVar {
                match self {
                    VarSpec::Name(name) => RequiredVar {
                        name,
                        ..Default::default()
                    },
                    VarSpec::Detailed { name, enum_spec } => {
                        let (enum_values, enum_ref) = match enum_spec {
                            Some(EnumSpec::List(values)) => (values, None),
                            Some(EnumSpec::Ref { expr }) => (Vec::new(), Some(expr)),
                            Some(EnumSpec::Other(_)) | None => (Vec::new(), None),
                        };
                        RequiredVar {
                            name,
                            enum_values,
                            enum_ref,
                        }
                    }
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
                vars,
            } => TaskDef {
                desc,
                summary,
                cmds: cmds.into_iter().filter_map(Cmd::into_string).collect(),
                requires: requires
                    .map(|r| r.vars.into_iter().map(VarSpec::into_required).collect())
                    .unwrap_or_default(),
                aliases,
                internal,
                vars,
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

    // Mirrors the "Using variable references for enum values" examples in the
    // Task docs: a global list var, a `sh` var read as JSON, and `sh` output
    // split into lines. `sh` runs in the Taskfile's directory.
    #[test]
    fn resolves_enum_refs_from_vars() {
        let root = std::env::temp_dir().join(format!("fzftask-enumref-{}", std::process::id()));
        std::fs::create_dir_all(root.join("services/api")).unwrap();
        std::fs::create_dir_all(root.join("services/web")).unwrap();
        std::fs::write(
            root.join("config.json"),
            r#"{"allowed_environments": ["dev", "prod"]}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("Taskfile.yml"),
            r#"
version: '3'
vars:
  ALLOWED_ENVS: [dev, staging, prod]
  CONFIG:
    sh: cat config.json
  AVAILABLE_SERVICES:
    sh: ls services/
tasks:
  list:
    requires:
      vars:
        - name: ENV
          enum:
            ref: .ALLOWED_ENVS
  json:
    requires:
      vars:
        - name: ENV
          enum:
            ref: ( .CONFIG | fromJson ).allowed_environments
  lines:
    requires:
      vars:
        - name: SERVICE
          enum:
            ref: .AVAILABLE_SERVICES | splitLines | compact
  task-var:
    vars:
      ALLOWED_ENVS: [qa]
    requires:
      vars:
        - name: ENV
          enum:
            ref: .ALLOWED_ENVS
  broken:
    requires:
      vars:
        - name: ENV
          enum:
            ref: .NOPE_NOT_DEFINED_ANYWHERE
"#,
        )
        .unwrap();

        let tf = Taskfile::load_from_dir(&root).unwrap();
        let enums = |task: &str| tf.tasks[task].requires[0].enum_values.clone();

        assert_eq!(enums("list"), ["dev", "staging", "prod"]);
        assert_eq!(enums("json"), ["dev", "prod"]);
        assert_eq!(enums("lines"), ["api", "web"]);
        // Task-level vars shadow global ones.
        assert_eq!(enums("task-var"), ["qa"]);
        // An unresolvable ref degrades to free-form input, not a load error.
        assert!(enums("broken").is_empty());

        std::fs::remove_dir_all(&root).ok();
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
    fn finds_taskfile_in_parent_directory() {
        // Taskfile at the "repo root", invoked from a nested subdirectory.
        let root = std::env::temp_dir().join(format!("fzftask-up-{}", std::process::id()));
        let nested = root.join("a/b/c");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            root.join("Taskfile.yml"),
            "version: '3'\ntasks:\n  build:\n    cmds: [cargo build]\n",
        )
        .unwrap();

        let tf = Taskfile::load_from_dir(&nested).unwrap();
        assert!(tf.tasks.contains_key("build"));

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
