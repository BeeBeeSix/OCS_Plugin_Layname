//! The "Layer Names" window: one modal dialog with three tabs (Import, Rename, Filter).
//!
//! The host's declarative dialog has no tab control, so the tab bar is three buttons;
//! a click stores the current field values and answers `ReplaceDialog` with the other
//! tab's layout. Nothing typed is lost when you switch tabs.

use std::collections::HashMap;

use ocs_plugin_api::dialog::{
    DialogActionResponse, DialogControl, DialogDef, DialogEvent, DialogResponse, DialogRow, DialogValue, TextStyle,
};
use ocs_plugin_api::host::{CommandStep, InteractiveCommand};

use crate::commands::{set_job, Job};
use crate::legend::{Params, Request, ScaleSource};
use crate::filters::{self, FilterDef};
use crate::host::LayerInfo;
use crate::lmd::{self, Existing, LmdFile};
use crate::rename::{self, RenameOp, Status};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Import,
    Rename,
    Filter,
    Legend,
}

const LEGEND_TYPES: [&str; 2] = [
    "Layer legend (one line per layer; pick the LOWER-LEFT point)",
    "Object legend (select lines/polylines, blocks, hatches; pick the TOP-LEFT point)",
];
const LEGEND_ACTIONS: [&str; 3] = [
    "Create the legend",
    "Save the settings to the JSON file, then create the legend",
    "Save the settings to the JSON file only",
];
const SCALE_SOURCES: [&str; 3] = ["DIMSCALE (drawing setting)", "Annotation scale (current)", "Custom value"];
const IMPORT_MODES: [&str; 2] = ["Import layers from the LMD file", "Export the drawing's layers to an LMD file"];
const EXISTING_MODES: [&str; 2] = ["Skip layers that already exist", "Update layers that already exist (colour, linetype, description)"];
const RENAME_OPS: [&str; 5] = ["Add prefix", "Replace prefix", "Add suffix", "Replace suffix", "Replace string"];
const FILTER_MODES: [&str; 3] = [
    "Create the filters in the list below",
    "Export the drawing's filters to a CSV file",
    "Import filters from a CSV file",
];
const PREVIEW_MAX: usize = 300;

pub struct LaynameDialog {
    tab: Tab,
    layers: Vec<LayerInfo>,
    filters: Vec<FilterDef>,
    /// Every field's last known value, across tabs.
    vals: HashMap<String, DialogValue>,
    /// The LMD file read in the Import tab: (path, contents).
    lmd: Option<(String, LmdFile)>,
    lmd_note: String,
    legend_note: String,
    done: bool,
}

// ── value helpers ──────────────────────────────────────────────────────────────

fn text_of(m: &HashMap<String, DialogValue>, id: &str, default: &str) -> String {
    match m.get(id) {
        Some(DialogValue::Text(s)) | Some(DialogValue::FilePath(s)) => s.clone(),
        _ => default.to_string(),
    }
}

fn choice_of(m: &HashMap<String, DialogValue>, id: &str) -> usize {
    match m.get(id) {
        Some(DialogValue::Choice { index, .. }) => *index,
        Some(DialogValue::Number(v)) => *v as usize,
        _ => 0,
    }
}

fn choice_of_d(m: &HashMap<String, DialogValue>, id: &str, default: usize) -> usize {
    if m.contains_key(id) { choice_of(m, id) } else { default }
}

fn bool_of(m: &HashMap<String, DialogValue>, id: &str, default: bool) -> bool {
    match m.get(id) {
        Some(DialogValue::Bool(b)) => *b,
        _ => default,
    }
}

fn num_of(m: &HashMap<String, DialogValue>, id: &str, default: f64) -> f64 {
    match m.get(id) {
        Some(DialogValue::Number(v)) => *v,
        Some(DialogValue::Text(t)) => t.trim().parse().unwrap_or(default),
        _ => default,
    }
}

fn number(id: &str, label: &str, v: f64) -> DialogControl {
    DialogControl::Number { id: id.into(), label: label.into(), default: v, min: Some(0.0), max: None, step: Some(0.5) }
}

fn path_of(m: &HashMap<String, DialogValue>, id: &str) -> Option<String> {
    let s = text_of(m, id, "");
    let s = s.trim().trim_matches('"').to_string();
    (!s.is_empty()).then_some(s)
}

fn table_of(m: &HashMap<String, DialogValue>, id: &str) -> Vec<Vec<String>> {
    match m.get(id) {
        Some(DialogValue::Table(rows)) => rows.clone(),
        _ => Vec::new(),
    }
}

fn choice(id: &str, label: &str, options: &[&str], index: usize) -> DialogControl {
    DialogControl::Choice {
        id: id.into(),
        label: label.into(),
        options: options.iter().map(|s| s.to_string()).collect(),
        default_index: index.min(options.len().saturating_sub(1)),
    }
}

fn text(id: &str, label: &str, default: String) -> DialogControl {
    DialogControl::Text { id: id.into(), label: label.into(), default }
}

fn info(t: &str) -> DialogControl {
    DialogControl::InfoText { text: t.into(), align_h: None }
}

