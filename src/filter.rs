//! Layer-manager property filters (Name / Colour / Linetype / Description / Objects),
//! mirroring the OCS "Layer Filter" dialog: Name, Under (parent), match all/any,
//! and rows of [NOT] property op value. Includes rapid builders and CSV import/export.
//!
//! CSV columns: Filter,Under,Match,Not,Property,Op,Value   (one row per condition)

use crate::csv;
use crate::host::LayerInfo;
use crate::wildcard;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Match { All, Any }

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Property { Name, Color, Linetype, Description, ObjectCount }

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op { Equal, NotEqual, Less, Greater }

#[derive(Debug, Clone, PartialEq)]
pub struct Condition {
    pub not: bool,
    pub property: Property,
    pub op: Op,
    /// Text properties accept `*` and `?` wildcards.
    pub value: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayerFilter {
    pub name: String,
    /// Parent folder in the layer manager ("Property filters" in the dialog).
    pub under: String,
    pub mode: Match,
    pub conditions: Vec<Condition>,
}

impl Property {
    fn label(self) -> &'static str {
        match self {
            Property::Name => "Name",
            Property::Color => "Color",
            Property::Linetype => "Linetype",
            Property::Description => "Description",
            Property::ObjectCount => "Objects",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_lowercase().as_str() {
            "name" => Property::Name,
            "color" | "colour" => Property::Color,
            "linetype" => Property::Linetype,
            "description" => Property::Description,
            "objects" | "objectcount" => Property::ObjectCount,
            _ => return None,
        })
    }
}
impl Op {
    fn label(self) -> &'static str {
        match self { Op::Equal => "=", Op::NotEqual => "!=", Op::Less => "<", Op::Greater => ">" }
    }
    fn parse(s: &str) -> Option<Self> {
        Some(match s.trim() {
            "=" | "==" => Op::Equal,
            "!=" | "<>" => Op::NotEqual,
            "<" => Op::Less,
            ">" => Op::Greater,
            _ => return None,
        })
    }
}

impl Condition {
    pub fn eval(&self, l: &LayerInfo) -> bool {
        let numeric = |actual: i64| -> bool {
            let Ok(v) = self.value.trim().parse::<i64>() else { return false };
            match self.op {
                Op::Equal => actual == v,
                Op::NotEqual => actual != v,
                Op::Less => actual < v,
                Op::Greater => actual > v,
            }
        };
        let text = |actual: &str| -> bool {
            match self.op {
                Op::Equal => wildcard::matches(&self.value, actual),
                Op::NotEqual => !wildcard::matches(&self.value, actual),
                Op::Less => actual.to_lowercase() < self.value.to_lowercase(),
                Op::Greater => actual.to_lowercase() > self.value.to_lowercase(),
            }
        };
        let r = match self.property {
            Property::Name => text(&l.name),
            Property::Color => numeric(l.color as i64),
            Property::Linetype => text(&l.linetype),
            Property::Description => text(&l.description),
            Property::ObjectCount => numeric(l.object_count as i64),
        };
        r != self.not
    }
}

impl LayerFilter {
    pub fn matches(&self, l: &LayerInfo) -> bool {
        if self.conditions.is_empty() {
            return true;
        }
        match self.mode {
            Match::All => self.conditions.iter().all(|c| c.eval(l)),
            Match::Any => self.conditions.iter().any(|c| c.eval(l)),
        }
    }
    /// Live-result list for the dialog.
    pub fn apply<'a>(&self, layers: &'a [LayerInfo]) -> Vec<&'a LayerInfo> {
        layers.iter().filter(|l| self.matches(l)).collect()
    }
}

// ---------- rapid builders ----------

fn name_cond(op: Op, value: String) -> Condition {
    Condition { not: false, property: Property::Name, op, value }
}
fn mk(name: String, mode: Match, conditions: Vec<Condition>) -> LayerFilter {
    LayerFilter { name, under: "Property filters".into(), mode, conditions }
}

pub fn by_prefix(prefix: &str) -> LayerFilter {
    mk(format!("{prefix}*"), Match::All, vec![name_cond(Op::Equal, format!("{prefix}*"))])
}
pub fn by_suffix(suffix: &str) -> LayerFilter {
    mk(format!("*{suffix}"), Match::All, vec![name_cond(Op::Equal, format!("*{suffix}"))])
}
pub fn by_contains(text: &str) -> LayerFilter {
    mk(format!("*{text}*"), Match::All, vec![name_cond(Op::Equal, format!("*{text}*"))])
}
pub fn by_color(color: i32) -> LayerFilter {
    mk(
        format!("Color {color}"),
        Match::All,
        vec![Condition { not: false, property: Property::Color, op: Op::Equal, value: color.to_string() }],
    )
}
pub fn by_linetype(lt: &str) -> LayerFilter {
    mk(
        format!("LT {lt}"),
        Match::All,
        vec![Condition { not: false, property: Property::Linetype, op: Op::Equal, value: lt.to_string() }],
    )
}
/// Hide xref-dependent layers (`Name != *|*`), same as the "XREF-off" filter.
pub fn xref_off() -> LayerFilter {
    mk("XREF-off".into(), Match::All, vec![name_cond(Op::NotEqual, "*|*".into())])
}
pub fn xref_only() -> LayerFilter {
    mk("XREF-only".into(), Match::All, vec![name_cond(Op::Equal, "*|*".into())])
}
pub fn empty_layers() -> LayerFilter {
    mk(
        "Empty layers".into(),
        Match::All,
        vec![Condition { not: false, property: Property::ObjectCount, op: Op::Equal, value: "0".into() }],
    )
}
/// "Any of" filter listing explicit names, e.g. every layer of one LMD group.
pub fn from_names(filter_name: &str, names: &[String]) -> LayerFilter {
    mk(filter_name.into(), Match::Any, names.iter().map(|n| name_cond(Op::Equal, n.clone())).collect())
}

