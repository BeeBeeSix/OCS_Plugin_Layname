//! Command dispatch and the apply step.
//!
//! Interactive callbacks (the dialog) get no host access, so on OK the dialog parks a
//! [`Job`] here and dispatches `LN_APPLY`, which comes back with full `HostApi` access.
//! `LN_APPLY` with nothing parked is a silent no-op (the host's command line probes a
//! typed line's first word before running it).

use std::path::Path;
use std::sync::Mutex;

use ocs_plugin_api::host::codec;
use ocs_plugin_api::host::{HostApi, LayerConfig, TableOperation};

use crate::dialog::{LaynameDialog, Tab};
use crate::filters::{self, FilterDef};
use crate::host::{LayerHost, LayerInfo};
use crate::legend::{self, Layout, Request, ScaleSource};
use crate::lmd::{self, Existing, LmdFile, LmdLayer};
use crate::rename::{self, RenameOp};

/// Work chosen in the dialog, done by `LN_APPLY`.
pub enum Job {
    Import {
        file: LmdFile,
        groups: Vec<usize>,
        existing: Existing,
        /// `Some(format)` = also create a layer group per LMD group (`{name}` in the format).
        group_format: Option<String>,
        /// Replace spaces in layer names with `_` before importing.
        underscores: bool,
    },
    Export { path: String, group: String, pattern: String, append: bool },
    Rename { op: RenameOp, scope: String },
    FilterCreate(Vec<FilterDef>),
    FilterExport(String),
    FilterImport(String),
    Legend(Request),
    /// Save the legend settings to JSON; `run` = also create the legend now.
    LegendSave { req: Request, path: String, run: bool },
    /// Object legend: the selection was made (handles) — pick the point next.
    LegendSelected(Request, Vec<codec::Handle>),
    /// A problem found while the dialog closed.
    Problem(String),
}

static PENDING: Mutex<Option<Job>> = Mutex::new(None);

pub fn set_job(job: Job) {
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(job);
}

fn take_job() -> Option<Job> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).take()
}

#[cfg(test)]
pub(crate) fn take_job_for_test() -> Option<Job> {
    take_job()
}

pub fn handle(host: &mut dyn HostApi, cmd: &str) -> bool {
    let verb = cmd.split_whitespace().next().unwrap_or("").to_ascii_uppercase();
    if !verb.starts_with("LN_") {
        return false;
    }
    match verb.as_str() {
        "LN_HELP" | "LN_" => help(host),
        "LN_IMPORT" | "LN_LAYNAME" => open(host, Tab::Import),
        "LN_RENAME" => open(host, Tab::Rename),
        "LN_FILTER" => open(host, Tab::Filter),
        "LN_LEGEND" => open(host, Tab::Legend),
        "LN_LEGENDQ" | "LN_LEGEND_QUICK" => legend_quick(host, cmd),
        "LN_APPLY" => {
            if let Some(job) = take_job() {
                apply(host, job);
            }
        }
        other => host.push_error(&format!("{other}: unknown command. LN_HELP lists them.")),
    }
    true
}

fn help(host: &mut dyn HostApi) {
    for line in [
        "Layer Names:",
        "  LN_IMPORT  import / export LMD layer maps (groups in [brackets])",
        "  LN_RENAME  bulk rename: prefix add/replace, suffix add/replace, string replace",
        "  LN_FILTER  create layer filters quickly; export / import them as CSV",
        "  LN_LEGEND  legend creator: layer legend or object legend (lines, blocks, hatches)",
        "  LN_LEGENDQ legend from the saved JSON settings, no window (LN_LEGENDQ file.json = another file)",
        "The window commands open the same window; the buttons at its top switch between the tabs.",
    ] {
        host.push_output(line);
    }
}

fn open(host: &mut dyn HostApi, tab: Tab) {
    let layers = Adapter(host).layers();
    let filters = filters::read_drawing_filters(host.document());
    host.start_interactive(Box::new(LaynameDialog::new(tab, layers, filters)));
}

// ───────────────────────── the LayerHost on the real host ─────────────────────────

struct Adapter<'a>(&'a mut dyn HostApi);

fn aci(c: i32) -> u8 {
    if (1..=255).contains(&c) { c as u8 } else { 7 }
}