fn heading(t: &str) -> DialogControl {
    DialogControl::StyledText { text: t.into(), style: TextStyle::Strong, align_h: None }
}

fn button(id: &str, label: &str, width: f32) -> DialogControl {
    DialogControl::Button { id: id.into(), label: label.into(), action: None, align_h: None, width: Some(width) }
}

fn picker(id: &str, label: &str, path: String, save: bool, name: &str, ext: &[&str]) -> DialogControl {
    DialogControl::FilePicker {
        id: id.into(),
        label: label.into(),
        default_path: path,
        filter_name: name.into(),
        extensions: ext.iter().map(|e| e.to_string()).collect(),
        save,
    }
}

impl LaynameDialog {
    pub fn new(tab: Tab, layers: Vec<LayerInfo>, filters: Vec<FilterDef>) -> Self {
        LaynameDialog { tab, layers, filters, vals: HashMap::new(), lmd: None, lmd_note: String::new(), legend_note: String::new(), done: false }
    }

    // ── building the three tabs ──

    fn tab_bar(&self) -> DialogRow {
        let b = |tab: Tab, id: &str, label: &str| {
            let l = if self.tab == tab { format!("● {label}") } else { label.to_string() };
            button(id, &l, 140.0)
        };
        DialogRow::new(vec![
            b(Tab::Import, "tab_import", "IMPORT / EXPORT"),
            b(Tab::Rename, "tab_rename", "RENAME"),
            b(Tab::Filter, "tab_filter", "FILTER"),
            b(Tab::Legend, "tab_legend", "LEGEND"),
        ])
    }

    pub fn build(&self) -> DialogDef {
        let mut d = DialogDef::new("Layer Names").width(680.0).cancel_label("Close");
        d = d.with_row(self.tab_bar());
        let (submit, controls) = match self.tab {
            Tab::Import => ("Run", self.import_controls()),
            Tab::Rename => ("Rename layers", self.rename_controls()),
            Tab::Filter => ("Run", self.filter_controls()),
            Tab::Legend => ("Create legend", self.legend_controls()),
        };
        for c in controls {
            d = d.with_control(c);
        }
        d.submit_label(submit)
    }

    fn import_controls(&self) -> Vec<DialogControl> {
        let v = &self.vals;
        let mut c = vec![
            heading("LMD layer maps"),
            choice("imp_mode", "What to do", &IMPORT_MODES, choice_of(v, "imp_mode")),
            heading("Import"),
            picker("imp_open", "LMD file to read", text_of(v, "imp_open", ""), false, "Layer map (LMD)", &["lmd", "csv", "txt"]),
        ];
        match &self.lmd {
            Some((_, f)) => {
                c.push(info(&format!("{} group(s) found. Tick the groups to import:", f.groups.len())));
                for (i, g) in f.groups.iter().enumerate() {
                    let id = format!("grp_{i}");
                    let name = if g.name.is_empty() { "(no group)" } else { g.name.as_str() };
                    c.push(DialogControl::Checkbox {
                        label: format!("{name}  ({} layers)", g.layers.len()),
                        default: bool_of(v, &id, true),
                        id,
                    });
                }
                c.push(DialogControl::row(vec![button("grp_all", "Tick all", 110.0), button("grp_none", "Tick none", 110.0)]));
            }
            None => c.push(info("Pick a file: its groups ([GroupName] lines) are listed here.")),
        }
        if !self.lmd_note.is_empty() {
            c.push(info(&self.lmd_note));
        }
        c.push(choice("imp_existing", "Layers already in the drawing", &EXISTING_MODES, choice_of(v, "imp_existing")));
        c.push(DialogControl::Checkbox {
            id: "imp_nospace".into(),
            label: "Replace spaces in layer names with _ (so every layer can join its group)".into(),
            default: bool_of(v, "imp_nospace", true),
        });
        c.push(DialogControl::Checkbox {
            id: "imp_mkgroups".into(),
            label: "Also create a layer group for each ticked LMD group".into(),
            default: bool_of(v, "imp_mkgroups", true),
        });
        c.push(text("imp_gfmt", "Group name ({name} = the LMD group name)", text_of(v, "imp_gfmt", "[{name}]")));
        c.push(info(
            "A layer whose linetype is not available in the drawing is still created, with Continuous, and listed in the \
             command line. A name repeated in several ticked groups is created once (the first one wins).",
        ));
        c.push(heading("Export"));
        c.push(picker("imp_save", "LMD file to write", text_of(v, "imp_save", ""), true, "Layer map (LMD)", &["lmd"]));
        c.push(text("imp_group", "Group name", text_of(v, "imp_group", "OCS-LAYERS")));
        c.push(text("imp_scope", "Only layers named (wildcards * ?)", text_of(v, "imp_scope", "*")));
        c.push(DialogControl::Checkbox {
            id: "imp_append".into(),
            label: "Keep the other groups already in the file (replace only this group)".into(),
            default: bool_of(v, "imp_append", true),
        });
        c
    }

