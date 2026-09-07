# 01 — The browser browses presets

Adam, 2026-09-05: *"i think i want that panel to also be able to browse and
load presets."*

`docs/plans/preset-system/` decided a preset's unit is a device and shipped
the whole mechanism — four preset classes, relative addressing, factory banks
for every effect kind and both instrument banks. It then stopped, explicitly,
waiting for something to design a browser against. Both banks now ship, so
nothing is blocking it.

## What already exists, and is worth reading before building anything

`mooloop_project::list_presets` (`crates/mooloop-project/src/lib.rs:1049`)
already returns everything a browser needs:

```rust
pub struct PresetSummary {
    pub path: PathBuf,
    pub name: String,
    pub category: String,
    pub tags: Vec<String>,
    pub kind: PresetKind,
}
```

It scans one directory level, sorts by `(category, name)`, and **silently
skips** bundles that fail to parse, are the wrong format version, or declare a
`contains` list this build cannot honour — a bundle that could not be loaded
is left out of the list rather than offered and then refused. That refusal
policy is already right for a browser and should not be re-litigated.

`summarize_preset` recognises four document types: `generator`, `channel`,
`effect`, and `effect_run` — the last being a container's whole run, which is
attributed to `EffectKind::Chain` because nothing else can be at the head of a
well-formed run.

The application scans these in `refresh_preset_menus`
(`crates/mooloop-ui/src/lib.rs:10386`), on every channel switch and project
load, into three lists on the session: `generator_presets` (the selected
channel's kind only), `channel_presets`, and `effect_presets` (every kind, in
one flat scan, filtered per rack row when the row is built). Its doc comment
says the cost is fine: "presets are a handful of small TOML manifests, not a
large library."

**The two fields nothing displays are `category` and `tags`.** They are
parsed, sorted on, and thrown away — the rail menus show a flat list of names.
That is the taxonomy surface `preset-system/` said was unblocked, and it is
most of what makes a browser better than the menu it replaces.

The browser panel itself is a flattened tree: Rust hands `main.slint` a
`[BrowserRow]` (`main.slint:553`), one row per visible line, built by
`crates/mooloop-session/src/browser.rs`. That file has **no reference to
presets at all** — it is `scan_browser_dir`, `is_playable_sample`, and folder
expansion, and it is the right shape to grow a second source.

## The step

Give the browser panel a preset half, fed by `list_presets` across the
well-known directories rather than by a user-added filesystem location.

- **A second tree, not a second panel.** The panel already has one flattened
  tree and one selection model. Presets are a second root beside the sample
  locations, grouped by kind and then by the `category` each summary already
  carries, with `tags` shown on the info line the panel already has
  (`browser-info-name` / `browser-info-stats`, `main.slint:571-573`).
- **Loading is the existing verb, not a new one.** An effect preset is a rack
  edit, not a document load — `preset-system/`'s second pass established that
  and it is why loading goes through the session. Reuse
  `load_effect_preset`, `load_effect_run` and the generator half already wired
  to the device rail. The browser chooses *which* preset; it does not learn
  how to apply one.
- **Where it lands is the selected device.** A preset's unit is a device, so
  the target is the selected rack row for an effect, the source device for a
  generator, and the selected channel for a channel preset. A preset whose
  kind does not match anything selected is visible but not loadable, and says
  so — the same shape as the container's "what it cannot carry" message.

## Two things to decide while building, not before

- **Whether the preset tree is filtered to the selection.** The rail menus
  are: a Delay row offers Delay presets. A browser that showed only what the
  current selection accepts would be predictable but would stop being a
  *browser* — you could not look at the Reverb bank without first selecting a
  reverb. The likely answer is show everything, sort the loadable to the top,
  but that is an ear-and-hand judgement, so build it switchable and try both.
- **Whether preview applies.** A sample previews on click. An effect preset
  cannot preview without being loaded, and loading is undoable, so "preview"
  and "load" may collapse into one gesture that you undo. Do not build an
  audition path for presets in this step.

## Do not

- **Do not add a preset format, a fragment format, or a tag editor.** The
  manifest says `contains = ["effect_params"]` precisely so a later fragment
  format can supersede it cleanly; that is a later plan and this step must not
  pre-empt it. Tags are displayed here, not authored.
- **Do not fix the factory-bank update problem here.** `LOOSE_ENDS.md`
  records that banks self-seed once behind a `.factory-v1` marker and can
  never update. A browser makes it more visible, and it is still its own
  change.
- **Do not curate the banks.** `FOCUS.md` is explicit that authoring content
  by taste is a deliberate later push, and that a device step must not be held
  open waiting for it. The same goes for a browser step.

## Done when

The browser panel lists every preset on disk grouped by kind and category,
shows a preset's tags, and can load one onto the selected device — including a
container's whole run — without the menu bar or a rail button, and without the
mouse once step 04 lands.
