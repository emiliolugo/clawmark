mod cli;
mod doctor;
mod report;
mod results;
mod runner;
mod sandbox;
mod swebench;

use clap::Parser;

use crate::cli::{Cli, Commands};

fn main() {
    let cli = Cli::parse();
    let exit_code = match cli.command {
        Commands::Doctor => doctor::run_doctor(),
        Commands::Run(args) => match args.validate() {
            Ok(validated) => match runner::run_ab(&validated) {
                Ok(()) => 0,
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
        Commands::Report(args) => match args.validate() {
            Ok(()) => match report::compute_report(&args.out) {
                Ok(report) => {
                    report::render_terminal_table(&report);
                    if let Err(message) = report::write_report_json(&args.out, &report) {
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
