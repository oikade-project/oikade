//! Supervised host for language-neutral Oikade plugins.

mod convert;
mod instance;
mod manifest;
mod session;

pub use instance::{Instance, InstanceSpec, Inventory, PluginStatus};
pub use manifest::{MANIFEST_FILENAME, Manifest, load_manifest};
