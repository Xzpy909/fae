@echo off
rem Builds the GPU core (wgpu + WGSL, see claude/gpu-core-plan.md):
rem   1. Python extension fae_gpu     -> installed into the current Python (pip)
rem   2. WebAssembly for the preview  -> ui\gpu\
rem Same Rust as the CPU core (core\rust-toolchain.toml). The live preview uses ui\gpu\ when every
rem tool of the edit runs on the GPU (ui\js\gpu-view.js), Process too (fastedit\core.py). Checks:
rem   python tests\gpu\tolerance.py       GPU vs CPU core on the parity cases
rem   python tests\gpu\bench_native.py    speed (DX12 / Vulkan)
rem   python tests\gpu\process_check.py   Process on the GPU vs the CPU core, own photos at full size
rem   python fae.py --gpu-spike           WebGPU in the app window, speed of the WebAssembly build
setlocal
cd /d "%~dp0"

where cargo >nul 2>nul || (
  echo Rust is not installed. Get it from https://rustup.rs and run this again.
  exit /b 1
)

echo === Python extension (fae_gpu) ===
python -m pip install -q -U "maturin>=1.7,<2" || goto :fail
if exist target\wheels rmdir /s /q target\wheels
pushd fae-gpu-py
python -m maturin build --release --out ..\target\wheels || (popd & goto :fail)
popd
for %%f in (target\wheels\fae_gpu-*.whl) do python -m pip install -q --force-reinstall --no-deps "%%f" || goto :fail
python -c "import fae_gpu as g; d = g.Gpu(); i = d.info(); print('fae_gpu', g.version(), '-', i['name'], 'via', i['backend'])" || goto :fail

echo === WebAssembly (WebGPU) ===
rustup target add wasm32-unknown-unknown || goto :fail
rem wasm-bindgen CLI must be exactly the version in Cargo.lock (the same as the CPU core's today)
set WBV=
for /f "tokens=2 delims=@" %%v in ('cargo pkgid -p wasm-bindgen') do set WBV=%%v
if "%WBV%"=="" goto :fail
set HAVE=
for /f "tokens=2" %%v in ('wasm-bindgen --version 2^>nul') do set HAVE=%%v
if not "%HAVE%"=="%WBV%" (
  echo Installing wasm-bindgen-cli %WBV% ^(one time, takes a few minutes^)
  cargo install wasm-bindgen-cli --version %WBV% --locked || goto :fail
)
rem plain single-threaded build: no atomics / build-std needed (the GPU does the work)
set RUSTFLAGS=
cargo build -p fae-gpu-wasm --release --target wasm32-unknown-unknown || goto :fail
if exist ..\..\ui\gpu rmdir /s /q ..\..\ui\gpu
wasm-bindgen --target web --out-dir ..\..\ui\gpu target\wasm32-unknown-unknown\release\fae_gpu_wasm.wasm || goto :fail

echo.
echo GPU core built: Python extension fae_gpu installed, WebAssembly in ui\gpu\
exit /b 0

:fail
echo.
echo Building the GPU core failed.
exit /b 1
