#Requires -Version 5.1
[CmdletBinding()]
param(
    [string]$WorkspacesRoot = (Split-Path -Parent $PSScriptRoot),
    [switch]$Check,
    [switch]$SkipPackageInstall
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$taskName = 'DevSpacePCBridge'
$port = 7676
$installDir = Join-Path $env:USERPROFILE '.devspace\pc-bridge'

function Refresh-Path {
    $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + [Environment]::GetEnvironmentVariable('Path', 'User') + ';' + $env:Path
}
function Find-Tailscale {
    $cmd = Get-Command tailscale.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    $fallback = Join-Path $env:ProgramFiles 'Tailscale\tailscale.exe'
    if (Test-Path $fallback) { return $fallback }
    return $null
}
function Ensure-Package([string]$Command, [string]$Package) {
    if (Get-Command $Command -ErrorAction SilentlyContinue) { return }
    if ($SkipPackageInstall) { throw "$Command ausente. Instale $Package e repita." }
    if (-not (Get-Command winget.exe -ErrorAction SilentlyContinue)) { throw 'winget ausente. Instale App Installer e repita.' }
    Write-Host "Instalando $Package..."
    & winget.exe install --exact --id $Package --accept-source-agreements --accept-package-agreements --silent
    if ($LASTEXITCODE -ne 0) { throw "Falha ao instalar $Package ($LASTEXITCODE)." }
    Refresh-Path
    if (-not (Get-Command $Command -ErrorAction SilentlyContinue)) { throw "$Package instalado; abra um novo PowerShell e repita." }
}

if ($Check) {
    Write-Host 'Preflight: nenhuma alteracao'
    foreach ($name in @('winget.exe', 'node.exe', 'npm.cmd', 'git.exe', 'devspace.cmd')) {
        Write-Host ("  {0}: {1}" -f $name, [bool](Get-Command $name -ErrorAction SilentlyContinue))
    }
    Write-Host ("  Tailscale: {0}" -f [bool](Find-Tailscale))
    Write-Host ("  Config existente: {0}" -f (Test-Path (Join-Path $env:USERPROFILE '.devspace\config.jsonc')))
    Write-Host ("  Tarefa deste projeto: {0}" -f [bool](Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue))
    return
}

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$admin = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $admin.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Abra o PowerShell como Administrador e execute novamente .\install.ps1'
}
$root = (Resolve-Path -LiteralPath $WorkspacesRoot -ErrorAction Stop).Path
if (-not (Test-Path -LiteralPath $root -PathType Container)) { throw "Raiz invalida: $root" }

Ensure-Package 'git.exe' 'Git.Git'
Ensure-Package 'node.exe' 'OpenJS.NodeJS.LTS'
Ensure-Package 'npm.cmd' 'OpenJS.NodeJS.LTS'
$version = [version]((& node.exe --version).TrimStart('v'))
if ($version -lt [version]'22.19.0' -or $version.Major -ge 27) {
    throw "Node $version nao suportado pelo DevSpace (requer >=22.19 e <27). Atualize para LTS."
}
if (-not (Find-Tailscale)) { Ensure-Package 'tailscale.exe' 'Tailscale.Tailscale' }
$tailscale = Find-Tailscale
if (-not $tailscale) { throw 'Tailscale nao encontrado; abra novo terminal e repita.' }
if (-not (Get-Command devspace.cmd -ErrorAction SilentlyContinue)) {
    Write-Host 'Instalando DevSpace oficial...'
    & npm.cmd install --global '@waishnav/devspace'
    if ($LASTEXITCODE -ne 0) { throw 'Falha ao instalar @waishnav/devspace.' }
    Refresh-Path
}
if (-not (Get-Command devspace.cmd -ErrorAction SilentlyContinue)) {
    throw 'devspace.cmd nao encontrado. Abra novo terminal e repita.'
}

$tsStatus = $null
try { $tsStatus = (& $tailscale status --json | ConvertFrom-Json) } catch {}
if (-not $tsStatus -or $tsStatus.BackendState -ne 'Running') {
    Write-Host 'Conclua login no Tailscale no navegador.'
    & $tailscale login
    if ($LASTEXITCODE -ne 0) { throw 'Login Tailscale pendente. Entre e repita.' }
    $tsStatus = & $tailscale status --json | ConvertFrom-Json
}
if ($tsStatus.BackendState -ne 'Running' -or -not $tsStatus.Self.DNSName) {
    throw 'Tailscale ainda nao esta online.'
}
$hostname = ([string]$tsStatus.Self.DNSName).TrimEnd('.')
if ($hostname -notmatch '^[a-z0-9][a-z0-9.-]*\.ts\.net$') { throw "Hostname inesperado: $hostname" }
$origin = "https://$hostname"

$configPath = Join-Path $env:USERPROFILE '.devspace\config.jsonc'
if (Test-Path -LiteralPath $configPath) {
    $cfg = (& devspace.cmd config get | Out-String | ConvertFrom-Json)
    if ($cfg.server.port -ne $port -or $cfg.server.publicBaseUrl -ne $origin) {
        throw "DevSpace existente usa outra porta ou origem. Nao alterado. Confira devspace config get antes de migrar para $origin."
    }
    if (-not (@($cfg.workspaces.allowedRoots) -contains $root)) {
        Write-Warning 'Raiz nao consta em allowedRoots. Config existente preservada; ajuste manualmente se necessario.'
    }
    Write-Host 'Configuracao DevSpace existente preservada.'
} else {
    Write-Host "Inicializando DevSpace, raiz: $root"
    & devspace.cmd init --yes --public-url $origin --roots $root
    if ($LASTEXITCODE -ne 0) { throw 'Falha em devspace init.' }
    Write-Host 'Guarde a senha Owner mostrada pelo DevSpace. Nunca publique auth.json.'
}

# Funnel e publico: o DevSpace exige OAuth. A ativacao pode solicitar permissao na conta.
Write-Host "Ativando Funnel HTTPS -> 127.0.0.1:$port..."
& $tailscale funnel --bg --yes $port
if ($LASTEXITCODE -ne 0) { throw 'Funnel falhou. Habilite Funnel/HTTPS no Tailscale e repita como Administrador.' }

New-Item -ItemType Directory -Path $installDir -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'scripts\bridge.ps1') -Destination (Join-Path $installDir 'bridge.ps1') -Force
@{ hostname = $hostname; port = $port; root = $root; installedAt = (Get-Date).ToString('o') } |
    ConvertTo-Json | Set-Content -LiteralPath (Join-Path $installDir 'settings.json') -Encoding UTF8

$installedScript = Join-Path $installDir 'bridge.ps1'
$existingTask = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
if ($existingTask -and ($existingTask.Actions | Where-Object { $_.Arguments -notlike "*$installedScript*" })) {
    throw "$taskName ja existe e aponta para outro script; nao foi substituida."
}
$action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument ('-NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File "' + $installedScript + '"')
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $identity.Name
$taskPrincipal = New-ScheduledTaskPrincipal -UserId $identity.Name -LogonType Interactive -RunLevel Limited
$taskSettings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Seconds 0) -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -MultipleInstances IgnoreNew
Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger -Principal $taskPrincipal -Settings $taskSettings -Description 'DevSpace e Tailscale Funnel apos login' -Force | Out-Null
Start-ScheduledTask -TaskName $taskName
Write-Host ''
Write-Host 'Instalacao concluida. Confirme o OAuth e execute scripts\doctor.ps1.'
Write-Host "MCP: $origin/mcp"
Write-Host "Owner: $origin/web/"
Write-Host "Tarefa: $taskName"
