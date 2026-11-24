# Filter kit: adding a filter

A new filter is two files in `core/gpu/fae-gpu/filters/`. Its island, its tile in the Filters list,
its place in the chain (re-orderable, stackable), presets, Process and the preview follow from them.

| File | What it is |
|---|---|
| `filters/<id>.json` | the descriptor: name, kind, sliders (the same rows as `FILTERS` in ui/js/state.js) |
| `filters/<id>.wgsl` | the maths: `fn fx(...)` (plus `fn stats()` for a statistics filter) |

Then `core\gpu\build_gpu.bat`, then `python tests\gpu\fx_check.py --record <id>`, check the images it
writes, and run the app.

Kit filters run **on the GPU only** (no CPU core version). Without a GPU (`--no-gpu`, no WebGPU, a GPU
failure) they are skipped: the status bar says "<name> skipped (needs the GPU)" in the CPU preview,
and Process's message says so when it ran on the CPU core. Preset thumbnails are drawn by the CPU
worker, so they don't show kit filters either.

## 1. The descriptor

```json
{
  "name": "Glow",
  "kind": "neighbourhood",
  "icon": "<circle cx=\"12\" cy=\"12\" r=\"4\"/>",
  "fields": [
    ["radius", "Radius", 20, 1, 100, 1, 0, ""],
    ["amount", "Amount", 0, 0, 100, 1, 0, "%"],
    { "type": "select", "key": "mode", "label": "Mode", "def": "soft", "options": [["soft", "Soft"], ["hard", "Hard"]] },
    "---",
    ["opacity", "Opacity", 100, 0, 100, 1, 0, "%"]
  ],
  "opacity": "opacity",
  "blur": "radius",
  "blur_scale": 0.5
}
```

- The file name is the filter's id (lowercase letters, digits, `_`). Never rename a filter that people
  have used: their edits and presets store values under `fx_<id>_<key>`.
- `kind`: `pixel`, `neighbourhood` or `statistics` (section 2).
- `fields`: slider rows `[key, label, default, min, max, step, decimals, unit]`, selects, `"---"`
  (divider), `{"head": "..."}` (sub-heading). The UI rules hold: standard slider, no explanations.
- **The defaults must change nothing.** A filter whose fields are all at their defaults is off (not
  in the chain); fx_check fails when the defaults change the image.
- `opacity` (optional): the field that is the filter's Opacity. The kit mixes it in (from a host f64
  table: GPUs fuse `f + (o - f) * op` into one fma and round the .5 cases the other way); the shader
  never sees it.
- `blur` (neighbourhood only): the field that sets the blur; sigma = value × `blur_scale` px at
  1440 px image height (so preview and Process agree at any size).
- `hidden: true`: a test filter, only in the Filters list with `python fae.py --test-filters`.
- `single: true`: not stackable (default: up to 8 instances, like the other filters).
- `icon`: the inner SVG of the tile (24×24 viewBox, stroked like the others).

