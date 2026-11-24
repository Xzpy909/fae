//! The filter kit's registry (docs/filter-kit.md): every `filters/<id>.json` + `filters/<id>.wgsl`
//! becomes
//!   - an entry of `FILTERS` (src/fx.rs includes `$OUT_DIR/fx_registry.rs`): kind, shader, parameter
//!     count, the blur field, plus a `const P_<FIELD>: u32` per parameter for the shader;
//!   - an entry of `FX_DEFS` in `ui/js/fx-filters.js` (the UI's card, chain, presets; generated, not
//!     edited by hand), written only when it changes.
//! Mistakes in a descriptor stop the build with a message naming the file.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const KINDS: [&str; 3] = ["pixel", "neighbourhood", "statistics"];
/// parameters one filter can have (kit.wgsl reads them from P[FXO..])
const MAX_PARAMS: usize = 48;

fn fail(file: &Path, msg: &str) -> ! {
    panic!("\n\nfilter kit: {}: {msg}\n", file.display());
}

struct Def {
    js: Value,
    rust: String,
}

fn ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars().next().unwrap().is_ascii_lowercase()
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn read(json_path: &Path) -> Def {
    let id = json_path.file_stem().unwrap().to_string_lossy().to_string();
    if !ident(&id) {
        fail(json_path, "the file name (the filter's id) must be lowercase letters, digits and _, starting with a letter");
    }
    let wgsl = json_path.with_extension("wgsl");
    if !wgsl.exists() {
        fail(json_path, &format!("no shader: {} is missing", wgsl.display()));
    }
    let text = fs::read_to_string(json_path).unwrap_or_else(|e| fail(json_path, &e.to_string()));
    let d: Value = serde_json::from_str(&text).unwrap_or_else(|e| fail(json_path, &format!("not valid JSON: {e}")));
    let s = |k: &str| d.get(k).and_then(Value::as_str);
    let name = s("name").unwrap_or_else(|| fail(json_path, "\"name\" (the tile / island title) is missing"));
    let kind = s("kind").unwrap_or_else(|| fail(json_path, "\"kind\" is missing"));
    if !KINDS.contains(&kind) {
        fail(json_path, &format!("\"kind\" must be one of {KINDS:?}"));
    }
    let fields = d.get("fields").and_then(Value::as_array).unwrap_or_else(|| fail(json_path, "\"fields\" (a list) is missing"));
    let pre = format!("fx_{id}_");
    // the UI's fields with the edit keys made unique (fx_<id>_<key>); the parameters in field order
    let mut ui_fields = vec![];
    let mut params: Vec<(String, String)> = vec![]; // (key, edit key)
    let mut keys = vec![];
    let opacity = s("opacity");
    for f in fields {
        match f {
            Value::String(x) if x == "---" => ui_fields.push(f.clone()),
            Value::Object(o) if o.contains_key("head") => ui_fields.push(f.clone()),
            Value::Array(a) => {
                // [key, label, default, min, max, step, decimals, unit?]
                if a.len() < 7 || !a[0].is_string() || !a[1].is_string() || !a[2..7].iter().all(Value::is_number) {
                    fail(json_path, &format!("field {f}: need [key, label, default, min, max, step, decimals, unit]"));
                }
                let key = a[0].as_str().unwrap().to_string();
                let (def, lo, hi) = (a[2].as_f64().unwrap(), a[3].as_f64().unwrap(), a[4].as_f64().unwrap());
                if !(lo <= def && def <= hi) {
                    fail(json_path, &format!("field {key}: the default {def} is outside {lo} .. {hi}"));
                }
                let mut a2 = a.clone();
                a2[0] = json!(format!("{pre}{key}"));
                ui_fields.push(Value::Array(a2));
                keys.push(key.clone());
                if Some(key.as_str()) != opacity {
                    params.push((key.clone(), format!("{pre}{key}")));
                }
            }
            Value::Object(o) if o.get("type").and_then(Value::as_str) == Some("select") => {
                let key = o.get("key").and_then(Value::as_str).unwrap_or_else(|| fail(json_path, "a select needs \"key\""));
                let opts = o.get("options").and_then(Value::as_array).filter(|v| !v.is_empty())
                    .unwrap_or_else(|| fail(json_path, &format!("select {key}: \"options\" [[value, label], ...] is missing")));
                let def = o.get("def").unwrap_or_else(|| fail(json_path, &format!("select {key}: \"def\" is missing")));
                if !opts.iter().any(|x| x.get(0) == Some(def)) {
                    fail(json_path, &format!("select {key}: \"def\" is not one of the options"));
                }
                let mut o2 = o.clone();
                o2.insert("key".into(), json!(format!("{pre}{key}")));
                ui_fields.push(Value::Object(o2));
                keys.push(key.to_string());
                params.push((key.to_string(), format!("{pre}{key}")));
            }
            _ => fail(json_path, &format!("field {f}: not a slider row, a select, \"---\" or a {{\"head\"}}")),
        }
    }
    for k in &keys {
        if !ident(k) {
            fail(json_path, &format!("field key {k:?}: lowercase letters, digits and _ only"));
        }
        if keys.iter().filter(|x| *x == k).count() > 1 {
            fail(json_path, &format!("field key {k:?} is used twice"));
        }
    }
    if let Some(op) = opacity {
        if !keys.iter().any(|k| k == op) {
            fail(json_path, &format!("\"opacity\": {op:?} is not one of the fields"));
        }
    }
    if params.len() > MAX_PARAMS {
        fail(json_path, &format!("{} parameters, at most {MAX_PARAMS}", params.len()));
    }
    // neighbourhood: which parameter is the blur's sigma (px at 1440 px image height, times "blur_scale")
    let blur = match (kind, s("blur")) {
        ("neighbourhood", None) => fail(json_path, "a neighbourhood filter needs \"blur\": the field that sets the blur's size"),
        ("neighbourhood", Some(b)) => Some(params.iter().position(|(k, _)| k == b)
            .unwrap_or_else(|| fail(json_path, &format!("\"blur\": {b:?} is not one of the parameters")))),
        (_, Some(_)) => fail(json_path, "\"blur\" is only for neighbourhood filters"),
        _ => None,
    };
    let blur_scale = d.get("blur_scale").and_then(Value::as_f64).unwrap_or(1.0);
    let hidden = d.get("hidden").and_then(Value::as_bool).unwrap_or(false);
    let consts: String = params.iter().enumerate()
        .map(|(i, (k, _))| format!("const P_{}: u32 = {i}u;\n", k.to_ascii_uppercase()))
        .collect();
    let rust = format!(
        "    FxDef {{ id: {id:?}, name: {name:?}, kind: FxKind::{kd}, src: include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/filters/{id}.wgsl\")), consts: {consts:?}, \
         nparams: {np}, blur: {blur}, blur_scale: {blur_scale:?}, hidden: {hidden} }},\n",
        kd = match kind { "pixel" => "Pixel", "neighbourhood" => "Neighbourhood", _ => "Statistics" },
        np = params.len(),
        blur = blur.map_or("None".into(), |b| format!("Some({b})")),
    );
    let js = json!({
        "id": id,
        "sec": format!("fx_{id}"),
        "name": name,
        "kind": kind,
        "hidden": hidden,
        "single": d.get("single").and_then(Value::as_bool).unwrap_or(false),
        "icon": s("icon").unwrap_or("<circle cx=\"12\" cy=\"12\" r=\"8\"/>"),
        "fields": ui_fields,
        "params": params.iter().map(|(_, k)| k.clone()).collect::<Vec<_>>(),
        "opacity": opacity.map(|o| format!("{pre}{o}")),
    });
    Def { js, rust }
}

