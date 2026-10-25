@echo off
rem scx bootstrap entry (Windows): rebuilds via Cargo (incremental) when available,
rem then forwards all args. A binary-only install still runs when Cargo is absent.
setlocal
set "DIR=%~dp0"
set "SKILL=%DIR%.."
set "BIN=%DIR%scx.exe"
if not defined SCX_TARGET_DIR set "SCX_TARGET_DIR=%TEMP%\scx-cargo-target"
set "BUILT=%SCX_TARGET_DIR%\release\scx.exe"
where cargo >nul 2>nul
if errorlevel 1 (
  if exist "%BIN%" goto run
  echo scx: cargo not found. Install Rust first: https://rustup.rs 1>&2
  exit /b 3
)
cargo build --release --quiet --locked --manifest-path "%SKILL%\Cargo.toml" --target-dir "%SCX_TARGET_DIR%"
if errorlevel 1 exit /b 3
copy /y "%BUILT%" "%BIN%" >nul
if errorlevel 1 (
  echo scx: failed to install binary to "%BIN%" 1>&2
  exit /b 3
)
:run
"%BIN%" %*
exit /b %errorlevel%
