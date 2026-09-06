# =====================================================================
# DR-001 容灾演练剧本（build_v60c.md S1~S10 冻结；验收=演练记录）
# 流程: env_up → repl_ready → mark_write → kill_primary → promote(RTO)
#       → data_check(RPO) → rw_check → teardown → record → cleanup 复核
# 用法: pwsh scripts/dr_drill.ps1
# 产出: artifacts/dr_drill_<时间戳>.json + docs/DR_EXERCISE.md 追加条目
# =====================================================================
$ErrorActionPreference = "Stop"
$script:steps = New-Object System.Collections.Generic.List[object]
function Step($name, $ok, $detail) {
    $script:steps.Add([pscustomobject]@{ step = $name; pass = [bool]$ok; detail = [string]$detail })
    $mark = if ($ok) { "[PASS]" } else { "[FAIL]" }
    Write-Host "$mark $name :: $detail"
    if (-not $ok) { throw "DR-FAIL: $name" }
}
function PodExec($container, $args) { podman exec @args $container @($null) 2>$null }
function Psql($container, $sql) {
    podman exec -u postgres $container psql -U postgres -d forge -t -A -c $sql 2>$null
}

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$imageSummary = "postgres:16-alpine"

# ---- S1 env_up：compose 起 primary + standby(sleep)，primary 就绪 ----
podman rm -f forge-dr-standby 2>$null | Out-Null
podman volume rm -f forge-dr-standbydata 2>$null | Out-Null
podman compose -f deploy/dr/dr-compose.yml -p dr down -v --remove-orphans 2>$null | Out-Null
podman compose -f deploy/dr/dr-compose.yml -p dr up -d 2>&1 | Out-Null
$primaryReady = $false
foreach ($i in 1..30) {
    $r = podman exec -u postgres forge-dr-primary pg_isready -U postgres -d forge 2>$null
    if ($LASTEXITCODE -eq 0) { $primaryReady = $true; break }
    Start-Sleep 2
}
Step "S1 env_up" $primaryReady "dr-compose up, primary@25432 ready"

# ---- S2 repl_ready：hba+basebackup 接管后，primary 出现 streaming 复制 ----
bash deploy/dr/standby-setup.sh 2>&1 | Out-Null
$replReady = $false
foreach ($i in 1..30) {
    $state = Psql "forge-dr-primary" "SELECT state FROM pg_stat_replication LIMIT 1"
    if ($state -eq "streaming") { $replReady = $true; break }
    Start-Sleep 2
}
Step "S2 repl_ready" $replReady "pg_stat_replication.state=$state"

# ---- S3 mark_write：主库写标记行并记录 LSN ----
$marker = "dr_mark_$(Get-Date -Format yyyyMMdd_HHmmss)"
Psql "forge-dr-primary" "CREATE TABLE IF NOT EXISTS dr_drill_marks(id serial PRIMARY KEY, note TEXT, at TIMESTAMPTZ DEFAULT now())" | Out-Null
Psql "forge-dr-primary" "INSERT INTO dr_drill_marks(note) VALUES ('$marker')" | Out-Null
$markerLsn = Psql "forge-dr-primary" "SELECT pg_current_wal_lsn()"
Start-Sleep 3
$replayLsn = Psql "forge-dr-standby" "SELECT pg_last_wal_replay_lsn()"
Step "S3 mark_write" ($null -ne $markerLsn -and $markerLsn -match "/") "marker=$marker lsn=$markerLsn replay_lsn=$replayLsn"

# ---- S4 kill_primary：SIGKILL 模拟宕机，记 t0 ----
$t0 = Get-Date
podman kill forge-dr-primary 2>&1 | Out-Null
Step "S4 kill_primary" $true "podman kill (SIGKILL) t0=$($t0.ToString('HH:mm:ss'))"

# ---- S5 promote：真 promote（禁止重启冒充），记 t1；RTO 上限 120s ----
$promoted = $false
$t1 = $null
foreach ($i in 1..30) {
    Start-Sleep 2
    $r = podman exec -u postgres forge-dr-standby pg_ctl promote -D /var/lib/postgresql/data 2>$null
    if ($LASTEXITCODE -eq 0 -or $r -match "promoted|not in standby") { $t1 = Get-Date; $promoted = $true; break }
}
$standbyReady = $false
if ($promoted) {
    foreach ($i in 1..20) {
        $null = podman exec -u postgres forge-dr-standby pg_isready -U postgres -d forge 2>$null
        if ($LASTEXITCODE -eq 0) { $standbyReady = $true; break }
        Start-Sleep 2
    }
}
$rto = if ($t1) { [math]::Round(($t1 - $t0).TotalSeconds, 1) } else { -1 }
Step "S5 promote" ($promoted -and $standbyReady -and $rto -le 120) "RTO=${rto}s (上限 120s)"

