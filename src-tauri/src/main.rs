// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // rsync's remote shell during a folder sync: no window, no Tauri.
    if std::env::args_os().nth(1).is_some_and(|a| a == "--rsync-relay") {
        std::process::exit(kade_lib::rsync::relay::main());
    }
    kade_lib::run()
}
