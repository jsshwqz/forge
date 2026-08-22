//! 拓扑排序（Kahn 算法）。

use crate::graph::Dag;
use forge_core::{ForgeError, ForgeResult};
use forge_planner::StepId;
use std::collections::{HashMap, HashSet, VecDeque};

/// 拓扑排序。有环返回 `InvalidState`。
///
/// 使用 Kahn 算法：反复取入度为 0 的节点。
pub fn topo_order(dag: &Dag) -> ForgeResult<Vec<StepId>> {
    if dag.nodes.is_empty() {
        return Ok(vec![]);
    }

    // 计算入度
    let mut in_degree: HashMap<StepId, usize> = HashMap::new();
    for node in &dag.nodes {
        in_degree.insert(node.clone(), 0);
    }
    for (_, to) in &dag.edges {
        *in_degree.entry(to.clone()).or_insert(0) += 1;
    }

    // 入度为 0 的节点入队（按字典序保证确定性）
    let mut queue: VecDeque<StepId> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(k, _)| k.clone())
        .collect::<Vec<_>>()
        .into_iter()
        .collect();

    // 构建邻接表
    let mut adj: HashMap<StepId, Vec<StepId>> = HashMap::new();
    for (from, to) in &dag.edges {
        adj.entry(from.clone()).or_default().push(to.clone());
    }

    let mut result: Vec<StepId> = Vec::new();
    let mut visited: HashSet<StepId> = HashSet::new();

    while let Some(node) = queue.pop_front() {
        if visited.contains(&node) {
            continue;
        }
        visited.insert(node.clone());
        result.push(node.clone());

        if let Some(neighbors) = adj.get(&node) {
            for neighbor in neighbors {
                if let Some(deg) = in_degree.get_mut(neighbor) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(neighbor.clone());
                    }
                }
            }
        }
    }

    if result.len() != dag.nodes.len() {
        return Err(ForgeError::InvalidState(
            "cycle detected: topological sort incomplete".into(),
        ));
    }

    Ok(result)
}
