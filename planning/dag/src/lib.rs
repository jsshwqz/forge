//! forge-dag：DAG 构建、环检测、拓扑排序、就绪步骤计算。
//!
//! 纯逻辑，零 IO，零 async。

pub mod graph;
pub mod topo;

pub use graph::{build_dag, ready_steps, Dag};
pub use topo::topo_order;
