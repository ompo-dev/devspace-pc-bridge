$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$script:failures = 0

function Assert-Equal($Actual, $Expected, [string]$Message) {
    if ($Actual -ne $Expected) {
        Write-Host "FALHOU: $Message (esperado '$Expected', obtido '$Actual')"
        $script:failures++
    } else {
        Write-Host "ok: $Message"
    }
}

# Pulls one function out of a script through the parser, so the real code is tested
# without running the rest of the script (which installs software or starts servers).
function Get-FunctionText([string]$File, [string]$Name) {
    $ast = [System.Management.Automation.Language.Parser]::ParseFile((Join-Path $repo $File), [ref]$null, [ref]$null)
    $definition = $ast.Find({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -eq $Name }, $true)
    if (-not $definition) { throw "Funcao $Name nao encontrada em $File" }
    return $definition.Extent.Text
}

# --- Get-BroadRootReason: same behaviour in install.ps1 and doctor.ps1, and no drift between them
$installText = Get-FunctionText 'install.ps1' 'Get-BroadRootReason'
$doctorText = Get-FunctionText 'scripts/doctor.ps1' 'Get-BroadRootReason'
Assert-Equal ($installText -replace '\s+', ' ') ($doctorText -replace '\s+', ' ') 'Get-BroadRootReason identica em install.ps1 e doctor.ps1'

Invoke-Expression $installText
$originalProfile = $env:USERPROFILE
$env:USERPROFILE = 'C:\Users\tester'
try {
    Assert-Equal (Get-BroadRootReason 'C:\') 'raiz de um disco' 'raiz do disco C'
    Assert-Equal (Get-BroadRootReason 'D:\') 'raiz de um disco' 'raiz do disco D'
    Assert-Equal (Get-BroadRootReason 'C:\Users\tester') 'pasta pessoal inteira' 'pasta pessoal'
    Assert-Equal (Get-BroadRootReason 'c:\users\TESTER\') 'pasta pessoal inteira' 'pasta pessoal sem diferenciar maiusculas'
    Assert-Equal (Get-BroadRootReason 'C:\Users') 'pasta que contem a pasta pessoal' 'pai da pasta pessoal'
    Assert-Equal (Get-BroadRootReason 'C:\Users\tester\.ssh') 'pasta com credenciais de ferramentas' '.ssh'
    Assert-Equal (Get-BroadRootReason 'C:\Users\tester\AppData\Local') 'pasta com credenciais de ferramentas' 'AppData'
    Assert-Equal (Get-BroadRootReason 'C:\Users\tester\projetos') $null 'pasta de projetos'
    Assert-Equal (Get-BroadRootReason 'D:\dev\app') $null 'outro disco'
} finally {
    $env:USERPROFILE = $originalProfile
}

# --- Get-ExposeUntil: the file format must match what DevSpacePCBridge.exe writes (Unix seconds)
Invoke-Expression (Get-FunctionText 'scripts/bridge.ps1' 'Get-ExposeUntil')
$untilFile = Join-Path ([IO.Path]::GetTempPath()) ('expose-until-test-' + [Guid]::NewGuid().ToString('N'))
try {
    Assert-Equal (Get-ExposeUntil) $null 'sem arquivo de prazo'
    Set-Content -LiteralPath $untilFile -Value '1700000000' -NoNewline
    Assert-Equal (Get-ExposeUntil) 1700000000 'prazo em segundos Unix'
    Set-Content -LiteralPath $untilFile -Value " 42`r`n" -NoNewline
    Assert-Equal (Get-ExposeUntil) 42 'prazo com espacos e quebra de linha'
    Set-Content -LiteralPath $untilFile -Value 'soon' -NoNewline
    Assert-Equal (Get-ExposeUntil) $null 'prazo invalido e ignorado'
    Set-Content -LiteralPath $untilFile -Value '' -NoNewline
    Assert-Equal (Get-ExposeUntil) $null 'arquivo de prazo vazio'
} finally {
    Remove-Item -LiteralPath $untilFile -Force -ErrorAction SilentlyContinue
}

# --- The supervisor must honour the pause markers, or it would switch the Funnel back on
$bridgeText = Get-Content -LiteralPath (Join-Path $repo 'scripts/bridge.ps1') -Raw
Assert-Equal ($bridgeText -match "'paused'") $true 'bridge.ps1 le o marcador paused'
Assert-Equal ($bridgeText -match "'expose-until'") $true 'bridge.ps1 le o marcador expose-until'
Assert-Equal ($bridgeText -match 'Stop-Funnel \$ts') $true 'bridge.ps1 fecha o Funnel durante a pausa'

# --- install.ps1 clears the markers, uninstall.ps1 never resets a shared Funnel silently
$installFull = Get-Content -LiteralPath (Join-Path $repo 'install.ps1') -Raw
Assert-Equal ($installFull -match "'paused'") $true 'install.ps1 limpa o marcador de pausa'
$uninstallFull = Get-Content -LiteralPath (Join-Path $repo 'scripts/uninstall.ps1') -Raw
Assert-Equal ($uninstallFull -match '\$Force') $true 'uninstall.ps1 exige -Force para resetar Funnel compartilhado'

if ($script:failures -gt 0) { throw "$($script:failures) teste(s) de script falharam" }
Write-Host 'Testes de script concluidos.'
