# Layer Names
2026.47 Compatible

External add-on for [Open CAD Studio](https://github.com/HakanSeven12/OpenCADStudio): layer
housekeeping in one window with four tabs). Ribbon tab **Layer Names**; commands start with `LN_`.

| Command | Opens |
|---|---|
| `LN_IMPORT` | the window on the **IMPORT / EXPORT** tab |
| `LN_RENAME` | the window on the **RENAME** tab |
| `LN_FILTER` | the window on the **FILTER** tab |
| `LN_LEGEND` | the window on the **LEGEND** tab |
| `LN_LEGENDQ [file.json]` | creates the legend from the saved JSON settings, **no window** |
| `LN_HELP` | command summary |

The buttons at the top of the window switch tabs; what you typed is kept.

## 1. IMPORT / EXPORT — LMD layer maps

```
[GroupName],,,
NAME ,color, Linetype , Description        (<null> = no description)
```

* **Import**: pick the file, tick the groups, choose what happens to layers that already exist
  (*skip* or *update colour/linetype/description*). One undo step.
* **Layer groups**: each ticked LMD group can also become a layer group in the Layer Manager. The
  name comes from a format (default `[{name}]` → `[CIVIL-ROADS]`; use `{name}` for no brackets).
  Re-importing refreshes the members of a group that already exists. The host only offers the
  `LAYERGROUP` command to plugins, and it splits on spaces: spaces in a group name become `_`.
* **Replace spaces with `_`** (default on): layer names such as `ROAD CL` are imported as `ROAD_CL`, so
  they can join their group. With it off, such layers are created with their original name but are not
  added to their group (listed on the command line).
* A name that appears in several ticked groups is created once (first wins); layer names are
  case-insensitive. The command line lists the repeats.
* A linetype the drawing doesn't have (and the CAD can't load by itself) is not an error: the layer
  is created with Continuous and listed, so you can load the linetype and re-import with *update*.
* **Export**: writes the drawing's layers (not `0`, `Defpoints`, xref layers) as one group, optionally
  limited by a wildcard. With *keep the other groups* the file is merged: only that group is replaced.

## 2. RENAME

Add prefix · Replace prefix · Add suffix · Replace suffix · Replace string (optional match case).
Limit it with a name wildcard (`UTIL_*`). The preview grid shows every change and its status:
`OK`, `invalid` (illegal character), `collision` (the result would duplicate another layer).
Only `OK` rows are renamed; chains and swaps (`A→B` with `B→C`) work. `0`, `Defpoints` and xref
layers are never touched. One undo step.

## 3. FILTER

* **Quick filter**: name starts with / ends with / contains / wildcard, colour, linetype, off, frozen,
  locked. Add several to the list, then *Run* creates them all.
* **Export** the drawing's filters to CSV, **Import** a CSV back.

CSV: `Filter,Parent,Match,Group,GroupMatch,Not,Property,Op,Value` — one row per condition.
Properties: Name, Description, Color, Linetype, On, Frozen, Locked, Plot, Xref.

**Limit of the host API (v23):** a plugin can create filters only through the host's `LAYERFILTER`
command, which supports `Name == pattern` (one), `Color ==`, `Linetype ==` and the On/Frozen/
Locked/Plot flags, AND-ed. OR is built with nested filters (`Name__1`, `Name__2` inside `Name`).
`!=`, `NOT Name`, Description, Xref and `< >` cannot be created — such a filter is reported and
skipped, never created broader than asked. That includes `Name != *|*` ("XREF-off"); make that one
once in the Layer Filter dialog.

## 4. LEGEND

Every size is a factor of the **scale**, chosen in the window: DIMSCALE, the current annotation scale
(1:100 → 100), or a custom value. Sizes (defaults): line 50 long, 7.5 apart; block cell 10; hatch swatch
25 × 5, 7.5 apart; text 2.5 high, 2.5 from the sample. All editable. Text goes on the current layer/style
unless you name a layer (created if missing) or style.

* **Layer legend** – pick the *lower-left* point. One polyline per layer (ByLayer, so it shows the layer's
  colour/linetype), alphabetical with the first on top, text `Name - Description` to the right.
  Options: wildcard filter, only layers in use, include xref layers, description on/off.
* **Object legend** – select lines/polylines, blocks, hatches (before or after OK), pick the *top-left*
  point. Order: lines/polylines (one sample per layer+colour+linetype), then blocks (one per block name,
  inserted with the first instance's scale into a 10 × scale cell, name beside it), then hatches (swatch
  with the same pattern/scale/colour, one per layer+pattern); each section alphabetical, label = layer
  name (+ description) for lines/hatches. Anonymous blocks (`*U…`) are skipped.
* Blocks and hatch swatches are centred on the middle of a sample line (25 × scale from the left at the
  defaults); all text starts at the same x, just past where a line sample ends. Each hatch swatch gets a
  rectangle on layer `LEGEND` (created if missing). The window opens on *Object legend* + *annotation scale*.
* **Annotation scale**: read from the host's live CANNOSCALE / CANNOSCALEVALUE (what the status bar shows),
  else from the drawing's saved header. The command line states what was read and from where
  (`annotation scale 1:100 (× 100) read from the host`). A name with no SCALE object is read as
  `paper:drawing`. If it ever reads wrong, choose *Custom value*.
* **Saved settings (JSON)**: in the LEGEND tab choose *Save the settings to the JSON file* (with or without
  creating the legend); *Load settings* fills the fields from a file. `LN_LEGENDQ` (ribbon *Legend (saved)*)
  then runs the legend straight from the last saved/loaded file: it only asks for the point (and the
  selection for an object legend). Default file: `%APPDATA%\OCS_Layname\legend.json` (Linux/macOS:
  `~/.config/OCS_Layname/legend.json`); `LN_LEGENDQ "C:\path\other.json"` uses another file. A file may
  hold only the fields you want to change; the rest take their defaults.
* The legend is plain geometry (polylines, inserts, hatches, text), one undo step. No cursor preview.

## Build

`cargo build --release` (toolchain pinned in `rust-toolchain.toml`, API pinned to the v2026.47
release commit like Raster Tools). `cargo test` runs the unit tests.
