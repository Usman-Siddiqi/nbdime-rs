//! Merging notebooks: the strategy table for a notebook, and the merge itself.

use crate::diffing::{diff_notebooks, NotebookDiffConfig};
use crate::value::Value;

use super::decisions::{apply_decisions, MergeDecision};
use super::generic::decide_merge_with_diff;
use super::strategies::Strategies;
use super::MergeError;

/// How a notebook merge resolves conflicts (the `nbmerge` options).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeArgs {
    /// `inline`, `use-base`, `use-local`, `use-remote` or `union`.
    pub merge_strategy: String,
    /// Strategy for sources and attachments; the merge strategy if unset.
    pub input_strategy: Option<String>,
    /// Strategy for outputs; the merge strategy if unset. Also `remove` and `clear-all`.
    pub output_strategy: Option<String>,
    /// Whether changes to execution counts and similar never conflict.
    pub ignore_transients: bool,
}

impl Default for MergeArgs {
    fn default() -> Self {
        MergeArgs {
            merge_strategy: "inline".into(),
            input_strategy: None,
            output_strategy: None,
            ignore_transients: true,
        }
    }
}

/// The strategies for each part of a notebook that `args` asks for; `None` means the defaults.
pub fn notebook_merge_strategies(args: Option<&MergeArgs>) -> Strategies {
    let mut strategies = Strategies::new();
    strategies.insert("/cells/*/id", Some("remove"));
    // These fields should never conflict, that would be an internal error.
    strategies.insert("/nbformat", Some("fail"));
    strategies.insert("/cells/*/cell_type", Some("fail"));
    // Pick the highest minor format.
    strategies.insert("/nbformat_minor", Some("take-max"));

    if args.is_none_or(|a| a.ignore_transients) {
        strategies.transients = [
            "/cells/*/execution_count",
            "/cells/*/outputs/*/execution_count",
            "/cells/*/metadata/collapsed",
            "/cells/*/metadata/autoscroll",
            "/cells/*/metadata/scrolled",
        ]
        .map(String::from)
        .to_vec();
        strategies.insert("/cells/*/execution_count", Some("clear"));
        strategies.insert("/cells/*/outputs/*/execution_count", Some("clear"));
    }

    // The default for the command line tools is inline, which makes a notebook that can be
    // edited by hand. Sources and outputs follow the merge strategy unless set separately.
    let default = MergeArgs::default();
    let args = args.unwrap_or(&default);
    let merge_strategy = args.merge_strategy.as_str();
    let non_empty = |s: &Option<String>| s.clone().filter(|s| !s.is_empty());
    let input_strategy = non_empty(&args.input_strategy).unwrap_or_else(|| merge_strategy.into());
    let output_strategy = non_empty(&args.output_strategy).unwrap_or_else(|| merge_strategy.into());
    let mut metadata_strategy = (merge_strategy != "union").then_some(merge_strategy);

    match merge_strategy {
        "inline" => strategies.insert("/cells", Some("inline-cells")),
        "union" => strategies.insert("/cells", Some("union")),
        other => strategies.insert("/", Some(other)),
    }

    // `inline` becomes a specific strategy for each field.
    let (source_strategy, attachments_strategy) = if input_strategy == "inline" {
        ("inline-source".to_owned(), "inline-attachments".to_owned())
    } else {
        (input_strategy.clone(), input_strategy)
    };
    let outputs_strategy = if output_strategy == "inline" {
        "inline-outputs".to_owned()
    } else {
        output_strategy
    };
    if metadata_strategy == Some("inline") {
        metadata_strategy = Some("record-conflict");
    }

    strategies.insert("/metadata", metadata_strategy);
    strategies.insert("/cells/*/metadata", metadata_strategy);
    strategies.insert("/cells/*/outputs/*/metadata", metadata_strategy);
    strategies.insert("/cells/*/source", Some(&source_strategy));
    strategies.insert("/cells/*/attachments", Some(&attachments_strategy));
    strategies.insert("/cells/*/outputs", Some(&outputs_strategy));
    strategies
}

/// The decisions that merge the notebooks `local` and `remote` from their shared ancestor
/// `base`. The notebook diffs use `diff_config`.
pub fn decide_notebook_merge(
    base: &Value,
    local: &Value,
    remote: &Value,
    args: Option<&MergeArgs>,
    diff_config: &NotebookDiffConfig,
) -> Result<Vec<MergeDecision>, MergeError> {
    let strategies = notebook_merge_strategies(args);
    let local_diff = diff_notebooks(base, local, diff_config)?;
    let remote_diff = diff_notebooks(base, remote, diff_config)?;
    decide_merge_with_diff(base, local, remote, &local_diff, &remote_diff, &strategies)
}

