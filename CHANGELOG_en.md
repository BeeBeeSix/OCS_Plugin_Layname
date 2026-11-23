# Changelog

## Unreleased

* Legend settings to/from JSON; `LN_LEGENDQ` + ribbon tool *Legend (saved)* run the legend without the window.
* Annotation scale: live CANNOSCALE/CANNOSCALEVALUE first, name parsing as fallback, result reported.

* LEGEND tab / `LN_LEGEND`: layer legend and object legend (lines, blocks, hatches), sized by DIMSCALE,
  annotation scale or a custom scale; all sizes, spacing and text options editable.
* Legend: hatch swatches framed on layer LEGEND; blocks/hatches centred 25 × scale from the left; all text
  aligned; defaults are Object legend + annotation scale.

## 0.1.0

First version.

* IMPORT / EXPORT: read and write LMD layer maps with `[Group]` sections; choose groups, skip or
  update existing layers, merge an exported group into an existing file.
* RENAME: add/replace prefix, add/replace suffix, replace string; wildcard scope, preview with
  collision and illegal-name checks; chains and swaps handled.
* FILTER: quick layer filters, CSV export/import (`LAYERFILTER` command based, see README for limits).
