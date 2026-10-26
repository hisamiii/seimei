//! The plugins built into the seimei binary, each from its crate under plugins/ (the binary and its tests share it).

use seimei_core::plugins::{FACTORIES, Factory};

pub const BUILTINS: [Factory; 3] = [seimei_plugin_music::PLUGIN, FACTORIES[0], seimei_plugin_discord::PLUGIN];
