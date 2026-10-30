//! A tiny evaluator for the Go-template expressions Task accepts in
//! `requires.vars[].enum.ref` (e.g. `.ALLOWED`, `( .CONFIG | fromJson ).envs`,
//! `.SERVICES | splitLines | compact`).
//!
//! Only the subset needed to turn a variable into a list of enum candidates is
//! supported: variable references with field access, parentheses, pipelines,
//! a prefix call (`fromJson .CONFIG`), and a handful of list/string functions.
//! Anything else evaluates to `None` so the caller can fall back gracefully.

use serde_yaml_ng::Value;

/// Evaluate `expr`, resolving `.NAME` references through `lookup`.
pub fn eval(expr: &str, lookup: &mut dyn FnMut(&str) -> Option<Value>) -> Option<Value> {
    let mut stages = split_pipeline(expr)?.into_iter();
    let mut value = eval_command(stages.next()?.trim(), lookup)?;
    for stage in stages {
        value = apply(stage.trim(), value)?;
    }
    Some(value)
}

/// Convert an evaluated value into enum candidates. Only lists qualify; scalar
/// items are stringified and anything nested is skipped.
pub fn into_strings(value: Value) -> Option<Vec<String>> {
    let Value::Sequence(items) = value else {
        return None;
    };
    Some(items.iter().filter_map(scalar_string).collect())
}

fn scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Split on `|` outside parentheses and quotes.
fn split_pipeline(expr: &str) -> Option<Vec<&str>> {
    let mut stages = Vec::new();
    let (mut depth, mut in_quote, mut start) = (0usize, false, 0);
    for (i, c) in expr.char_indices() {
        match c {
            '"' => in_quote = !in_quote,
            '(' if !in_quote => depth += 1,
            ')' if !in_quote => depth = depth.checked_sub(1)?,
            '|' if !in_quote && depth == 0 => {
                stages.push(&expr[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if depth != 0 || in_quote {
        return None;
    }
    stages.push(&expr[start..]);
    Some(stages)
}

/// The first pipeline stage: a term, or a prefix call such as `fromJson .X`.
fn eval_command(cmd: &str, lookup: &mut dyn FnMut(&str) -> Option<Value>) -> Option<Value> {
    if cmd.starts_with(|c: char| c.is_ascii_alphabetic()) {
        let (name, arg) = cmd.split_once(char::is_whitespace)?;
        let arg = eval_term(arg.trim(), lookup)?;
        return apply(name, arg);
    }
    eval_term(cmd, lookup)
}

/// `.NAME.field...` or `( pipeline ).field...`.
fn eval_term(term: &str, lookup: &mut dyn FnMut(&str) -> Option<Value>) -> Option<Value> {
    let (mut value, fields) = if let Some(inner) = term.strip_prefix('(') {
        let close = matching_paren(inner)?;
        (eval(&inner[..close], lookup)?, inner[close + 1..].trim())
    } else {
        let path = term.strip_prefix('.')?;
        let (name, fields) = path.split_at(path.find('.').unwrap_or(path.len()));
        if !is_ident(name) {
            return None;
        }
        (lookup(name)?, fields)
    };
    if !fields.is_empty() {
        for field in fields.strip_prefix('.')?.split('.') {
            if !is_ident(field) {
                return None;
            }
            value = value.get(field)?.clone();
        }
    }
    Some(value)
}

/// Index of the `)` closing an already-consumed `(` in `s`.
fn matching_paren(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => return Some(i),
            ')' => depth -= 1,
            _ => {}
        }
    }
    None
}

fn is_ident(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Apply a single-argument template function.
fn apply(name: &str, value: Value) -> Option<Value> {
    match (name, value) {
        ("splitLines", Value::String(s)) => Some(Value::Sequence(
            s.split('\n')
                .map(|l| Value::String(l.trim_end_matches('\r').to_string()))
                .collect(),
        )),
        ("trim", Value::String(s)) => Some(Value::String(s.trim().to_string())),
        // JSON is a subset of YAML, so the YAML parser reads it too.
        ("fromJson", Value::String(s)) => serde_yaml_ng::from_str(&s).ok(),
        ("compact", Value::Sequence(items)) => Some(Value::Sequence(
            items
                .into_iter()
                .filter(|v| !matches!(v, Value::Null) && v.as_str() != Some(""))
                .collect(),
        )),
        ("uniq", Value::Sequence(items)) => {
            let mut out: Vec<Value> = Vec::new();
            for v in items {
                if !out.contains(&v) {
                    out.push(v);
                }
            }
            Some(Value::Sequence(out))
        }
        ("sortAlpha", Value::Sequence(items)) => {
            let mut strings: Vec<String> = items.iter().filter_map(scalar_string).collect();
            strings.sort();
            Some(Value::Sequence(strings.into_iter().map(Value::String).collect()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(name: &str) -> Option<Value> {
        let yaml = match name {
            "ALLOWED" => "[dev, staging, prod]",
            "CONFIG" => r#"'{"allowed_environments": ["dev", "prod"], "n": 1}'"#,
            "LINES" => "\"api\\n\\nweb\\nworker\\n\"",
            _ => return None,
        };
        serde_yaml_ng::from_str(yaml).ok()
    }

    fn eval_list(expr: &str) -> Option<Vec<String>> {
        eval(expr, &mut vars).and_then(into_strings)
    }

    #[test]
    fn plain_reference() {
        assert_eq!(eval_list(".ALLOWED").unwrap(), ["dev", "staging", "prod"]);
    }

    #[test]
    fn parenthesised_from_json_with_field() {
        assert_eq!(
            eval_list("( .CONFIG | fromJson ).allowed_environments").unwrap(),
            ["dev", "prod"]
        );
        // Prefix-call form of the same expression.
        assert_eq!(
            eval_list("(fromJson .CONFIG).allowed_environments").unwrap(),
            ["dev", "prod"]
        );
    }

    #[test]
    fn split_lines_and_compact() {
        assert_eq!(
            eval_list(".LINES | splitLines | compact").unwrap(),
            ["api", "web", "worker"]
        );
    }

    #[test]
    fn unsupported_or_non_list_is_none() {
        assert!(eval_list(".MISSING").is_none());
        assert!(eval_list(".CONFIG").is_none()); // a string, not a list
        assert!(eval_list(".ALLOWED | unknownFn").is_none());
        assert!(eval_list("( .ALLOWED").is_none());
    }
}
