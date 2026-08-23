$ErrorActionPreference = 'Continue'
$env:PATH = "C:\Users\Administrator\.cargo\bin;C:\Program Files\Git\bin;$env:PATH"
Set-Location 'D:\test\aionui\新forge\aion-forge'
$env:CARGO_INCREMENTAL = '0'
$env:CARGO_HTTP_CHECK_REVOKE = 'false'
$env:FORGE_PG_URL = 'postgres://postgres:forge@localhost:15432/forge'
$env:FORGE_MINIO_URL = 'http://localhost:19000'
$env:FORGE_MINIO_AK = 'forgeadmin'
$env:FORGE_MINIO_SK = 'forgeadmin123'
$env:FORGE_LLM_LIVE = '1'
$llmLine = (Get-Content .env | Select-String '^FORGE_LLM_BASE_URL=').Line
$env:FORGE_LLM_BASE_URL = $llmLine -replace '^FORGE_LLM_BASE_URL=', ''
$keyLine = (Get-Content .env | Select-String '^FORGE_LLM_API_KEY=').Line
$env:FORGE_LLM_API_KEY = $keyLine -replace '^FORGE_LLM_API_KEY=', ''

$out = @()
$out += '=== cargo test --workspace ==='
cmd /c "cargo test --workspace > dod_test.log 2>&1"
$out += '见 dod_test.log（原生重定向）'
$tl = Get-Content 'dod_test.log'
$failed = ($tl | Select-String 'FAILED').Count
$sum = 0
foreach ($l in $tl) { if ($l -match 'ok\. (\d+) passed') { $sum += [int]$Matches[1] } }
$out += "TEST_FAILED_COUNT=$failed"
$out += "TOTAL_TESTS=$sum"
$out += '=== clippy --all-targets ==='
$cl = cargo clippy --workspace --all-targets 2>&1 | OutString
$warn = ($cl -split "`n" | Select-String '^warning|^error').Count
$out += "clippy warn/error lines: $warn"
$sum = ($t -split "`n" | Select-String 'ok\. (\d+) passed' | ForEach-Object { [int]$_.Matches[0].Groups[1].Value } | Measure-Object -Sum).Sum
$out += "TOTAL_TESTS=$sum"
$out += 'ALL_DONE'
$out | Set-Content 'dod_result.txt' -Encoding utf8BOM
