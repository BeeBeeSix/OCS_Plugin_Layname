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
use crate::lmd::{self, Existing, LmdFile, LmdLayer};
use crate::rename::{self, RenameOp};

/// Work chosen in the dialog, done by `LN_APPLY`.
pub enum Job {
    Import { file: LmdFile, groups: Vec<usize>, existing: Existing },
    Export { path: String, group: String, pattern: String, append: bool },
    Rename { op: RenameOp, scope: String },
    FilterCreate(Vec<FilterDef>),
    FilterExport(String),
    FilterImport(String),
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
        "All three open the same window; the buttons at its top switch between the tabs.",
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

        Job::Import { file, groups, existing } => {
            host.begin_undo_group("LN_IMPORT");
            let rep = lmd::import(&mut Adapter(host), &file, &groups, existing);
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
