@echo off
rem Builds dist\FastImageEditor\FastImageEditor.exe (folder build, no console window).
rem Encoders are not bundled: they are copied next to the exe into encoders\ if present.
cd /d "%~dp0"

python -m pip install -U pyinstaller || goto :fail
python -m pip install -r requirements.txt || goto :fail

set ICON=
if exist app.ico set ICON=--icon app.ico

python -m PyInstaller --noconfirm --clean --windowed --onedir ^
  --name FastImageEditor %ICON% ^
  --add-data "ui;ui" ^
  --collect-all pillow_heif ^
  --collect-all pillow_jxl ^
  fast_image_editor.py || goto :fail

if not exist "dist\FastImageEditor\encoders" mkdir "dist\FastImageEditor\encoders"
if exist "encoders\*.exe" copy /y "encoders\*.exe" "dist\FastImageEditor\encoders\" >nul
copy /y README.md "dist\FastImageEditor\" >nul

echo.
echo Done: dist\FastImageEditor\FastImageEditor.exe
echo Zip the whole dist\FastImageEditor folder for the release.
pause
exit /b 0

:fail
echo.
echo Build failed.
pause
exit /b 1
