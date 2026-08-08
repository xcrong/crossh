#![allow(dead_code)]

#[path = "../infrastructure/update/installer.rs"]
mod installer;
#[path = "../infrastructure/update/model.rs"]
mod model;

fn main() {
    if let Err(error) = installer::run_from_args(std::env::args_os().skip(1)) {
        eprintln!("crossh updater failed: {error}");
        std::process::exit(1);
    }
}
