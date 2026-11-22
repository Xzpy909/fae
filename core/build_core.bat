@echo off
rem Builds the Rust core:
rem   1. Python extension fastedit_core  -> installed into the current Python (pip)
rem   2. WebAssembly for the live preview -> ui\wasm\
rem Needs Rust (https://rustup.rs, with the "Desktop development with C++" / MSVC build tools).
rem The app needs the core (there is no NumPy / JS fallback), so build.bat stops when this fails.
setlocal
cd /d "%~dp0"

where cargo >nul 2>nul || (
  echo Rust is not installed. Get it from https://rustup.rs and run this again.
  exit /b 1
)

echo === Python extension ===
python -m pip install -q -U "maturin>=1.7,<2" || goto :fail
if exist target\wheels rmdir /s /q target\wheels
pushd fastedit-py
rem x86-64-v2 (SSE4.2, every x86 CPU since ~2009): floor() becomes one instruction instead of a
rem library call. Results are the same: floor is exact either way and nothing gets fused.
set "RUSTFLAGS=-C target-cpu=x86-64-v2"
python -m maturin build --release --out ..\target\wheels || (popd & goto :fail)
set RUSTFLAGS=
popd
for %%f in (target\wheels\fastedit_core-*.whl) do python -m pip install -q --force-reinstall --no-deps "%%f" || goto :fail
python -c "import fastedit_core as c; print('fastedit_core', c.version(), 'threads', c.threads())" || goto :fail

echo === WebAssembly ===
rustup target add wasm32-unknown-unknown || goto :fail
rem Threads (wasm-bindgen-rayon) need the standard library rebuilt with atomics: rust-src and
rem -Z build-std. RUSTC_BOOTSTRAP=1 allows that flag on the stable compiler, for this build only.
rustup component add rust-src || goto :fail
rem wasm-bindgen CLI must be exactly the version in Cargo.lock
set WBV=
for /f "tokens=2 delims=@" %%v in ('cargo pkgid -p wasm-bindgen') do set WBV=%%v
if "%WBV%"=="" goto :fail
set HAVE=
for /f "tokens=2" %%v in ('wasm-bindgen --version 2^>nul') do set HAVE=%%v
if not "%HAVE%"=="%WBV%" (
  echo Installing wasm-bindgen-cli %WBV% ^(one time, takes a few minutes^)
  cargo install wasm-bindgen-cli --version %WBV% --locked || goto :fail
)
rem atomics + bulk-memory + shared, imported memory (up to 4 GB): the thread pool's workers share it.
rem SIMD: WebView2 (Chromium) supports it, and it doesn't change float results (same IEEE ops).
set "RUSTFLAGS=-C target-feature=+atomics,+bulk-memory,+simd128 -C link-arg=--shared-memory -C link-arg=--import-memory -C link-arg=--max-memory=4294967296 -C link-arg=--export=__wasm_init_tls -C link-arg=--export=__tls_size -C link-arg=--export=__tls_align -C link-arg=--export=__tls_base"
set RUSTC_BOOTSTRAP=1
cargo build -p fastedit-wasm --release --target wasm32-unknown-unknown -Z build-std=std,panic_abort || goto :fail
set RUSTFLAGS=
set RUSTC_BOOTSTRAP=
if exist ..\ui\wasm rmdir /s /q ..\ui\wasm
wasm-bindgen --target web --out-dir ..\ui\wasm target\wasm32-unknown-unknown\release\fastedit_wasm.wasm || goto :fail

echo.
echo Core built: Python extension installed, WebAssembly in ui\wasm\
exit /b 0

:fail
echo.
echo Building the Rust core failed.
exit /b 1
