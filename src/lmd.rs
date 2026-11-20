//! LMD layer-map files.
//!
//! ```text
//! [GroupName],,,
//! NAME ,color, Linetype , Description      (Description may be <null>)
//! ```
//! Fields are trimmed on read. Group headers may carry junk after `]` (e.g.
//! `[TEL SKM Utilities Subdivisions] , <null>,,`) which is ignored.

use crate::host::{LayerHost, LayerInfo};
use crate::wildcard;
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq)]
pub struct LmdLayer {
    pub name: String,
    pub color: i32,
    pub linetype: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LmdGroup {
    /// Empty name = layers that appeared before any [Group] header.
    pub name: String,
    pub layers: Vec<LmdLayer>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LmdFile {
    pub groups: Vec<LmdGroup>,
}

pub fn parse(text: &str) -> (LmdFile, Vec<String>) {
    let mut file = LmdFile::default();
    let mut warnings = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim_start_matches('\u{feff}').trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            if let Some(end) = line.find(']') {
                file.groups.push(LmdGroup { name: line[1..end].trim().to_string(), layers: vec![] });
                continue;
            }
        }
        let f: Vec<&str> = line.splitn(4, ',').map(str::trim).collect();
        if f.len() < 3 || f[0].is_empty() {
            warnings.push(format!("line {}: expected NAME,color,linetype[,description]", i + 1));
            continue;
        }
        let color: i32 = match f[1].parse() {
            Ok(c) => c,
            Err(_) => {
                warnings.push(format!("line {}: bad colour '{}' for layer {}", i + 1, f[1], f[0]));
                continue;
            }
        };
        let desc = f.get(3).copied().unwrap_or("");
        let desc = if desc.eq_ignore_ascii_case("<null>") { "" } else { desc };
        if file.groups.is_empty() {
            file.groups.push(LmdGroup::default());
        }
        file.groups.last_mut().unwrap().layers.push(LmdLayer {
            name: f[0].to_string(),
            color,
            linetype: if f[2].is_empty() { "Continuous".into() } else { f[2].to_string() },
            description: desc.to_string(),
        });
    }
    (file, warnings)
}

