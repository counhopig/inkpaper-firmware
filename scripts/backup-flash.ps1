param(
  [Parameter(Mandatory = $true)]
  [string]$Port,

  [string]$OutDir = "backups",
  [int]$Baud = 921600
)

$ErrorActionPreference = "Stop"

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$outFile = Join-Path $OutDir "note4-factory-$timestamp.bin"

Write-Host "Reading full 16 MiB flash from $Port to $outFile"
esptool.py -p $Port -b $Baud read_flash 0x0 0x1000000 $outFile

Write-Host "Backup complete: $outFile"
