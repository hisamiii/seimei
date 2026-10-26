//! The plugins built into the seimei binary, each from its crate under plugins/ (the binary and its tests share it).

use seimei_core::plugins::Factory;

pub const BUILTINS: [Factory; 3] =
    [seimei_plugin_music::PLUGIN, seimei_plugin_queue::PLUGIN, seimei_plugin_discord::PLUGIN];
