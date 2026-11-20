# TESTING - Build release DLL and copy to OpenCAD Studio local plugins folder
Set-Location (Split-Path $PSScriptRoot -Parent)

cargo build --release
if ($LASTEXITCODE -ne 0) {
    Write-Error "Cargo build failed."
    Pause
    exit $LASTEXITCODE
}

$d = "$env:APPDATA\OpenCADStudio\plugins\opencad_layname_plugin"
New-Item -ItemType Directory -Path $d -Force | Out-Null

Copy-Item "target\release\opencad_layname_plugin.dll" -Destination $d -Force
Copy-Item "plugin.toml" -Destination $d -Force

Write-Host "Copied opencad_layname_plugin.dll and plugin.toml to $d" -ForegroundColor Green
Pause


