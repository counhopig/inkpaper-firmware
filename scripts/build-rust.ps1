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

if (-not $env:LIBCLANG_PATH) {
    # esp-idf-sys's bindgen step needs espup's esp-clang, which clang-sys does
    # not find on its own. Locate it under the active `esp` rustup toolchain
    # instead of hardcoding a path, so this works on any machine.
    $espToolchain = (rustup toolchain list -v 2>$null) |
        Where-Object { $_ -match '^esp\s' } |
        ForEach-Object { ($_ -split '\s+')[-1] } |
        Select-Object -First 1

    if ($espToolchain) {
        $clangDir = Get-ChildItem -Path (Join-Path $espToolchain "xtensa-esp32-elf-clang") -Filter "esp-clang" -Recurse -Directory -ErrorAction SilentlyContinue |
            ForEach-Object {
                Get-ChildItem -Path $_.FullName -Include "bin", "lib" -Directory -ErrorAction SilentlyContinue |
                    Where-Object { Get-ChildItem -Path $_.FullName -Filter "libclang.dll" -ErrorAction SilentlyContinue }
            } |
            Select-Object -First 1 -ExpandProperty FullName

        if ($clangDir) {
            $env:LIBCLANG_PATH = $clangDir
        }
    }

    if (-not $env:LIBCLANG_PATH) {
        throw "Could not locate esp-clang's libclang.dll under the 'esp' rustup toolchain. Install it with 'espup install', or set LIBCLANG_PATH manually."
    }
}

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
