param(
    [switch]$Release
)

$ErrorActionPreference = "Stop"
$idfExport = "C:\Espressif\frameworks\esp-idf-v5.5.5-2\export.ps1"
$idfPython = "C:\Espressif\python_env\idf5.5_py3.11_env\Scripts"
$idfGit = "C:\Espressif\tools\idf-git\2.44.0\cmd"
$projectDir = Join-Path $PSScriptRoot "..\rust-firmware"

if (-not (Test-Path -LiteralPath $idfExport)) {
    throw "ESP-IDF export script not found: $idfExport"
}

$env:Path = "$idfPython;$idfGit;$env:Path"
$env:IDF_TOOLS_PATH = "C:\Espressif"
. $idfExport

Push-Location $projectDir
try {
    $cargoArgs = @("build")
    if ($Release) {
        $cargoArgs += "--release"
    }

    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }
} finally {
    Pop-Location
}