impl LmdFile {
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        for g in &self.groups {
            if !g.name.is_empty() {
                s.push_str(&format!("[{}],,,\r\n", g.name));
            }
            for l in &g.layers {
                let d = if l.description.is_empty() { "<null>" } else { &l.description };
                s.push_str(&format!("{},{},{},{}\r\n", l.name, l.color, l.linetype, d));
            }
        }
        s
    }

    /// Replace the group of the same name (case-insensitive) or append it.
    pub fn merge_group(&mut self, group: LmdGroup) {
        match self.groups.iter_mut().find(|g| g.name.eq_ignore_ascii_case(&group.name)) {
            Some(g) => *g = group,
            None => self.groups.push(group),
        }
    }

    /// Layers of the chosen groups (by index). Layer names are case-insensitive in DWG, so a
    /// name that appears twice (this kind of file repeats DIM, XREF, FENCE ... in several
    /// groups) is created once: the first occurrence wins and the rest are reported.
    pub fn select(&self, group_indexes: &[usize]) -> (Vec<&LmdLayer>, Vec<String>) {
        let mut seen = HashSet::new();
        let (mut out, mut dups) = (Vec::new(), Vec::new());
        for (gi, g) in self.groups.iter().enumerate() {
            if !group_indexes.contains(&gi) {
                continue;
            }
            for l in &g.layers {
                if seen.insert(l.name.to_lowercase()) {
                    out.push(l);
                } else {
                    dups.push(format!("{} (in [{}])", l.name, g.name));
                }
            }
        }
        (out, dups)
    }

    /// One group built from the drawing's layers whose names match `pattern`.
    /// Skips `0`, `Defpoints` and xref-dependent layers.
    pub fn group_from_layers(group: &str, layers: &[LayerInfo], pattern: &str) -> LmdGroup {
        LmdGroup {
            name: group.to_string(),
            layers: layers
                .iter()
                .filter(|l| {
                    !l.name.contains('|')
                        && l.name != "0"
                        && !l.name.eq_ignore_ascii_case("defpoints")
                        && wildcard::matches(pattern, &l.name)
                })
                .map(|l| LmdLayer {
                    name: l.name.clone(),
                    color: if (1..=255).contains(&l.color) { l.color } else { 7 },
                    linetype: l.linetype.clone(),
                    description: l.description.clone(),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Existing {
    Skip,
    Update,
}

#[derive(Debug, Default)]
pub struct ImportReport {
    pub created: Vec<String>,
    pub updated: Vec<String>,
    pub skipped_existing: Vec<String>,
    pub duplicates_in_file: Vec<String>,
    /// "LAYER (LINETYPE)" for layers whose linetype is not in the drawing.
    pub linetype_fallback: Vec<String>,
    pub errors: Vec<String>,
}

pub fn import(host: &mut dyn LayerHost, file: &LmdFile, groups: &[usize], existing: Existing) -> ImportReport {
    let mut rep = ImportReport::default();
    let have: HashSet<String> = host.layers().iter().map(|l| l.name.to_lowercase()).collect();
    let (sel, dups) = file.select(groups);
    rep.duplicates_in_file = dups;
    for l in sel {
        let exists = have.contains(&l.name.to_lowercase());
        let result = match (exists, existing) {
            (true, Existing::Skip) => {
                rep.skipped_existing.push(l.name.clone());
                continue;
            }
            (true, Existing::Update) => host.update_layer(l).map(|ok| (ok, true)),
            (false, _) => host.create_layer(l).map(|ok| (ok, false)),
        };
        match result {
            Ok((exact, was_update)) => {
                if !exact {
                    rep.linetype_fallback.push(format!("{} ({})", l.name, l.linetype));
                }
                if was_update { rep.updated.push(l.name.clone()) } else { rep.created.push(l.name.clone()) }
            }
            Err(e) => rep.errors.push(format!("{}: {}", l.name, e)),
        }
    }
    rep
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const SAMPLE: &str = "[TT-STANDARD],,,\nD25 ,7, Continuous , Drawing 0.25 line width\nDIM ,7, Continuous , dimensions\n[CIVIL-ROADS],,,\nROAD CL ,1, CENTER , <null>\n[SVC] , <null>,,\nDIM ,2, Continuous , <null>\nDRAIN-LEFT,3,DRAIN-L,Drains (normal)\nbad line\n";

    #[test]
    fn parses_groups_and_fields() {
        let (f, w) = parse(SAMPLE);
        let names: Vec<&str> = f.groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["TT-STANDARD", "CIVIL-ROADS", "SVC"]);
        assert_eq!(f.groups[0].layers[0].description, "Drawing 0.25 line width");
        assert_eq!(f.groups[1].layers[0].name, "ROAD CL");
        assert_eq!(f.groups[1].layers[0].description, "");
        assert_eq!(f.groups[2].layers[1].linetype, "DRAIN-L");
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn dedups_across_groups() {
        let (f, _) = parse(SAMPLE);
        let (sel, dups) = f.select(&[0, 2]);
        assert_eq!(sel.len(), 3);
        assert_eq!(dups, ["DIM (in [SVC])"]);
        assert_eq!(sel.iter().find(|l| l.name == "DIM").unwrap().color, 7);
    }

    #[test]
    fn write_then_read_is_stable() {
        let (f, _) = parse(SAMPLE);
        let (f2, w2) = parse(&f.to_text());
        assert_eq!(f, f2);
        assert!(w2.is_empty());
    }

    #[test]
    fn merge_replaces_same_group() {
        let (mut f, _) = parse(SAMPLE);
        f.merge_group(LmdGroup { name: "civil-roads".into(), layers: vec![] });
        assert_eq!(f.groups.len(), 3);
        assert!(f.groups[1].layers.is_empty());
        f.merge_group(LmdGroup { name: "NEW".into(), layers: vec![] });
        assert_eq!(f.groups.len(), 4);
    }

    #[test]
    fn export_group_skips_protected_and_filters() {
        let mk = |n: &str, c: i32| LayerInfo { name: n.into(), color: c, linetype: "Continuous".into(), description: String::new() };
        let layers = vec![mk("0", 7), mk("Defpoints", 7), mk("X|A", 1), mk("UTIL_GAS", 90), mk("ROAD", -1)];
        let g = LmdFile::group_from_layers("G", &layers, "*");
        assert_eq!(g.layers.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), ["UTIL_GAS", "ROAD"]);
        assert_eq!(g.layers[1].color, 7);
        assert_eq!(LmdFile::group_from_layers("G", &layers, "util*").layers.len(), 1);
    }

    struct Mock {
        layers: Vec<String>,
        bad_lt: Vec<&'static str>,
    }
    impl LayerHost for Mock {
        fn layers(&self) -> Vec<LayerInfo> {
            self.layers.iter().map(|n| LayerInfo { name: n.clone(), color: 7, linetype: "Continuous".into(), description: String::new() }).collect()
        }
        fn create_layer(&mut self, l: &LmdLayer) -> Result<bool, String> {
            self.layers.push(l.name.clone());
            Ok(!self.bad_lt.contains(&l.linetype.as_str()))
        }
        fn update_layer(&mut self, l: &LmdLayer) -> Result<bool, String> {
            Ok(!self.bad_lt.contains(&l.linetype.as_str()))
        }
        fn rename_layer(&mut self, _: &str, _: &str) -> Result<(), String> { Ok(()) }
    }

    #[test]
    fn import_skip_update_and_fallback() {
        let (f, _) = parse(SAMPLE);
        let mut m = Mock { layers: vec!["0".into(), "dim".into()], bad_lt: vec!["DRAIN-L"] };
        let r = import(&mut m, &f, &[0, 1, 2], Existing::Skip);
        assert_eq!(r.skipped_existing, ["DIM"]); // case-insensitive
        assert_eq!(r.created, ["D25", "ROAD CL", "DRAIN-LEFT"]);
        assert_eq!(r.linetype_fallback, ["DRAIN-LEFT (DRAIN-L)"]);
        let mut m = Mock { layers: vec!["0".into(), "dim".into()], bad_lt: vec![] };
        let r = import(&mut m, &f, &[0], Existing::Update);
        assert_eq!(r.updated, ["DIM"]);
        assert_eq!(r.created, ["D25"]);
    }
}
