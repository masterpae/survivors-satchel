//! Survivor's Satchel's core: everything that decides about and moves saves and addons.
//! No UI and no Windows API here, so all of it is tested on any OS.

pub mod activity;
pub mod addons;
pub mod changelog;
pub mod decide;
pub mod engine;
pub mod fsx;
pub mod game_settings;
pub mod gate;
pub mod index;
pub mod log;
pub mod markers;
pub mod names;
pub mod paths;
pub mod playtime;
pub mod savefile;
pub mod session;
pub mod settings;
pub mod state;
