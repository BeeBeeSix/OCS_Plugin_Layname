//! The "Layer Names" ribbon tab. Pure data — the host renders it.

use ocs_plugin_api::ribbon::{CadModule, IconKind, ModuleEvent, RibbonGroup, RibbonItem, ToolDef};

pub struct LaynameModule;

fn cmd(id: &'static str, label: &'static str, svg: &'static [u8]) -> RibbonItem {
    RibbonItem::LargeTool(ToolDef {
        id,
        label,
        icon: IconKind::Svg(svg),
        event: ModuleEvent::Command(id.to_string()),
    })
}

impl CadModule for LaynameModule {
    fn id(&self) -> &'static str {
        "layname"
    }

    fn title(&self) -> &'static str {
        "Layer Names"
    }

    fn ribbon_groups(&self) -> &[RibbonGroup] {
        static GROUPS: std::sync::OnceLock<Vec<RibbonGroup>> = std::sync::OnceLock::new();
        GROUPS.get_or_init(|| {
            vec![
                RibbonGroup {
                    title: "Layers",
                    tools: vec![
                        cmd("LN_IMPORT", "Import / Export", include_bytes!("../icons/import.svg")),
                        cmd("LN_RENAME", "Rename", include_bytes!("../icons/rename.svg")),
                        cmd("LN_FILTER", "Filters", include_bytes!("../icons/filter.svg")),
                        cmd("LN_LEGEND", "Legend", include_bytes!("../icons/legend.svg")),
                        cmd("LN_LEGENDQ", "Legend (saved)", include_bytes!("../icons/legend_quick.svg")),
                    ],
                },
                RibbonGroup {
                    title: "Help",
                    tools: vec![cmd("LN_HELP", "Help", include_bytes!("../icons/help.svg"))],
                },
            ]
        })
    }
}
