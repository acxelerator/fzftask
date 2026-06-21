use indexmap::IndexMap;
use serde::Deserialize;
use std::{fmt, path::Path};

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
    /// Task name -> definition. `IndexMap` preserves the order tasks appear
    /// in the file so the UI lists them the same way.
    #[serde(default)]
    pub tasks: IndexMap<String, TaskDef>,
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
    /// Find and parse the nearest Taskfile in `dir`.
    pub fn load_from_dir(dir: &Path) -> Result<Self, LoadError> {
        let path = TASKFILE_NAMES
            .iter()
            .map(|name| dir.join(name))
            .find(|p| p.is_file())
            .ok_or(LoadError::NotFound)?;

        let contents = std::fs::read_to_string(&path).map_err(LoadError::Io)?;
        serde_yaml_ng::from_str(&contents).map_err(LoadError::Parse)
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

        /// A command entry: either a plain string or a mapping with a `cmd` key.
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
        }

        impl Cmd {
            fn into_string(self) -> Option<String> {
                match self {
                    Cmd::Str(s) => Some(s),
                    Cmd::Map { cmd: Some(c), .. } => Some(c),
                    Cmd::Map { task: Some(t), .. } => Some(format!("task: {t}")),
                    Cmd::Map { .. } => None,
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
            } => TaskDef {
                desc,
                summary,
                cmds: cmds.into_iter().filter_map(Cmd::into_string).collect(),
                requires: requires
                    .map(|r| r.vars.into_iter().map(VarSpec::into_required).collect())
                    .unwrap_or_default(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
