@echo off
rem Builds the Rust back end (docs/rust-backend-plan.md): Python extension fae_backend -> installed into
rem the current Python (pip). Required by the app (fastedit\backend.py). Checks:
rem   python tests\stage\stage_check.py --impl rust   the stage (decode, ...) in Rust vs the goldens
rem   python tests\backend\commands_check.py   the commands moved so far, against their Python behaviour
setlocal
cd /d "%~dp0"

where cargo >nul 2>nul || (
  echo Rust is not installed. Get it from https://rustup.rs and run this again.
  exit /b 1
)

echo === Python extension (fae_backend) ===
python -m pip install -q -U "maturin>=1.7,<2" || goto :fail
if exist target\wheels rmdir /s /q target\wheels
rem libjpeg-turbo's SIMD code needs nasm to build: same pixels, about 40 %% faster on big JPEGs
set FEAT=
where nasm >nul 2>nul && set FEAT=--features jpeg-simd
if "%FEAT%"=="" echo nasm not found: JPEG decoding without SIMD (same pixels, slower on big photos). Get it with: winget install nasm
pushd fae-backend-py
python -m maturin build --release %FEAT% --out ..\target\wheels || (popd & goto :fail)
popd
for %%f in (target\wheels\fae_backend-*.whl) do python -m pip install -q --force-reinstall --no-deps "%%f" || goto :fail
python -c "import fae_backend as b; print('fae_backend', b.version(), '-', len(b.commands()), 'of', len(b.table()), 'commands in Rust')" || goto :fail
python -c "import sys; sys.path.insert(0, r'..\..'); from fastedit import backend; import fae_backend as f; print('features:', f.features() or 'none'); b = backend.shared(); print('HEIC: libheif', b._b.heif_info()[1], 'from', b._b.heif_info()[0])" || echo HEIC: libheif not found - HEIC files can't be opened. It comes with pillow-heif: pip install pillow-heif

echo.
echo Rust back end built: Python extension fae_backend installed
exit /b 0

:fail
echo.
echo Building the Rust back end failed.
exit /b 1
