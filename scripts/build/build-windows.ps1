# Build tokenade on Windows.
#
#   .\scripts\build\build-windows.ps1                                     # x64 (x86_64-pc-windows-gnu)
#   .\scripts\build\build-windows.ps1 -Target aarch64-pc-windows-gnullvm  # ARM64 (optional)
#
# x64 needs: stable Rust (rustup) and a MinGW-w64 toolchain on PATH (gcc, g++),
# e.g. MSYS2 `pacman -S mingw-w64-x86_64-toolchain` then add C:\msys64\mingw64\bin.
# The mingw CRT is linked statically (.cargo/config.toml), so tokenade.exe
# runs without any DLL next to it.
#
# ARM64 needs llvm-mingw (https://github.com/mstorsjo/llvm-mingw) on PATH
# (aarch64-w64-mingw32-clang). rquickjs-sys ships no bindings for gnullvm, so
# the msvc ones (same ABI) are copied over, as the release CI does.
param([string]$Target = "x86_64-pc-windows-gnu")
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..\..")

function Need($cmd, $hint) {
    if (-not (Get-Command $cmd -ErrorAction SilentlyContinue)) {
        Write-Error "'$cmd' not found on PATH. $hint"
        exit 1
    }
}

Need cargo "Install Rust from https://rustup.rs"
rustup target add $Target
if ($LASTEXITCODE) { exit $LASTEXITCODE }

switch ($Target) {
    "x86_64-pc-windows-gnu" {
        Need gcc "Install MinGW-w64 (MSYS2: pacman -S mingw-w64-x86_64-toolchain) and add its bin dir to PATH."
        Need g++ "Install MinGW-w64 (MSYS2: pacman -S mingw-w64-x86_64-toolchain) and add its bin dir to PATH."
    }
    "aarch64-pc-windows-gnullvm" {
        Need aarch64-w64-mingw32-clang "Install llvm-mingw (https://github.com/mstorsjo/llvm-mingw/releases) and add its bin dir to PATH."
        $env:CC_aarch64_pc_windows_gnullvm = "aarch64-w64-mingw32-clang"
        $env:CXX_aarch64_pc_windows_gnullvm = "aarch64-w64-mingw32-clang++"
        $env:AR_aarch64_pc_windows_gnullvm = "llvm-ar"
        $env:CARGO_TARGET_AARCH64_PC_WINDOWS_GNULLVM_LINKER = "aarch64-w64-mingw32-clang"
        cargo fetch --locked --target $Target
        if ($LASTEXITCODE) { exit $LASTEXITCODE }
        $cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $HOME ".cargo" }
        Get-ChildItem (Join-Path $cargoHome "registry\src\*\rquickjs-sys-*\src\bindings") -Directory | ForEach-Object {
            $dst = Join-Path $_.FullName "aarch64-pc-windows-gnullvm.rs"
            if (-not (Test-Path $dst)) { Copy-Item (Join-Path $_.FullName "aarch64-pc-windows-msvc.rs") $dst }
        }
    }
}

cargo build --release --locked --bin tokenade --target $Target
if ($LASTEXITCODE) { exit $LASTEXITCODE }

$out = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "target" }
$bin = Join-Path $out "$Target\release\tokenade.exe"
Write-Host "built: $bin"
try { & $bin --version } catch { Write-Host "(cannot run $bin on this machine)" }
