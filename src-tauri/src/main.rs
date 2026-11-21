// Unter Windows im Release kein Konsolenfenster
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    minimal_library_lib::run()
}