build.rs checks the descriptor (a mistake stops the build with the file and the problem) and
regenerates `ui/js/fx-filters.js` (commit it with the filter; don't edit it).

## 2. The shader

The kit builds the filter's module from `shaders/common.wgsl` + `shaders/kit/common.wgsl` + the
template of its kind + a constant per parameter + the filter's file. The filter writes:

```wgsl
fn fx(c: vec3<f32>, at: vec2<u32>) -> vec3<f32> {
    let a = par(P_AMOUNT) / 100.0;          // the slider's value as shown (here 0..100)
    return c + a * (c - blur(at));
}
```

`c` = the pixel's levels (whole numbers 0..255), `at` = its position. Return levels as floats: the
kit rounds (`floor(x + 0.5)`), clamps to 0..255 and applies Opacity.

Available everywhere: `par(P_<KEY>)` (the parameters; a select gives its option's index 0, 1, ...),
`size()` (the image's pixels: the preview's stage and Process's image differ in size, so anything
measured in pixels should scale, e.g. with `f32(size().y) / 1440.0`), `luma(c)` (exact Rec.709 luma
of whole levels), `rnd`, `u8c`, `div`, `qdiv` (correctly rounded division by a whole number ≤ 4096).

| Kind | Passes | Extra in the shader |
|---|---|---|
| `pixel` | apply | – |
| `neighbourhood` | the input blurred (Sharpen's block means + Gaussian across + down, on a shrunk copy when sigma ≥ 3), apply | `blur(at)`: the blurred input (levels, not rounded) |
| `statistics` | R, G, B and luma histograms of the input (integer atomics), `stats()` once, apply | in `fn stats()`: `hist(ch, level)`, `pixels()`, `pct(ch, q)` (lowest level with ≥ q of the pixels at or below it), `put(i, v)` (i < 64); in `fx`: `stat(i)`. Channels `CH_R`, `CH_G`, `CH_B`, `CH_LUMA` |

Examples: `filters/test_pixel`, `test_blur`, `test_stats` (the kit's hidden test filters).

### Precision rules (learned in G2–G4, see claude/gpu-core-status.md)

- NVIDIA fuses `a * b + c` into one fma and divides through a reciprocal; lavapipe (the container)
  does neither. Where a result lands exactly on .5 or on a whole level before rounding, the two can
  differ by one level. Usually harmless; when a slider value multiplies a whole-level difference and
  the result is rounded, it is structured (many pixels at once) → compute that part on the host
  (the kit does this for Opacity; a filter that needs its own table is the moment to add one to the
  kit, as an optional `"tables"` entry).
- Divide by whole numbers with `qdiv` / `div`, not `/`, when exact quotients must stay exact.
- No big freely indexed `var<private>` arrays: D3D12's compiler takes seconds on them (G6: 3.6 s for
  one shader at start-up). Use `var<workgroup>` or storage memory.
- Shaders of the Filters list compile at start-up behind the loading screen (one at a time), hidden
  ones on first use.

## 3. Tests

```
python tests\gpu\fx_check.py --record <id>     # store the results (after looking at them)
python tests\gpu\fx_check.py                   # check every filter against its stored results
python tests\gpu\fx_check.py -k <id> -v        # one filter, every case listed
```

The cases come from the descriptor: the defaults (must be the input exactly), Opacity 0, every
parameter alone at its minimum and maximum (each option of a select), two seeded random sets at 100 %
and 37 % opacity; on three synthetic images (colour ramps, colour patches, a tall striped one for
big blurs on the shrunk copy). `--record` also writes every result as a PNG to
`tests/gpu/.work/fx/<id>/`: look at them before committing `tests/gpu/fx_results/<id>.zip`.
A check passes when each result is identical to the stored one or within tolerance.py's per-step
rule (another GPU rounds some f32 results the other way). Re-record after a change that is meant to
change the filter's look.

## 4. What goes where (for changes to the kit itself)

| Piece | File |
|---|---|
| registry, descriptor checks, ui/js/fx-filters.js | core/gpu/fae-gpu/build.rs |
| step, passes, parameter layout, pipelines | core/gpu/fae-gpu/src/fx.rs (+ `Kind::Fx` / `Aux::Fx` in lib.rs) |
| templates, shared helpers, the histogram block | core/gpu/fae-gpu/shaders/kit/ |
| bindings | fae-gpu-py `("fx", id, [params], opacity)`, `fae_gpu.filters()`; fae-gpu-wasm `step_fx(id, params, op)` |
| UI | state.js (`FX_LIST` → FILTERS / chain), controls.js (islands), edit-math.js (`fxParams`, the `fx` step), gpu-view.js (plan), wasm-params.js (skipped), process.js (skipped note) |
| Process | fastedit/core.py (`_steps(gpu=True)`, `render(skipped=...)`), api.py (`r["skipped"]`) |

Not in the kit yet (add when a filter needs it): per-filter host tables, box / guided-filter /
pyramid blocks, more than one blur per filter, statistics over a grid of cells.
