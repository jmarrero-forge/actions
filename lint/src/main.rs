//! Denies runner labels in workflows' `runs-on`.
//!
//! actionlint checks that a label exists and zizmor has no audit for
//! labels, so this is the one place that says which labels are refused:
//! `ubuntu-latest`, which moves to a new Ubuntu under a repository without
//! a change of its own, plus whatever the caller adds in
//! RUNNER_LABELS_DENY. Every other label, including self-hosted ones, is
//! allowed. It catches a workflow written with the wrong label, not one
//! built to evade it: a label assembled by `format()`, read from a
//! variable at run time or passed along a chain of `needs` jobs is out of
//! its reach.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use yaml_rust2::{Yaml, YamlLoader};

/// Always denied; the caller's list only adds to it.
const DEFAULT_DENY: &str = "ubuntu-latest";

fn main() -> ExitCode {
    let deny = deny_list(&std::env::var("RUNNER_LABELS_DENY").unwrap_or_default());
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let args = if args.is_empty() {
        vec![PathBuf::from(".github/workflows")]
    } else {
        args
    };
    let files = match workflow_files(&args) {
        Ok(files) => files,
        Err(e) => {
            eprintln!("runner-labels: {e}");
            return ExitCode::from(2);
        }
    };
    let mut failed = false;
    for file in &files {
        let source = match std::fs::read_to_string(file) {
            Ok(source) => source,
            Err(e) => {
                eprintln!("{}: {e}", file.display());
                return ExitCode::from(2);
            }
        };
        match check(&source, &deny) {
            Ok(findings) => {
                for finding in findings {
                    println!("{}: {finding}", file.display());
                    failed = true;
                }
            }
            Err(e) => {
                eprintln!("{}: {e}", file.display());
                return ExitCode::from(2);
            }
        }
    }
    if failed {
        println!("denied runner labels: {}", deny.join(", "));
        ExitCode::FAILURE
    } else {
        println!("runner-labels: {} workflow(s) checked", files.len());
        ExitCode::SUCCESS
    }
}

/// The default label plus the caller's, separated by whitespace or commas.
/// Labels are case-insensitive, as GitHub matches them.
fn deny_list(extra: &str) -> Vec<String> {
    let mut deny = vec![DEFAULT_DENY.to_string()];
    for label in extra.split(|c: char| c.is_whitespace() || c == ',') {
        let label = label.to_ascii_lowercase();
        if !label.is_empty() && !deny.contains(&label) {
            deny.push(label);
        }
    }
    deny
}

/// A directory stands for its `.yml` and `.yaml` files, in any case, as
/// workflows-check takes them (GitHub reads no
/// subdirectories); a file is taken as it is. A missing path is an error,
/// so a wrong path cannot pass by checking nothing.
fn workflow_files(args: &[PathBuf]) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for arg in args {
        if arg.is_dir() {
            let mut found = Vec::new();
            for entry in std::fs::read_dir(arg)? {
                let path = entry?.path();
                let yaml = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(runner_labels::has_workflow_extension);
                if yaml && path.is_file() {
                    found.push(path);
                }
            }
            found.sort();
            files.extend(found);
        } else if arg.is_file() {
            files.push(arg.clone());
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{}: no such file or directory", Path::new(arg).display()),
            ));
        }
    }
    Ok(files)
}

