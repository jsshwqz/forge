#!/usr/bin/env bash
# DR-001: standby 接管三步（R1：必须 pg_basebackup -R，禁止手工 touch 伪搭建）
set -euo pipefail

# 1) primary 落 hba 复制行并 reload
podman exec -u postgres forge-dr-primary bash -c \
  "grep -q 'host replication replicator' \$PGDATA/pg_hba.conf || echo 'host replication replicator 0.0.0.0/0 scram-sha-256' >> \$PGDATA/pg_hba.conf; pg_ctl reload -D \$PGDATA" >/dev/null

# 2) basebackup 写入 standby 卷（-R 生成 standby.signal + primary_conninfo）
podman exec -u postgres -e PGPASSWORD=repl_forge forge-dr-standby bash -c \
  "pg_basebackup -h forge-dr-primary -U replicator -D \$PGDATA -R -P" >/dev/null

# 3) 重建 standby 容器为真 postgres（同一卷，entrypoint 识别 standby.signal 进入恢复）
podman rm -f forge-dr-standby >/dev/null 2>&1 || true
podman run -d --name forge-dr-standby --network forge-dr-net -p 25433:5432 \
  -v forge-dr-standbydata:/var/lib/postgresql/data postgres:16-alpine >/dev/null

echo "standby rebuilt (standby.signal + primary_conninfo)"
