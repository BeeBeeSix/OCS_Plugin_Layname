//! Legend creator.
//!
//! * **Layers legend** — one sample polyline per layer (lower-left pick point), the layer
//!   name (and description) to the right. Alphabetical, first layer on top.
//! * **Objects legend** — from selected lines / polylines, blocks and hatches (top-left pick
//!   point): line samples, then blocks, then hatch swatches, each section alphabetical.
//!
//! Every distance is a factor of the *scale* (DIMSCALE, the annotation scale, or a custom
//! value), so `50` means "50 × scale" drawing units. Gathering the items and laying them out
//! are separate steps; `build` is pure and unit-tested.

use std::collections::{BTreeMap, HashMap, HashSet};

use ocs_plugin_api::host::codec;
use ocs_plugin_api::host::codec::entities::{BoundaryEdge, BoundaryPath, EntityCommon, Hatch, Insert, LwPolyline, LwVertex, PolylineEdge, Text};
use ocs_plugin_api::host::codec::objects::ObjectType;
use ocs_plugin_api::host::codec::types::{Vector2, Vector3};
use ocs_plugin_api::host::codec::EntityType;

use crate::wildcard;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScaleSource {
    DimScale,
    Annotation,
    Custom,
}

/// Dialog input, before the scale is resolved.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Request {
    pub by_layers: bool,
    /// Layers legend: wildcard on layer names.
    pub scope: String,
    pub only_used: bool,
    pub include_xref: bool,
    pub scale_source: ScaleSource,
    pub custom_scale: f64,
    pub params: Params,
}

/// All sizes are factors of the scale.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Params {
    /// The resolved scale (1 until resolved).
    pub scale: f64,
    pub line_len: f64,
    pub line_pitch: f64,
    pub block_cell: f64,
    pub hatch_w: f64,
    pub hatch_h: f64,
    pub hatch_pitch: f64,
    pub text_height: f64,
    pub text_gap: f64,
    pub include_desc: bool,
    /// Blank = the drawing's current layer / text style (resolved by the caller).
    pub text_layer: String,
    pub text_style: String,
}

impl Default for Request {
    fn default() -> Self {
        Request {
            by_layers: false,
            scope: "*".into(),
            only_used: false,
            include_xref: false,
            scale_source: ScaleSource::Annotation,
            custom_scale: 1.0,
            params: Params::default(),
        }
    }
}

impl Default for Params {
    fn default() -> Self {
        Params {
            scale: 1.0,
            line_len: 50.0,
            line_pitch: 7.5,
            block_cell: 10.0,
            hatch_w: 25.0,
            hatch_h: 5.0,
            hatch_pitch: 7.5,
            text_height: 2.5,
            text_gap: 2.5,
            include_desc: true,
            text_layer: String::new(),
            text_style: String::new(),
        }
    }
}

