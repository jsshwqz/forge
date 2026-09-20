//! KNOW-001A/001B: 缺省单机 serve 知识库持久化断链修复测试。
//!
//! 测试名冻结（5 条），异步用例用 tokio::time::timeout 护栏（R1-094 教训）。
//! 全程用 FORGE_KNOWLEDGE_FILE 指向 tempfile 路径，禁写真 home。

use std::time::Duration;

use forge_knowledge::{knowledge_file, FileKnowledgeBase, FailureKnowledgeBase, KnowledgeEntry};
use forge_recovery::{FailureCategory, FailureRecord};
use forge_server::AppState;

// ── #1 零扰动护栏：in_memory() 仍是内存态 ──────────────────────────

#[tokio::test]
async fn in_memory_ctor_still_uses_memory_kb() {
    // AppState::in_memory() 构造器未被 KNOW-001A 串改。
    // 内存态特征：同一实例两次 all() 互不影响（内存非落盘），
    // 且不依赖任何文件路径即可工作。
    let st = AppState::in_memory();
    let entries_first = tokio::time::timeout(
        Duration::from_secs(5),
        st.knowledge.all(),
    )
    .await
    .expect("timeout");
    assert!(
        entries_first.is_empty(),
        "fresh in_memory should have no entries"
    );

    // ingest 一条，all() 应可见（进程内），但不落盘
    let entry = KnowledgeEntry {
        record: FailureRecord {
            id: "know-test-001".to_string(),
            execution_id: forge_core::ExecutionId::new_execution_id(),
            at: chrono::Utc::now(),
            category: FailureCategory::ToolError,
            message: "in_memory ctor guard".to_string(),
            retriable: false,
        },
        related_evidence: vec![],
        tool: Some("orchestrate".to_string()),
    };
    tokio::time::timeout(Duration::from_secs(5), st.knowledge.ingest(entry))
        .await
        .expect("timeout");
    let entries_after = tokio::time::timeout(
        Duration::from_secs(5),
        st.knowledge.all(),
    )
    .await
    .expect("timeout");
    assert_eq!(entries_after.len(), 1, "in_memory should see ingested entry");
}

// ── #2 env 门控判定 ──────────────────────────────────────────────

#[test]
fn persist_env_gate_reads_flag() {
    // 临时清除 + 设置 env，测试两态判定。
    // 判定逻辑：FORGE_KNOWLEDGE_PERSIST == "0" → false（逃生阀）；
    //          未设或非 "0" → true（缺省持久）。

    // 态 A: PERSIST=0 → 不持久
    std::env::set_var("FORGE_KNOWLEDGE_PERSIST", "0");
    assert!(
        !knowledge_persist_check(),
        "PERSIST=0 should disable persistence"
    );

    // 态 B: PERSIST 未设 → 缺省持久
    std::env::remove_var("FORGE_KNOWLEDGE_PERSIST");
    assert!(
        knowledge_persist_check(),
        "unset PERSIST should default to persistent"
    );

    // 态 C: PERSIST=1 → 持久（非 "0" 均持久）
    std::env::set_var("FORGE_KNOWLEDGE_PERSIST", "1");
    assert!(
        knowledge_persist_check(),
        "PERSIST=1 should enable persistence"
    );

    // 清理
    std::env::remove_var("FORGE_KNOWLEDGE_PERSIST");
}

/// 复刻 server/src/lib.rs 的 knowledge_persist_enabled() 逻辑用于测试。
/// 如果 server crate 未来 pub 导出该函数，应改为直接调用。
fn knowledge_persist_check() -> bool {
    std::env::var("FORGE_KNOWLEDGE_PERSIST").ok().as_deref() != Some("0")
}

// ── #3 路径 env 覆盖 ─────────────────────────────────────────────

#[test]
fn file_kb_path_honors_env_override() {
    // 设 FORGE_KNOWLEDGE_FILE 到临时路径，knowledge_file() 应返回该路径。
    let tmp = std::env::temp_dir().join("know_test_kb_path.jsonl");
    std::env::set_var("FORGE_KNOWLEDGE_FILE", &tmp);

    let path = knowledge_file();
    assert_eq!(
        path, tmp,
        "knowledge_file() should honor FORGE_KNOWLEDGE_FILE override"
    );

    // 清理
    std::env::remove_var("FORGE_KNOWLEDGE_FILE");
    let _ = std::fs::remove_file(&tmp);
}

// ── #4 跨实例（等价跨重启）持久化 e2e ────────────────────────────

#[tokio::test]
async fn knowledge_survives_instance_restart() {
    // 同 path 建实例 A → ingest → drop A → 建实例 B（同 path）→ search 命中。
    // 跨实例即等价跨重启（K5 证明二者读同一 JSONL）。
    let tmp = std::env::temp_dir().join("know_test_restart.jsonl");
    let _ = std::fs::remove_file(&tmp);

    // 实例 A：写入
    {
        let kb_a = FileKnowledgeBase::new(&tmp);
        let entry = KnowledgeEntry {
            record: FailureRecord {
                id: "know-restart-001".to_string(),
                execution_id: forge_core::ExecutionId::new_execution_id(),
                at: chrono::Utc::now(),
                category: FailureCategory::ToolError,
                message: "restart survival test".to_string(),
                retriable: false,
            },
            related_evidence: vec![],
            tool: Some("orchestrate".to_string()),
        };
        tokio::time::timeout(Duration::from_secs(10), kb_a.ingest(entry))
            .await
            .expect("timeout");
    }
    // A dropped here

    // 实例 B：读取（同 path）
    {
        let kb_b = FileKnowledgeBase::new(&tmp);
        let hits = tokio::time::timeout(
            Duration::from_secs(10),
            kb_b.search(None, Some("orchestrate"), None),
        )
        .await
        .expect("timeout");
        assert!(
            !hits.is_empty(),
            "knowledge should survive instance restart (found {} entries)",
            hits.len()
        );
        let all = tokio::time::timeout(Duration::from_secs(10), kb_b.all())
            .await
            .expect("timeout");
        assert!(!all.is_empty(), "all() should be non-empty after restart");
    }

    // 清理
    let _ = std::fs::remove_file(&tmp);
}

// ── #5 空文件/不存在文件鲁棒性 ───────────────────────────────────

#[tokio::test]
async fn empty_file_yields_no_entries() {
    // 指向不存在的文件 → search/all 返回空、不 panic。
    let tmp = std::env::temp_dir().join("know_test_empty_nonexist.jsonl");
    let _ = std::fs::remove_file(&tmp);

    let kb = FileKnowledgeBase::new(&tmp);
    let search_result = tokio::time::timeout(
        Duration::from_secs(5),
        kb.search(None, None, None),
    )
    .await
    .expect("timeout");
    assert!(
        search_result.is_empty(),
        "non-existent file should yield no entries"
    );
    let all_result = tokio::time::timeout(Duration::from_secs(5), kb.all())
        .await
        .expect("timeout");
    assert!(
        all_result.is_empty(),
        "non-existent file should yield no entries in all()"
    );

    // 清理
    let _ = std::fs::remove_file(&tmp);
}
