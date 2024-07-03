use std::{
    io::{self, ErrorKind},
    process::{exit, Child, Command},
};

pub mod config;
pub mod commands;

pub trait CommandExt {
    fn spawn_handling_not_found(&mut self) -> io::Result<Child>;
}

impl CommandExt for Command {
    fn spawn_handling_not_found(&mut self) -> io::Result<Child> {
        let command_name = self.get_program().to_string_lossy().to_string();
        self.spawn().map_err(|err| match err.kind() {
            ErrorKind::NotFound => {
                eprintln!("error: command `{}` not found", command_name);
                eprintln!("Please refer to the documentation for installing vexide's dependencies on your platform.");
                eprintln!("> https://github.com/vexide/vexide#compiling");
                exit(1);
            }
            _ => err,
        })
    }
}