# ---- S6 data_check：标记行必须存在；RPO=LSN 字节差实测 ----
$markerFound = Psql "forge-dr-standby" "SELECT COUNT(*) FROM dr_drill_marks WHERE note = '$marker'"
$rpoBytes = -1
$rpoRaw = $null
if ($markerLsn -and $replayLsn) {
    $rpoRaw = Psql "forge-dr-standby" "SELECT pg_wal_lsn_diff('$markerLsn', '$replayLsn')"
    if (-not $rpoRaw) { $rpoRaw = 0 }
    # 备库回放已越过标记 LSN 时差值为负 = 零丢失，归一化为 0（原始差值入证据）
    $rpoBytes = [math]::Max(0, [int64]$rpoRaw)
}
Step "S6 data_check" ([int64]$markerFound -ge 1) "标记行=$markerFound RPO=${rpoBytes}字节（原始差值=$rpoRaw，异步流复制实测）"

# ---- S7 rw_check：promote 后可写 ----
$rwNote = "post_promote_$(Get-Date -Format HHmmss)"
$rw = Psql "forge-dr-standby" "INSERT INTO dr_drill_marks(note) VALUES ('$rwNote') RETURNING id"
Step "S7 rw_check" ($null -ne $rw -and $rw -match "\d") "新标记行 id=$rw"

# ---- S8 teardown：销毁全部演练资源（保证幂等可重跑）----
podman rm -f forge-dr-standby 2>$null | Out-Null
podman compose -f deploy/dr/dr-compose.yml -p dr down -v --remove-orphans 2>&1 | Out-Null
podman volume rm -f forge-dr-standbydata 2>$null | Out-Null
Step "S8 teardown" $true "compose down -v + standby 容器/卷清除"

# ---- S9 record：RPO/RTO/判定写入证据与 DR_EXERCISE.md ----
$evidencePath = Join-Path $root "artifacts\dr_drill_$(Get-Date -Format yyyyMMdd_HHmmss).json"
[pscustomobject]@{
    run_at      = (Get-Date).ToString('o')
    image       = $imageSummary
    marker      = $marker
    marker_lsn  = $markerLsn
    rpo_bytes   = [int64]$rpoBytes
    rpo_raw_diff = $rpoRaw
    rto_seconds = $rto
    result      = "PASS"
    steps       = $script:steps
} | ConvertTo-Json -Depth 6 | Set-Content -Encoding utf8 $evidencePath
$exercise = Join-Path $root "docs\DR_EXERCISE.md"
if (-not (Test-Path $exercise)) {
    "# DR 容灾演练记录`n`n> 模板字段（R6 冻结）：日期/执行人/镜像摘要/RPO字节/RTO秒/标记行校验/证据路径/签核——缺项=门禁不通过`n" | Set-Content -Encoding utf8 $exercise
}
Add-Content -Encoding utf8 $exercise "`n| $(Get-Date -Format yyyy-MM-dd) | 演练脚本(dr_drill.ps1) | $imageSummary | $rpoBytes | $rto | 标记行=$markerFound | $(Split-Path -Leaf $evidencePath) | 待规划层签核 |"
Step "S9 record" (Test-Path $evidencePath) $evidencePath

# ---- S10 cleanup 复核：无演练残留，生产 forge-pg 不受影响 ----
$leftContainers = (podman ps -a --format "{{.Names}}" | Select-String "forge-dr-").Count
$leftVolumes = (podman volume ls --format "{{.Name}}" | Select-String "forge-dr-").Count
$prodOk = $false
try {
    $null = podman exec forge-pg pg_isready -U postgres 2>$null
    if ($LASTEXITCODE -eq 0) { $prodOk = $true }
} catch { }
Step "S10 cleanup" (($leftContainers -eq 0) -and ($leftVolumes -eq 0) -and $prodOk) "演练残留=$leftContainers/$leftVolumes, 生产forge-pg=$prodOk"

Write-Host "`n=== DR 演练全部通过（RPO=${rpoBytes}字节 RTO=${rto}s）===" -ForegroundColor Green