/// One message per job whose runs-on can resolve to a denied label.
fn check(source: &str, deny: &[String]) -> Result<Vec<String>, String> {
    let docs = YamlLoader::load_from_str(source).map_err(|e| e.to_string())?;
    let mut findings = Vec::new();
    for doc in &docs {
        let Some(jobs) = doc["jobs"].as_hash() else {
            continue;
        };
        for (id, job) in jobs {
            let id = id.as_str().unwrap_or("?");
            let mut text = String::new();
            strings(&job["runs-on"], &mut text);
            let mut hits = denied(&text, deny);
            // A job calling a reusable workflow has no runs-on of its own;
            // what it passes in `with` is there to become the called
            // workflow's, so it is followed like runs-on.
            let mut with = String::new();
            strings(&job["with"], &mut with);
            for label in denied(&with, deny) {
                hits.push(format!("{label} (passed in with)"));
            }
            text.push_str(&with);
            // A label can come from the matrix or an input. Any mention of
            // the matrix takes in every matrix value but those of `exclude`,
            // since an include can add a key the expression names; that
            // refuses an unrelated key's denied label too.
            let matrix = &job["strategy"]["matrix"];
            if text.to_ascii_lowercase().contains("matrix") {
                let mut values = String::new();
                match matrix.as_hash() {
                    Some(keys) => {
                        for (key, value) in keys {
                            if key.as_str() != Some("exclude") {
                                strings(value, &mut values);
                            }
                        }
                    }
                    // A whole matrix in one expression.
                    None => strings(matrix, &mut values),
                }
                for label in denied(&values, deny) {
                    hits.push(format!("{label} (in strategy.matrix)"));
                }
                // A matrix value may itself be an input.
                text.push_str(&values);
            }
            // A label can come from another job's outputs, as in the
            // dynamic matrix a setup job writes to $GITHUB_OUTPUT. What a
            // mistake looks like there is a literal somewhere in that job,
            // so every string of its outputs, env and steps is taken in,
            // one level deep: a mention of ubuntu-latest in its scripts
            // for another purpose is refused too.
            for name in referenced(&text, "needs") {
                let Some(producer) = job_named(jobs, &name) else {
                    continue;
                };
                let mut values = String::new();
                strings(&producer["outputs"], &mut values);
                strings(&producer["env"], &mut values);
                for step in producer["steps"].as_vec().into_iter().flatten() {
                    for key in ["run", "with", "env"] {
                        strings(&step[key], &mut values);
                    }
                }
                for label in denied(&values, deny) {
                    hits.push(format!("{label} (in job {name}, read through needs)"));
                }
                // The producer's values may themselves be inputs.
                text.push_str(&values);
            }
            for input in referenced(&text, "inputs") {
                for trigger in ["workflow_call", "workflow_dispatch"] {
                    let mut values = String::new();
                    strings(&input_values(doc, trigger, &input), &mut values);
                    for label in denied(&values, deny) {
                        hits.push(format!(
                            "{label} (default or option of {trigger} input {input})"
                        ));
                    }
                }
            }
            if !hits.is_empty() {
                let mut message = format!("job `{id}` runs on a denied label:");
                for hit in hits {
                    let _ = write!(message, " {hit}");
                }
                findings.push(message);
            }
        }
    }
    Ok(findings)
}

/// Every scalar under a node, newline-separated; runs-on may be a string, a
/// list or a `group`/`labels` mapping, and a matrix nests anything.
fn strings(node: &Yaml, out: &mut String) {
    match node {
        Yaml::String(s) | Yaml::Real(s) => {
            out.push_str(s);
            out.push('\n');
        }
        Yaml::Array(items) => items.iter().for_each(|i| strings(i, out)),
        Yaml::Hash(map) => map.values().for_each(|v| strings(v, out)),
        _ => {}
    }
}

/// The denied labels that occur in the text as whole labels: bounded by
/// characters a label cannot contain, so `'ubuntu-latest'` in an
/// expression and `["ubuntu-latest"]` in a fromJSON literal count and
/// `ubuntu-latest-arm` does not.
fn denied(text: &str, deny: &[String]) -> Vec<String> {
    let text = text.to_ascii_lowercase();
    let mut hits = Vec::new();
    for word in text.split(|c: char| !is_label_char(c)) {
        if let Some(label) = deny.iter().find(|d| d.as_str() == word)
            && !hits.contains(label)
        {
            hits.push(label.clone());
        }
    }
    hits
}

fn is_label_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')
}

/// The default and, for a `choice`, the options of a trigger's input: the
/// values it takes without a dispatcher typing one. GitHub matches input
/// names case-insensitively, so this does too.
fn input_values(doc: &Yaml, trigger: &str, name: &str) -> Yaml {
    doc["on"][trigger]["inputs"]
        .as_hash()
        .into_iter()
        .flatten()
        .find(|(key, _)| key.as_str().is_some_and(|k| k.eq_ignore_ascii_case(name)))
        .map(|(_, input)| Yaml::Array(vec![input["default"].clone(), input["options"].clone()]))
        .unwrap_or(Yaml::BadValue)
}

/// The job of that id, which GitHub's expressions match case-insensitively.
fn job_named<'a>(jobs: &'a yaml_rust2::yaml::Hash, name: &str) -> Option<&'a Yaml> {
    jobs.iter()
        .find(|(id, _)| id.as_str().is_some_and(|id| id.eq_ignore_ascii_case(name)))
        .map(|(_, job)| job)
}

