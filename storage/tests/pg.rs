//! PH2-001 集成测试：对真实 PostgreSQL 验证三个存储 trait。
//!
//! 运行方式（DoD 必须带环境变量执行）：
//! ```powershell
//! $env:FORGE_PG_URL = "postgres://postgres:forge@127.0.0.1:15432/forge"
//! cargo test -p forge-storage
//! ```
//! 未设置 `FORGE_PG_URL` 时打印跳过说明（不失败）——但 DoD 不接受跳过态。

use forge_artifact::{ArtifactKind, ArtifactStore};
use forge_core::TaskId;
use forge_evidence::EvidenceStore;
use forge_session::{SessionEventKind, SessionState, SessionStore};
use chrono::Utc;
use forge_storage::*;

async fn pool_or_skip() -> Option<sqlx::PgPool> {
    match std::env::var("FORGE_PG_URL") {
        Ok(url) => Some(connect_and_migrate(&url).await.unwrap()),
        Err(_) => {
            eprintln!("[skip] FORGE_PG_URL 未设置——本测试需真实 PostgreSQL");
            None
        }
    }
}

#[tokio::test]
async fn sessions_full_state_machine_flow() {
    let Some(pool) = pool_or_skip().await else { return };
    let store = PgSessionStore::new(pool);

    // 创建
    let s = store.create(TaskId::new_task_id()).await.unwrap();
    assert_eq!(s.state, SessionState::Active);
    assert!(s.events.is_empty());

    // 正常事件序列
    store
        .append(&s.id, SessionEventKind::TaskReceived, serde_json::json!({"n":1}))
        .await
        .unwrap();
    store
        .append(&s.id, SessionEventKind::ActionDispatched, serde_json::json!({"n":2}))
        .await
        .unwrap();

    // 失败→恢复→完成 全链路
    store.append(&s.id, SessionEventKind::Failed, serde_json::json!({})).await.unwrap();
    assert_eq!(
        store.get(&s.id).await.unwrap().state,
        SessionState::Failed
    );
    store.append(&s.id, SessionEventKind::Recovered, serde_json::json!({})).await.unwrap();
    store
        .append(&s.id, SessionEventKind::ActionDispatched, serde_json::json!({}))
        .await
        .unwrap();
    assert_eq!(
        store.get(&s.id).await.unwrap().state,
        SessionState::Active
    );
    store.append(&s.id, SessionEventKind::Completed, serde_json::json!({})).await.unwrap();

    // 读回校验：seq 连续、kind 往返、状态终态
    let got = store.get(&s.id).await.unwrap();
    assert_eq!(got.state, SessionState::Completed);
    let seqs: Vec<u64> = got.events.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, vec![1, 2, 3, 4, 5, 6]);
    assert_eq!(got.events[0].kind, SessionEventKind::TaskReceived);
    assert_eq!(got.events[2].kind, SessionEventKind::Failed);

    // 终态后追加 → InvalidState
    let err = store
        .append(&s.id, SessionEventKind::TaskReceived, serde_json::json!({}))
        .await;
    assert!(err.is_err(), "completed session must reject further events");

    // list 包含该会话
    assert!(store.list().await.unwrap().iter().any(|x| *x == s.id));
}

#[tokio::test]
async fn artifacts_put_read_checksum() {
    let Some(pool) = pool_or_skip().await else { return };
    let store = PgArtifactStore::new(pool);

    let content = b"hello forge".to_vec();
    let art = store
        .put("hello.txt".into(), ArtifactKind::Document, content.clone(), serde_json::json!({"src":"test"}))
        .await
        .unwrap();
    assert_eq!(art.size_bytes, content.len() as u64);
    assert_eq!(art.checksum_sha256.len(), 64);

    // 读回内容一致
    let back = store.read(&art.id).await.unwrap();
    assert_eq!(back, content);

    // 元数据一致
    let meta = store.get_meta(&art.id).await.unwrap();
    assert_eq!(meta.name, "hello.txt");
    assert_eq!(meta.kind, ArtifactKind::Document);
    assert_eq!(meta.meta["src"], "test");

    // 相同内容不同 id、相同 checksum
    let art2 = store
        .put("again.txt".into(), ArtifactKind::Log, content, serde_json::json!({}))
        .await
        .unwrap();
    assert_ne!(art.id, art2.id);
    assert_eq!(art.checksum_sha256, art2.checksum_sha256);

    // 未找到 → NotFound
    use forge_core::ArtifactId;
    assert!(store.read(&ArtifactId::new_artifact_id()).await.is_err());
}

#[tokio::test]
async fn evidence_immutable_and_query() {
    let Some(pool) = pool_or_skip().await else { return };
    let store = PgEvidenceStore::new(pool);

    use chrono::TimeZone;
    let crit = format!("AC-{}", Utc::now().timestamp_millis()); // 每次运行唯一，避免跨运行累积干扰

    // 固定时间写入 → 原样保留
    let fixed = Utc.with_ymd_and_hms(2026, 8, 23, 0, 0, 0).unwrap();
    let e1 = forge_evidence::Evidence {
        id: forge_core::new_evidence_id(),
        kind: forge_evidence::EvidenceKind::CommandOutput,
        criterion_id: crit.clone(),
        content: "exit code 0".into(),
        produced_by: "CommandVerifier".into(),
        at: fixed,
    };
    let id1 = store.put(e1.clone()).await.unwrap();
    let got = store.get(&id1).await.unwrap();
    assert_eq!(got.at, fixed, "非零 at 必须原样保留");
    assert_eq!(got.content, "exit code 0");

    // 零值 at → 存储补齐
    let mut e2 = forge_evidence::Evidence {
        id: forge_core::new_evidence_id(),
        kind: forge_evidence::EvidenceKind::TestReport,
        criterion_id: crit.clone(),
        content: "report".into(),
        produced_by: "Tester".into(),
        at: chrono::DateTime::<Utc>::default(),
    };
    store.put(e2.clone()).await.unwrap();
    let g2 = store.get(&e2.id).await.unwrap();
    assert_ne!(g2.at, chrono::DateTime::<Utc>::default());

    // 同 ID 重复写入 → InvalidState
    e2.id = id1.clone();
    assert!(store.put(e2).await.is_err());

    // 按 criterion 查询命中且 id 正确回读
    let hits = store.by_criterion(&crit).await.unwrap();
    assert_eq!(hits.len(), 2);
    assert!(hits.iter().any(|h| h.id == id1));
}
