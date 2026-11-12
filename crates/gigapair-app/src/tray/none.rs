//! Platforms without a tray implementation: closing the window quits.

use super::{ActionSender, TrayView};

pub struct Tray;

impl Tray {
    pub fn new(_actions: ActionSender) -> Result<Self, String> {
        Err("no tray support on this platform".into())
    }

    pub fn set(&mut self, _view: &TrayView) {}
}