/// Names an expression reads from a context as `CONTEXT.NAME` or
/// `CONTEXT['NAME']`, lowercased: the inputs (also under `github.event.`)
/// or the jobs of `needs`.
fn referenced(text: &str, context: &str) -> Vec<String> {
    let text = text.to_ascii_lowercase();
    let mut names = Vec::new();
    for (at, _) in text.match_indices(context) {
        let before = &text[..at];
        let part_of_other = before
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-');
        if part_of_other && !(context == "inputs" && before.ends_with("github.event.")) {
            continue;
        }
        let after = &text[at + context.len()..];
        let Some(after) = after
            .strip_prefix('.')
            .or_else(|| after.strip_prefix("['"))
            .or_else(|| after.strip_prefix("[\""))
        else {
            continue;
        };
        let name: String = after
            .chars()
            .take_while(|&c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            .collect();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(source: &str) -> Vec<String> {
        check(source, &deny_list("")).unwrap()
    }

    #[test]
    fn plain_label() {
        let f = run("on: push\njobs:\n  a:\n    runs-on: ubuntu-latest\n    steps: []\n");
        assert_eq!(f, ["job `a` runs on a denied label: ubuntu-latest"]);
    }

    #[test]
    fn allowed_labels() {
        let src = "on: push\njobs:
  a: {runs-on: ubuntu-26.04}
  b: {runs-on: [self-hosted, ubuntu-latest-arm, my-ubuntu-latest]}
  c: {runs-on: {group: big, labels: [custom]}}
  d: {uses: ./.github/workflows/x.yml}
  e: {runs-on: \"${{ inputs.runner || 'ubuntu-26.04' }}\"}
";
        assert!(run(src).is_empty());
    }

    #[test]
    fn list_mapping_and_case() {
        let src = "jobs:
  a: {runs-on: [self-hosted, Ubuntu-Latest]}
  b: {runs-on: {group: g, labels: ubuntu-latest}}
";
        assert_eq!(run(src).len(), 2);
    }

    #[test]
    fn expression_literals() {
        let src = r#"jobs:
  a: {runs-on: "${{ inputs.runner || 'ubuntu-latest' }}"}
  b: {runs-on: "${{ fromJSON('[\"ubuntu-latest\"]') }}"}
"#;
        assert_eq!(run(src).len(), 2);
    }

    #[test]
    fn matrix_values() {
        let src = "jobs:
  a:
    runs-on: ${{ matrix.os }}
    strategy:
      matrix:
        os: [ubuntu-26.04]
        include:
          - os: ubuntu-latest
  b:
    runs-on: ubuntu-26.04
    strategy: {matrix: {image: [ubuntu-latest]}}
";
        assert_eq!(
            run(src),
            ["job `a` runs on a denied label: ubuntu-latest (in strategy.matrix)"]
        );
    }

    #[test]
    fn input_defaults() {
        let src = "on:
  workflow_call:
    inputs:
      runner: {type: string, default: ubuntu-latest}
      other: {type: string, default: ubuntu-latest}
jobs:
  a: {runs-on: '${{ inputs.runner }}'}
  b: {runs-on: '${{ fromJSON(inputs.other) }}'}
  c: {runs-on: '${{ github.event.inputs.other2 }}'}
";
        let f = run(src);
        assert_eq!(f.len(), 2);
        assert!(f[0].contains("default or option of workflow_call input runner"));
    }

    #[test]
    fn event_inputs_and_case() {
        let src = "on:
  workflow_dispatch:
    inputs:
      Runner: {default: ubuntu-latest}
jobs:
  a: {runs-on: '${{ github.event.inputs.runner }}'}
  b: {runs-on: '${{ inputs.RUNNER }}'}
  c: {runs-on: '${{ steps.inputs.runner }}'}
";
        let f = run(src);
        assert_eq!(f.len(), 2, "{f:?}");
        assert!(f[0].starts_with("job `a`") && f[1].starts_with("job `b`"));
    }

    #[test]
    fn indexed_and_matrix_fed_inputs() {
        let src = "on:
  workflow_call:
    inputs:
      os: {type: string, default: ubuntu-latest}
jobs:
  a: {runs-on: \"${{ inputs['os'] }}\"}
  b:
    runs-on: ${{ Matrix['os'] }}
    strategy: {matrix: {os: ['${{ inputs.os }}']}}
  c: {runs-on: '${{ inputs }}'}
";
        let f = run(src);
        assert_eq!(f.len(), 2, "{f:?}");
        assert!(f[1].starts_with("job `b`"));
    }

    #[test]
    fn reusable_workflow_with() {
        let src = "jobs:
  a: {uses: ./.github/workflows/x.yml, with: {runner: ubuntu-latest}}
  b: {uses: ./.github/workflows/x.yml, with: {runner: ubuntu-26.04}}
";
        assert_eq!(
            run(src),
            ["job `a` runs on a denied label: ubuntu-latest (passed in with)"]
        );
    }

    #[test]
    fn caller_matrix_input_and_choice() {
        let src = r#"on:
  workflow_dispatch:
    inputs:
      r: {type: choice, default: ubuntu-26.04, options: [ubuntu-26.04, ubuntu-latest]}
jobs:
  a:
    strategy: {matrix: {os: [ubuntu-latest]}}
    uses: ./.github/workflows/b.yml
    with: {runner: "${{ matrix.os }}"}
  b: {uses: ./.github/workflows/b.yml, with: {runner: "${{ inputs.r }}"}}
  c:
    runs-on: ${{ matrix.os }}
    strategy: {matrix: "${{ fromJSON('{\"os\":[\"ubuntu-latest\"]}') }}"}
"#;
        let f = run(src);
        assert_eq!(f.len(), 3, "{f:?}");
    }

    #[test]
    fn matrix_exclude_is_not_use() {
        let src = "jobs:
  a:
    runs-on: ${{ matrix.os }}
    strategy: {matrix: {os: [ubuntu-26.04], exclude: [{os: ubuntu-latest}]}}
";
        assert!(run(src).is_empty());
    }

    #[test]
    fn workflow_files_takes_yaml_only() {
        let dir = std::env::temp_dir().join(format!("runner-labels-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub.yml")).unwrap();
        for name in ["b.yaml", "a.yml", "c.YML", "notes.md"] {
            std::fs::write(dir.join(name), "").unwrap();
        }
        let files = workflow_files(std::slice::from_ref(&dir)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            files,
            [dir.join("a.yml"), dir.join("b.yaml"), dir.join("c.YML")]
        );
    }

    #[test]
    fn dynamic_matrix_from_needs() {
        let src = r#"on: push
jobs:
  setup:
    runs-on: ubuntu-26.04
    outputs: {matrix: "${{ steps.m.outputs.matrix }}"}
    steps:
      - id: m
        run: echo 'matrix={"os":["ubuntu-latest"]}' >> "$GITHUB_OUTPUT"
  build:
    needs: setup
    strategy: {matrix: "${{ fromJSON(needs.setup.outputs.matrix) }}"}
    runs-on: ${{ matrix.os }}
    steps: []
"#;
        assert_eq!(
            run(src),
            [
                "job `build` runs on a denied label: ubuntu-latest (in job setup, read through needs)"
            ]
        );
    }

    #[test]
    fn needs_env_bracket_and_inputs() {
        let src = r#"on:
  workflow_dispatch:
    inputs:
      runner: {default: ubuntu-latest}
jobs:
  Target:
    runs-on: ubuntu-26.04
    env: {RUNNER: "${{ vars.R || '\"ubuntu-latest\"' }}"}
    outputs: {runner: "${{ steps.t.outputs.runner }}"}
    steps: [{id: t, run: echo "runner=$RUNNER" >> "$GITHUB_OUTPUT"}]
  fed:
    runs-on: ubuntu-26.04
    outputs: {runner: "${{ inputs.runner }}"}
    steps: []
  noisy:
    runs-on: ubuntu-26.04
    outputs: {runner: '"ubuntu-26.04"'}
    steps: [{run: echo ubuntu-latest is not used here}]
  a: {needs: target, runs-on: "${{ fromJSON(needs.target.outputs.runner) }}"}
  b: {needs: target, uses: ./x.yml, with: {runner: "${{ needs['target'].outputs.runner }}"}}
  c: {needs: fed, runs-on: "${{ needs.fed.outputs.runner }}"}
  d: {needs: noisy, runs-on: "${{ fromJSON(needs.noisy.outputs.runner) }}"}
  e: {needs: [target], runs-on: ubuntu-26.04, steps: [{run: "echo ${{ needs.target.outputs.runner }}"}]}
"#;
        let f = run(src);
        // d's producer mentions ubuntu-latest in a script: refused, as the
        // check takes the whole job in.
        assert_eq!(f.len(), 4, "{f:?}");
        assert!(f[0].starts_with("job `a`") && f[0].contains("in job target"));
        assert!(f[1].starts_with("job `b`"));
        assert!(f[2].starts_with("job `c`") && f[2].contains("workflow_dispatch input runner"));
        assert!(f[3].starts_with("job `d`"));
    }

    #[test]
    fn anchors_resolve() {
        let src = "x: &r ubuntu-latest\njobs:\n  a: {runs-on: *r}\n";
        assert_eq!(run(src).len(), 1);
    }

    #[test]
    fn caller_extends_but_cannot_empty() {
        assert_eq!(deny_list(""), ["ubuntu-latest"]);
        assert_eq!(
            deny_list(" Windows-Latest,macos-latest  ubuntu-latest"),
            ["ubuntu-latest", "windows-latest", "macos-latest"]
        );
        let f = check(
            "jobs:\n  a: {runs-on: windows-latest}\n",
            &deny_list("windows-latest"),
        )
        .unwrap();
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn invalid_yaml_is_an_error() {
        assert!(check("jobs: [", &deny_list("")).is_err());
    }

    #[test]
    fn missing_path_is_an_error() {
        assert!(workflow_files(&[PathBuf::from("/nonexistent/workflows")]).is_err());
    }
}
