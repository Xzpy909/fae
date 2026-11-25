//! FAE_GPU_VERSION: the GPU core's version (core/gpu's workspace version). Cargo gives a path dependency
//! from another workspace the version of the nearest enclosing workspace (core/, the CPU core's), so
//! fae-gpu's own CARGO_PKG_VERSION would be wrong here.
fn main() {
    let p = "../../gpu/Cargo.toml";
    println!("cargo:rerun-if-changed={p}");
    let txt = std::fs::read_to_string(p).unwrap_or_default();
    let v = txt
        .lines()
        .skip_while(|l| l.trim() != "[workspace.package]")
        .find_map(|l| l.trim().strip_prefix("version").map(|r| r.trim_start_matches([' ', '=']).trim().trim_matches('"').to_string()))
        .unwrap_or_else(|| "?".into());
    println!("cargo:rustc-env=FAE_GPU_VERSION={v}");
}