// ---------- CSV ----------

const HEADER: &str = "Filter,Under,Match,Not,Property,Op,Value";

pub fn to_csv(filters: &[LayerFilter]) -> String {
    let mut s = format!("{HEADER}\r\n");
    for f in filters {
        let mode = if f.mode == Match::All { "all" } else { "any" };
        if f.conditions.is_empty() {
            s.push_str(&csv::write_line(&[&f.name, &f.under, mode, "", "", "", ""]));
            s.push_str("\r\n");
        }
        for c in &f.conditions {
            s.push_str(&csv::write_line(&[
                &f.name, &f.under, mode,
                if c.not { "NOT" } else { "" },
                c.property.label(), c.op.label(), &c.value,
            ]));
            s.push_str("\r\n");
        }
    }
    s
}

pub fn from_csv(text: &str) -> (Vec<LayerFilter>, Vec<String>) {
    let mut out: Vec<LayerFilter> = Vec::new();
    let mut warn = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim_start_matches('\u{feff}');
        if line.trim().is_empty() || (i == 0 && line.to_lowercase().starts_with("filter,")) {
            continue;
        }
        let f = csv::parse_line(line);
        if f.len() < 7 || f[0].trim().is_empty() {
            warn.push(format!("line {}: expected 7 columns ({HEADER})", i + 1));
            continue;
        }
        let name = f[0].trim().to_string();
        if !out.iter().any(|x| x.name.eq_ignore_ascii_case(&name)) {
            let mode = if f[2].trim().eq_ignore_ascii_case("any") { Match::Any } else { Match::All };
            let under = if f[1].trim().is_empty() { "Property filters".to_string() } else { f[1].trim().to_string() };
            out.push(LayerFilter { name: name.clone(), under, mode, conditions: vec![] });
        }
        if f[4].trim().is_empty() {
            continue; // filter with no conditions
        }
        let (Some(property), Some(op)) = (Property::parse(&f[4]), Op::parse(&f[5])) else {
            warn.push(format!("line {}: unknown property '{}' or op '{}'", i + 1, f[4], f[5]));
            continue;
        };
        let flt = out.iter_mut().find(|x| x.name.eq_ignore_ascii_case(&name)).unwrap();
        flt.conditions.push(Condition { not: f[3].trim().eq_ignore_ascii_case("not"), property, op, value: f[6].clone() });
    }
    (out, warn)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(name: &str, color: i32, lt: &str, n: usize) -> LayerInfo {
        LayerInfo { name: name.into(), color, linetype: lt.into(), description: String::new(), object_count: n }
    }

    #[test]
    fn xref_off_matches_screenshot_behaviour() {
        let layers = vec![l("0", 7, "Continuous", 4004), l("GRID", 1, "CENTER", 0), l("XR-MOTSAM|GRID", 1, "CENTER", 3)];
        let r = xref_off().apply(&layers);
        assert_eq!(r.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["0", "GRID"]);
    }

    #[test]
    fn builders() {
        let layers = vec![l("UTIL_EX_GAS", 90, "X", 0), l("UTIL_PR_GAS", 90, "X", 2), l("ROAD", 1, "Continuous", 0)];
        assert_eq!(by_prefix("UTIL_EX_").apply(&layers).len(), 1);
        assert_eq!(by_suffix("_GAS").apply(&layers).len(), 2);
        assert_eq!(by_color(90).apply(&layers).len(), 2);
        assert_eq!(empty_layers().apply(&layers).len(), 2);
        assert_eq!(from_names("g", &["ROAD".into(), "UTIL_PR_GAS".into()]).apply(&layers).len(), 2);
    }

    #[test]
    fn csv_roundtrip() {
        let mut not_cond = by_contains("a,b");
        not_cond.conditions[0].not = true;
        not_cond.under = "Utilities".into();
        let fs = vec![xref_off(), from_names("Pick", &["A".into(), "B".into()]), not_cond];
        let (back, warn) = from_csv(&to_csv(&fs));
        assert!(warn.is_empty(), "{warn:?}");
        assert_eq!(back, fs);
    }
}