    fn legend_controls(&self) -> Vec<DialogControl> {
        let v = &self.vals;
        let d = Params::default();
        let n = |id: &str, label: &str, def: f64| number(id, label, num_of(v, id, def));
        vec![
            heading("Legend creator"),
            choice("leg_type", "Legend type", &LEGEND_TYPES, choice_of_d(v, "leg_type", 1)),
            heading("Saved settings (run later with LN_LEGENDQ, no window)"),
            choice("leg_action", "When you press the button", &LEGEND_ACTIONS, choice_of(v, "leg_action")),
            picker("leg_json", "Settings file (JSON)", text_of(v, "leg_json", &crate::legend::current_settings_path()), true, "Legend settings (JSON)", &["json"]),
            button("leg_load", "Load settings from that file", 220.0),
            heading("Scale"),
            choice("leg_scale", "Size everything by", &SCALE_SOURCES, choice_of_d(v, "leg_scale", 1)),
            n("leg_custom", "Custom scale value", 1.0),
            info("Every size below is multiplied by the scale (e.g. 50 with scale 100 = 5000 drawing units)."),
            heading("Sizes (× scale)"),
            n("leg_len", "Line length", d.line_len),
            n("leg_pitch", "Line spacing", d.line_pitch),
            n("leg_block", "Block cell size / spacing", d.block_cell),
            n("leg_hw", "Hatch swatch width", d.hatch_w),
            n("leg_hh", "Hatch swatch height", d.hatch_h),
            n("leg_hpitch", "Hatch spacing", d.hatch_pitch),
            heading("Text"),
            n("leg_th", "Text height (× scale)", d.text_height),
            n("leg_gap", "Gap between sample and text (× scale)", d.text_gap),
            text("leg_tlayer", "Text layer (blank = current layer)", text_of(v, "leg_tlayer", "")),
            text("leg_tstyle", "Text style (blank = current style)", text_of(v, "leg_tstyle", "")),
            DialogControl::Checkbox { id: "leg_desc".into(), label: "Add the layer description: \"Name - Description\"".into(), default: bool_of(v, "leg_desc", true) },
            heading("Layer legend only"),
            text("leg_scope", "Only layers named (wildcards * ?)", text_of(v, "leg_scope", "*")),
            DialogControl::Checkbox { id: "leg_used".into(), label: "Only layers that have objects in the drawing".into(), default: bool_of(v, "leg_used", false) },
            DialogControl::Checkbox { id: "leg_xref".into(), label: "Include xref layers (name|layer)".into(), default: bool_of(v, "leg_xref", false) },
            info(&self.legend_note),
            info("Object legend (the default; annotation scale is the default scale): select the objects first (or after OK), then pick the top-left point. Lines/polylines are listed by layer, then blocks, then hatches; each section alphabetical."),
        ]
    }

    /// Put a loaded request's values into the fields.
    fn apply_request(&mut self, r: &Request) {
        let ch = |i: usize| DialogValue::Choice { index: i, text: String::new() };
        let p = &r.params;
        let set: Vec<(&str, DialogValue)> = vec![
            ("leg_type", ch(if r.by_layers { 0 } else { 1 })),
            ("leg_scale", ch(match r.scale_source { ScaleSource::DimScale => 0, ScaleSource::Annotation => 1, ScaleSource::Custom => 2 })),
            ("leg_custom", DialogValue::Number(r.custom_scale)),
            ("leg_len", DialogValue::Number(p.line_len)),
            ("leg_pitch", DialogValue::Number(p.line_pitch)),
            ("leg_block", DialogValue::Number(p.block_cell)),
            ("leg_hw", DialogValue::Number(p.hatch_w)),
            ("leg_hh", DialogValue::Number(p.hatch_h)),
            ("leg_hpitch", DialogValue::Number(p.hatch_pitch)),
            ("leg_th", DialogValue::Number(p.text_height)),
            ("leg_gap", DialogValue::Number(p.text_gap)),
            ("leg_tlayer", DialogValue::Text(p.text_layer.clone())),
            ("leg_tstyle", DialogValue::Text(p.text_style.clone())),
            ("leg_desc", DialogValue::Bool(p.include_desc)),
            ("leg_scope", DialogValue::Text(r.scope.clone())),
            ("leg_used", DialogValue::Bool(r.only_used)),
            ("leg_xref", DialogValue::Bool(r.include_xref)),
        ];
        for (k, v) in set {
            self.vals.insert(k.to_string(), v);
        }
        self.legend_note = "Settings loaded.".into();
    }

    fn legend_request(&self) -> Request {
        let v = &self.vals;
        let d = Params::default();
        let g = |id: &str, def: f64| num_of(v, id, def);
        Request {
            by_layers: choice_of_d(v, "leg_type", 1) == 0,
            scope: text_of(v, "leg_scope", "*"),
            only_used: bool_of(v, "leg_used", false),
            include_xref: bool_of(v, "leg_xref", false),
            scale_source: match choice_of_d(v, "leg_scale", 1) {
                1 => ScaleSource::Annotation,
                2 => ScaleSource::Custom,
                _ => ScaleSource::DimScale,
            },
            custom_scale: g("leg_custom", 1.0),
            params: Params {
                scale: 1.0,
                line_len: g("leg_len", d.line_len),
                line_pitch: g("leg_pitch", d.line_pitch),
                block_cell: g("leg_block", d.block_cell),
                hatch_w: g("leg_hw", d.hatch_w),
                hatch_h: g("leg_hh", d.hatch_h),
                hatch_pitch: g("leg_hpitch", d.hatch_pitch),
                text_height: g("leg_th", d.text_height),
                text_gap: g("leg_gap", d.text_gap),
                include_desc: bool_of(v, "leg_desc", true),
                text_layer: text_of(v, "leg_tlayer", "").trim().to_string(),
                text_style: text_of(v, "leg_tstyle", "").trim().to_string(),
            },
        }
    }

