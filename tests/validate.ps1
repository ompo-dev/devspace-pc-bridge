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
}
& (Join-Path $repo 'install.ps1') -Check
Write-Host 'Validacao offline concluida.'