impl LayerHost for Adapter<'_> {
    fn layers(&self) -> Vec<LayerInfo> {
        self.0
            .document()
            .layers
            .iter()
            .map(|l| LayerInfo {
                name: l.name.clone(),
                color: l.color.index().map(|i| i as i32).unwrap_or(-1),
                linetype: l.line_type.clone(),
                description: l.description.clone(),
            })
            .collect()
    }

    fn create_layer(&mut self, l: &LmdLayer) -> Result<bool, String> {
        let cfg = |lt: Option<String>| LayerConfig {
            name: l.name.clone(),
            color: Some(codec::types::Color::Index(aci(l.color))),
            linetype: lt,
            description: (!l.description.is_empty()).then(|| l.description.clone()),
            ..Default::default()
        };
        // The host refuses a linetype the drawing doesn't carry (and can't load): retry plain.
        if self.0.add_layer(cfg(Some(l.linetype.clone()))).is_some() {
            return Ok(true);
        }
        if self.0.add_layer(cfg(None)).is_some() {
            return Ok(false);
        }
        Err("the CAD refused the layer (invalid name?)".into())
    }

    fn update_layer(&mut self, l: &LmdLayer) -> Result<bool, String> {
        let cfg = |lt: Option<String>| LayerConfig {
            name: l.name.clone(),
            color: Some(codec::types::Color::Index(aci(l.color))),
            linetype: lt,
            description: Some(l.description.clone()),
            ..Default::default()
        };
        if self.0.modify_layer(cfg(Some(l.linetype.clone()))) {
            return Ok(true);
        }
        if self.0.modify_layer(cfg(None)) {
            return Ok(false);
        }
        Err("the CAD refused the change".into())
    }

    fn rename_layer(&mut self, old: &str, new: &str) -> Result<(), String> {
        self.0
            .table_operation(TableOperation::LayerRename { from: old.to_string(), to: new.to_string() })
            .map(|_| ())
    }
}

// ───────────────────────── apply ─────────────────────────

/// First `max` items, then "… and N more".
fn list(items: &[String], max: usize) -> String {
    let mut s = items.iter().take(max).cloned().collect::<Vec<_>>().join(", ");
    if items.len() > max {
        s.push_str(&format!(" … and {} more", items.len() - max));
    }
    s
}

fn read_text(path: &str) -> Result<String, String> {
    std::fs::read(path)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .map_err(|e| format!("cannot read \"{path}\": {e}"))
}

