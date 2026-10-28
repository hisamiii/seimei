//! The plugins built into the seimei binary, each from its crate under plugins/ (the binary and its tests share it).

use std::sync::Arc;

use seimei_core::plugins::{Factory, Plugins};
use seimei_plugin_browser::Browser;
use seimei_plugin_desktop::DesktopPlugin;

/// In the order setup offers them.
pub const BUILTINS: [Factory; 4] = [
    seimei_plugin_desktop::PLUGIN,
    seimei_plugin_browser::PLUGIN,
    seimei_plugin_music::PLUGIN,
    seimei_plugin_queue::PLUGIN,
];

/// What the built-ins need of each other: the browser's agent browser starts in the desktop's agent display.
pub fn link(plugins: &Plugins) {
    if let (Some(b), Some(d)) = (plugins.get::<Browser>("browser"), plugins.get::<DesktopPlugin>("desktop")) {
        let d: Arc<_> = d.0.clone();
        b.set_starter(Box::new(move |sock| {
            let d = d.clone();
            Box::pin(async move { d.agent_browser(&sock).await })
        }));
    }
}
