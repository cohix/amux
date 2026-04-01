use anyhow::{bail, Result};
use std::collections::{HashMap, HashSet};

use super::parser::WorkflowStep;

/// Validate that all `Depends-on` references point to existing steps.
/// Returns an error if any dependency is missing from the step list.
pub fn validate_references(steps: &[WorkflowStep]) -> Result<()> {
    let names: HashSet<&str> = steps.iter().map(|s| s.name.as_str()).collect();
    for step in steps {
        for dep in &step.depends_on {
            if !names.contains(dep.as_str()) {
                bail!(
                    "Step '{}' depends-on '{}' which does not exist in the workflow.",
                    step.name,
                    dep
                );
            }
        }
    }
    Ok(())
}

/// Detect cycles in the dependency graph using DFS.
/// Returns an error naming the step that forms a cycle.
pub fn detect_cycle(steps: &[WorkflowStep]) -> Result<()> {
    // Build forward-dependency adjacency: step → what it depends on.
    let adjacency: HashMap<&str, Vec<&str>> = steps
        .iter()
        .map(|s| (s.name.as_str(), s.depends_on.iter().map(String::as_str).collect()))
        .collect();

    let mut visited: HashSet<&str> = HashSet::new();
    let mut in_stack: HashSet<&str> = HashSet::new();

    for step in steps {
        if !visited.contains(step.name.as_str()) {
            dfs(step.name.as_str(), &adjacency, &mut visited, &mut in_stack)?;
        }
    }
    Ok(())
}

fn dfs<'a>(
    node: &'a str,
    adjacency: &HashMap<&'a str, Vec<&'a str>>,
    visited: &mut HashSet<&'a str>,
    in_stack: &mut HashSet<&'a str>,
) -> Result<()> {
    visited.insert(node);
    in_stack.insert(node);

    if let Some(deps) = adjacency.get(node) {
        for &dep in deps {
            if in_stack.contains(dep) {
                bail!(
                    "Workflow DAG contains a cycle involving step '{}'.",
                    dep
                );
            }
            if !visited.contains(dep) {
                dfs(dep, adjacency, visited, in_stack)?;
            }
        }
    }

    in_stack.remove(node);
    Ok(())
}

/// Return the names of steps that are ready to run:
/// all their dependencies are in `completed` and they are not themselves completed or running.
///
/// The order of the returned names matches the original step order from the workflow file.
pub fn ready_steps(steps: &[WorkflowStep], completed: &HashSet<String>) -> Vec<String> {
    steps
        .iter()
        .filter(|s| {
            !completed.contains(&s.name)
                && s.depends_on.iter().all(|d| completed.contains(d))
        })
        .map(|s| s.name.clone())
        .collect()
}

/// Compute a topological ordering of the steps (post-order DFS on the dependency graph).
/// Returns step names in an order where all dependencies come before dependents.
pub fn topological_order(steps: &[WorkflowStep]) -> Vec<String> {
    // `adjacency[s]` = steps that `s` depends on (predecessors).
    // The post-order DFS on this predecessor graph naturally yields
    // a topological order (dependencies appear before the nodes that need them).
    let adjacency: HashMap<&str, Vec<&str>> = steps
        .iter()
        .map(|s| (s.name.as_str(), s.depends_on.iter().map(String::as_str).collect()))
        .collect();

    let mut visited: HashSet<&str> = HashSet::new();
    let mut order: Vec<String> = Vec::new();

    for step in steps {
        if !visited.contains(step.name.as_str()) {
            topo_dfs(step.name.as_str(), &adjacency, &mut visited, &mut order);
        }
    }

    // No reversal needed: each node is appended after its dependencies,
    // so the order is already correct (dependencies first).
    order
}

fn topo_dfs<'a>(
    node: &'a str,
    adjacency: &HashMap<&'a str, Vec<&'a str>>,
    visited: &mut HashSet<&'a str>,
    order: &mut Vec<String>,
) {
    visited.insert(node);
    if let Some(deps) = adjacency.get(node) {
        for &dep in deps {
            if !visited.contains(dep) {
                topo_dfs(dep, adjacency, visited, order);
            }
        }
    }
    order.push(node.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::parser::WorkflowStep;

    fn step(name: &str, deps: &[&str]) -> WorkflowStep {
        WorkflowStep {
            name: name.to_string(),
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            prompt_template: String::new(),
        }
    }

    // --- validate_references ---

    #[test]
    fn validate_references_ok_when_all_deps_exist() {
        let steps = vec![step("a", &[]), step("b", &["a"])];
        assert!(validate_references(&steps).is_ok());
    }

    #[test]
    fn validate_references_error_on_missing_dep() {
        let steps = vec![step("b", &["missing"])];
        let err = validate_references(&steps).unwrap_err();
        assert!(err.to_string().contains("missing"));
    }

    // --- detect_cycle ---

    #[test]
    fn detect_cycle_no_cycle() {
        let steps = vec![step("a", &[]), step("b", &["a"]), step("c", &["b"])];
        assert!(detect_cycle(&steps).is_ok());
    }

    #[test]
    fn detect_cycle_simple_ab_ba() {
        let steps = vec![step("a", &["b"]), step("b", &["a"])];
        let err = detect_cycle(&steps).unwrap_err();
        assert!(err.to_string().contains("cycle"));
    }

    #[test]
    fn detect_cycle_self_loop() {
        let steps = vec![step("a", &["a"])];
        assert!(detect_cycle(&steps).is_err());
    }

    #[test]
    fn detect_cycle_longer_chain() {
        let steps = vec![step("a", &["c"]), step("b", &["a"]), step("c", &["b"])];
        assert!(detect_cycle(&steps).is_err());
    }

    // --- ready_steps ---

    #[test]
    fn ready_steps_root_steps_have_no_deps() {
        let steps = vec![step("a", &[]), step("b", &[]), step("c", &["a"])];
        let ready = ready_steps(&steps, &HashSet::new());
        assert!(ready.contains(&"a".to_string()));
        assert!(ready.contains(&"b".to_string()));
        assert!(!ready.contains(&"c".to_string()));
    }

    #[test]
    fn ready_steps_unlocks_after_dep_done() {
        let steps = vec![step("a", &[]), step("b", &["a"])];
        let completed: HashSet<String> = ["a".to_string()].into_iter().collect();
        let ready = ready_steps(&steps, &completed);
        assert!(!ready.contains(&"a".to_string())); // already done
        assert!(ready.contains(&"b".to_string()));
    }

    #[test]
    fn ready_steps_empty_when_all_done() {
        let steps = vec![step("a", &[]), step("b", &["a"])];
        let completed: HashSet<String> = ["a".to_string(), "b".to_string()].into_iter().collect();
        let ready = ready_steps(&steps, &completed);
        assert!(ready.is_empty());
    }

    #[test]
    fn ready_steps_preserves_file_order() {
        let steps = vec![step("first", &[]), step("second", &[]), step("third", &[])];
        let ready = ready_steps(&steps, &HashSet::new());
        assert_eq!(ready, vec!["first", "second", "third"]);
    }

    // --- topological_order ---

    #[test]
    fn topological_order_simple_chain() {
        let steps = vec![step("a", &[]), step("b", &["a"]), step("c", &["b"])];
        let order = topological_order(&steps);
        let a = order.iter().position(|s| s == "a").unwrap();
        let b = order.iter().position(|s| s == "b").unwrap();
        let c = order.iter().position(|s| s == "c").unwrap();
        assert!(a < b && b < c);
    }
}
