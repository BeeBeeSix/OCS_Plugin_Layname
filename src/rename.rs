//! Bulk layer renaming: prefix add/replace, suffix add/replace, string replace.
//! `plan` is a pure dry-run (drives the preview grid); `apply` executes the OK entries.

use crate::host::LayerHost;
use crate::wildcard;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq)]
pub enum RenameOp {
    AddPrefix(String),
    /// Layers not starting with `from` are left unchanged.
    ReplacePrefix { from: String, to: String },
    AddSuffix(String),
    /// Layers not ending with `from` are left unchanged.
    ReplaceSuffix { from: String, to: String },
    ReplaceString { from: String, to: String, case_sensitive: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Ok,
    Unchanged,
    Protected,
    Invalid(String),
    Collision(String),
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::Ok => "OK".into(),
            Status::Unchanged => "unchanged".into(),
            Status::Protected => "protected".into(),
            Status::Invalid(m) => format!("invalid: {m}"),
            Status::Collision(m) => format!("collision: {m}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub old: String,
    pub new: String,
    pub status: Status,
}

const BAD_CHARS: &[char] = &['<', '>', '/', '\\', '"', ':', ';', '?', '*', '|', '=', '`', ','];

pub fn is_protected(name: &str) -> bool {
    name == "0" || name.eq_ignore_ascii_case("defpoints") || name.contains('|') // xref-dependent
}

fn starts_with_ci(s: &str, p: &str) -> bool {
    s.len() >= p.len() && s.is_char_boundary(p.len()) && s[..p.len()].eq_ignore_ascii_case(p)
}
fn ends_with_ci(s: &str, p: &str) -> bool {
    s.len() >= p.len() && s.is_char_boundary(s.len() - p.len()) && s[s.len() - p.len()..].eq_ignore_ascii_case(p)
}

fn replace_ci(s: &str, from: &str, to: &str) -> String {
    if from.is_empty() {
        return s.to_string();
    }
    let (ls, lf) = (s.to_lowercase(), from.to_lowercase());
    // lowercase can change byte length for exotic chars; fall back to exact replace then
    if ls.len() != s.len() {
        return s.replace(from, to);
    }
    let mut out = String::new();
    let mut i = 0;
    while let Some(pos) = ls[i..].find(&lf) {
        out.push_str(&s[i..i + pos]);
        out.push_str(to);
        i += pos + lf.len();
    }
    out.push_str(&s[i..]);
    out
}

pub fn transform(name: &str, op: &RenameOp) -> String {
    match op {
        RenameOp::AddPrefix(p) => format!("{p}{name}"),
        RenameOp::AddSuffix(s) => format!("{name}{s}"),
        RenameOp::ReplacePrefix { from, to } => {
            if starts_with_ci(name, from) {
                format!("{to}{}", &name[from.len()..])
            } else {
                name.to_string()
            }
        }
        RenameOp::ReplaceSuffix { from, to } => {
            if ends_with_ci(name, from) {
                format!("{}{to}", &name[..name.len() - from.len()])
            } else {
                name.to_string()
            }
        }
        RenameOp::ReplaceString { from, to, case_sensitive } => {
            if *case_sensitive {
                if from.is_empty() { name.to_string() } else { name.replace(from.as_str(), to) }
            } else {
                replace_ci(name, from, to)
            }
        }
    }
}

/// Names matching the wildcard `scope` (blank = all).
pub fn in_scope(all: &[String], scope: &str) -> Vec<String> {
    let scope = scope.trim();
    all.iter()
        .filter(|n| scope.is_empty() || wildcard::matches(scope, n))
        .cloned()
        .collect()
}

/// `all` = every layer name in the drawing (for collision checks); `selected` = those to rename.
pub fn plan(all: &[String], selected: &[String], op: &RenameOp) -> Vec<Entry> {
    let mut entries: Vec<Entry> = selected
        .iter()
        .map(|old| {
            if is_protected(old) {
                return Entry { old: old.clone(), new: old.clone(), status: Status::Protected };
            }
            let new = transform(old, op);
            let status = if new == *old {
                Status::Unchanged
            } else if new.trim().is_empty() {
                Status::Invalid("empty name".into())
            } else if let Some(c) = new.chars().find(|c| BAD_CHARS.contains(c) || c.is_control()) {
                Status::Invalid(format!("illegal character '{c}'"))
            } else if new.chars().count() > 255 {
                Status::Invalid("longer than 255 characters".into())
            } else {
                Status::Ok
            };
            Entry { old: old.clone(), new, status }
        })
        .collect();

    // Final name of every layer if all Ok entries are applied; duplicates (case-insensitive) collide.
    let renamed: HashMap<&str, &str> = entries
        .iter()
        .filter(|e| e.status == Status::Ok)
        .map(|e| (e.old.as_str(), e.new.as_str()))
        .collect();
    let mut count: HashMap<String, usize> = HashMap::new();
    for n in all {
        let fin = renamed.get(n.as_str()).copied().unwrap_or(n.as_str());
        *count.entry(fin.to_lowercase()).or_default() += 1;
    }
    for e in entries.iter_mut().filter(|e| e.status == Status::Ok) {
        if count.get(&e.new.to_lowercase()).copied().unwrap_or(0) > 1 {
            e.status = Status::Collision(format!("'{}' would already exist or be produced twice", e.new));
        }
    }
    entries
}

#[derive(Debug, Default)]
pub struct ApplyReport {
    pub renamed: usize,
    pub errors: Vec<String>,
}

/// Executes `Status::Ok` entries. Handles chains/swaps (a->b while b->c, a<->b) via a temp name.
pub fn apply(host: &mut dyn LayerHost, entries: &[Entry]) -> ApplyReport {
    let mut rep = ApplyReport::default();
    let mut pending: Vec<(String, String)> = entries
        .iter()
        .filter(|e| e.status == Status::Ok)
        .map(|e| (e.old.clone(), e.new.clone()))
        .collect();
    let mut current: HashSet<String> = host.layers().iter().map(|l| l.name.to_lowercase()).collect();
    while !pending.is_empty() {
        let before = pending.len();
        let mut next = Vec::new();
        for (old, new) in pending {
            if current.contains(&new.to_lowercase()) && !old.eq_ignore_ascii_case(&new) {
                next.push((old, new));
                continue;
            }
            match host.rename_layer(&old, &new) {
                Ok(()) => {
                    current.remove(&old.to_lowercase());
                    current.insert(new.to_lowercase());
                    rep.renamed += 1;
                }
                Err(err) => rep.errors.push(format!("{old} -> {new}: {err}")),
            }
        }
        pending = next;
        if !pending.is_empty() && pending.len() == before {
            // cycle: park the first entry on a temp name, then retry it from there
            let (old, new) = pending.remove(0);
            let mut tmp = format!("{old}__tmp_rename");
            while current.contains(&tmp.to_lowercase()) {
                tmp.push('_');
            }
            match host.rename_layer(&old, &tmp) {
                Ok(()) => {
                    current.remove(&old.to_lowercase());
                    current.insert(tmp.to_lowercase());
                    pending.push((tmp, new));
                }
                Err(err) => rep.errors.push(format!("{old}: {err}")),
            }
        }
    }
    rep
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::LayerInfo;
    use crate::lmd::LmdLayer;

    fn s(v: &[&str]) -> Vec<String> { v.iter().map(|x| x.to_string()).collect() }

    #[test]
    fn transforms() {
        assert_eq!(transform("GAS", &RenameOp::AddPrefix("UTIL_".into())), "UTIL_GAS");
        assert_eq!(transform("UTIL_EX_GAS", &RenameOp::ReplacePrefix { from: "util_ex_".into(), to: "U-EX-".into() }), "U-EX-GAS");
        assert_eq!(transform("OTHER", &RenameOp::ReplacePrefix { from: "UTIL_".into(), to: "X".into() }), "OTHER");
        assert_eq!(transform("WATER_AG", &RenameOp::ReplaceSuffix { from: "_AG".into(), to: "_ABOVE".into() }), "WATER_ABOVE");
        assert_eq!(transform("A_B_A", &RenameOp::ReplaceString { from: "a".into(), to: "x".into(), case_sensitive: true }), "A_B_A");
        assert_eq!(transform("A_B_A", &RenameOp::ReplaceString { from: "a".into(), to: "x".into(), case_sensitive: false }), "x_B_x");
    }

    #[test]
    fn scope_wildcard() {
        let all = s(&["UTIL_GAS", "UTIL_WATER", "ROAD"]);
        assert_eq!(in_scope(&all, "util_*").len(), 2);
        assert_eq!(in_scope(&all, "").len(), 3);
    }

    #[test]
    fn protects_and_detects_collisions() {
        let all = s(&["0", "GAS", "UTIL_GAS", "XR|WALL", "WATER"]);
        let sel = s(&["0", "GAS", "XR|WALL", "WATER"]);
        let p = plan(&all, &sel, &RenameOp::AddPrefix("UTIL_".into()));
        let st = |n: &str| p.iter().find(|e| e.old == n).unwrap().status.clone();
        assert_eq!(st("0"), Status::Protected);
        assert_eq!(st("XR|WALL"), Status::Protected);
        assert!(matches!(st("GAS"), Status::Collision(_)));
        assert_eq!(st("WATER"), Status::Ok);
    }

    #[test]
    fn case_only_rename_is_not_a_collision() {
        let all = s(&["Gas"]);
        let p = plan(&all, &all, &RenameOp::ReplaceString { from: "gas".into(), to: "GAS".into(), case_sensitive: false });
        assert_eq!(p[0].status, Status::Ok);
    }

    #[test]
    fn rejects_bad_names() {
        let all = s(&["A"]);
        let p = plan(&all, &all, &RenameOp::AddSuffix("|x".into()));
        assert!(matches!(p[0].status, Status::Invalid(_)));
    }

    struct Mock(Vec<String>);
    impl LayerHost for Mock {
        fn layers(&self) -> Vec<LayerInfo> {
            self.0.iter().map(|n| LayerInfo { name: n.clone(), color: 7, linetype: "Continuous".into(), description: "".into() }).collect()
        }
        fn create_layer(&mut self, l: &LmdLayer) -> Result<bool, String> { self.0.push(l.name.clone()); Ok(true) }
        fn update_layer(&mut self, _: &LmdLayer) -> Result<bool, String> { Ok(true) }
        fn rename_layer(&mut self, o: &str, n: &str) -> Result<(), String> {
            if self.0.iter().any(|x| x.eq_ignore_ascii_case(n)) { return Err("exists".into()); }
            let i = self.0.iter().position(|x| x == o).ok_or("missing")?;
            self.0[i] = n.into();
            Ok(())
        }
    }

    #[test]
    fn apply_handles_chain() {
        let mut m = Mock(s(&["A", "B"]));
        let entries = vec![
            Entry { old: "A".into(), new: "B".into(), status: Status::Ok },
            Entry { old: "B".into(), new: "C".into(), status: Status::Ok },
        ];
        let r = apply(&mut m, &entries);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.renamed, 2);
        let mut got = m.0.clone();
        got.sort();
        assert_eq!(got, ["B", "C"]);
    }

    #[test]
    fn apply_handles_swap() {
        let mut m = Mock(s(&["A", "B"]));
        let entries = vec![
            Entry { old: "A".into(), new: "B".into(), status: Status::Ok },
            Entry { old: "B".into(), new: "A".into(), status: Status::Ok },
        ];
        let r = apply(&mut m, &entries);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.renamed, 2);
    }
}
