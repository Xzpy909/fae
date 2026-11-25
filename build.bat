@echo off
rem Builds dist\Fae\Fae.exe (folder build, no console window).
rem Encoders are not bundled: they are copied next to the exe into encoders\ if present.
cd /d "%~dp0"

python -m pip install -U pyinstaller || goto :fail
python -m pip install -r requirements.txt || goto :fail

rem Rust core (required: the live preview's WebAssembly, ui\wasm\; there is no JS fallback)
call core\build_core.bat || goto :fail

rem Rust back end (required: the stage, the encoders, Process and the encoded compare; it has both cores
rem built in, so Process uses the GPU when there is one)
call core\backend\build_backend.bat || goto :fail

rem GPU core (optional: the live preview's WebGPU build, ui\gpu\; without it the preview uses the CPU core)
call core\gpu\build_gpu.bat || echo GPU core not built: the live preview will use the CPU core only.

set ICON=
if exist app.ico set ICON=--icon app.ico

python -m PyInstaller --noconfirm --clean --windowed --onedir ^
  --name Fae %ICON% ^
  --add-data "ui;ui" ^
  --collect-all pillow_heif ^
  --collect-all pillow_jxl ^
  fae.py || goto :fail

if not exist "dist\Fae\encoders" mkdir "dist\Fae\encoders"
if exist "encoders\*.exe" copy /y "encoders\*.exe" "dist\Fae\encoders\" >nul
copy /y README.md "dist\Fae\" >nul

echo.
echo Done: dist\Fae\Fae.exe
echo Zip the whole dist\Fae folder for the release.
pause
exit /b 0

:fail
echo.
echo Build failed.
pause
exit /b 1