/// Merges the changes that `local` and `remote` make to `base`. Returns the merged notebook,
/// which is partly merged if there are conflicts, and the decisions that made it.
pub fn merge_notebooks(
    base: &Value,
    local: &Value,
    remote: &Value,
    args: Option<&MergeArgs>,
    diff_config: &NotebookDiffConfig,
) -> Result<(Value, Vec<MergeDecision>), MergeError> {
    let decisions = decide_notebook_merge(base, local, remote, args, diff_config)?;
    let merged = apply_decisions(base, &decisions)?;
    Ok((merged, decisions))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::parse;

    fn v(text: &str) -> Value {
        parse(text).unwrap()
    }

    #[test]
    fn default_strategies() {
        let s = notebook_merge_strategies(None);
        assert_eq!(s.get("/cells"), Some("inline-cells"));
        assert_eq!(s.get("/cells/3/source"), Some("inline-source"));
        assert_eq!(s.get("/cells/3/attachments"), Some("inline-attachments"));
        assert_eq!(s.get("/cells/3/outputs"), Some("inline-outputs"));
        assert_eq!(s.get("/metadata"), Some("record-conflict"));
        assert_eq!(
            s.get("/cells/3/outputs/1/metadata"),
            Some("record-conflict")
        );
        assert_eq!(s.get("/nbformat"), Some("fail"));
        assert_eq!(s.get("/nbformat_minor"), Some("take-max"));
        assert_eq!(s.get("/cells/3/execution_count"), Some("clear"));
        assert_eq!(s.get("/"), None);
        assert_eq!(s.transients.len(), 5);
    }

    #[test]
    fn explicit_arguments() {
        let args = MergeArgs {
            merge_strategy: "use-local".into(),
            input_strategy: Some("use-remote".into()),
            output_strategy: Some("clear-all".into()),
            ignore_transients: false,
        };
        let s = notebook_merge_strategies(Some(&args));
        assert_eq!(s.get("/"), Some("use-local"));
        assert_eq!(s.get("/cells"), None);
        assert_eq!(s.get("/cells/0/source"), Some("use-remote"));
        assert_eq!(s.get("/cells/0/outputs"), Some("clear-all"));
        assert_eq!(s.get("/metadata"), Some("use-local"));
        assert!(s.transients.is_empty());
        assert_eq!(s.get("/cells/0/execution_count"), None);

        let union = MergeArgs {
            merge_strategy: "union".into(),
            ..MergeArgs::default()
        };
        let s = notebook_merge_strategies(Some(&union));
        assert_eq!(s.get("/cells"), Some("union"));
        assert_eq!(s.get("/metadata"), None);
        assert_eq!(s.get("/cells/0/source"), Some("union"));
    }

    fn notebook(cells: &str) -> Value {
        v(&format!(
            r#"{{"cells":{cells},"metadata":{{}},"nbformat":4,"nbformat_minor":5}}"#
        ))
    }

    fn code(id: &str, source: &str) -> String {
        format!(
            r#"{{"id":"{id}","cell_type":"code","source":"{source}","metadata":{{}},"outputs":[],"execution_count":null}}"#
        )
    }

    #[test]
    fn merges_edits_of_different_cells() {
        let base = notebook(&format!("[{},{}]", code("a", "x = 1"), code("b", "y = 2")));
        let local = notebook(&format!("[{},{}]", code("a", "x = 10"), code("b", "y = 2")));
        let remote = notebook(&format!("[{},{}]", code("a", "x = 1"), code("b", "y = 20")));
        let config = NotebookDiffConfig::new();
        let (merged, decisions) = merge_notebooks(&base, &local, &remote, None, &config).unwrap();
        assert!(decisions.iter().all(|d| !d.conflict));
        let cells = merged.get("cells").unwrap().as_list().unwrap();
        assert_eq!(cells[0].get("source"), Some(&Value::from("x = 10")));
        assert_eq!(cells[1].get("source"), Some(&Value::from("y = 20")));
    }

    #[test]
    fn conflicting_inserts_get_marker_cells() {
        let base = notebook("[]");
        let local = notebook(&format!("[{}]", code("l", "print('local side')")));
        let remote = notebook(&format!("[{}]", code("r", "something else entirely")));
        let config = NotebookDiffConfig::new();
        let (merged, decisions) = merge_notebooks(&base, &local, &remote, None, &config).unwrap();
        assert!(decisions.iter().any(|d| d.conflict));
        let cells = merged.get("cells").unwrap().as_list().unwrap();
        assert_eq!(cells.len(), 5);
        let marker = |i: usize| cells[i].get("source").unwrap().as_str().unwrap();
        assert!(marker(0).contains("<<<<<<< local"));
        assert!(marker(2).contains("======="));
        assert!(marker(4).contains(">>>>>>> remote"));
        let ids: std::collections::HashSet<_> = cells
            .iter()
            .map(|c| c.get("id").unwrap().as_str().unwrap())
            .collect();
        assert_eq!(ids.len(), 5);
    }

    #[test]
    fn conflicting_sources_are_rendered_inline() {
        let base = notebook(&format!("[{}]", code("a", "x = 1")));
        let local = notebook(&format!("[{}]", code("a", "x = 2")));
        let remote = notebook(&format!("[{}]", code("a", "x = 3")));
        let config = NotebookDiffConfig::new();
        let (merged, decisions) = merge_notebooks(&base, &local, &remote, None, &config).unwrap();
        assert!(decisions.iter().any(|d| d.conflict));
        let source = merged.get("cells").unwrap().as_list().unwrap()[0]
            .get("source")
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            source,
            "<<<<<<< local\nx = 2\n=======\nx = 3\n>>>>>>> remote"
        );
        let args = MergeArgs {
            merge_strategy: "use-remote".into(),
            ..MergeArgs::default()
        };
        let (merged, decisions) =
            merge_notebooks(&base, &local, &remote, Some(&args), &config).unwrap();
        assert!(decisions.iter().all(|d| !d.conflict));
        assert_eq!(
            merged.get("cells").unwrap().as_list().unwrap()[0].get("source"),
            Some(&Value::from("x = 3"))
        );
    }
}
