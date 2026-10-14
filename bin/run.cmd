@echo off
rem scx bootstrap entry (Windows): auto-builds the binary when missing, then forwards all args.
rem Source changes: delete bin\scx.exe (or rerun this) to force a rebuild; see README.md.
setlocal
set "DIR=%~dp0"
set "SKILL=%DIR%.."
set "BIN=%DIR%scx.exe"
if exist "%BIN%" goto run
where cargo >nul 2>nul
if errorlevel 1 (
  echo scx: cargo not found. Install Rust first: https://rustup.rs 1>&2
  exit /b 3
)
cargo build --release --manifest-path "%SKILL%\rs\Cargo.toml" --target-dir "%TEMP%\scx-cargo-target"
if errorlevel 1 exit /b 3
copy /y "%TEMP%\scx-cargo-target\release\scx.exe" "%BIN%" >nul
:run
"%BIN%" %*
exit /b %errorlevel%
