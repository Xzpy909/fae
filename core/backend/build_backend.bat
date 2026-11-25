@echo off
rem Builds the Rust back end (docs/rust-backend-plan.md): Python extension fae_backend -> installed into
rem the current Python (pip). Required by the app (fastedit\backend.py). Checks:
rem   python tests\stage\stage_check.py --impl rust   the stage (decode, ...) in Rust vs the goldens
rem   python tests\backend\commands_check.py   the commands moved so far, against their Python behaviour
rem   python tests\backend\encoders_check.py   the encoders (B4), against the Python ones
rem   python tests\backend\process_check.py   Process and the encoded compare (B5), against the Python path
rem It links the CPU core (core\fastedit-core) and the GPU core (core\gpu\fae-gpu) as crates: Process and the
rem encoded compare run in it; their own build scripts are still needed for the live preview's WebAssembly.
setlocal
cd /d "%~dp0"

where cargo >nul 2>nul || (
  echo Rust is not installed. Get it from https://rustup.rs and run this again.
  exit /b 1
)

echo === Python extension (fae_backend) ===
python -m pip install -q -U "maturin>=1.7,<2" || goto :fail
if exist target\wheels rmdir /s /q target\wheels
rem libjpeg-turbo's SIMD code and rav1e's assembly need nasm to build: same pixels, about 40 %% faster on big
rem JPEGs, AVIF through rav1e (alpha / the aom choice) several times faster
set FEAT=
where nasm >nul 2>nul && set FEAT=--features simd
if "%FEAT%"=="" echo nasm not found: JPEG decoding and rav1e without SIMD (same pixels, slower). Get it with: winget install nasm
pushd fae-backend-py
rem x86-64-v2 (SSE4.2), as the CPU core's build_core.bat: floor() is one instruction, same results (no FMA)
set "RUSTFLAGS=-C target-cpu=x86-64-v2"
python -m maturin build --release %FEAT% --out ..\target\wheels || (set RUSTFLAGS= & popd & goto :fail)
set RUSTFLAGS=
popd
for %%f in (target\wheels\fae_backend-*.whl) do python -m pip install -q --force-reinstall --no-deps "%%f" || goto :fail
python -c "import fae_backend as b; print('fae_backend', b.version(), '-', len(b.commands()), 'of', len(b.table()), 'commands in Rust')" || goto :fail
python -c "import sys; sys.path.insert(0, r'..\..'); from fastedit import backend; i = backend.shared().call('core_info'); g = i['gpu']; print('CPU core', i['version'], '-', i['threads'], 'threads; GPU core:', (g['version'] + ' on ' + g['name'] + ' (' + g['backend'] + ')') if g['ok'] else g['error'])" || goto :fail
python -c "import sys; sys.path.insert(0, r'..\..'); from fastedit import backend; import fae_backend as f; print('features:', f.features() or 'none'); b = backend.shared(); print('HEIC: libheif', b._b.heif_info()[1], 'from', b._b.heif_info()[0])" || echo HEIC: libheif not found - HEIC files can't be opened. It comes with pillow-heif: pip install pillow-heif

echo.
echo Rust back end built: Python extension fae_backend installed
exit /b 0

:fail
echo.
echo Building the Rust back end failed.
exit /b 1
