@echo off
rem scx bootstrap entry (Windows): rebuilds via Cargo (incremental) when available,
rem then forwards all args. A binary-only install still runs when Cargo is absent.
setlocal
set "DIR=%~dp0"
set "SKILL=%DIR%.."
set "BIN=%DIR%scx.exe"
where cargo >nul 2>nul
if errorlevel 1 (
  if exist "%BIN%" goto run
  echo scx: cargo not found. Install Rust first: https://rustup.rs 1>&2
  exit /b 3
)
cargo build --release --quiet --locked --manifest-path "%SKILL%\Cargo.toml" --target-dir "%TEMP%\scx-cargo-target"
if errorlevel 1 exit /b 3
copy /y "%TEMP%\scx-cargo-target\release\scx.exe" "%BIN%" >nul
if errorlevel 1 (
  echo scx: failed to install binary to "%BIN%" 1>&2
  exit /b 3
)
:run
"%BIN%" %*
exit /b %errorlevel%
