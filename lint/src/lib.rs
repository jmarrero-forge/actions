//! What the crate's two binaries share.

/// Whether a file name has a workflow's extension, `.yml` or `.yaml` in any
/// case. runner-labels and workflows-check both ask this, so one cannot
/// skip a file (`a.YML`) that the other takes for a workflow.
pub fn has_workflow_extension(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".yml") || lower.ends_with(".yaml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_any_case() {
        for name in ["a.yml", "a.yaml", "a.YML", "a.Yaml"] {
            assert!(has_workflow_extension(name), "{name}");
        }
        for name in ["a.md", "a.yml.txt", "yml"] {
            assert!(!has_workflow_extension(name), "{name}");
        }
    }
}
