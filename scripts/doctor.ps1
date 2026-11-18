#Requires -Version 5.1
[CmdletBinding()]
param()
$ErrorActionPreference = 'Continue'
$cfgPath = Join-Path $env:USERPROFILE '.devspace\pc-bridge\settings.json'
$settings = if (Test-Path $cfgPath) { Get-Content $cfgPath -Raw | ConvertFrom-Json } else { $null }
$existingDevSpace = Test-Path (Join-Path $env:USERPROFILE '.devspace\config.jsonc')
$existingTailscale = Join-Path $env:USERPROFILE '.devspace\autostart\tailscale.json'
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
$tailscale = Get-Command tailscale.exe -ErrorAction SilentlyContinue
if (-not $tailscale) {
    $path = Join-Path $env:ProgramFiles 'Tailscale\tailscale.exe'
    if (Test-Path $path) { $tailscale = @{ Source = $path } }
}
Write-Host ("Tailscale: {0}" -f $(if ($tailscale) { 'OK' } else { 'AUSENTE' }))
$task = Get-ScheduledTask -TaskName DevSpacePCBridge -ErrorAction SilentlyContinue
Write-Host ("Tarefa DevSpacePCBridge: {0}" -f $(if ($task) { $task.State } else { 'AUSENTE' }))
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
Write-Host 'Health 200 nao comprova autenticacao MCP. Teste open_workspace e read no ChatGPT.'
