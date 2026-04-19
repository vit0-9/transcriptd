use std::io;

use clap::CommandFactory;

pub fn cmd_completion(shell: clap_complete::Shell) {
    let mut app = crate::cli::Cli::command();
    let name = app.get_name().to_string();
    clap_complete::generate(shell, &mut app, name, &mut io::stdout());
}