    fn rename_op(&self) -> Result<RenameOp, String> {
        let v = &self.vals;
        let (find, to) = (text_of(v, "ren_find", ""), text_of(v, "ren_text", ""));
        let need_find = || if find.is_empty() { Err("\"Find\" is empty".to_string()) } else { Ok(()) };
        let need_text = || if to.is_empty() { Err("the text to add is empty".to_string()) } else { Ok(()) };
        Ok(match choice_of(v, "ren_op") {
            0 => { need_text()?; RenameOp::AddPrefix(to) }
            1 => { need_find()?; RenameOp::ReplacePrefix { from: find, to } }
            2 => { need_text()?; RenameOp::AddSuffix(to) }
            3 => { need_find()?; RenameOp::ReplaceSuffix { from: find, to } }
            _ => { need_find()?; RenameOp::ReplaceString { from: find, to, case_sensitive: bool_of(v, "ren_case", false) } }
        })
    }

    /// Preview rows (layer, new name, status) and a one-line summary.
    fn rename_preview(&self) -> (Vec<Vec<String>>, String) {
        let op = match self.rename_op() {
            Ok(op) => op,
            Err(e) => return (Vec::new(), format!("Fill in the fields: {e}.")),
        };
        let all: Vec<String> = self.layers.iter().map(|l| l.name.clone()).collect();
        let selected = rename::in_scope(&all, &text_of(&self.vals, "ren_scope", "*"));
        let entries = rename::plan(&all, &selected, &op);
        let count = |f: fn(&Status) -> bool| entries.iter().filter(|e| f(&e.status)).count();
        let (ok, bad) = (
            count(|s| matches!(s, Status::Ok)),
            count(|s| matches!(s, Status::Invalid(_) | Status::Collision(_))),
        );
        let shown: Vec<Vec<String>> = entries
            .iter()
            .filter(|e| !matches!(e.status, Status::Unchanged | Status::Protected))
            .take(PREVIEW_MAX)
            .map(|e| vec![e.old.clone(), e.new.clone(), e.status.label()])
            .collect();
        let mut summary = format!("{} layer(s) in scope: {ok} will be renamed, {bad} have a problem (not renamed).", selected.len());
        if ok + bad > PREVIEW_MAX {
            summary.push_str(&format!(" Showing the first {PREVIEW_MAX}."));
        }
        (shown, summary)
    }

    fn rename_controls(&self) -> Vec<DialogControl> {
        let v = &self.vals;
        let (rows, summary) = self.rename_preview();
        vec![
            heading("Rename layers"),
            choice("ren_op", "Operation", &RENAME_OPS, choice_of(v, "ren_op")),
            text("ren_find", "Find (replace prefix / suffix / string)", text_of(v, "ren_find", "")),
            text("ren_text", "New text (or the text to add)", text_of(v, "ren_text", "")),
            DialogControl::Checkbox {
                id: "ren_case".into(),
                label: "Match case (Replace string)".into(),
                default: bool_of(v, "ren_case", false),
            },
            text("ren_scope", "Only layers named (wildcards * ?)", text_of(v, "ren_scope", "*")),
            button("ren_refresh", "Update preview", 150.0),
            DialogControl::Label { id: "ren_summary".into(), text: summary, align_h: None },
            DialogControl::Table {
                id: "ren_preview".into(),
                label: Some("Preview".into()),
                headers: vec!["Layer".into(), "New name".into(), "Status".into()],
                rows,
                allow_add: false,
                allow_delete: false,
            },
            info("Layer 0, Defpoints and xref layers (name|layer) are never renamed. One undo step."),
        ]
    }

    fn filter_controls(&self) -> Vec<DialogControl> {
        let v = &self.vals;
        let kinds: Vec<&str> = filters::QUICK_TYPES.iter().map(|t| t.0).collect();
        let existing: Vec<Vec<String>> = self
            .filters
            .iter()
            .map(|f| vec![f.name.clone(), f.parent.clone().unwrap_or_default(), f.describe()])
            .collect();
        vec![
            heading("Layer filters"),
            choice("flt_mode", "What to do", &FILTER_MODES, choice_of(v, "flt_mode")),
            heading("Quick filter"),
            choice("flt_kind", "Type", &kinds, choice_of(v, "flt_kind")),
            text("flt_value", "Value", text_of(v, "flt_value", "")),
            text("flt_name", "Filter name (blank = automatic)", text_of(v, "flt_name", "")),
            button("flt_add", "Add to the list", 150.0),
            DialogControl::Table {
                id: "flt_new".into(),
                label: Some("Filters to create".into()),
                headers: vec!["Filter name".into(), "Type".into(), "Value".into()],
                rows: table_of(v, "flt_new"),
                allow_add: false,
                allow_delete: true,
            },
            heading("CSV"),
            picker("flt_csv_out", "CSV file to write", text_of(v, "flt_csv_out", ""), true, "Filter list (CSV)", &["csv"]),
            picker("flt_csv_in", "CSV file to read", text_of(v, "flt_csv_in", ""), false, "Filter list (CSV)", &["csv"]),
            DialogControl::Table {
                id: "flt_existing".into(),
                label: Some("Filters in this drawing".into()),
                headers: vec!["Filter".into(), "Under".into(), "Conditions".into()],
                rows: existing,
                allow_add: false,
                allow_delete: false,
            },
            info(
                "Filters are created through the host's LAYERFILTER command: Name (one wildcard pattern), Color, Linetype and \
                 On/Frozen/Locked/Plot, combined with AND; OR is made with nested filters. A filter using != , NOT Name, \
                 Description or Xref is reported and not created.",
            ),
        ]
    }

