//! Layer Names — an external Open CAD Studio add-on for layer housekeeping.
//!
//! * **Import** — read / write LMD layer maps (`[Group]` + `NAME,color,linetype,description`);
//! * **Rename** — bulk layer rename: prefix add/replace, suffix add/replace, string replace;
//! * **Filter** — rapid layer-filter creation, export/import of filters as CSV.
//!
//! `lmd`, `rename`, `filters`, `wildcard` and `csvutil` are host-free and unit-tested;
//! `dialog` (the tabbed form) and `commands` (the apply step) are the glue to the host.

use ocs_plugin_api::host::{BuiltinPlugin, HostApi};
use ocs_plugin_api::manifest::{ApiVersion, PluginManifest};
use ocs_plugin_api::ribbon::CadModule;

pub mod commands;
pub mod csvutil;
pub mod dialog;
pub mod filters;
pub mod host;
pub mod legend;
pub mod lmd;
pub mod rename;
mod ribbon;
pub mod wildcard;

/// Reverse-DNS plugin id; also the folder name under `plugins/`.
pub const PLUGIN_ID: &str = "opencad.layname";

// Keep in sync with plugin.toml (the version comes from Cargo.toml).
static MANIFEST: PluginManifest = PluginManifest {
    id: PLUGIN_ID,
    name: "Layer Names",
    version: env!("CARGO_PKG_VERSION"),
    description: "Layer maps (LMD import/export), bulk layer rename (prefix, suffix, string) and layer \
                  filters (quick create, CSV export/import).",
    api_version: ApiVersion::CURRENT,
    ribbon_order: 61,
    xdata_apps: &[],
    command_prefixes: &["LN_"],
};

struct LaynamePlugin;

impl BuiltinPlugin for LaynamePlugin {
    fn manifest(&self) -> &'static PluginManifest {
        &MANIFEST
    }

    fn ribbon(&self) -> Box<dyn CadModule> {
        Box::new(ribbon::LaynameModule)
    }

    fn dispatch(&self, host: &mut dyn HostApi, cmd: &str) -> bool {
        commands::handle(host, cmd)
    }
}

ocs_plugin_api::export_plugin!(LaynamePlugin);
