@echo off
rem Builds the Rust back end (claude/rust-backend-plan.md): Python extension fae_backend -> installed into
rem the current Python (pip). Required by the app (fastedit\backend.py). Checks:
rem   python tests\stage\stage_check.py   the stage (decode, geometry, resize, colour balance) vs the goldens
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
pushd fae-backend-py
python -m maturin build --release --out ..\target\wheels || (popd & goto :fail)
popd
for %%f in (target\wheels\fae_backend-*.whl) do python -m pip install -q --force-reinstall --no-deps "%%f" || goto :fail
python -c "import fae_backend as b; print('fae_backend', b.version(), '-', len(b.commands()), 'of', len(b.table()), 'commands in Rust')" || goto :fail

echo.
echo Rust back end built: Python extension fae_backend installed
exit /b 0

:fail
echo.
echo Building the Rust back end failed.
exit /b 1