    // ── events ──

    fn load_lmd(&mut self, path: &str) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| format!("cannot read \"{path}\": {e}"))?;
        let (file, warn) = lmd::parse(&String::from_utf8_lossy(&bytes));
        if file.groups.is_empty() {
            return Err("no layers found in that file".into());
        }
        self.lmd_note = if warn.is_empty() { String::new() } else { format!("{} line(s) skipped: {}", warn.len(), warn[0]) };
        for i in 0..64 {
            self.vals.remove(&format!("grp_{i}"));
        }
        self.lmd = Some((path.to_string(), file));
        Ok(())
    }

    fn set_all_groups(&mut self, on: bool) {
        if let Some((_, f)) = &self.lmd {
            for i in 0..f.groups.len() {
                self.vals.insert(format!("grp_{i}"), DialogValue::Bool(on));
            }
        }
    }

    fn replace(&self) -> DialogActionResponse {
        DialogActionResponse::ReplaceDialog(Box::new(self.build()))
    }

    /// The job for the current tab, from the submitted values.
    fn make_job(&mut self) -> Job {
        match self.tab {
            Tab::Import => self.import_job(),
            Tab::Rename => match self.rename_op() {
                Ok(op) => Job::Rename { op, scope: text_of(&self.vals, "ren_scope", "*") },
                Err(e) => Job::Problem(format!("rename: {e}.")),
            },
            Tab::Filter => self.filter_job(),
            Tab::Legend => {
                let r = self.legend_request();
                let action = choice_of(&self.vals, "leg_action");
                let valid = r.params.validate();
                if valid.is_ok() && action > 0 {
                    let path = path_of(&self.vals, "leg_json").unwrap_or_else(crate::legend::current_settings_path);
                    return Job::LegendSave { req: r, path, run: action == 1 };
                }
                match valid {
                    Ok(()) if r.scale_source == ScaleSource::Custom && !(r.custom_scale > 0.0) => Job::Problem("legend: the custom scale must be greater than 0.".into()),
                    Ok(()) => Job::Legend(r),
                    Err(e) => Job::Problem(format!("legend: {e}.")),
                }
            }
        }
    }

    fn import_job(&mut self) -> Job {
        if choice_of(&self.vals, "imp_mode") == 1 {
            let Some(path) = path_of(&self.vals, "imp_save") else { return Job::Problem("choose the LMD file to write.".into()) };
            let group = text_of(&self.vals, "imp_group", "").trim().to_string();
            if group.is_empty() || group.contains(['[', ']', ',']) {
                return Job::Problem("the group name is empty or contains [ ] or a comma.".into());
            }
            return Job::Export {
                path,
                group,
                pattern: text_of(&self.vals, "imp_scope", "*"),
                append: bool_of(&self.vals, "imp_append", true),
            };
        }
        // Import: read the typed path now if the picker was never used.
        let typed = path_of(&self.vals, "imp_open");
        let loaded = self.lmd.as_ref().map(|(p, _)| p.clone());
        if let Some(p) = typed.filter(|p| Some(p) != loaded.as_ref()) {
            if let Err(e) = self.load_lmd(&p) {
                return Job::Problem(e);
            }
        }
        let Some((_, file)) = &self.lmd else { return Job::Problem("choose the LMD file to read.".into()) };
        let groups: Vec<usize> = (0..file.groups.len()).filter(|i| bool_of(&self.vals, &format!("grp_{i}"), true)).collect();
        if groups.is_empty() {
            return Job::Problem("no group is ticked.".into());
        }
        let existing = if choice_of(&self.vals, "imp_existing") == 1 { Existing::Update } else { Existing::Skip };
        let group_format = if bool_of(&self.vals, "imp_mkgroups", true) {
            let f = text_of(&self.vals, "imp_gfmt", "[{name}]");
            if !f.contains("{name}") {
                return Job::Problem("the group name must contain {name}.".into());
            }
            Some(f)
        } else {
            None
        };
        Job::Import { file: file.clone(), groups, existing, group_format, underscores: bool_of(&self.vals, "imp_nospace", true) }
    }

    fn filter_job(&mut self) -> Job {
        match choice_of(&self.vals, "flt_mode") {
            1 => path_of(&self.vals, "flt_csv_out").map(Job::FilterExport).unwrap_or_else(|| Job::Problem("choose the CSV file to write.".into())),
            2 => path_of(&self.vals, "flt_csv_in").map(Job::FilterImport).unwrap_or_else(|| Job::Problem("choose the CSV file to read.".into())),
            _ => {
                let mut defs = Vec::new();
                for row in table_of(&self.vals, "flt_new") {
                    let kind = filters::QUICK_TYPES.iter().position(|t| row.get(1).is_some_and(|l| l == t.0)).unwrap_or(usize::MAX);
                    match filters::quick(kind, row.get(2).map(String::as_str).unwrap_or(""), row.first().map(String::as_str).unwrap_or("")) {
                        Ok(d) => defs.push(d),
                        Err(e) => return Job::Problem(format!("filter \"{}\": {e}.", row.first().cloned().unwrap_or_default())),
                    }
                }
                if defs.is_empty() {
                    return Job::Problem("the list of filters to create is empty (use \"Add to the list\").".into());
                }
                Job::FilterCreate(defs)
            }
        }
    }
}

