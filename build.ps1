# Build ClearMic. Usage: .\build.ps1 [-Test] [-Installer] [-NoDriver]
#   -Test       run the test suite first
#   -Installer  build dist\ClearMic-Setup-<version>.exe (app + VB-CABLE virtual microphone driver)
#   -NoDriver   with -Installer: leave the VB-CABLE driver out of the installer
param([switch]$Test, [switch]$Installer, [switch]$NoDriver)
$ErrorActionPreference = "Stop"

$toolchain = Get-ChildItem "$env:USERPROFILE\.rustup\toolchains" -Directory |
    Where-Object { $_.Name -like "*x86_64-pc-windows-gnu" } | Select-Object -First 1
if (-not $toolchain) {
    throw "Rust GNU toolchain not found. Run: rustup toolchain install stable-x86_64-pc-windows-gnu"
}

# The GNU toolchain needs GNU binutils (dlltool.exe + as.exe) to create Windows import libraries,
# and gcc to assemble the inference kernels. Rust itself still links with its bundled linker.
# They are fetched once from the w64devkit project into %USERPROFILE%\.clearmic-build.
$tools = "$env:USERPROFILE\.clearmic-build"
if (-not ((Test-Path "$tools\bin\as.exe") -and (Test-Path "$tools\bin\dlltool.exe") -and (Test-Path "$tools\w64devkit\bin\gcc.exe"))) {
    Write-Host "Fetching GNU binutils (w64devkit) once..."
    New-Item -ItemType Directory -Force "$tools\bin" | Out-Null
    $sfx = Join-Path $env:TEMP "w64devkit.7z.exe"
    $url = "https://github.com/skeeto/w64devkit/releases/download/v2.10.0/w64devkit-x64-2.10.0.7z.exe"
    Invoke-WebRequest $url -OutFile $sfx
    & $sfx "-o$tools" -y | Out-Null
    Copy-Item "$tools\w64devkit\bin\as.exe" "$tools\bin\as.exe"
    Copy-Item "$tools\w64devkit\bin\dlltool.exe" "$tools\bin\dlltool.exe"
}
$env:PATH = "$env:USERPROFILE\.cargo\bin;$tools\bin;$env:PATH"
# tract compiles hand-written assembly kernels and needs a C compiler driver for that.
$env:CC_x86_64_pc_windows_gnu = "$tools\w64devkit\bin\gcc.exe"
$env:AR_x86_64_pc_windows_gnu = "$tools\w64devkit\bin\ar.exe"

Set-Location $PSScriptRoot
if ($Test) {
    cargo test
    if ($LASTEXITCODE -ne 0) { throw "tests failed" }
}
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "build failed" }
$exe = "target\x86_64-pc-windows-gnu\release\clearmic.exe"

# The icon is drawn by the app itself. Generate it once, then relink so it is embedded.
if (-not (Test-Path "assets\clearmic.ico")) {
    Start-Process $exe -ArgumentList "--export-icon", "assets\clearmic.ico" -Wait -NoNewWindow
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "build failed" }
}
Write-Host "Built: $exe"

if ($Installer) {
    $isccArgs = @("installer\clearmic.iss")
    if ($NoDriver) {
        $isccArgs = @("/DNoDriver=1") + $isccArgs
    } elseif (-not (Test-Path "installer\vbcable\pack\VBCABLE_Setup_x64.exe")) {
        # Official, unmodified VB-CABLE package from VB-Audio (www.vb-cable.com).
        Write-Host "Fetching the VB-CABLE driver pack from VB-Audio..."
        New-Item -ItemType Directory -Force "installer\vbcable" | Out-Null
        $zip = "installer\vbcable\VBCABLE_Driver_Pack45.zip"
        Invoke-WebRequest "https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack45.zip" -OutFile $zip
        $expected = "b950e39f01af1d04ea623c8f6d8eb9b6ea5c477c637295fabf20631c85116bfb"
        $actual = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
        if ($actual -ne $expected) { throw "VB-CABLE pack checksum mismatch ($actual). Check the download and update build.ps1." }
        Expand-Archive $zip -DestinationPath "installer\vbcable\pack" -Force
    }
    $iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
    if (-not (Test-Path $iscc)) { $iscc = "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe" }
    if (-not (Test-Path $iscc)) { throw "Inno Setup 6 not found (https://jrsoftware.org/isinfo.php)" }
    & $iscc @isccArgs
    if ($LASTEXITCODE -ne 0) { throw "installer build failed" }
    Write-Host "Installer written to dist\"
}
