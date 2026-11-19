#Requires -Version 5.1
[CmdletBinding()]
param(
    [switch]$DisableFunnel,
    [switch]$Force
)
$ErrorActionPreference = 'Stop'
$taskName = 'DevSpacePCBridge'
$port = '7676'
$folder = Join-Path $env:USERPROFILE '.devspace\pc-bridge'
$task = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
if ($task) {
    Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false
}
if ($DisableFunnel) {
    $tailscale = Get-Command tailscale.exe -ErrorAction SilentlyContinue
    $tailscalePath = if ($tailscale) { $tailscale.Source } else { Join-Path $env:ProgramFiles 'Tailscale\tailscale.exe' }
    if (-not (Test-Path -LiteralPath $tailscalePath)) { throw 'Tarefa removida, mas tailscale.exe ausente. Desative Funnel manualmente.' }
    # "funnel reset" clears every Funnel/Serve rule on this machine, not only ours,
    # so refuse when other local ports are published unless -Force is given.
    $status = (& $tailscalePath funnel status | Out-String)
    $ports = @([regex]::Matches($status, '(?:127\.0\.0\.1|localhost):(\d+)') | ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique)
    $others = @($ports | Where-Object { $_ -ne $port })
    if ($others.Count -gt 0 -and -not $Force) {
        throw ("O Funnel tambem publica outras portas ({0}); 'funnel reset' apagaria tudo. Repita com -Force para confirmar, ou remova so a regra da porta {1} manualmente." -f ($others -join ', '), $port)
    }
    & $tailscalePath funnel reset
    if ($LASTEXITCODE -ne 0) { throw 'Nao foi possivel desativar o Funnel.' }
    Write-Host 'Funnel desativado.'
}
Write-Host "Tarefa removida. Configuracao e credenciais preservadas em $folder."
Write-Host 'DevSpace, Node, Git, Tailscale e dados de usuario NAO removidos.'
if (-not $DisableFunnel) { Write-Warning 'Funnel pode permanecer publico. Para desativar, use -DisableFunnel apos conferir outros servicos.' }
