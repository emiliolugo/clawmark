mod cli;
mod doctor;
mod report;
mod results;
mod runner;
mod sandbox;
mod swebench;

use clap::Parser;
use tokio::runtime::Builder;

use crate::cli::{Cli, Commands};

fn main() {
    let cli = Cli::parse();
    let exit_code = match cli.command {
        Commands::Doctor => doctor::run_doctor(),
        Commands::Run(args) => match args.validate() {
            Ok(validated) => {
                let rt = match Builder::new_multi_thread()
                    .build()
                    .map_err(|e| format!("failed to build tokio runtime: {e}"))
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        eprintln!("error: {e}");
                        std::process::exit(1);
                    }
                };
                match rt.block_on(runner::run_ab(&validated)) {
                    Ok(()) => 0,
                    Err(message) => {
                        eprintln!("error: {message}");
                        1
                    }
                }
            }
            Err(message) => {
                eprintln!("error: {message}");
                1
            }
        },
        Commands::Report(args) => match args.validate() {
            Ok(()) => match report::compute_report(&args.out) {
                Ok(computed) => {
                    report::render_terminal_table(&computed);
                    if args.show_patches {
                        if let Err(message) = report::render_patches(&args.out) {
                            eprintln!("warning: could not render patches: {message}");
                        }
                    }
                    if let Err(message) = report::write_report_json(&args.out, &computed) {
                        eprintln!("error: {message}");
                        1
                    } else {
                        0
                    }
                }
                Err(message) => {
                    eprintln!("error: {message}");
                    1
                }
            },
            Err(message) => {
                eprintln!("error: {message}");
                1
            }
        },
    };
    std::process::exit(exit_code);
}
