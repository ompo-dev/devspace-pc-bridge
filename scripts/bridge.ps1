#Requires -Version 5.1
[CmdletBinding()]
param([switch]$Once)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$base = $PSScriptRoot
$settingsFile = Join-Path $base 'settings.json'
$logDir = Join-Path $base 'logs'
New-Item -ItemType Directory -Path $logDir -Force | Out-Null
$logFile = Join-Path $logDir 'bridge.log'
# Markers shared with DevSpacePCBridge.exe: publishing paused on purpose, and a
# deadline (Unix seconds) after which publishing is paused automatically.
$pausedFlag = Join-Path $base 'paused'
$untilFile = Join-Path $base 'expose-until'
$maxServerLogs = 10
$maxBridgeLogBytes = 1MB
function Log([string]$message) { Add-Content -LiteralPath $logFile -Value ("{0} {1}" -f (Get-Date -Format 'o'), $message) }
function Healthy([string]$url) {
    try { return (Invoke-WebRequest -Uri $url -UseBasicParsing -TimeoutSec 6).StatusCode -eq 200 }
    catch { return $false }
}
function Find-Tailscale {
    $cmd = Get-Command tailscale.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $path = Join-Path $env:ProgramFiles 'Tailscale\tailscale.exe'
    if (Test-Path $path) { return $path }
    return $null
}
function Get-ExposeUntil {
    if (-not (Test-Path -LiteralPath $untilFile)) { return $null }
    $raw = Get-Content -LiteralPath $untilFile -Raw
    if ([string]::IsNullOrWhiteSpace($raw)) { return $null }
    $parsed = 0L
    if ([long]::TryParse($raw.Trim(), [ref]$parsed)) { return $parsed }
    return $null
}
# "funnel reset" is the only documented way to drop a rule and it clears all of them,
# so it is used only when this bridge's port is the only thing published.
function Stop-Funnel([string]$tailscale) {
    $status = (& $tailscale funnel status 2>$null | Out-String)
    $ports = @([regex]::Matches($status, '(?:127\.0\.0\.1|localhost):(\d+)') | ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique)
    if ($ports.Count -eq 0) { return }
    $others = @($ports | Where-Object { $_ -ne [string]$port })
    if ($others.Count -gt 0) {
        Log ("Pausa nao aplicada: o Funnel tambem publica outras portas ({0}); 'funnel reset' apagaria tudo." -f ($others -join ', '))
        return
    }
    & $tailscale funnel reset 2>$null | Out-Null
    if ($LASTEXITCODE -ne 0) { Log 'Nao foi possivel desativar o Funnel durante a pausa.' }
    else { Log 'Funnel desativado pela pausa.' }
}
# Every DevSpace start creates a new pair of logs, so keep only the most recent ones.
function Remove-OldLogs {
    $current = Get-Item -LiteralPath $logFile -ErrorAction SilentlyContinue
    if ($current -and $current.Length -gt $maxBridgeLogBytes) {
        Move-Item -LiteralPath $logFile -Destination "$logFile.1" -Force
    }
    foreach ($pattern in @('devspace-*.out.log', 'devspace-*.err.log')) {
        Get-ChildItem -LiteralPath $logDir -Filter $pattern -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending |
            Select-Object -Skip $maxServerLogs |
            Remove-Item -Force -ErrorAction SilentlyContinue
    }
}
if (-not (Test-Path -LiteralPath $settingsFile)) { throw 'settings.json ausente. Execute install.ps1.' }
$settings = Get-Content -LiteralPath $settingsFile -Raw | ConvertFrom-Json
$port = [int]$settings.port
$hostname = [string]$settings.hostname
if ($port -ne 7676 -or $hostname -notmatch '^[a-z0-9][a-z0-9.-]*\.ts\.net$') { throw 'settings.json invalido.' }
$localHealth = "http://127.0.0.1:$port/healthz"
$publicHealth = "https://$hostname/healthz"
$mutex = New-Object System.Threading.Mutex($false, "Local\DevSpacePCBridge-$env:USERNAME")
$acquired = $false
try {
    try { $acquired = $mutex.WaitOne(0) }
    catch [System.Threading.AbandonedMutexException] { $acquired = $true }
    if (-not $acquired) { Log 'Supervisor duplicado ignorado.'; return }
    Log 'Supervisor iniciado.'
    $server = $null
    $lastHealth = ''
    # Back off while DevSpace keeps failing to come up: 30s, 60s, 120s ... capped at 10 minutes.
    $startFailures = 0
    $nextStart = [datetime]::MinValue
    do {
        try {
            Remove-OldLogs
            $until = Get-ExposeUntil
            $paused = Test-Path -LiteralPath $pausedFlag
            if (-not $paused -and $null -ne $until -and [DateTimeOffset]::UtcNow.ToUnixTimeSeconds() -ge $until) {
                Log 'Prazo de exposicao encerrado; pausando a publicacao.'
                Set-Content -LiteralPath $pausedFlag -Value 'expired'
                Remove-Item -LiteralPath $untilFile -Force -ErrorAction SilentlyContinue
                $paused = $true
            }
            $ts = Find-Tailscale
            if ($ts -and $paused) {
                # Checked every cycle so a Funnel switched on by hand during a pause is closed again.
                Stop-Funnel $ts
            } elseif ($ts) {
                $status = (& $ts funnel status --json 2>$null | Out-String)
                if ($status -notmatch [regex]::Escape($hostname) -or $status -notmatch "127\.0\.0\.1:$port") {
                    Log 'Funnel ausente; tentativa de restauracao.'
                    & $ts funnel --bg --yes $port 2>$null | Out-Null
                    if ($LASTEXITCODE -ne 0) { Log 'Funnel nao restaurado; pode exigir permissoes.' }
                }
            }
            if (Healthy $localHealth) {
                $startFailures = 0
                $nextStart = [datetime]::MinValue
            } else {
                $cmd = Get-Command devspace.cmd -ErrorAction SilentlyContinue
                $node = Get-Command node.exe -ErrorAction SilentlyContinue
                if ($server) { $server.Refresh() }
                $listening = @(Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue).Count -gt 0
                $mayStart = (Get-Date) -ge $nextStart
                if (-not $listening -and $node -and $cmd -and (-not $server -or $server.HasExited) -and $mayStart) {
                    $js = Join-Path (Split-Path $cmd.Source -Parent) 'node_modules\@waishnav\devspace\bin\devspace.js'
                    if (Test-Path $js) {
                        $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
                        $out = Join-Path $logDir "devspace-$stamp.out.log"
                        $err = Join-Path $logDir "devspace-$stamp.err.log"
                        $server = Start-Process -FilePath $node.Source -ArgumentList ('"' + $js + '" serve') -WindowStyle Hidden -PassThru -RedirectStandardOutput $out -RedirectStandardError $err
                        $delay = [Math]::Min(30 * [Math]::Pow(2, $startFailures), 600)
                        $startFailures++
                        $nextStart = (Get-Date).AddSeconds($delay)
                        Log ("DevSpace iniciado (tentativa {0}; nova tentativa so apos {1}s sem health)." -f $startFailures, $delay)
                    } else { Log 'Arquivo DevSpace nao encontrado.' }
                } elseif ($listening) { Log 'Porta ocupada, health falhou; nao iniciando servidor duplicado.' }
            }
            $health = if ($paused) { 'paused' } elseif ((Healthy $localHealth) -and (Healthy $publicHealth)) { 'healthy' } else { 'unavailable' }
            if ($health -ne $lastHealth) { Log "Estado: $health"; $lastHealth = $health }
        } catch { Log "Erro: $($_.Exception.Message)" }
        if (-not $Once) { Start-Sleep -Seconds 30 }
    } while (-not $Once)
} finally {
    if ($acquired) { $mutex.ReleaseMutex() }
    $mutex.Dispose()
}
