-- DR-001: 主库初始化（build_v60c.md 原样抄入）
CREATE ROLE replicator WITH LOGIN REPLICATION PASSWORD 'repl_forge';
-- hba 追加行（standby-setup.sh 负责落盘生效）:
-- host replication replicator 0.0.0.0/0 scram-sha-256
