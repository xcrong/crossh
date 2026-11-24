fn main() {
    // updater 也是独立进程：先初始化日志，保证失败与 panic 能落盘 run.log。
    crossh_core::logging::init();

    if let Err(error) = crossh_update::run_from_args(std::env::args_os().skip(1)) {
        eprintln!("crossh updater failed: {error}");
        std::process::exit(1);
    }
}