fn apply(host: &mut dyn HostApi, job: Job) {
    match job {
        Job::Problem(msg) => host.push_error(&format!("Layer Names: {msg}")),

        Job::Import { file, groups, existing, group_format, underscores } => {
            let file = if underscores { file.with_underscores() } else { file };
            host.begin_undo_group("LN_IMPORT");
            let rep = lmd::import(&mut Adapter(host), &file, &groups, existing);
            let plans = group_format.map(|f| lmd::group_plans(&file, &groups, &f)).unwrap_or_default();
            let mut made = 0;
            let mut refused = Vec::new();
            for p in &plans {
                if lmd::group_commands(p).iter().all(|c| host.execute_command(c)) {
                    made += 1;
                } else {
                    refused.push(p.name.clone());
                }
            }
            host.end_undo_group();
            host.push_info(&format!(
                "LN_IMPORT: {} layer(s) created, {} updated, {} skipped (already exist).",
                rep.created.len(), rep.updated.len(), rep.skipped_existing.len()
            ));
            if !rep.linetype_fallback.is_empty() {
                host.push_output(&format!(
                    "  Linetype not in the drawing — created/updated with the layer's current or Continuous linetype ({}): {}. \
                     Load the linetypes (LINETYPE) and import again with \"Update existing layers\".",
                    rep.linetype_fallback.len(), list(&rep.linetype_fallback, 8)
                ));
            }
            if !plans.is_empty() {
                host.push_output(&format!("  {made} layer group(s) created or updated: {}", list(&plans.iter().map(|p| p.name.clone()).collect::<Vec<_>>(), 6)));
                let skipped: Vec<String> = plans.iter().flat_map(|p| p.skipped.iter().map(move |l| format!("{} [{}]", l, p.name))).collect();
                if !skipped.is_empty() {
                    host.push_output(&format!(
                        "  {} layer(s) were not put in their group — a name with a space can't go through the command line: {}. \
                         Add them in the Layer Manager.",
                        skipped.len(), list(&skipped, 8)
                    ));
                }
                if plans.iter().any(|p| p.name.contains('_')) {
                    host.push_output("  Spaces in group names were replaced with '_'.");
                }
                if !refused.is_empty() {
                    host.push_error(&format!("  the host did not accept the group command for: {}", list(&refused, 6)));
                }
            }
            if !rep.duplicates_in_file.is_empty() {
                host.push_output(&format!(
                    "  {} name(s) appear in more than one selected group; the first one was used: {}",
                    rep.duplicates_in_file.len(), list(&rep.duplicates_in_file, 8)
                ));
            }
            for e in rep.errors.iter().take(10) {
                host.push_error(&format!("  {e}"));
            }
        }

        Job::Export { path, group, pattern, append } => {
            let layers = Adapter(host).layers();
            let g = LmdFile::group_from_layers(&group, &layers, &pattern);
            let n = g.layers.len();
            let mut file = LmdFile::default();
            if append && Path::new(&path).exists() {
                match read_text(&path) {
                    Ok(t) => file = lmd::parse(&t).0,
                    Err(e) => return host.push_error(&format!("LN_IMPORT: {e}")),
                }
            }
            file.merge_group(g);
            match std::fs::write(&path, file.to_text()) {
                Ok(()) => host.push_info(&format!("LN_IMPORT: {n} layer(s) written to group [{group}] in \"{path}\".")),
                Err(e) => host.push_error(&format!("LN_IMPORT: cannot write \"{path}\": {e}")),
            }
        }

        Job::Rename { op, scope } => {
            let names: Vec<String> = Adapter(host).layers().into_iter().map(|l| l.name).collect();
            let selected = rename::in_scope(&names, &scope);
            let entries = rename::plan(&names, &selected, &op);
            let problems: Vec<String> = entries
                .iter()
                .filter(|e| matches!(e.status, rename::Status::Invalid(_) | rename::Status::Collision(_)))
                .map(|e| format!("{} → {} ({})", e.old, e.new, e.status.label()))
                .collect();
            host.begin_undo_group("LN_RENAME");
            let rep = rename::apply(&mut Adapter(host), &entries);
            host.end_undo_group();
            host.push_info(&format!("LN_RENAME: {} layer(s) renamed, {} not renamed.", rep.renamed, problems.len() + rep.errors.len()));
            for p in problems.iter().chain(rep.errors.iter()).take(10) {
                host.push_output(&format!("  {p}"));
            }
        }

        Job::Legend(req) => start_legend(host, req),
        Job::LegendSave { req, path, run } => {
            let wrote = Path::new(&path)
                .parent()
                .filter(|d| !d.as_os_str().is_empty())
                .map(std::fs::create_dir_all)
                .unwrap_or(Ok(()))
                .and_then(|_| std::fs::write(&path, legend::to_json(&req)));
            match wrote {
                Ok(()) => {
                    legend::remember_settings_path(&path);
                    host.push_info(&format!("LN_LEGEND: settings saved to \"{path}\" (LN_LEGENDQ runs the legend with them)."));
                    if run {
                        start_legend(host, req);
                    }
                }
                Err(e) => host.push_error(&format!("LN_LEGEND: cannot write \"{path}\": {e}")),
            }
        }
        Job::LegendSelected(req, handles) => {
            let items = legend::items_from_selection(host.document(), &handles, req.params.include_desc);
            begin_pick(host, req, items);
        }

        Job::FilterCreate(defs) => create_filters(host, defs, Vec::new()),

        Job::FilterExport(path) => {
            let defs = filters::read_drawing_filters(host.document());
            if defs.is_empty() {
                return host.push_info("LN_FILTER: the drawing has no layer filters that the plugin can read; nothing written.");
            }
            match std::fs::write(&path, filters::to_csv(&defs)) {
                Ok(()) => host.push_info(&format!("LN_FILTER: {} filter(s) written to \"{path}\".", defs.len())),
                Err(e) => host.push_error(&format!("LN_FILTER: cannot write \"{path}\": {e}")),
            }
        }

        Job::FilterImport(path) => match read_text(&path) {
            Ok(t) => {
                let (defs, warn) = filters::from_csv(&t);
                create_filters(host, defs, warn);
            }
            Err(e) => host.push_error(&format!("LN_FILTER: {e}")),
        },
    }
}

