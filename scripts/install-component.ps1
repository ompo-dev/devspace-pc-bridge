#Requires -Version 5.1
[CmdletBinding()]
param(
    [ValidateSet('Git', 'Node', 'Tailscale', 'Cloudflared', 'DevSpace')]
    [string]$Component,
    [switch]$Check
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$components = @{
    Git = @{ Command = 'git.exe'; Package = 'Git.Git' }
    Node = @{ Command = 'node.exe'; Package = 'OpenJS.NodeJS.LTS' }
    Tailscale = @{ Command = 'tailscale.exe'; Package = 'Tailscale.Tailscale' }
    Cloudflared = @{ Command = 'cloudflared.exe'; Package = 'Cloudflare.cloudflared' }
    DevSpace = @{ Command = 'devspace.cmd'; Package = '@waishnav/devspace@1.1.1' }
}

function Refresh-Path {
    $env:Path = [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + [Environment]::GetEnvironmentVariable('Path', 'User') + ';' + $env:Path
}

function Find-Component([string]$Name) {
    $command = $components[$Name].Command
    if (Get-Command $command -ErrorAction SilentlyContinue) { return $true }
    if ($Name -eq 'Tailscale') {
        return (Test-Path (Join-Path $env:ProgramFiles 'Tailscale\tailscale.exe'))
    }
    if ($Name -eq 'Cloudflared') {
        foreach ($key in @('ProgramFiles', 'ProgramFiles(x86)')) {
            $folder = [Environment]::GetEnvironmentVariable($key)
            if ($folder -and (Test-Path (Join-Path $folder 'cloudflared\cloudflared.exe'))) { return $true }
        }
    }
    return $false
}

if ($Check) {
    foreach ($name in @('Git', 'Node', 'Tailscale', 'Cloudflared', 'DevSpace')) {
        Write-Output ("{0}: {1}" -f $name, (Find-Component $name))
    }
    return
}

if (-not $Component) { throw 'Select a specific component.' }
Write-Output '[0%] Checking installed programs...'
if (Find-Component $Component) {
    Write-Output ("[100%] {0} is already installed; nothing changed." -f $Component)
    return
}
if ($Component -eq 'DevSpace') {
    if (-not (Get-Command npm.cmd -ErrorAction SilentlyContinue)) {
        throw 'Node.js/npm is missing. Install Node first.'
    }
    Write-Output '[25%] npm available. Installing pinned DevSpace 1.1.1...'
    & npm.cmd install --global '@waishnav/devspace@1.1.1' --no-audit --no-fund
} else {
    if (-not (Get-Command winget.exe -ErrorAction SilentlyContinue)) {
        throw 'Windows Package Manager (winget) is missing. Install Microsoft App Installer first.'
    }
    $package = $components[$Component].Package
    Write-Output ("[25%] Installing official package {0}..." -f $package)
    & winget.exe install --exact --id $package --accept-source-agreements --accept-package-agreements --disable-interactivity
}
if ($LASTEXITCODE -ne 0) { throw ("Installation of {0} failed with exit code {1}." -f $Component, $LASTEXITCODE) }
Write-Output '[85%] Checking installation...'
Refresh-Path
if (-not (Find-Component $Component)) {
    Write-Output '[95%] Installer exited successfully. Reopen this application after Windows refreshes PATH.'
    return
}
Write-Output ("[100%] {0} is installed. OAuth/login and publishing are separate actions." -f $Component)
