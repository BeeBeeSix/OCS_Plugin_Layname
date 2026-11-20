//! The seam between the pure logic (`lmd`, `rename`) and Open CAD Studio.
//! `commands.rs` implements it on top of `HostApi`; the unit tests use a mock.

use crate::lmd::LmdLayer;

#[derive(Debug, Clone, PartialEq)]
pub struct LayerInfo {
    pub name: String,
    /// ACI index, or -1 when the layer has a true colour / no index.
    pub color: i32,
    pub linetype: String,
    pub description: String,
}

pub trait LayerHost {
    fn layers(&self) -> Vec<LayerInfo>;
    /// Create a layer. `Ok(true)` = created as asked; `Ok(false)` = created, but the
    /// linetype is not available in the drawing so it fell back to Continuous.
    fn create_layer(&mut self, layer: &LmdLayer) -> Result<bool, String>;
    /// Change colour / linetype / description of an existing layer (same return meaning).
    fn update_layer(&mut self, layer: &LmdLayer) -> Result<bool, String>;
    fn rename_layer(&mut self, old: &str, new: &str) -> Result<(), String>;
}
