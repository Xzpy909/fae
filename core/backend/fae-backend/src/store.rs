//! The app's JSON files (settings, presets), read and written as fastedit/api.py did: read failures give
//! an empty object, files are written whole (to `<file>.tmp`, then swapped in, so a crash can't leave
//! half a file), 2-space indentation, key order kept.

use serde_json::{Map, Value};
use std::io::Write;
use std::path::Path;

/// The JSON in `path`; `{}` when it is missing, unreadable, not JSON, or (with `any` false) not an object.
pub fn read_object(path: &Path, any: bool) -> Value {
    let v = std::fs::read(path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    match v {
        Some(v) if any || v.is_object() => v,
        _ => Value::Object(Map::new()),
    }
}

/// Write `v` to `path` (indent 2, UTF-8), through `<path>.tmp`.
pub fn write_json(path: &Path, v: &Value) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(v).map_err(std::io::Error::other)?;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    // Windows: rename replaces an existing file (MoveFileEx with MOVEFILE_REPLACE_EXISTING)
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}