impl Params {
    pub fn validate(&self) -> Result<(), String> {
        let fields = [
            ("line length", self.line_len), ("line spacing", self.line_pitch), ("block spacing", self.block_cell),
            ("hatch width", self.hatch_w), ("hatch height", self.hatch_h), ("hatch spacing", self.hatch_pitch),
            ("text height", self.text_height),
        ];
        for (n, v) in fields {
            if !(v.is_finite() && v > 0.0) {
                return Err(format!("{n} must be greater than 0"));
            }
        }
        if !self.text_gap.is_finite() || self.text_gap < 0.0 {
            return Err("text gap cannot be negative".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub enum Kind {
    Line,
    Block { name: String, scale: [f64; 3] },
    Hatch(Box<Hatch>),
}

#[derive(Clone, Debug)]
pub struct Item {
    pub kind: Kind,
    pub label: String,
    /// Layer / colour / linetype / weight of the sample (a fresh common block).
    pub look: EntityCommon,
}

// ───────────────────────── scale ─────────────────────────

/// Scale factor (paper / drawing) for a scale *name*: the drawing's SCALE object of that
/// name, else the name read as `paper:drawing` ("1:100" → 0.01, "2:1" → 2).
pub fn scale_name_factor(doc: &codec::Document, name: &str) -> Option<f64> {
    let name = name.trim();
    if let Some(f) = doc.objects.values().find_map(|o| match o {
        ObjectType::Scale(s) if s.name.trim().eq_ignore_ascii_case(name) && s.drawing_units.abs() > 1e-12 => Some(s.factor()),
        _ => None,
    }) {
        return Some(f);
    }
    let (a, b) = name.split_once(':')?;
    let (a, b): (f64, f64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    (a > 0.0 && b > 0.0).then(|| a / b)
}

/// The current annotation scale as (name, drawing units per paper unit): 1:100 → 100.
/// `host_name` / `host_value` are the live CANNOSCALE / CANNOSCALEVALUE when the host
/// offers them (they follow the status-bar selection); the saved header is the fallback.
pub fn annotation_info(doc: &codec::Document, host_name: Option<&str>, host_value: Option<f64>) -> Option<(String, f64)> {
    let header = &doc.header;
    let name = host_name.map(str::trim).filter(|n| !n.is_empty()).unwrap_or(header.current_annotation_scale.trim()).to_string();
    let same_as_header = name.eq_ignore_ascii_case(header.current_annotation_scale.trim());
    let factor = scale_name_factor(doc, &name)
        .or(host_value.filter(|v| *v > 1e-9))
        .or(same_as_header.then_some(header.annotation_scale_value));
    factor.filter(|f| *f > 1e-9).map(|f| (name, 1.0 / f))
}

/// Drawing units per paper unit of the drawing's saved annotation scale (1:100 → 100).
pub fn annotation_multiplier(doc: &codec::Document) -> Option<f64> {
    annotation_info(doc, None, None).map(|(_, m)| m)
}

// ───────────────────────── saved settings (JSON) ─────────────────────────

/// Folder for the plugin's settings: %APPDATA%\OCS_Layname, or ~/.config/ocs_layname.
pub fn settings_dir() -> std::path::PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("OCS_Layname")
}

pub fn default_settings_path() -> std::path::PathBuf {
    settings_dir().join("legend.json")
}

/// The settings file last saved or loaded, else the default one.
pub fn current_settings_path() -> String {
    std::fs::read_to_string(settings_dir().join("last_legend.txt"))
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| default_settings_path().to_string_lossy().into_owned())
}

pub fn remember_settings_path(path: &str) {
    let d = settings_dir();
    if std::fs::create_dir_all(&d).is_ok() {
        let _ = std::fs::write(d.join("last_legend.txt"), path);
    }
}

pub fn to_json(req: &Request) -> String {
    let mut r = req.clone();
    r.params.scale = 1.0; // resolved at run time, never stored
    serde_json::to_string_pretty(&r).unwrap_or_default() + "\n"
}

pub fn from_json(text: &str) -> Result<Request, String> {
    let r: Request = serde_json::from_str(text).map_err(|e| format!("not a valid legend settings file: {e}"))?;
    r.params.validate()?;
    if r.scale_source == ScaleSource::Custom && !(r.custom_scale > 0.0) {
        return Err("the custom scale must be greater than 0".into());
    }
    Ok(r)
}

// ───────────────────────── gathering ─────────────────────────

fn look_from(src: &EntityCommon) -> EntityCommon {
    let mut c = EntityCommon::new();
    c.layer = src.layer.clone();
    c.color = src.color;
    c.line_weight = src.line_weight;
    c.linetype = src.linetype.clone();
    c.linetype_scale = src.linetype_scale;
    c.transparency = src.transparency;
    c
}

fn label_for(layer: &str, descriptions: &HashMap<String, String>, include_desc: bool) -> String {
    match descriptions.get(&layer.to_lowercase()).map(|d| d.trim()).filter(|d| include_desc && !d.is_empty()) {
        Some(d) => format!("{layer} - {d}"),
        None => layer.to_string(),
    }
}

/// layer name (lowercase) → description.
pub fn layer_descriptions(doc: &codec::Document) -> HashMap<String, String> {
    doc.layers.iter().map(|l| (l.name.to_lowercase(), l.description.clone())).collect()
}

/// One sample line per layer, alphabetical. `0`, `Defpoints` are never listed;
/// xref layers (`name|layer`) only when asked for.
pub fn items_from_layers(doc: &codec::Document, req: &Request) -> Vec<Item> {
    let used: HashSet<String> = if req.only_used {
        doc.entities().map(|e| e.common().layer.to_lowercase()).collect()
    } else {
        HashSet::new()
    };
    let descriptions = layer_descriptions(doc);
    let mut layers: Vec<&_> = doc
        .layers
        .iter()
        .filter(|l| {
            l.name != "0"
                && !l.name.eq_ignore_ascii_case("defpoints")
                && (req.include_xref || !l.name.contains('|'))
                && (req.scope.trim().is_empty() || wildcard::matches(req.scope.trim(), &l.name))
                && (!req.only_used || used.contains(&l.name.to_lowercase()))
        })
        .collect();
    layers.sort_by_key(|l| l.name.to_lowercase());
    layers
        .into_iter()
        .map(|l| {
            let mut look = EntityCommon::new();
            look.layer = l.name.clone(); // colour, linetype, weight: ByLayer
            Item { kind: Kind::Line, label: label_for(&l.name, &descriptions, req.params.include_desc), look }
        })
        .collect()
}

/// Items for the selected objects: line samples, then blocks, then hatches.
pub fn items_from_selection(doc: &codec::Document, handles: &[codec::Handle], include_desc: bool) -> Vec<Item> {
    let entities: Vec<&EntityType> = handles.iter().filter_map(|h| doc.get_entity(*h)).collect();
    items_from_entities(entities.into_iter(), &layer_descriptions(doc), include_desc)
}

pub fn items_from_entities<'a>(
    entities: impl Iterator<Item = &'a EntityType>,
    descriptions: &HashMap<String, String>,
    include_desc: bool,
) -> Vec<Item> {
    let mut lines: BTreeMap<(String, String), Item> = BTreeMap::new();
    let mut blocks: BTreeMap<String, Item> = BTreeMap::new();
    let mut hatches: BTreeMap<(String, String), Item> = BTreeMap::new();
    for e in entities {
        match e {
            EntityType::Line(_) | EntityType::LwPolyline(_) | EntityType::Polyline(_) | EntityType::Polyline2D(_) | EntityType::Polyline3D(_) => {
                let c = e.common();
                let key = (
                    c.layer.to_lowercase(),
                    format!("{:?}|{}|{:?}", c.color, c.linetype.to_lowercase(), c.line_weight),
                );
                lines.entry(key).or_insert_with(|| Item { kind: Kind::Line, label: label_for(&c.layer, descriptions, include_desc), look: look_from(c) });
            }
            EntityType::Insert(i) if !i.block_name.starts_with('*') => {
                blocks.entry(i.block_name.to_lowercase()).or_insert_with(|| Item {
                    kind: Kind::Block { name: i.block_name.clone(), scale: [i.x_scale(), i.y_scale(), i.z_scale()] },
                    label: i.block_name.clone(),
                    look: look_from(&i.common),
                });
            }
            EntityType::Hatch(h) => {
                let key = (
                    h.common.layer.to_lowercase(),
                    format!("{}|{}|{:.6}|{:.4}|{:?}", h.pattern.name.to_lowercase(), h.is_solid, h.pattern_scale, h.pattern_angle, h.common.color),
                );
                hatches.entry(key).or_insert_with(|| {
                    let mut t = (**h).clone();
                    t.common = look_from(&h.common);
                    t.paths.clear();
                    t.seed_points.clear();
                    t.is_associative = false;
                    let label = label_for(&h.common.layer, descriptions, include_desc);
                    Item { kind: Kind::Hatch(Box::new(t)), label, look: look_from(&h.common) }
                });
            }
            _ => {}
        }
    }
    // Two different hatches on one layer would read the same: say which pattern.
    let mut seen: HashMap<String, usize> = HashMap::new();
    hatches.values().for_each(|i| *seen.entry(i.label.clone()).or_default() += 1);
    for i in hatches.values_mut() {
        if seen.get(&i.label).copied().unwrap_or(0) > 1 {
            if let Kind::Hatch(h) = &i.kind {
                i.label = format!("{} [{}]", i.label, if h.is_solid { "SOLID" } else { h.pattern.name.as_str() });
            }
        }
    }
    let mut out: Vec<Item> = lines.into_values().collect();
    out.sort_by_key(|i| i.label.to_lowercase()); // layer-name order across colour/linetype variants
    let mut b: Vec<Item> = blocks.into_values().collect();
    b.sort_by_key(|i| i.label.to_lowercase());
    let mut h: Vec<Item> = hatches.into_values().collect();
    h.sort_by_key(|i| i.label.to_lowercase());
    out.extend(b);
    out.extend(h);
    out
}

// ───────────────────────── layout ─────────────────────────

/// Layer of the rectangle drawn around each hatch swatch.
pub const FRAME_LAYER: &str = "LEGEND";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// Layers legend: the pick point is the lower-left; the first item is on top.
    LowerLeft,
    /// Objects legend: the pick point is the top-left; items run downward.
    TopLeft,
}

fn text(p: &Params, label: &str, x: f64, y: f64, z: f64) -> EntityType {
    let mut t = Text::with_value(label, Vector3::new(x, y - 0.35 * p.text_height * p.scale, z)).with_height(p.text_height * p.scale);
    if !p.text_layer.is_empty() {
        t.common.layer = p.text_layer.clone();
    }
    if !p.text_style.is_empty() {
        t.style = p.text_style.clone();
    }
    EntityType::Text(t)
}

/// Entities for the legend, anchored at `origin`.
pub fn build(items: &[Item], p: &Params, layout: Layout, origin: [f64; 3]) -> Vec<EntityType> {
    let s = p.scale;
    let [x0, y0, z] = origin;
    let pitch = |k: &Kind| match k {
        Kind::Line => p.line_pitch * s,
        Kind::Block { .. } => p.block_cell * s,
        Kind::Hatch(_) => p.hatch_pitch * s,
    };
    let mut out = Vec::new();
    let mut cursor = y0;
    for (i, item) in items.iter().enumerate() {
        let cell = pitch(&item.kind);
        let y = match layout {
            Layout::LowerLeft => y0 + (items.len() - 1 - i) as f64 * cell,
            Layout::TopLeft => {
                let yc = cursor - cell / 2.0;
                cursor -= cell;
                yc
            }
        };
        // All text starts at the same x: just past the end of a sample line.
        let text_x = x0 + p.line_len * s + p.text_gap * s;
        // Blocks and hatch swatches are centred on the middle of a sample line (25 in at the defaults).
        let cx = x0 + p.line_len * s / 2.0;
        match &item.kind {
            Kind::Line => {
                let mut pl = LwPolyline::new();
                pl.add_vertex(LwVertex::new(Vector2::new(x0, y)));
                pl.add_vertex(LwVertex::new(Vector2::new(x0 + p.line_len * s, y)));
                pl.elevation = z;
                pl.common = item.look.clone();
                out.push(EntityType::LwPolyline(pl));
            }
            Kind::Block { name, scale } => {
                let mut ins = Insert::new(name.clone(), Vector3::new(cx, y, z)).with_scale(scale[0], scale[1], scale[2]);
                ins.common = item.look.clone();
                out.push(EntityType::Insert(ins));
            }
            Kind::Hatch(template) => {
                let (w, h) = (p.hatch_w * s, p.hatch_h * s);
                let (xa, xb, ya, yb) = (cx - w / 2.0, cx + w / 2.0, y - h / 2.0, y + h / 2.0);
                let mut hatch = (**template).clone();
                let mut path = BoundaryPath::external();
                path.add_edge(BoundaryEdge::Polyline(PolylineEdge::new(
                    vec![Vector2::new(xa, ya), Vector2::new(xb, ya), Vector2::new(xb, yb), Vector2::new(xa, yb)],
                    true,
                )));
                hatch.paths = vec![path];
                hatch.elevation = z;
                out.push(EntityType::Hatch(Box::new(hatch)));
                // Frame around the swatch, on layer LEGEND.
                let mut r = LwPolyline::new();
                for (x, yy) in [(xa, ya), (xb, ya), (xb, yb), (xa, yb)] {
                    r.add_vertex(LwVertex::new(Vector2::new(x, yy)));
                }
                r.is_closed = true;
                r.elevation = z;
                r.common.layer = FRAME_LAYER.to_string();
                out.push(EntityType::LwPolyline(r));
            }
        }
        out.push(text(p, &item.label, text_x, y, z));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_item(layer: &str, label: &str) -> Item {
        let mut look = EntityCommon::new();
        look.layer = layer.into();
        Item { kind: Kind::Line, label: label.into(), look }
    }

    fn params(scale: f64) -> Params {
        Params { scale, ..Params::default() }
    }

    fn pl(e: &EntityType) -> (f64, f64, f64) {
        match e {
            EntityType::LwPolyline(p) => (p.vertices[0].location.x, p.vertices[1].location.x, p.vertices[0].location.y),
            _ => panic!("not a polyline"),
        }
    }

    #[test]
    fn layers_legend_sizes_and_order() {
        let items = vec![line_item("A", "A - first"), line_item("B", "B"), line_item("C", "C")];
        let es = build(&items, &params(100.0), Layout::LowerLeft, [1000.0, 2000.0, 0.0]);
        assert_eq!(es.len(), 6); // polyline + text each
        // A is on top; C (last) sits on the pick point
        let ys: Vec<f64> = [0, 2, 4].iter().map(|&i| pl(&es[i]).2).collect();
        assert_eq!(ys, [2000.0 + 2.0 * 750.0, 2000.0 + 750.0, 2000.0]);
        let (x0, x1, _) = pl(&es[0]);
        assert_eq!((x0, x1), (1000.0, 1000.0 + 5000.0)); // 50 × scale 100
        let EntityType::Text(t) = &es[1] else { panic!() };
        assert_eq!(t.value, "A - first");
        assert_eq!(t.height, 250.0); // 2.5 × 100
        assert_eq!(t.insertion_point.x, 1000.0 + 5000.0 + 250.0);
        match &es[0] { EntityType::LwPolyline(p) => assert_eq!(p.common.layer, "A"), _ => panic!() }
    }

    #[test]
    fn objects_legend_runs_down_with_section_pitches() {
        let mut tpl = Hatch::new();
        tpl.paths.clear();
        let items = vec![
            line_item("L1", "L1"),
            Item { kind: Kind::Block { name: "TREE".into(), scale: [2.0, 2.0, 2.0] }, label: "TREE".into(), look: EntityCommon::new() },
            Item { kind: Kind::Hatch(Box::new(tpl)), label: "H1".into(), look: EntityCommon::new() },
        ];
        let es = build(&items, &params(10.0), Layout::TopLeft, [0.0, 1000.0, 0.0]);
        // line: pitch 75 -> centre 1000-37.5
        assert_eq!(pl(&es[0]).2, 962.5);
        // block: cell 100, starts at 925 -> centre 875, x centred in the 100-wide cell
        let EntityType::Insert(i) = &es[2] else { panic!() };
        assert_eq!((i.insert_point.x, i.insert_point.y), (250.0, 875.0)); // 25 × scale from the left
        assert_eq!((i.x_scale(), i.y_scale()), (2.0, 2.0));
        let EntityType::Text(t) = &es[3] else { panic!() };
        assert_eq!(t.insertion_point.x, 500.0 + 25.0); // same x as every other text
        // hatch: cell 75 starts at 825 -> centre 787.5; swatch 250 x 50
        let EntityType::Hatch(h) = &es[4] else { panic!() };
        let BoundaryEdge::Polyline(e) = &h.paths[0].edges[0] else { panic!() };
        let (minx, maxx) = (e.vertices.iter().map(|v| v.x).fold(f64::MAX, f64::min), e.vertices.iter().map(|v| v.x).fold(f64::MIN, f64::max));
        let (miny, maxy) = (e.vertices.iter().map(|v| v.y).fold(f64::MAX, f64::min), e.vertices.iter().map(|v| v.y).fold(f64::MIN, f64::max));
        assert_eq!((maxx - minx, maxy - miny), (250.0, 50.0));
        assert_eq!((miny + maxy) / 2.0, 787.5);
        assert_eq!((minx + maxx) / 2.0, 250.0);
        let EntityType::LwPolyline(r) = &es[5] else { panic!() };
        assert!(r.is_closed && r.common.layer == "LEGEND");
        let EntityType::Text(t) = &es[6] else { panic!() };
        assert_eq!(t.insertion_point.x, 525.0);
    }

    #[test]
    fn text_layer_and_style_are_applied() {
        let mut p = params(1.0);
        p.text_layer = "LEGEND".into();
        p.text_style = "Standard".into();
        let es = build(&[line_item("A", "A")], &p, Layout::LowerLeft, [0.0; 3]);
        let EntityType::Text(t) = &es[1] else { panic!() };
        assert_eq!((t.common.layer.as_str(), t.style.as_str()), ("LEGEND", "Standard"));
    }

    #[test]
    fn selection_groups_sorts_and_dedups() {
        let mk_line = |layer: &str| {
            let mut l = codec::entities::Line::new();
            l.common.layer = layer.into();
            EntityType::Line(l)
        };
        let mk_ins = |name: &str, sx: f64| {
            let mut i = Insert::new(name, Vector3::ZERO);
            i.set_scale_unchecked(sx, sx, sx);
            EntityType::Insert(i)
        };
        let mk_hatch = |layer: &str, pat: &str| {
            let mut h = Hatch::new();
            h.common.layer = layer.into();
            h.pattern.name = pat.into();
            h.is_solid = pat == "SOLID";
            EntityType::Hatch(Box::new(h))
        };
        let es = vec![
            mk_line("ZED"), mk_line("alpha"), mk_line("ZED"), // dup layer
            mk_ins("tree", 3.0), mk_ins("Bench", 1.0), mk_ins("TREE", 9.0), mk_ins("*U12", 1.0), // dup + anonymous
            mk_hatch("GRASS", "ANSI31"), mk_hatch("GRASS", "ANSI37"), mk_hatch("ROCK", "SOLID"),
        ];
        let desc = HashMap::from([("zed".to_string(), "the end".to_string())]);
        let items = items_from_entities(es.iter(), &desc, true);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["alpha", "ZED - the end", "Bench", "tree", "GRASS [ANSI31]", "GRASS [ANSI37]", "ROCK"]);
        // first instance of a repeated block decides the symbol scale
        assert!(matches!(&items[3].kind, Kind::Block { scale, .. } if scale[0] == 3.0));
        let none = items_from_entities(es.iter(), &desc, false);
        assert!(none.iter().any(|i| i.label == "ZED"));
    }

    #[test]
    fn settings_round_trip_and_partial_files() {
        let mut r = Request::default();
        r.params.line_len = 40.0;
        r.scale_source = ScaleSource::Custom;
        r.custom_scale = 250.0;
        r.params.text_layer = "LEGEND-TXT".into();
        let back = from_json(&to_json(&r)).unwrap();
        assert_eq!(back, r);
        // a hand-written file may name only some fields
        let p = from_json(r#"{"by_layers": true, "params": {"text_height": 3.5}}"#).unwrap();
        assert!(p.by_layers);
        assert_eq!((p.params.text_height, p.params.line_len), (3.5, 50.0));
        assert_eq!(p.scale_source, ScaleSource::Annotation);
        assert!(from_json("{nope").is_err());
        assert!(from_json(r#"{"params": {"line_len": 0}}"#).is_err());
    }

    #[test]
    fn annotation_scale_from_name_and_values() {
        let doc = codec::Document::new();
        // a live name the drawing has no SCALE object for is read as paper:drawing
        assert_eq!(annotation_info(&doc, Some("1:100"), None).map(|(n, m)| (n, m.round())), Some(("1:100".into(), 100.0)));
        assert_eq!(annotation_info(&doc, Some("2:1"), None).map(|(_, m)| m), Some(0.5));
        // unparseable name: the live value decides
        assert_eq!(annotation_info(&doc, Some("1/8\" = 1'-0\""), Some(1.0 / 96.0)).map(|(_, m)| m.round()), Some(96.0));
        // nothing from the host: the header (default 1:1)
        assert_eq!(annotation_info(&doc, None, None).map(|(_, m)| m), Some(1.0));
    }

    #[test]
    fn params_are_validated() {
        assert!(Params::default().validate().is_ok());
        assert!(Params { line_len: 0.0, ..Params::default() }.validate().is_err());
        assert!(Params { text_gap: -1.0, ..Params::default() }.validate().is_err());
    }
}
