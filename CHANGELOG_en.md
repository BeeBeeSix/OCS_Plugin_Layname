# Changelog

All notable changes to LayerManager are recorded here.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: [Semantic Versioning](https://semver.org/) —
`0.x.y` means an initial version, still being tested.

This is `CHANGELOG.md`  which the GitHub release notes are taken from. 
`scripts/release.ps1` moves the entries under **[Unreleased]** into the new version when a release is made.

## [Unreleased]
First version.
### Added
* IMPORT / EXPORT: read and write LMD layer maps with `[Group]` sections; choose groups, skip or
  update existing layers, merge an exported group into an existing file.
* RENAME: add/replace prefix, add/replace suffix, replace string; wildcard scope, preview with
  collision and illegal-name checks; chains and swaps handled.
* FILTER: quick layer filters, CSV export/import (`LAYERFILTER` command based, see README for limits).
