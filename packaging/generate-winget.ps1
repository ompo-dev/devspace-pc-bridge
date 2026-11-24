#Requires -Version 5.1
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version,
    [Parameter(Mandatory=$true)][string]$InstallerPath,
    [string]$Destination = 'winget-manifests'
)

$ErrorActionPreference = 'Stop'
$installer = (Resolve-Path -LiteralPath $InstallerPath).Path
$fileName = Split-Path -Leaf $installer
$sha256 = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToUpperInvariant()
$url = "https://github.com/ompo-dev/devspace-pc-bridge/releases/download/v$Version/$fileName"
$base = Join-Path $Destination "manifests/o/OmPo/DevSpacePCBridge/$Version"
New-Item -ItemType Directory -Path $base -Force | Out-Null

@"
PackageIdentifier: OmPo.DevSpacePCBridge
PackageVersion: $Version
DefaultLocale: en-US
ManifestType: version
ManifestVersion: 1.12.0
"@ | Set-Content -LiteralPath (Join-Path $base 'OmPo.DevSpacePCBridge.yaml') -Encoding UTF8

@"
PackageIdentifier: OmPo.DevSpacePCBridge
PackageVersion: $Version
InstallerType: nullsoft
Scope: user
Installers:
  - Architecture: x64
    InstallerUrl: $url
    InstallerSha256: $sha256
ManifestType: installer
ManifestVersion: 1.12.0
"@ | Set-Content -LiteralPath (Join-Path $base 'OmPo.DevSpacePCBridge.installer.yaml') -Encoding UTF8

@"
PackageIdentifier: OmPo.DevSpacePCBridge
PackageVersion: $Version
PackageLocale: en-US
Publisher: OmPo
PackageName: DevSpace PC Bridge
License: MIT
ShortDescription: Connect ChatGPT and Claude to authorized Windows projects and apps through MCP.
PackageUrl: https://github.com/ompo-dev/devspace-pc-bridge
ManifestType: defaultLocale
ManifestVersion: 1.12.0
"@ | Set-Content -LiteralPath (Join-Path $base 'OmPo.DevSpacePCBridge.locale.en-US.yaml') -Encoding UTF8

Write-Output "Winget manifest generated for $Version, SHA256 $sha256"
Write-Output "Directory: $base"
