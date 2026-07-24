use std::env;
use std::process::ExitCode;

use herdr_workbench::app;
use herdr_workbench::controller::{Action, Controller};
use herdr_workbench::herdr::LiveHerdr;
use herdr_workbench::preview;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("herdr-workbench: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let command = env::args().nth(1).ok_or(
        "Herdr Workbench is a companion plugin and does not launch standalone; invoke its toggle action from Herdr",
    )?;
    match command.as_str() {
        "sidebar" => app::run(),
        "preview" => preview::run(),
        "show" => {
            Controller::from_env(LiveHerdr::from_env())?.execute(Action::Show)?;
            Ok(())
        }
        "hide" => {
            Controller::from_env(LiveHerdr::from_env())?.execute(Action::Hide)?;
            Ok(())
        }
        "toggle" => {
            Controller::from_env(LiveHerdr::from_env())?.execute(Action::Toggle)?;
            Ok(())
        }
        "focus" => {
            Controller::from_env(LiveHerdr::from_env())?.execute(Action::Focus)?;
            Ok(())
        }
        "restore" => {
            Controller::from_env(LiveHerdr::from_env())?.restore()?;
            Ok(())
        }
        "validate-manifest" => herdr_workbench::config::validate_manifest(),
        other => Err(format!(
            "unknown command {other:?}; expected sidebar, preview, show, hide, toggle, focus, restore, or validate-manifest"
        )
        .into()),
    }
}
