//! Desktop binary — a thin wrapper so the same crate can also be linked as a
//! library by the Tauri mobile runners (see lib.rs).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    solum_app_lib::run()
}
