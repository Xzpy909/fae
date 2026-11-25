@echo off
rem Builds dist\Fae\Fae.exe: the app (Tauri window around the Rust back end, core\backend\fae-app),
rem no Python at run time. Folder build: Fae.exe with ui\, encoders\ (if present) and heif\ (pillow-heif's
rem libheif, if installed) next to it; settings and presets are kept there too.
rem Needs Rust (https://rustup.rs) and the WebView2 runtime (part of Windows 10 / 11). The build itself still
rem uses Python for the WebAssembly preview's build script (core\build_core.bat) and pillow-heif's libraries
rem (requirements.txt).
rem Checks: cargo test -p fae-app (in core\backend), then Fae.exe --selftest (cross-origin isolated, WASM).
setlocal
cd /d "%~dp0"

where cargo >nul 2>nul || (
  echo Rust is not installed. Get it from https://rustup.rs and run this again.
  goto :fail
)

python -m pip install -q -r requirements.txt || goto :fail

rem the live preview's WebAssembly (ui\wasm\, required) and its WebGPU build (ui\gpu\, optional)
call core\build_core.bat || goto :fail
call core\gpu\build_gpu.bat || echo GPU core not built: the live preview will use the CPU core only.

echo === Fae.exe (Tauri) ===
set FEAT=
rem nasm's installer does not always put it on PATH (winget installs it per user): look in its usual folders too
where nasm >nul 2>nul || for %%d in ("%ProgramFiles%\NASM" "%LOCALAPPDATA%\bin\NASM" "%LOCALAPPDATA%\Programs\NASM") do if exist "%%~d\nasm.exe" set "PATH=%%~d;%PATH%"
where nasm >nul 2>nul && set FEAT=--features simd
if "%FEAT%"=="" echo nasm not found: JPEG decoding and rav1e without SIMD (same pixels, slower). Get it with: winget install nasm
pushd core\backend
rem x86-64-v2 as the other builds; the C runtime linked in, so Fae.exe needs no Visual C++ redistributable
set "RUSTFLAGS=-C target-cpu=x86-64-v2 -C target-feature=+crt-static"
cargo build --release -p fae-app %FEAT% || (set RUSTFLAGS= & popd & goto :fail)
set RUSTFLAGS=
popd

set OUT=dist\Fae
if exist %OUT% rmdir /s /q %OUT%
mkdir %OUT% || goto :fail
copy /y core\backend\target\release\Fae.exe %OUT%\ >nul || goto :fail
robocopy ui %OUT%\ui /e /njh /njs /nfl /ndl >nul
if errorlevel 8 goto :fail
mkdir %OUT%\encoders
if exist "encoders\*.exe" copy /y "encoders\*.exe" "%OUT%\encoders\" >nul
python core\backend\fae-app\copy_heif.py %OUT%\heif
copy /y README.md %OUT%\ >nul

echo.
echo Done: %OUT%\Fae.exe
echo Zip the whole %OUT% folder for the release.
exit /b 0

:fail
echo.
echo Build failed.
exit /b 1
