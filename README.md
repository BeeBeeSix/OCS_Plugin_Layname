# Layer Names (`opencad.layname`)

External add-on for [Open CAD Studio](https://github.com/HakanSeven12/OpenCADStudio): layer
housekeeping in one window with three tabs. Ribbon tab **Layer Names**; commands start with `LN_`.

| Command | Opens |
|---|---|
| `LN_IMPORT` | the window on the **IMPORT / EXPORT** tab |
| `LN_RENAME` | the window on the **RENAME** tab |
| `LN_FILTER` | the window on the **FILTER** tab |
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

## Build

`cargo build --release` (toolchain pinned in `rust-toolchain.toml`, API pinned to the v2026.46
release commit like Raster Tools). `cargo test` runs the unit tests.