fn create_filters(host: &mut dyn HostApi, defs: Vec<FilterDef>, mut warnings: Vec<String>) {
    if defs.is_empty() {
        host.push_info("LN_FILTER: no filters to create.");
        for w in warnings.iter().take(10) {
            host.push_output(&format!("  {w}"));
        }
        return;
    }
    let top: Vec<String> = filters::read_drawing_filters(host.document())
        .into_iter()
        .filter(|f| f.parent.is_none())
        .map(|f| f.name)
        .collect();
    let plan = filters::plan_commands(&defs, &top);
    warnings.extend(plan.warnings.iter().cloned());
    host.begin_undo_group("LN_FILTER");
    let mut failed = None;
    for c in &plan.commands {
        if !host.execute_command(c) {
            failed = Some(c.clone());
            break;
        }
    }
    host.end_undo_group();
    host.push_info(&format!("LN_FILTER: {} filter(s) created.", plan.created.len()));
    if let Some(c) = failed {
        host.push_error(&format!("  the host did not accept \"{c}\"; the rest was not run."));
    }
    for w in warnings.iter().take(15) {
        host.push_output(&format!("  {w}"));
    }
    if warnings.len() > 15 {
        host.push_output(&format!("  … and {} more", warnings.len() - 15));
    }
}

// ───────────────────────── legend ─────────────────────────

fn resolve_scale(host: &dyn HostApi, req: &Request) -> Result<f64, String> {
    use ocs_plugin_api::host::HostSettingValue as V;
    let s = match req.scale_source {
        ScaleSource::Custom => req.custom_scale,
        ScaleSource::DimScale => match host.system_variable("DIMSCALE") {
            Some(V::Number(v)) if v > 0.0 => v,
            Some(V::Text(t)) => t.trim().parse().ok().filter(|v: &f64| *v > 0.0).unwrap_or(1.0),
            _ => 1.0, // DIMSCALE 0 / unreadable: paper-space style, 1
        },
        ScaleSource::Annotation => {
            // The live settings follow the status-bar scale; the saved header only the last save.
            let name = match host.system_variable("CANNOSCALE") {
                Some(V::Text(t)) => Some(t),
                _ => None,
            };
            let value = match host.system_variable("CANNOSCALEVALUE") {
                Some(V::Number(v)) => Some(v),
                Some(V::Text(t)) => t.trim().parse().ok(),
                _ => None,
            };
            let live = name.is_some() || value.is_some();
            let (n, m) = legend::annotation_info(host.document(), name.as_deref(), value)
                .ok_or("the current annotation scale could not be read (use DIMSCALE or a custom value)")?;
            note_scale(&n, m, live);
            m
        }
    };
    if s.is_finite() && s > 0.0 { Ok(s) } else { Err("the scale is not a positive number".into()) }
}

static SCALE_NOTE: Mutex<Option<String>> = Mutex::new(None);

fn note_scale(name: &str, mult: f64, live: bool) {
    let src = if live { "from the host" } else { "from the drawing's saved header" };
    let warn = if name.eq_ignore_ascii_case("1:1") { " — if the status bar shows another scale, pick \"Custom value\"" } else { "" };
    *SCALE_NOTE.lock().unwrap_or_else(|e| e.into_inner()) = Some(format!("annotation scale {name} (× {mult}) read {src}{warn}"));
}

fn start_legend(host: &mut dyn HostApi, mut req: Request) {
    match resolve_scale(host, &req) {
        Ok(s) => req.params.scale = s,
        Err(e) => return host.push_error(&format!("LN_LEGEND: {e}")),
    }
    if req.params.text_layer.is_empty() {
        // Blank = current layer; the Text constructor already uses the host's current layer.
    } else if !host.document().layers.iter().any(|l| l.name.eq_ignore_ascii_case(&req.params.text_layer)) {
        let name = req.params.text_layer.clone();
        host.add_layer(LayerConfig { name, ..Default::default() });
    }
    if !req.by_layers && !host.document().layers.iter().any(|l| l.name.eq_ignore_ascii_case(legend::FRAME_LAYER)) {
        host.add_layer(LayerConfig { name: legend::FRAME_LAYER.to_string(), ..Default::default() });
    }
    if req.by_layers {
        let items = legend::items_from_layers(host.document(), &req);
        return begin_pick(host, req, items);
    }
    let pre = host.selection();
    if pre.is_empty() {
        host.push_info("LN_LEGEND: select the lines, polylines, blocks and hatches for the legend, then press Enter.");
        host.start_interactive(Box::new(SelectPicker { req, handles: Vec::new() }));
    } else {
        let items = legend::items_from_selection(host.document(), &pre, req.params.include_desc);
        begin_pick(host, req, items);
    }
}

