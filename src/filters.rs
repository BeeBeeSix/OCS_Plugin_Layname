//! Layer filters: model, CSV exchange, reading the drawing's filters, and turning
//! a filter into host `LAYERFILTER` commands.
//!
//! What the host lets a plugin do (API 23): filters are created through the command
//! line — `LAYERFILTER NEW <name> <name-pattern>`, `SET <name> COLOR|LINETYPE|ON|OFF|
//! FROZEN|THAWED|LOCKED|UNLOCKED|PLOT|NOPLOT`, `NEST <parent> <child>`, `REMOVE <name>`.
//! That covers `Name == pattern`, `Color ==`, `Linetype ==` and the on/frozen/locked/plot
//! flags, all AND-ed; OR is built with nested filters. It cannot express `!=`, NOT,
//! Description, Xref or `< >` comparisons, so [`plan_commands`] refuses such a filter as
//! a whole (reporting why) instead of creating a broader one than asked for.
//!
//! CSV (one row per condition):
//! `Filter,Parent,Match,Group,GroupMatch,Not,Property,Op,Value`

use crate::csvutil;
use ocs_plugin_api::host::codec;
use ocs_plugin_api::host::codec::objects::{ObjectType, XRecordValue};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prop { Name, Description, Color, Linetype, On, Frozen, Locked, Plot, Xref }