fn main() {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let fdir = dir.join("filters");
    println!("cargo:rerun-if-changed=filters");
    let mut files: Vec<PathBuf> = fs::read_dir(&fdir)
        .map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).collect())
        .unwrap_or_default();
    files.retain(|p| p.extension().is_some_and(|e| e == "json"));
    files.sort();
    let defs: Vec<Def> = files.iter().map(|p| {
        println!("cargo:rerun-if-changed={}", p.display());
        println!("cargo:rerun-if-changed={}", p.with_extension("wgsl").display());
        read(p)
    }).collect();

    let mut rs = String::from("// generated by build.rs from filters/*.json\npub static FILTERS: &[FxDef] = &[\n");
    for d in &defs {
        rs.push_str(&d.rust);
    }
    rs.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("fx_registry.rs");
    fs::write(out, rs).unwrap();

    // the UI's copy (only when the app's ui/js is next to this crate, as in the repository)
    let ui = dir.join("../../../ui/js");
    if ui.is_dir() {
        let list: Vec<String> = defs.iter().map(|d| serde_json::to_string(&d.js).unwrap()).collect();
        let list = if list.is_empty() { "[]".to_string() } else { format!("[\n  {},\n]", list.join(",\n  ")) };
        let js = format!(
            "'use strict';\n/* GENERATED by core/gpu/fae-gpu/build.rs from core/gpu/fae-gpu/filters/*.json (runs with every\n   \
             build of the GPU core) - edit the .json, not this file. The filter kit's filters (docs/filter-kit.md):\n   \
             GPU only; without the GPU they are skipped. state.js adds them to FILTERS. */\nconst FX_DEFS = {list};\n"
        );
        let path = ui.join("fx-filters.js");
        if fs::read_to_string(&path).ok().as_deref() != Some(js.as_str()) {
            fs::write(&path, js).unwrap();
        }
    }
}