/// `LN_LEGENDQ [file.json]`: the legend from saved settings, no window.
fn legend_quick(host: &mut dyn HostApi, cmd: &str) {
    let arg = cmd.trim().split_once(char::is_whitespace).map(|(_, r)| r.trim().trim_matches('"').to_string()).filter(|a| !a.is_empty());
    let path = arg.unwrap_or_else(legend::current_settings_path);
    let req = match read_text(&path).and_then(|t| legend::from_json(&t)) {
        Ok(r) => r,
        Err(e) => {
            return host.push_error(&format!(
                "LN_LEGENDQ: {e}. Save settings first in the LEGEND tab (LN_LEGEND) — settings file: \"{path}\"."
            ))
        }
    };
    host.push_info(&format!("LN_LEGENDQ: settings from \"{path}\"."));
    start_legend(host, req);
}

fn begin_pick(host: &mut dyn HostApi, req: Request, items: Vec<legend::Item>) {
    if items.is_empty() {
        return host.push_error("LN_LEGEND: nothing to list (no matching layers, or no lines/polylines, blocks or hatches selected).");
    }
    let layout = if req.by_layers { Layout::LowerLeft } else { Layout::TopLeft };
    host.push_info(&format!(
        "LN_LEGEND: {} entr{} at scale {}.",
        items.len(), if items.len() == 1 { "y" } else { "ies" }, req.params.scale
    ));
    if let Some(n) = SCALE_NOTE.lock().unwrap_or_else(|e| e.into_inner()).take() {
        host.push_output(&format!("  {n}"));
    }
    host.start_interactive(Box::new(PointPicker { items, params: req.params, layout }));
}

/// Gathers the selection; Enter hands the handles back through `LN_APPLY`.
struct SelectPicker {
    req: Request,
    handles: Vec<codec::Handle>,
}

impl ocs_plugin_api::host::InteractiveCommand for SelectPicker {
    fn prompt(&self) -> String {
        "Legend: select lines, polylines, blocks, hatches (Enter to finish)".into()
    }
    fn on_point(&mut self, _pt: [f64; 3]) -> ocs_plugin_api::host::CommandStep {
        ocs_plugin_api::host::CommandStep::NeedPoint
    }
    fn is_selection_gathering(&self) -> bool {
        true
    }
    fn accepts_drag_selection(&self) -> bool {
        true
    }
    fn on_selection_complete(&mut self, handles: Vec<codec::Handle>) -> ocs_plugin_api::host::CommandStep {
        self.handles = handles;
        ocs_plugin_api::host::CommandStep::NeedPoint
    }
    fn on_enter(&mut self) -> ocs_plugin_api::host::CommandStep {
        use ocs_plugin_api::host::CommandStep;
        if self.handles.is_empty() {
            return CommandStep::Cancel;
        }
        set_job(Job::LegendSelected(self.req.clone(), std::mem::take(&mut self.handles)));
        CommandStep::Dispatch("LN_APPLY".into())
    }
}

/// Waits for the legend's anchor point, then commits the whole legend.
struct PointPicker {
    items: Vec<legend::Item>,
    params: legend::Params,
    layout: Layout,
}

impl ocs_plugin_api::host::InteractiveCommand for PointPicker {
    fn prompt(&self) -> String {
        match self.layout {
            Layout::LowerLeft => "Legend: pick the LOWER-LEFT point".into(),
            Layout::TopLeft => "Legend: pick the TOP-LEFT point".into(),
        }
    }
    fn on_point(&mut self, pt: [f64; 3]) -> ocs_plugin_api::host::CommandStep {
        ocs_plugin_api::host::CommandStep::CommitManyAndEnd(legend::build(&self.items, &self.params, self.layout, pt))
    }
}