impl Prop {
    const ALL: [Prop; 9] = [Prop::Name, Prop::Description, Prop::Color, Prop::Linetype, Prop::On, Prop::Frozen, Prop::Locked, Prop::Plot, Prop::Xref];
    /// Key in the host's stored expression.
    fn key(self) -> &'static str {
        match self {
            Prop::Name => "NAME", Prop::Description => "DESCRIPTION", Prop::Color => "COLOR", Prop::Linetype => "LINETYPE",
            Prop::On => "ON", Prop::Frozen => "FROZEN", Prop::Locked => "LOCKED", Prop::Plot => "PLOTTABLE", Prop::Xref => "XREF",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Prop::Name => "Name", Prop::Description => "Description", Prop::Color => "Color", Prop::Linetype => "Linetype",
            Prop::On => "On", Prop::Frozen => "Frozen", Prop::Locked => "Locked", Prop::Plot => "Plot", Prop::Xref => "Xref",
        }
    }
    pub fn is_flag(self) -> bool {
        matches!(self, Prop::On | Prop::Frozen | Prop::Locked | Prop::Plot | Prop::Xref)
    }
    fn parse(s: &str) -> Option<Prop> {
        let s = s.trim();
        Prop::ALL.into_iter().find(|p| p.label().eq_ignore_ascii_case(s) || p.key().eq_ignore_ascii_case(s) || (*p == Prop::Color && s.eq_ignore_ascii_case("colour")))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op { Eq, Ne, Gt, Ge, Lt, Le }

impl Op {
    fn sym(self) -> &'static str {
        match self { Op::Eq => "==", Op::Ne => "!=", Op::Gt => ">", Op::Ge => ">=", Op::Lt => "<", Op::Le => "<=" }
    }
    fn parse(s: &str) -> Option<Op> {
        Some(match s.trim() {
            "=" | "==" => Op::Eq, "!=" | "<>" => Op::Ne, ">" => Op::Gt, ">=" => Op::Ge, "<" => Op::Lt, "<=" => Op::Le,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cond {
    pub not: bool,
    pub prop: Prop,
    pub op: Op,
    /// Flags are normalised to "1" / "0".
    pub value: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub any: bool,
    pub conds: Vec<Cond>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item { Cond(Cond), Group(Group) }

#[derive(Clone, Debug, PartialEq)]
pub struct FilterDef {
    pub name: String,
    pub parent: Option<String>,
    /// How the top-level items combine (`false` = all of them).
    pub any: bool,
    pub items: Vec<Item>,
}

impl Cond {
    pub fn new(prop: Prop, op: Op, value: &str) -> Cond {
        Cond { not: false, prop, op, value: norm_value(prop, value) }
    }
}

fn norm_value(prop: Prop, v: &str) -> String {
    let v = v.trim().trim_matches('"');
    if prop.is_flag() {
        let t = matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on");
        return if t { "1" } else { "0" }.into();
    }
    v.to_string()
}

impl FilterDef {
    pub fn simple(name: &str, conds: Vec<Cond>) -> FilterDef {
        FilterDef { name: name.into(), parent: None, any: false, items: conds.into_iter().map(Item::Cond).collect() }
    }
    /// Human summary, e.g. `Name == "A*" AND Color == "1"`.
    pub fn describe(&self) -> String {
        let join = if self.any { " OR " } else { " AND " };
        let one = |c: &Cond| {
            let v = if c.prop.is_flag() { if c.value == "1" { "True" } else { "False" }.to_string() } else { c.value.clone() };
            format!("{}{} {} \"{v}\"", if c.not { "NOT " } else { "" }, c.prop.label(), c.op.sym())
        };
        self.items
            .iter()
            .map(|i| match i {
                Item::Cond(c) => one(c),
                Item::Group(g) => format!("({})", g.conds.iter().map(one).collect::<Vec<_>>().join(if g.any { " OR " } else { " AND " })),
            })
            .collect::<Vec<_>>()
            .join(join)
    }
}

// ───────────────────────── quick builders (dialog) ─────────────────────────

/// (label shown in the dialog, needs a value)
pub const QUICK_TYPES: [(&str, bool); 9] = [
    ("Name starts with", true),
    ("Name ends with", true),
    ("Name contains", true),
    ("Name (wildcards * ?)", true),
    ("Colour (ACI number)", true),
    ("Linetype", true),
    ("Layers that are Off", false),
    ("Layers that are Frozen", false),
    ("Layers that are Locked", false),
];

/// A rapid filter from a dialog choice. `name` empty = automatic name.
pub fn quick(kind: usize, value: &str, name: &str) -> Result<FilterDef, String> {
    let v = value.trim();
    if QUICK_TYPES.get(kind).map_or(true, |t| t.1) && v.is_empty() {
        return Err("a value is needed".into());
    }
    let (auto, cond) = match kind {
        0 => (format!("{v}*"), Cond::new(Prop::Name, Op::Eq, &format!("{v}*"))),
        1 => (format!("*{v}"), Cond::new(Prop::Name, Op::Eq, &format!("*{v}"))),
        2 => (format!("*{v}*"), Cond::new(Prop::Name, Op::Eq, &format!("*{v}*"))),
        3 => (v.to_string(), Cond::new(Prop::Name, Op::Eq, v)),
        4 => {
            let n: u16 = v.parse().map_err(|_| "colour must be a number 0-255".to_string())?;
            if n > 255 { return Err("colour must be a number 0-255".into()); }
            (format!("Color_{n}"), Cond::new(Prop::Color, Op::Eq, &n.to_string()))
        }
        5 => (format!("LT_{v}"), Cond::new(Prop::Linetype, Op::Eq, v)),
        6 => ("Off_layers".into(), Cond::new(Prop::On, Op::Eq, "0")),
        7 => ("Frozen_layers".into(), Cond::new(Prop::Frozen, Op::Eq, "1")),
        8 => ("Locked_layers".into(), Cond::new(Prop::Locked, Op::Eq, "1")),
        _ => return Err("unknown filter type".into()),
    };
    let name = if name.trim().is_empty() { auto } else { name.trim().to_string() };
    Ok(FilterDef::simple(&name, vec![cond]))
}

// ───────────────────────── CSV ─────────────────────────

pub const CSV_HEADER: &str = "Filter,Parent,Match,Group,GroupMatch,Not,Property,Op,Value";

fn csv_value(c: &Cond) -> String {
    if c.prop.is_flag() { if c.value == "1" { "True" } else { "False" }.into() } else { c.value.clone() }
}

pub fn to_csv(defs: &[FilterDef]) -> String {
    let mut s = format!("{CSV_HEADER}\r\n");
    let mut push = |f: &[&str]| {
        s.push_str(&csvutil::write_line(f));
        s.push_str("\r\n");
    };
    for d in defs {
        let parent = d.parent.as_deref().unwrap_or("");
        let m = if d.any { "any" } else { "all" };
        if d.items.is_empty() {
            push(&[&d.name, parent, m, "", "", "", "", "", ""]);
        }
        for (gi, item) in d.items.iter().enumerate() {
            let cond_row = |push: &mut dyn FnMut(&[&str]), grp: &str, gm: &str, c: &Cond| {
                push(&[&d.name, parent, m, grp, gm, if c.not { "NOT" } else { "" }, c.prop.label(), c.op.sym(), &csv_value(c)]);
            };
            match item {
                Item::Cond(c) => cond_row(&mut push, "", "", c),
                Item::Group(g) => {
                    let label = format!("g{}", gi + 1);
                    for c in &g.conds {
                        cond_row(&mut push, &label, if g.any { "any" } else { "all" }, c);
                    }
                }
            }
        }
    }
    s
}

pub fn from_csv(text: &str) -> (Vec<FilterDef>, Vec<String>) {
    let mut out: Vec<FilterDef> = Vec::new();
    let mut warn = Vec::new();
    let mut prev_group: Option<(String, String)> = None;
    for (i, line) in text.lines().enumerate() {
        let line = line.trim_start_matches('\u{feff}');
        if line.trim().is_empty() || (i == 0 && line.to_lowercase().starts_with("filter,")) {
            continue;
        }
        let f = csvutil::parse_line(line);
        if f.len() < 9 || f[0].trim().is_empty() {
            warn.push(format!("line {}: expected 9 columns ({CSV_HEADER})", i + 1));
            continue;
        }
        let name = f[0].trim().to_string();
        let pos = match out.iter().position(|d| d.name.eq_ignore_ascii_case(&name)) {
            Some(p) => p,
            None => {
                let parent = f[1].trim();
                out.push(FilterDef {
                    name: name.clone(),
                    parent: (!parent.is_empty()).then(|| parent.to_string()),
                    any: f[2].trim().eq_ignore_ascii_case("any"),
                    items: vec![],
                });
                out.len() - 1
            }
        };
        if f[6].trim().is_empty() {
            continue; // a filter with no conditions
        }
        let (Some(prop), Some(op)) = (Prop::parse(&f[6]), Op::parse(&f[7])) else {
            warn.push(format!("line {}: unknown property '{}' or operator '{}'", i + 1, f[6], f[7]));
            continue;
        };
        let cond = Cond { not: f[5].trim().eq_ignore_ascii_case("not"), prop, op, value: norm_value(prop, &f[8]) };
        let label = f[3].trim();
        if label.is_empty() {
            out[pos].items.push(Item::Cond(cond));
            prev_group = None;
        } else {
            // consecutive rows of one filter with the same label form one group
            let any = f[4].trim().eq_ignore_ascii_case("any");
            let key = (name.to_lowercase(), label.to_string());
            match out[pos].items.last_mut() {
                Some(Item::Group(g)) if prev_group.as_ref() == Some(&key) => g.conds.push(cond),
                _ => out[pos].items.push(Item::Group(Group { any, conds: vec![cond] })),
            }
            prev_group = Some(key);
        }
    }
    (out, warn)
}

// ───────────────────── reading the drawing's filters ─────────────────────

const KIND: &str = "AcLyLayerFilter";
const DICTIONARY: &str = "ACLYDICTIONARY";

/// Parse the stored expression: `( NAME== "A*" AND ( COLOR== "1" OR … ) )`.
pub fn parse_expression(expression: &str) -> Option<(bool, Vec<Item>)> {
    let text = strip_parens(expression);
    if text.is_empty() {
        return Some((false, vec![]));
    }
    let (any, parts) = split_top(text)?;
    let mut items = Vec::new();
    for part in parts {
        let part = strip_parens(part);
        let (gany, inner) = split_top(part)?;
        if inner.len() > 1 {
            let conds = inner.into_iter().map(|c| parse_condition(strip_parens(c))).collect::<Option<Vec<_>>>()?;
            items.push(Item::Group(Group { any: gany, conds }));
        } else {
            items.push(Item::Cond(parse_condition(part)?));
        }
    }
    Some((any, items))
}

fn strip_parens(mut text: &str) -> &str {
    loop {
        text = text.trim();
        let Some(inner) = text.strip_prefix('(').and_then(|t| t.strip_suffix(')')) else { return text };
        let (mut depth, mut quoted) = (0i32, false);
        for c in inner.chars() {
            match c {
                '"' => quoted = !quoted,
                '(' if !quoted => depth += 1,
                ')' if !quoted => {
                    depth -= 1;
                    if depth < 0 { return text; }
                }
                _ => {}
            }
        }
        text = inner;
    }
}

/// Split at the outermost AND / OR; returns (is_any, parts). `None` when both appear.
fn split_top(text: &str) -> Option<(bool, Vec<&str>)> {
    let upper = text.to_ascii_uppercase();
    let bytes = text.as_bytes();
    let (mut depth, mut quoted, mut mode) = (0i32, false, None::<bool>);
    let (mut parts, mut from, mut at) = (Vec::new(), 0, 0);
    while at < bytes.len() {
        match bytes[at] {
            b'"' => quoted = !quoted,
            b'(' if !quoted => depth += 1,
            b')' if !quoted => depth -= 1,
            _ => {}
        }
        if depth == 0 && !quoted {
            let joiner = [(" AND ", false), (" OR ", true)].into_iter().find(|(w, _)| upper.as_bytes()[at..].starts_with(w.as_bytes()));
            if let Some((word, any)) = joiner {
                if mode.is_some_and(|m| m != any) { return None; }
                mode = Some(any);
                parts.push(&text[from..at]);
                at += word.len();
                from = at;
                continue;
            }
        }
        at += 1;
    }
    parts.push(&text[from..]);
    Some((mode.unwrap_or(false), parts))
}

fn parse_condition(part: &str) -> Option<Cond> {
    let (not, part) = match part.get(..4) {
        Some(h) if h.eq_ignore_ascii_case("NOT ") => (true, part[4..].trim()),
        _ => (false, part),
    };
    let (key, op, value) = ["==", "!=", ">=", "<=", ">", "<", "="].iter().find_map(|op| part.split_once(op).map(|(k, v)| (k.trim(), *op, v.trim())))?;
    let prop = Prop::parse(key)?;
    Some(Cond { not, prop, op: Op::parse(op)?, value: norm_value(prop, value) })
}

fn text_of(v: &XRecordValue) -> Option<&str> {
    v.as_string()
}

/// The drawing's layer filters (with their parents), as the host stored them.
/// Empty when the drawing has none — or when the host's document snapshot does not carry
/// the layer table's extension dictionary.
pub fn read_drawing_filters(doc: &codec::Document) -> Vec<FilterDef> {
    let mut out = Vec::new();
    let Some(ext) = doc.extension_dictionary_handle(doc.layers.handle()) else { return out };
    let Some(ObjectType::Dictionary(ext)) = doc.objects.get(&ext) else { return out };
    let Some((_, root)) = ext.entries.iter().find(|(k, _)| k.eq_ignore_ascii_case(DICTIONARY)) else { return out };
    walk(doc, *root, None, 0, &mut out);
    out
}

fn walk(doc: &codec::Document, dict: codec::Handle, parent: Option<&str>, depth: usize, out: &mut Vec<FilterDef>) {
    let Some(ObjectType::Dictionary(d)) = doc.objects.get(&dict) else { return };
    if depth > 16 { return; }
    for (_, h) in &d.entries {
        let Some(ObjectType::XRecord(x)) = doc.objects.get(h) else { continue };
        let mut it = x.entries.iter();
        if !it.next().is_some_and(|k| k.code == 1 && text_of(&k.value) == Some(KIND)) { continue; }
        let (mut name, mut expr) = (None, None);
        for e in it {
            match (e.code, text_of(&e.value)) {
                (300, Some(v)) if name.is_none() => name = Some(v.to_string()),
                (301, Some(v)) if expr.is_none() => expr = Some(v.to_string()),
                _ => {}
            }
        }
        let Some(name) = name else { continue };
        let (any, items) = parse_expression(expr.as_deref().unwrap_or("")).unwrap_or((false, vec![]));
        out.push(FilterDef { name: name.clone(), parent: parent.map(str::to_string), any, items });
        // nested filters: ACLYDICTIONARY in the record's extension dictionary
        if let Some(xd) = x.xdictionary_handle {
            if let Some(ObjectType::Dictionary(xd)) = doc.objects.get(&xd) {
                if let Some((_, nested)) = xd.entries.iter().find(|(k, _)| k.eq_ignore_ascii_case(DICTIONARY)) {
                    walk(doc, *nested, Some(&name), depth + 1, out);
                }
            }
        }
    }
}

// ───────────────────── creating filters via LAYERFILTER ─────────────────────

#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    /// Command lines for the host command line, in order.
    pub commands: Vec<String>,
    pub created: Vec<String>,
    pub warnings: Vec<String>,
}

struct Lowered {
    pattern: String,
    sets: Vec<String>,
}

fn token(s: &str, space: char) -> String {
    s.trim().chars().map(|c| if c.is_whitespace() { space } else { c }).collect()
}

/// AND of conditions → `NEW` pattern + `SET` arguments, or why it can't be done.
fn lower_all(conds: &[&Cond]) -> Result<Lowered, String> {
    let mut pattern: Option<String> = None;
    let mut sets = Vec::new();
    for c in conds {
        if !matches!(c.op, Op::Eq | Op::Ne) {
            return Err(format!("{} {} is not supported by the host command line", c.prop.label(), c.op.sym()));
        }
        let positive = (c.op == Op::Eq) != c.not;
        match c.prop {
            Prop::Name => {
                if !positive { return Err("'Name !=' / NOT Name cannot be created through the host command line".into()); }
                if pattern.is_some() { return Err("only one Name condition per filter is supported".into()); }
                if c.value.is_empty() { return Err("empty Name pattern".into()); }
                pattern = Some(token(&c.value, '?'));
            }
            Prop::Color => {
                if !positive { return Err("'Color !=' / NOT Color cannot be created through the host command line".into()); }
                let n: u16 = c.value.parse().map_err(|_| format!("colour '{}' is not a number", c.value))?;
                if n > 255 { return Err(format!("colour {n} is out of range 0-255")); }
                sets.push(format!("COLOR {n}"));
            }
            Prop::Linetype => {
                if !positive { return Err("'Linetype !=' / NOT Linetype cannot be created through the host command line".into()); }
                if c.value.is_empty() || c.value.contains(char::is_whitespace) { return Err(format!("linetype '{}' has a space or is empty", c.value)); }
                sets.push(format!("LINETYPE {}", c.value));
            }
            Prop::On | Prop::Frozen | Prop::Locked | Prop::Plot => {
                let want = (c.value == "1") == positive;
                sets.push(match (c.prop, want) {
                    (Prop::On, true) => "ON", (Prop::On, false) => "OFF",
                    (Prop::Frozen, true) => "FROZEN", (Prop::Frozen, false) => "THAWED",
                    (Prop::Locked, true) => "LOCKED", (Prop::Locked, false) => "UNLOCKED",
                    (Prop::Plot, true) => "PLOT", _ => "NOPLOT",
                }.to_string());
            }
            Prop::Description | Prop::Xref => return Err(format!("{} conditions cannot be created through the host command line", c.prop.label())),
        }
    }
    Ok(Lowered { pattern: pattern.unwrap_or_else(|| "*".into()), sets })
}

/// Alternatives (OR) as separate AND-lists, plus the filter's own AND-list.
fn lower_def(d: &FilterDef) -> Result<(Lowered, Vec<Lowered>), String> {
    let mut own: Vec<&Cond> = Vec::new();
    let mut alts: Vec<Vec<&Cond>> = Vec::new();
    if d.any {
        for i in &d.items {
            match i {
                Item::Cond(c) => alts.push(vec![c]),
                Item::Group(g) if g.any => g.conds.iter().for_each(|c| alts.push(vec![c])),
                Item::Group(g) => alts.push(g.conds.iter().collect()),
            }
        }
    } else {
        let mut any_groups = 0;
        for i in &d.items {
            match i {
                Item::Cond(c) => own.push(c),
                Item::Group(g) if !g.any => own.extend(g.conds.iter()),
                Item::Group(g) => {
                    any_groups += 1;
                    g.conds.iter().for_each(|c| alts.push(vec![c]));
                }
            }
        }
        if any_groups > 1 { return Err("more than one OR group inside an AND filter is not supported".into()); }
    }
    let alts = alts.iter().map(|a| lower_all(a)).collect::<Result<Vec<_>, _>>()?;
    Ok((lower_all(&own)?, alts))
}

/// Commands that create `defs` in the drawing. `existing_top` = names of filters already in
/// the drawing at the top level: nested filters are built as temporary top-level filters
/// (the host's NEST copies them), so a name clash would overwrite one — those are refused.
pub fn plan_commands(defs: &[FilterDef], existing_top: &[String]) -> Plan {
    let mut plan = Plan::default();
    let names: Vec<String> = defs.iter().map(|d| d.name.to_lowercase()).collect();
    let is_root = |d: &FilterDef| match &d.parent {
        None => true,
        Some(p) => !names.contains(&p.to_lowercase()),
    };
    for d in defs.iter().filter(|d| is_root(d)) {
        if let Some(p) = &d.parent {
            plan.warnings.push(format!("{}: parent '{}' is not in the list, created at the top level", d.name, p));
        }
        build(d, defs, existing_top, &mut plan, 0);
    }
    plan
}

fn build(d: &FilterDef, all: &[FilterDef], existing_top: &[String], plan: &mut Plan, depth: usize) -> bool {
    let name = token(&d.name, '_');
    if name.is_empty() {
        plan.warnings.push("a filter with an empty name was skipped".into());
        return false;
    }
    if depth > 8 {
        plan.warnings.push(format!("{}: nesting too deep", d.name));
        return false;
    }
    let (own, alts) = match lower_def(d) {
        Ok(x) => x,
        Err(e) => {
            plan.warnings.push(format!("{}: not created — {}", d.name, e));
            return false;
        }
    };
    let children: Vec<&FilterDef> = all.iter().filter(|c| c.parent.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(&d.name))).collect();
    if !alts.is_empty() && !children.is_empty() {
        plan.warnings.push(format!("{}: not created — a filter cannot have both OR conditions and nested filters", d.name));
        return false;
    }
    if name != d.name {
        plan.warnings.push(format!("{}: spaces replaced, created as '{}'", d.name, name));
    }
    // Build into a scratch plan so a failed child doesn't leave half a filter behind.
    let mut local = Plan::default();
    let emit = |p: &mut Plan, name: &str, l: &Lowered| {
        p.commands.push(format!("LAYERFILTER NEW {name} {}", l.pattern));
        for s in &l.sets {
            p.commands.push(format!("LAYERFILTER SET {name} {s}"));
        }
    };
    emit(&mut local, &name, &own);
    for (i, alt) in alts.iter().enumerate() {
        let child = format!("{name}__{}", i + 1);
        emit(&mut local, &child, alt);
        local.commands.push(format!("LAYERFILTER NEST {name} {child}"));
        local.commands.push(format!("LAYERFILTER REMOVE {child}"));
    }
    for c in children {
        let cname = token(&c.name, '_');
        if existing_top.iter().any(|e| e.eq_ignore_ascii_case(&cname)) {
            plan.warnings.push(format!("{}: nested filter not created — a filter with that name already exists at the top level", c.name));
            continue;
        }
        let mut sub = Plan::default();
        if build(c, all, existing_top, &mut sub, depth + 1) {
            local.commands.extend(sub.commands);
            local.commands.push(format!("LAYERFILTER NEST {name} {cname}"));
            local.commands.push(format!("LAYERFILTER REMOVE {cname}"));
            local.created.extend(sub.created);
        }
        plan.warnings.extend(sub.warnings);
    }
    plan.commands.extend(local.commands);
    plan.created.push(d.name.clone());
    plan.created.extend(local.created);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expression_roundtrip_with_host_format() {
        let (any, items) = parse_expression("( NAME== \"A*\" AND ( COLOR== \"1\" OR COLOR== \"3\" ) AND NOT FROZEN== \"True\" )").unwrap();
        assert!(!any);
        assert_eq!(items.len(), 3);
        assert!(matches!(&items[1], Item::Group(g) if g.any && g.conds.len() == 2));
        assert!(matches!(&items[2], Item::Cond(c) if c.not && c.prop == Prop::Frozen && c.value == "1"));
        let d = FilterDef { name: "x".into(), parent: None, any, items };
        assert_eq!(d.describe(), "Name == \"A*\" AND (Color == \"1\" OR Color == \"3\") AND NOT Frozen == \"True\"");
    }

    #[test]
    fn xref_off_is_refused_not_inverted() {
        let mut c = Cond::new(Prop::Name, Op::Ne, "*|*");
        c.not = false;
        let p = plan_commands(&[FilterDef::simple("XREF-off", vec![c])], &[]);
        assert!(p.commands.is_empty());
        assert!(p.warnings[0].contains("not created"), "{:?}", p.warnings);
    }

    #[test]
    fn simple_filters_become_commands() {
        let d = FilterDef::simple("Roads", vec![Cond::new(Prop::Name, Op::Eq, "ROAD *"), Cond::new(Prop::Color, Op::Eq, "3"), Cond::new(Prop::Frozen, Op::Eq, "0")]);
        let p = plan_commands(&[d], &[]);
        assert_eq!(p.commands, ["LAYERFILTER NEW Roads ROAD?*", "LAYERFILTER SET Roads COLOR 3", "LAYERFILTER SET Roads THAWED"]);
        assert_eq!(p.created, ["Roads"]);
    }

    #[test]
    fn negated_flag_and_double_negation() {
        let mut c = Cond::new(Prop::Locked, Op::Eq, "1");
        c.not = true; // NOT locked == True  ->  UNLOCKED
        let mut n = Cond::new(Prop::Name, Op::Ne, "X*");
        n.not = true; // NOT (name != X*) -> name == X*
        let p = plan_commands(&[FilterDef::simple("F", vec![c, n])], &[]);
        assert_eq!(p.commands, ["LAYERFILTER NEW F X*", "LAYERFILTER SET F UNLOCKED"]);
    }

    #[test]
    fn or_uses_nested_filters() {
        let d = FilterDef { name: "Util".into(), parent: None, any: true, items: vec![Item::Cond(Cond::new(Prop::Name, Op::Eq, "GAS*")), Item::Cond(Cond::new(Prop::Name, Op::Eq, "WATER*"))] };
        let p = plan_commands(&[d], &[]);
        assert_eq!(p.commands, [
            "LAYERFILTER NEW Util *",
            "LAYERFILTER NEW Util__1 GAS*", "LAYERFILTER NEST Util Util__1", "LAYERFILTER REMOVE Util__1",
            "LAYERFILTER NEW Util__2 WATER*", "LAYERFILTER NEST Util Util__2", "LAYERFILTER REMOVE Util__2",
        ]);
    }

    #[test]
    fn parent_child_hierarchy() {
        let p = FilterDef::simple("Parent", vec![Cond::new(Prop::Name, Op::Eq, "U*")]);
        let mut c = FilterDef::simple("Child", vec![Cond::new(Prop::Color, Op::Eq, "1")]);
        c.parent = Some("parent".into());
        let plan = plan_commands(&[c.clone(), p.clone()], &[]);
        assert_eq!(plan.commands, [
            "LAYERFILTER NEW Parent U*",
            "LAYERFILTER NEW Child *", "LAYERFILTER SET Child COLOR 1", "LAYERFILTER NEST Parent Child", "LAYERFILTER REMOVE Child",
        ]);
        let plan = plan_commands(&[c, p], &["child".into()]);
        assert!(plan.warnings.iter().any(|w| w.contains("already exists")));
    }

    #[test]
    fn unsupported_things_are_reported() {
        let cases = [
            Cond::new(Prop::Description, Op::Eq, "x"),
            Cond::new(Prop::Color, Op::Gt, "3"),
            Cond::new(Prop::Xref, Op::Eq, "1"),
            Cond::new(Prop::Linetype, Op::Eq, "A B"),
        ];
        for c in cases {
            let p = plan_commands(&[FilterDef::simple("F", vec![c])], &[]);
            assert!(p.commands.is_empty() && p.warnings.len() == 1, "{p:?}");
        }
    }

    #[test]
    fn quick_builders() {
        assert_eq!(quick(0, "UTIL_", "").unwrap().name, "UTIL_*");
        assert_eq!(quick(1, "_AG", "").unwrap().items, vec![Item::Cond(Cond::new(Prop::Name, Op::Eq, "*_AG"))]);
        assert_eq!(quick(4, "90", "").unwrap().name, "Color_90");
        assert!(quick(4, "999", "").is_err());
        assert!(quick(0, "", "").is_err());
        assert!(quick(6, "", "").is_ok());
        let p = plan_commands(&[quick(7, "", "").unwrap()], &[]);
        assert_eq!(p.commands, ["LAYERFILTER NEW Frozen_layers *", "LAYERFILTER SET Frozen_layers FROZEN"]);
    }

    #[test]
    fn csv_roundtrip_with_groups_and_parent() {
        let a = FilterDef { name: "A".into(), parent: None, any: false, items: vec![
            Item::Cond(Cond::new(Prop::Name, Op::Eq, "a,b*")),
            Item::Group(Group { any: true, conds: vec![Cond::new(Prop::Color, Op::Eq, "1"), Cond::new(Prop::Color, Op::Eq, "3")] }),
            Item::Cond({ let mut c = Cond::new(Prop::Frozen, Op::Eq, "1"); c.not = true; c }),
        ] };
        let mut b = FilterDef::simple("B", vec![Cond::new(Prop::Linetype, Op::Eq, "HIDDEN")]);
        b.parent = Some("A".into());
        let empty = FilterDef { name: "Empty".into(), parent: None, any: true, items: vec![] };
        let defs = vec![a, b, empty];
        let (back, warn) = from_csv(&to_csv(&defs));
        assert!(warn.is_empty(), "{warn:?}");
        assert_eq!(back, defs);
    }

    #[test]
    fn csv_reports_bad_rows() {
        let (d, w) = from_csv("Filter,Parent,Match,Group,GroupMatch,Not,Property,Op,Value\nA,,all,,,,Bogus,==,1\nshort,row\n");
        assert_eq!(d.len(), 1);
        assert_eq!(w.len(), 2);
    }
}
