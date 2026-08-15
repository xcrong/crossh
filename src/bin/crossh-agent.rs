//! Standalone interactive terminal agent entry point.
//!
//! This binary owns only the TUI presentation (`agent_cli`); agent logic,
//! sessions, and protocol work live in the `crossh-agent` crate. It shares
//! `settings.toml` with the GUI through `crossh_agent::load_agent_settings`.

#[path = "../agent_cli.rs"]
mod agent_cli;

fn main() {
    let options = match agent_cli::parse_options(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(error) if error == "help" => {
            agent_cli::print_help();
            return;
        }
        Err(error) => {
            eprintln!("crossh-agent: {error}\n");
            agent_cli::print_help();
            std::process::exit(2);
        }
    };
    if let Err(error) = agent_cli::run_with_options(crossh_agent::load_agent_settings(), options) {
        eprintln!("crossh-agent: {error}");
        std::process::exit(1);
    }
}
