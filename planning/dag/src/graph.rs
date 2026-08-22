//! DAG 构建与就绪步骤计算。

use forge_core::{ForgeError, ForgeResult};
use forge_planner::{Plan, StepId};
use std::collections::{HashMap, HashSet};

/// DAG 图。
#[derive(Debug)]
pub struct Dag {
    /// 节点列表（按 Plan 中 step 的顺序）。
    pub nodes: Vec<StepId>,
    /// 边列表：(from, to) 表示 from → to。
    pub edges: Vec<(StepId, StepId)>,
}

/// 从 Plan 构建图。
///
/// - 引用不存在的 step → `DependencyMissing`
/// - 自依赖或环 → `InvalidState("cycle detected")`
pub fn build_dag(plan: &Plan) -> ForgeResult<Dag> {
    let node_set: HashSet<&StepId> = plan.steps.iter().map(|s| &s.id).collect();
    let nodes: Vec<StepId> = plan.steps.iter().map(|s| s.id.clone()).collect();
    let mut edges: Vec<(StepId, StepId)> = Vec::new();

    for step in &plan.steps {
        for dep in &step.depends_on {
            // 自依赖
            if dep == &step.id {
                return Err(ForgeError::InvalidState(format!(
                    "cycle detected: self-dependency on {}",
                    step.id
                )));
            }
            // 悬空依赖
            if !node_set.contains(dep) {
                return Err(ForgeError::DependencyMissing(format!(
                    "step {} depends on non-existent step {}",
                    step.id, dep
                )));
            }
            // 边方向：dep → step（dep 完成后 step 才能执行）
            edges.push((dep.clone(), step.id.clone()));
        }
    }

    let dag = Dag { nodes, edges };

    // 检查环
    if has_cycle(&dag) {
        return Err(ForgeError::InvalidState("cycle detected".into()));
    }

    Ok(dag)
}

/// 使用染色法检测环。
fn has_cycle(dag: &Dag) -> bool {
    let adj = build_adjacency(dag);
    let mut white: HashSet<StepId> = dag.nodes.iter().cloned().collect();
    let mut gray: HashSet<StepId> = HashSet::new();
    let mut black: HashSet<StepId> = HashSet::new();

    while let Some(node) = white.iter().next().cloned() {
        white.remove(&node);
        if dfs_visit(&node, &adj, &mut white, &mut gray, &mut black) {
            return true;
        }
    }
    false
}

fn dfs_visit(
    node: &StepId,
    adj: &HashMap<StepId, Vec<StepId>>,
    white: &mut HashSet<StepId>,
    gray: &mut HashSet<StepId>,
    black: &mut HashSet<StepId>,
) -> bool {
    gray.insert(node.clone());

    if let Some(neighbors) = adj.get(node) {
        for neighbor in neighbors {
            if black.contains(neighbor) {
                continue;
            }
            if gray.contains(neighbor) {
                return true; // 后向边 → 环
            }
            white.remove(neighbor);
            if dfs_visit(neighbor, adj, white, gray, black) {
                return true;
            }
        }
    }

    gray.remove(node);
    black.insert(node.clone());
    false
}

fn build_adjacency(dag: &Dag) -> HashMap<StepId, Vec<StepId>> {
    let mut adj: HashMap<StepId, Vec<StepId>> = HashMap::new();
    for node in &dag.nodes {
        adj.entry(node.clone()).or_default();
    }
    for (from, to) in &dag.edges {
        adj.entry(from.clone()).or_default().push(to.clone());
    }
    adj
}

