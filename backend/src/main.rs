#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    zkmove_mint_gui::gui::run();
}
