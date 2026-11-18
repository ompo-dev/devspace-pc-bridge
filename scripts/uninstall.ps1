#Requires -Version 5.1
[CmdletBinding()]
param([switch]$DisableFunnel)
$ErrorActionPreference = 'Stop'
$taskName = 'DevSpacePCBridge'
$folder = Join-Path $env:USERPROFILE '.devspace\pc-bridge'
$task = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
if ($task) {
    Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false
}
if ($DisableFunnel) {
    # Funnel pode ser compartilhado com outros servicos; a remocao e opcional.
    $ts = Get-Command tailscale.exe -ErrorAction SilentlyContinue
    if (-not $ts) { throw 'Tarefa removida, mas tailscale.exe ausente. Desative Funnel manualmente.' }
    & $ts.Source funnel reset
    if ($LASTEXITCODE -ne 0) { throw 'Nao foi possivel desativar o Funnel.' }
}
Write-Host "Tarefa removida. Configuracao e credenciais preservadas em $folder."
Write-Host 'DevSpace, Node, Git, Tailscale e dados de usuario NAO removidos.'
if (-not $DisableFunnel) { Write-Warning 'Funnel pode permanecer publico. Para desativar, use -DisableFunnel apos conferir outros servicos.' }