/// 给定已完成集合，返回所有依赖已满足、自身未完成的步骤（可并行集）。
///
/// 返回的 Vec 按 StepId 字典序排序（保证确定性）。
pub fn ready_steps(dag: &Dag, done: &HashSet<StepId>) -> ForgeResult<Vec<StepId>> {
    let mut ready: Vec<StepId> = Vec::new();

    for node in &dag.nodes {
        if done.contains(node) {
            continue;
        }
        // 检查所有依赖是否已完成
        let deps_satisfied: bool = dag
            .edges
            .iter()
            .filter(|(_, to)| to == node)
            .all(|(from, _)| done.contains(from));

        if deps_satisfied {
            ready.push(node.clone());
        }
    }

    ready.sort();
    Ok(ready)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topo::topo_order;
    use forge_core::PlanId;
    use forge_planner::{Plan, PlanStatus, PlanStep, StepAction};

    fn make_plan(steps: Vec<PlanStep>) -> Plan {
        Plan {
            id: PlanId::new_plan_id(),
            task_id: forge_core::TaskId::new_task_id(),
            steps,
            status: PlanStatus::Ready,
        }
    }

    fn make_step(id: &str, deps: Vec<&str>) -> PlanStep {
        PlanStep {
            id: id.into(),
            title: id.into(),
            depends_on: deps.into_iter().map(String::from).collect(),
            action: StepAction::CallCapability {
                capability: "cap".into(),
                input: serde_json::json!({}),
            },
        }
    }

    #[test]
    fn test_linear_chain() {
        let plan = make_plan(vec![
            make_step("step_1", vec![]),
            make_step("step_2", vec!["step_1"]),
            make_step("step_3", vec!["step_2"]),
        ]);
        let dag = build_dag(&plan).unwrap();

        let topo = topo_order(&dag).unwrap();
        assert_eq!(topo, vec!["step_1", "step_2", "step_3"]);

        let done = HashSet::new();
        let ready = ready_steps(&dag, &done).unwrap();
        assert_eq!(ready, vec!["step_1"]);

        let mut done = HashSet::new();
        done.insert("step_1".to_string());
        let ready = ready_steps(&dag, &done).unwrap();
        assert_eq!(ready, vec!["step_2"]);
    }

    #[test]
    fn test_diamond_parallel() {
        let plan = make_plan(vec![
            make_step("step_1", vec![]),
            make_step("step_2", vec!["step_1"]),
            make_step("step_3", vec!["step_1"]),
            make_step("step_4", vec!["step_2", "step_3"]),
        ]);
        let dag = build_dag(&plan).unwrap();

        let mut done = HashSet::new();
        done.insert("step_1".to_string());
        let ready = ready_steps(&dag, &done).unwrap();
        assert_eq!(ready, vec!["step_2", "step_3"]);
    }

    #[test]
    fn test_cycle_detected() {
        let plan = make_plan(vec![
            make_step("a", vec!["b"]),
            make_step("b", vec!["a"]),
        ]);
        let result = build_dag(&plan);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("cycle") || err.contains("DependencyMissing"));
    }

    #[test]
    fn test_self_dependency() {
        let plan = make_plan(vec![make_step("a", vec!["a"])]);
        let result = build_dag(&plan);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cycle"));
    }

    #[test]
    fn test_dangling_dependency() {
        let plan = make_plan(vec![make_step("a", vec!["nonexistent"])]);
        let result = build_dag(&plan);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("dependency missing"));
    }

    #[test]
    fn test_empty_plan() {
        let plan = make_plan(vec![]);
        let dag = build_dag(&plan).unwrap();
        assert!(dag.nodes.is_empty());
        assert!(dag.edges.is_empty());

        let topo = topo_order(&dag).unwrap();
        assert!(topo.is_empty());

        let ready = ready_steps(&dag, &HashSet::new()).unwrap();
        assert!(ready.is_empty());
    }

    #[test]
    fn test_ready_steps_sorted() {
        let plan = make_plan(vec![
            make_step("zebra", vec![]),
            make_step("apple", vec![]),
            make_step("mango", vec![]),
        ]);
        let dag = build_dag(&plan).unwrap();
        let ready = ready_steps(&dag, &HashSet::new()).unwrap();
        assert_eq!(ready, vec!["apple", "mango", "zebra"]);
    }
}
