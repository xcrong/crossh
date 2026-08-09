fn main() {
    if let Err(error) = crossh_update::run_from_args(std::env::args_os().skip(1)) {
        eprintln!("crossh updater failed: {error}");
        std::process::exit(1);
    }
}
