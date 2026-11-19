$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$scripts = @('install.ps1', 'scripts/bridge.ps1', 'scripts/doctor.ps1', 'scripts/uninstall.ps1')
foreach ($file in $scripts) {
    $path = Join-Path $repo $file
    $tokens = $null
    $errors = $null
    [System.Management.Automation.Language.Parser]::ParseFile($path, [ref]$tokens, [ref]$errors) | Out-Null
    if ($errors.Count -gt 0) { throw "Erro de sintaxe em $file : $($errors | Out-String)" }
    Write-Host "Syntax OK: $file"
    # Windows PowerShell 5.1 reads BOM-less files as ANSI, so any non-ASCII byte would be garbled.
    $nonAscii = [IO.File]::ReadAllBytes($path) | Where-Object { $_ -gt 127 } | Select-Object -First 1
    if ($null -ne $nonAscii) { throw "$file contem caracteres nao ASCII; use apenas ASCII nos scripts PowerShell." }
    Write-Host "ASCII OK: $file"
}
$devspaceVersions = foreach ($file in @('install.ps1', 'scripts/doctor.ps1')) {
    $match = Select-String -LiteralPath (Join-Path $repo $file) -Pattern "^\`$DevSpaceVersion = '([^']+)'" | Select-Object -First 1
    if (-not $match) { throw "$file nao define `$DevSpaceVersion." }
    $match.Matches[0].Groups[1].Value
}
if (@($devspaceVersions | Select-Object -Unique).Count -ne 1) {
    throw "A versao do DevSpace difere entre install.ps1 e scripts/doctor.ps1: $($devspaceVersions -join ' / ')"
}
Write-Host "DevSpace fixado em $($devspaceVersions[0])"
& (Join-Path $repo 'install.ps1') -Check
Write-Host 'Validacao offline concluida.'
