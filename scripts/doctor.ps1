#Requires -Version 5.1
[CmdletBinding()]
param()
$ErrorActionPreference = 'Continue'
# Version this bridge was tested with (keep in sync with install.ps1).
$DevSpaceVersion = '1.1.1'
$cfgPath = Join-Path $env:USERPROFILE '.devspace\pc-bridge\settings.json'
$settings = if (Test-Path $cfgPath) { Get-Content $cfgPath -Raw | ConvertFrom-Json } else { $null }
$existingDevSpace = Test-Path (Join-Path $env:USERPROFILE '.devspace\config.jsonc')
$existingTailscale = Join-Path $env:USERPROFILE '.devspace\autostart\tailscale.json'

function Get-BroadRootReason([string]$Path) {
    try { $full = [IO.Path]::GetFullPath($Path).TrimEnd('\') } catch { return 'caminho invalido' }
    $profileDir = $env:USERPROFILE.TrimEnd('\')
    if ($full.Length -le 2) { return 'raiz de um disco' }
    if ($full -ieq $profileDir) { return 'pasta pessoal inteira' }
    if ($profileDir.StartsWith($full + '\', [StringComparison]::OrdinalIgnoreCase)) { return 'pasta que contem a pasta pessoal' }
    $sensitive = @('.ssh', '.aws', '.gnupg', '.kube', '.claude', '.codex', '.devspace', 'appdata')
    foreach ($part in $full.Split('\')) {
        if ($sensitive -contains $part.ToLowerInvariant()) { return 'pasta com credenciais de ferramentas' }
    }
    return $null
}

if (-not $settings -and (Test-Path $existingTailscale)) {
    try {
        $oldSettings = Get-Content $existingTailscale -Raw | ConvertFrom-Json
        if ($oldSettings.hostname -match '^[a-z0-9][a-z0-9.-]*\.ts\.net$') {
            $settings = [pscustomobject]@{
                hostname = [string]$oldSettings.hostname
                port = 7676
                root = '(instalacao anterior, consulte DevSpace allowedRoots)'
            }
            Write-Host 'Configuracao de DevSpace/Tailscale existente encontrada (modo somente leitura).'
        }
    } catch { Write-Host 'Nao foi possivel ler a configuracao anterior do Tailscale.' }
}
Write-Host '=== DevSpace PC Bridge / diagnostico somente leitura ==='
foreach ($name in @('node.exe', 'git.exe', 'devspace.cmd')) {
    $cmd = Get-Command $name -ErrorAction SilentlyContinue
    Write-Host ("{0}: {1}" -f $name, $(if ($cmd) { 'OK' } else { 'AUSENTE' }))
}
$devspaceCmd = Get-Command devspace.cmd -ErrorAction SilentlyContinue
if ($devspaceCmd) {
    $pkg = Join-Path (Split-Path $devspaceCmd.Source -Parent) 'node_modules\@waishnav\devspace\package.json'
    if (Test-Path -LiteralPath $pkg) {
        try {
            $installed = (Get-Content -LiteralPath $pkg -Raw | ConvertFrom-Json).version
            Write-Host ("DevSpace versao: {0} (testada com o bridge: {1})" -f $installed, $DevSpaceVersion)
        } catch { Write-Host 'Nao foi possivel ler a versao do DevSpace.' }
    }
}
$tailscale = Get-Command tailscale.exe -ErrorAction SilentlyContinue
if (-not $tailscale) {
    $path = Join-Path $env:ProgramFiles 'Tailscale\tailscale.exe'
    if (Test-Path $path) { $tailscale = @{ Source = $path } }
}
Write-Host ("Tailscale: {0}" -f $(if ($tailscale) { 'OK' } else { 'AUSENTE' }))
$task = Get-ScheduledTask -TaskName DevSpacePCBridge -ErrorAction SilentlyContinue
Write-Host ("Tarefa DevSpacePCBridge: {0}" -f $(if ($task) { $task.State } else { 'AUSENTE' }))
$bridgeDir = Join-Path $env:USERPROFILE '.devspace\pc-bridge'
if (Test-Path -LiteralPath (Join-Path $bridgeDir 'paused')) { Write-Host 'Publicacao: PAUSADA (arquivo paused presente)' }
$untilPath = Join-Path $bridgeDir 'expose-until'
if (Test-Path -LiteralPath $untilPath) {
    $untilRaw = (Get-Content -LiteralPath $untilPath -Raw)
    $untilSeconds = 0L
    if ($untilRaw -and [long]::TryParse($untilRaw.Trim(), [ref]$untilSeconds)) {
        Write-Host ('Exposicao ate: {0}' -f [DateTimeOffset]::FromUnixTimeSeconds($untilSeconds).LocalDateTime)
    }
}
if ($settings) {
    Write-Host ("Hostname: {0}" -f $settings.hostname)
    Write-Host ("Raiz: {0}" -f $settings.root)
    foreach ($target in @("http://127.0.0.1:$($settings.port)/healthz", "https://$($settings.hostname)/healthz")) {
        try { $resp = Invoke-WebRequest -Uri $target -UseBasicParsing -TimeoutSec 8; Write-Host "$target => HTTP $($resp.StatusCode)" }
        catch { Write-Host "$target => INDISPONIVEL" }
    }
    if ($tailscale) { & $tailscale.Source funnel status }
    Write-Host ("MCP: https://{0}/mcp" -f $settings.hostname)
} elseif ($existingDevSpace) {
    Write-Host 'DevSpace existente detectado, mas URL MCP nao localizada. Revise publicBaseUrl sem expor credenciais.'
} else { Write-Host 'Nenhuma instalacao identificada. Execute install.ps1.' }

# The Funnel makes these folders reachable from the internet, so check what the server really allows.
if ($devspaceCmd -and $existingDevSpace) {
    try {
        $cfg = (& devspace.cmd config get | Out-String | ConvertFrom-Json)
        foreach ($allowed in @($cfg.workspaces.allowedRoots)) {
            if (-not $allowed) { continue }
            $reason = Get-BroadRootReason ([string]$allowed)
            if ($reason) { Write-Host "ATENCAO: allowedRoots inclui '$allowed' ($reason). Restrinja no DevSpace." }
            else { Write-Host "allowedRoots: $allowed" }
        }
    } catch { Write-Host 'Nao foi possivel ler allowedRoots (devspace config get).' }
}
Write-Host 'Health 200 nao comprova autenticacao MCP. Teste open_workspace e read no ChatGPT.'