impl InteractiveCommand for LaynameDialog {
    fn prompt(&self) -> String {
        "Layer Names: use the window".into()
    }
    fn on_point(&mut self, _pt: [f64; 3]) -> CommandStep {
        CommandStep::NeedPoint
    }
    fn on_enter(&mut self) -> CommandStep {
        CommandStep::Cancel
    }
    fn modal_dialog(&self) -> Option<DialogDef> {
        (!self.done).then(|| self.build())
    }

    fn on_dialog_action(&mut self, event: DialogEvent) -> DialogActionResponse {
        self.vals.extend(event.current_values.clone());
        if let Some(v) = &event.value {
            if matches!(v, DialogValue::FilePath(_)) {
                self.vals.insert(event.control_id.clone(), v.clone());
            }
        }
        let (id, action) = (event.control_id.as_str(), event.action.as_str());
        match (id, action) {
            ("tab_import", "click") | ("tab_rename", "click") | ("tab_filter", "click") | ("tab_legend", "click") => {
                self.tab = match id {
                    "tab_import" => Tab::Import,
                    "tab_rename" => Tab::Rename,
                    "tab_legend" => Tab::Legend,
                    _ => Tab::Filter,
                };
                self.replace()
            }
            ("imp_open", "pick") => {
                if let Some(p) = path_of(&self.vals, "imp_open") {
                    if let Err(e) = self.load_lmd(&p) {
                        self.lmd = None;
                        self.lmd_note = e;
                    }
                }
                self.replace()
            }
            ("leg_load", "click") => {
                let path = path_of(&self.vals, "leg_json").unwrap_or_else(crate::legend::current_settings_path);
                match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| crate::legend::from_json(&t)) {
                    Ok(r) => {
                        self.apply_request(&r);
                        crate::legend::remember_settings_path(&path);
                    }
                    Err(e) => self.legend_note = format!("Could not load \"{path}\": {e}"),
                }
                self.replace()
            }
            ("grp_all", "click") | ("grp_none", "click") => {
                self.set_all_groups(id == "grp_all");
                self.replace()
            }
            ("ren_refresh", "click") => {
                let (rows, summary) = self.rename_preview();
                DialogActionResponse::UpdateValues(HashMap::from([
                    ("ren_preview".to_string(), DialogValue::Table(rows)),
                    ("ren_summary".to_string(), DialogValue::Text(summary)),
                ]))
            }
            ("flt_add", "click") => {
                let kind = choice_of(&self.vals, "flt_kind");
                let (value, name) = (text_of(&self.vals, "flt_value", ""), text_of(&self.vals, "flt_name", ""));
                match filters::quick(kind, &value, &name) {
                    Ok(def) => {
                        let mut rows = table_of(&self.vals, "flt_new");
                        rows.push(vec![def.name.clone(), filters::QUICK_TYPES[kind].0.to_string(), value.trim().to_string()]);
                        self.vals.insert("flt_new".into(), DialogValue::Table(rows.clone()));
                        self.vals.insert("flt_value".into(), DialogValue::Text(String::new()));
                        self.vals.insert("flt_name".into(), DialogValue::Text(String::new()));
                        DialogActionResponse::UpdateValues(HashMap::from([
                            ("flt_new".to_string(), DialogValue::Table(rows)),
                            ("flt_value".to_string(), DialogValue::Text(String::new())),
                            ("flt_name".to_string(), DialogValue::Text(String::new())),
                        ]))
                    }
                    Err(_) => DialogActionResponse::None,
                }
            }
            _ => DialogActionResponse::None,
        }
    }

    fn on_dialog_response(&mut self, response: DialogResponse) -> CommandStep {
        self.done = true;
        let DialogResponse::Submitted(map) = response else { return CommandStep::Cancel };
        self.vals.extend(map);
        let job = self.make_job();
        set_job(job);
        CommandStep::Dispatch("LN_APPLY".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::take_job_for_test;

    fn layer(n: &str) -> LayerInfo {
        LayerInfo { name: n.into(), color: 7, linetype: "Continuous".into(), description: String::new() }
    }

    fn ev(id: &str, action: &str, vals: &[(&str, DialogValue)]) -> DialogEvent {
        DialogEvent {
            control_id: id.into(),
            action: action.into(),
            value: None,
            current_values: vals.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
        }
    }

    fn ids(d: &DialogDef) -> Vec<String> {
        fn walk(c: &DialogControl, out: &mut Vec<String>) {
            if let Some(i) = c.id() { out.push(i.to_string()); }
            if let DialogControl::Row { controls, .. } | DialogControl::Column { controls, .. } = c {
                controls.iter().for_each(|c| walk(c, out));
            }
        }
        let mut out = Vec::new();
        d.rows.iter().flat_map(|r| r.controls.iter()).for_each(|c| walk(c, &mut out));
        out
    }

    #[test]
    fn tabs_switch_and_keep_values() {
        let mut d = LaynameDialog::new(Tab::Import, vec![layer("GAS")], vec![]);
        assert!(ids(&d.build()).contains(&"imp_open".to_string()));
        let r = d.on_dialog_action(ev("tab_rename", "click", &[("imp_group", DialogValue::Text("MINE".into()))]));
        let DialogActionResponse::ReplaceDialog(def) = r else { panic!("no replace") };
        assert!(ids(&def).contains(&"ren_op".to_string()));
        let DialogActionResponse::ReplaceDialog(back) = d.on_dialog_action(ev("tab_import", "click", &[])) else { panic!() };
        let kept = back.rows.iter().flat_map(|r| r.controls.iter()).any(|c| matches!(c, DialogControl::Text { id, default, .. } if id == "imp_group" && default == "MINE"));
        assert!(kept, "typed group name was lost across tabs");
    }

    #[test]
    fn rename_preview_updates_and_submits() {
        let mut d = LaynameDialog::new(Tab::Rename, vec![layer("0"), layer("GAS"), layer("UTIL_GAS"), layer("WATER")], vec![]);
        let r = d.on_dialog_action(ev("ren_refresh", "click", &[
            ("ren_op", DialogValue::Choice { index: 0, text: "Add prefix".into() }),
            ("ren_text", DialogValue::Text("UTIL_".into())),
            ("ren_scope", DialogValue::Text("*".into())),
        ]));
        let DialogActionResponse::UpdateValues(m) = r else { panic!() };
        let Some(DialogValue::Table(rows)) = m.get("ren_preview") else { panic!() };
        // scope "*": UTIL_GAS moves too (-> UTIL_UTIL_GAS), so GAS -> UTIL_GAS is fine
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|r| r[2] == "OK"), "{rows:?}");
        assert!(rows.iter().any(|r| r[0] == "WATER" && r[1] == "UTIL_WATER"));

        // scope "GAS": UTIL_GAS stays, so GAS -> UTIL_GAS collides and is not renamed
        let r = d.on_dialog_action(ev("ren_refresh", "click", &[("ren_scope", DialogValue::Text("GAS".into()))]));
        let DialogActionResponse::UpdateValues(m) = r else { panic!() };
        let Some(DialogValue::Table(rows)) = m.get("ren_preview") else { panic!() };
        assert_eq!(rows.len(), 1);
        assert!(rows[0][2].starts_with("collision"), "{rows:?}");
        d.vals.insert("ren_scope".into(), DialogValue::Text("*".into()));

        let step = d.on_dialog_response(DialogResponse::Submitted(HashMap::new()));
        assert!(matches!(step, CommandStep::Dispatch(ref s) if s == "LN_APPLY"));
        assert!(matches!(take_job_for_test(), Some(Job::Rename { op: RenameOp::AddPrefix(p), .. }) if p == "UTIL_"));
    }

    #[test]
    fn rename_with_empty_fields_is_a_problem_not_a_rename() {
        let mut d = LaynameDialog::new(Tab::Rename, vec![layer("A")], vec![]);
        d.on_dialog_response(DialogResponse::Submitted(HashMap::new()));
        assert!(matches!(take_job_for_test(), Some(Job::Problem(_))));
    }

    #[test]
    fn import_reads_file_lists_groups_and_submits_selection() {
        let dir = std::env::temp_dir().join("layname_dialog_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.lmd");
        std::fs::write(&path, lmd_text()).unwrap();
        let p = path.to_string_lossy().to_string();

        let mut d = LaynameDialog::new(Tab::Import, vec![], vec![]);
        let mut e = ev("imp_open", "pick", &[("imp_open", DialogValue::FilePath(p.clone()))]);
        e.value = Some(DialogValue::FilePath(p));
        let DialogActionResponse::ReplaceDialog(def) = d.on_dialog_action(e) else { panic!() };
        let all = ids(&def);
        assert!(all.contains(&"grp_0".to_string()) && all.contains(&"grp_1".to_string()), "{all:?}");

        // untick group 0, keep group 1; update existing
        let step = d.on_dialog_response(DialogResponse::Submitted(HashMap::from([
            ("grp_0".to_string(), DialogValue::Bool(false)),
            ("grp_1".to_string(), DialogValue::Bool(true)),
            ("imp_existing".to_string(), DialogValue::Choice { index: 1, text: String::new() }),
        ])));
        assert!(matches!(step, CommandStep::Dispatch(_)));
        match take_job_for_test() {
            Some(Job::Import { groups, existing, group_format, underscores, .. }) => {
                assert!(underscores); // on by default
                assert_eq!(group_format.as_deref(), Some("[{name}]")); // groups on by default
                assert_eq!(groups, vec![1]);
                assert_eq!(existing, Existing::Update);
            }
            _ => panic!("expected an import job"),
        }
    }

    fn lmd_text() -> &'static str {
        "[A],,,\nL1 ,7, Continuous , x\n[B],,,\nL2 ,1, CENTER , <null>\n"
    }

    #[test]
    fn legend_tab_defaults_and_job() {
        let mut d = LaynameDialog::new(Tab::Legend, vec![], vec![]);
        assert!(ids(&d.build()).contains(&"leg_len".to_string()));
        d.on_dialog_response(DialogResponse::Submitted(HashMap::from([
            ("leg_type".to_string(), DialogValue::Choice { index: 1, text: String::new() }),
            ("leg_scale".to_string(), DialogValue::Choice { index: 1, text: String::new() }),
            ("leg_len".to_string(), DialogValue::Number(40.0)),
        ])));
        match take_job_for_test() {
            Some(Job::Legend(r)) => {
                assert!(!r.by_layers);
                assert_eq!(r.scale_source, ScaleSource::Annotation);
                assert_eq!((r.params.line_len, r.params.line_pitch, r.params.hatch_w), (40.0, 7.5, 25.0));
            }
            _ => panic!("expected Legend"),
        }
        let mut def = LaynameDialog::new(Tab::Legend, vec![], vec![]);
        def.on_dialog_response(DialogResponse::Submitted(HashMap::new()));
        assert!(matches!(take_job_for_test(), Some(Job::Legend(r)) if !r.by_layers && r.scale_source == ScaleSource::Annotation));
        let mut z = LaynameDialog::new(Tab::Legend, vec![], vec![]);
        z.on_dialog_response(DialogResponse::Submitted(HashMap::from([("leg_len".to_string(), DialogValue::Number(0.0))])));
        assert!(matches!(take_job_for_test(), Some(Job::Problem(_))));
    }

    #[test]
    fn legend_settings_save_and_load() {
        let dir = std::env::temp_dir().join("layname_legend_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("l.json").to_string_lossy().to_string();
        let mut d = LaynameDialog::new(Tab::Legend, vec![], vec![]);
        d.on_dialog_response(DialogResponse::Submitted(HashMap::from([
            ("leg_action".to_string(), DialogValue::Choice { index: 2, text: String::new() }),
            ("leg_json".to_string(), DialogValue::FilePath(path.clone())),
            ("leg_th".to_string(), DialogValue::Number(4.0)),
        ])));
        let Some(Job::LegendSave { req, path: p, run }) = take_job_for_test() else { panic!("expected LegendSave") };
        assert!(!run && p == path && req.params.text_height == 4.0);
        std::fs::write(&p, crate::legend::to_json(&req)).unwrap();

        let mut e = LaynameDialog::new(Tab::Legend, vec![], vec![]);
        let DialogActionResponse::ReplaceDialog(_) = e.on_dialog_action(ev("leg_load", "click", &[("leg_json", DialogValue::FilePath(path))])) else { panic!() };
        e.on_dialog_response(DialogResponse::Submitted(HashMap::new()));
        assert!(matches!(take_job_for_test(), Some(Job::Legend(r)) if r.params.text_height == 4.0));
    }

    #[test]
    fn export_and_filter_jobs() {
        let mut d = LaynameDialog::new(Tab::Import, vec![], vec![]);
        d.on_dialog_response(DialogResponse::Submitted(HashMap::from([
            ("imp_mode".to_string(), DialogValue::Choice { index: 1, text: String::new() }),
            ("imp_save".to_string(), DialogValue::FilePath("/tmp/x.lmd".into())),
            ("imp_group".to_string(), DialogValue::Text("G1".into())),
        ])));
        assert!(matches!(take_job_for_test(), Some(Job::Export { group, append: true, .. }) if group == "G1"));

        let mut f = LaynameDialog::new(Tab::Filter, vec![], vec![]);
        let add = |f: &mut LaynameDialog, kind: usize, value: &str| {
            f.on_dialog_action(ev("flt_add", "click", &[
                ("flt_kind", DialogValue::Choice { index: kind, text: String::new() }),
                ("flt_value", DialogValue::Text(value.into())),
            ]))
        };
        assert!(matches!(add(&mut f, 0, "UTIL_"), DialogActionResponse::UpdateValues(_)));
        assert!(matches!(add(&mut f, 4, "90"), DialogActionResponse::UpdateValues(_)));
        assert!(matches!(add(&mut f, 0, ""), DialogActionResponse::None)); // value required
        f.on_dialog_response(DialogResponse::Submitted(HashMap::new()));
        match take_job_for_test() {
            Some(Job::FilterCreate(defs)) => {
                assert_eq!(defs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["UTIL_*", "Color_90"]);
            }
            _ => panic!("expected FilterCreate"),
        }
    }
}
