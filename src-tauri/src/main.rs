// Hide the extra console window on Windows release builds. Do not remove.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    lumanote_shell_lib::run();
}
