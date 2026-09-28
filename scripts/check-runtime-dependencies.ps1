param(
    [string]$Executable = (Join-Path $PSScriptRoot '..\target\release\recordscreen.exe')
)

$ErrorActionPreference = 'Stop'
$resolvedExe = (Resolve-Path -LiteralPath $Executable).Path
$dumpbinCommand = Get-Command dumpbin.exe -ErrorAction SilentlyContinue
if ($dumpbinCommand) {
    $dumpbinPath = $dumpbinCommand.Source
} else {
    $vswherePath = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswherePath)) {
        throw 'Visual Studio vswhere.exe is required to locate dumpbin.exe.'
    }
    $dumpbinPath = @(& $vswherePath -latest -products '*' -find 'VC\Tools\MSVC\**\bin\Hostx64\x64\dumpbin.exe') |
        Sort-Object -Descending | Select-Object -First 1
    if (-not $dumpbinPath) { throw 'dumpbin.exe was not found. Install MSVC build tools.' }
}

$output = @(& $dumpbinPath /nologo /dependents $resolvedExe 2>&1)
if ($LASTEXITCODE -ne 0) { throw "dumpbin failed for $resolvedExe" }
$dependencies = @($output | ForEach-Object {
    if ("$_" -match '^\s+([\w.-]+\.dll)\s*$') { $Matches[1] }
} | Sort-Object -Unique)
if ($dependencies.Count -eq 0) { throw 'No DLL imports were parsed; dependency verification cannot pass.' }

$forbidden = @($dependencies | Where-Object { $_ -match '^(vcruntime|msvcp|msvcr|concrt|vcomp)\d.*\.dll$' })
if ($forbidden.Count -gt 0) {
    throw "External Visual C++ runtime dependency detected: $($forbidden -join ', '). Rebuild with +crt-static before packaging."
}
[pscustomobject]@{
    Executable = $resolvedExe
    ExternalVisualCppRuntime = $false
    ImportedDlls = $dependencies
    SHA256 = (Get-FileHash -LiteralPath $resolvedExe -Algorithm SHA256).Hash
}
