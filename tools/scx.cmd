@echo off
rem scx bootstrap entry (Windows): builds via Cargo when available, then forwards
rem all args from the build target. No binary is copied into the source tree; a
rem prebuilt target binary still runs when Cargo is absent.
setlocal
set "SKILL=%~dp0.."
if not defined SCX_TARGET_DIR set "SCX_TARGET_DIR=%TEMP%\scx-cargo-target"
set "BIN=%SCX_TARGET_DIR%\release\scx.exe"
where cargo >nul 2>nul
if errorlevel 1 (
  if exist "%BIN%" goto run
  echo scx: cargo not found. Install Rust first: https://rustup.rs 1>&2
  exit /b 3
)
cargo build --release --quiet --locked --manifest-path "%SKILL%\Cargo.toml" --target-dir "%SCX_TARGET_DIR%"
if errorlevel 1 exit /b 3
:run
"%BIN%" %*
exit /b %errorlevel%
