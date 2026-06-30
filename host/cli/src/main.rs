//! terrane-host — the CLI host.
//!
//! A superset of the `terrane` binary: every standard command works (delegated
//! to the shared host CLI adapter), plus a top-level `run <app> [input…]` that
//! executes an app backend via its cataloged runtime. It is the first
//! concrete "host" — the same spine a native shell will wrap, minus the UI.

use std::env;
use std::process::ExitCode;
use std::time::Duration;

use terrane_host::InvokeFailure;

const DEFAULT_ADMIN_BASE_URL: &str = terrane_host::permission::DEFAULT_ADMIN_BASE_URL;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    match run(&argv) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("terrane-host: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn run(argv: &[&str]) -> Result<(), String> {
    match argv {
        // Top-level run: `terrane-host run [permission flags] <app> [input…]`.
        ["run", args @ ..] => run_app(args),
        [] | ["help"] | ["--help"] | ["-h"] => {
            print_host_help();
            terrane_host::cli::print_help();
            Ok(())
        }
        // Everything else is a standard terrane command.
        _ => terrane_host::cli::run(argv),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PermissionUi {
    Web,
    Mac,
    Print,
    None,
}

#[derive(Debug, Clone)]
struct RunOptions {
    permission_ui: PermissionUi,
    permission_wait: bool,
    permission_timeout: Duration,
}

fn run_app(args: &[&str]) -> Result<(), String> {
    let (options, app, input) = parse_run_args(args)?;
    let mut core = terrane_host::open()?;
    match terrane_host::invoke_app_input_checked_with_admin_base_and_source(
        &mut core,
        app,
        &input,
        DEFAULT_ADMIN_BASE_URL,
        "cli",
    ) {
        Ok(output) => {
            println!("{output}");
            Ok(())
        }
        Err(InvokeFailure::PermissionRequired(required)) => {
            let required = *required;
            print_permission_required(&required, options.permission_ui);
            if !options.permission_wait {
                return Err(format!(
                    "permission_required: request {} is {}",
                    required.request_id, required.request_status
                ));
            }
            let view = terrane_host::permission::wait_for_permission_decision_at_home(
                terrane_host::home_dir(),
                &required.request_id,
                DEFAULT_ADMIN_BASE_URL,
                options.permission_timeout,
            )?;
            match view.as_ref().map(|view| view.status.as_str()) {
                Some("approved") => {
                    let mut core = terrane_host::open()?;
                    let output = terrane_host::invoke_app_input_checked_with_admin_base_and_source(
                        &mut core,
                        app,
                        &input,
                        DEFAULT_ADMIN_BASE_URL,
                        "cli",
                    )
                    .map_err(|e| e.message())?;
                    println!("{output}");
                    Ok(())
                }
                Some("pending") => Err(format!(
                    "permission_required: timed out waiting for request {}",
                    required.request_id
                )),
                Some(status) => Err(format!(
                    "permission_required: request {} resolved as {}",
                    required.request_id, status
                )),
                None => Err(format!(
                    "permission_required: request {} was not found before timeout",
                    required.request_id
                )),
            }
        }
        Err(InvokeFailure::Other(message)) => Err(message),
    }
}

fn parse_run_args<'a>(args: &'a [&'a str]) -> Result<(RunOptions, &'a str, Vec<String>), String> {
    let mut options = RunOptions {
        permission_ui: permission_ui_from_env()?,
        permission_wait: false,
        permission_timeout: Duration::from_secs(300),
    };
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "--permission-ui" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err("usage: --permission-ui web|mac|print|none".into());
                };
                options.permission_ui = parse_permission_ui(value)?;
            }
            "--permission-wait" => options.permission_wait = true,
            "--permission-timeout" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err("usage: --permission-timeout <seconds>".into());
                };
                let seconds = value
                    .parse::<u64>()
                    .map_err(|_| format!("permission timeout must be seconds, got {value:?}"))?;
                options.permission_timeout = Duration::from_secs(seconds);
            }
            "--no-open" => options.permission_ui = PermissionUi::Print,
            "--" => {
                index += 1;
                break;
            }
            arg if arg.starts_with("--") => {
                return Err(format!("unknown run option: {arg}"));
            }
            _ => break,
        }
        index += 1;
    }
    let Some(app) = args.get(index) else {
        return Err("usage: terrane-host run [--permission-ui web|mac|print|none] [--permission-wait] [--permission-timeout seconds] <app> [input…]".into());
    };
    let input = args[index + 1..]
        .iter()
        .map(|arg| (*arg).to_string())
        .collect();
    Ok((options, app, input))
}

fn permission_ui_from_env() -> Result<PermissionUi, String> {
    match env::var("TERRANE_PERMISSION_UI") {
        Ok(value) if !value.trim().is_empty() => parse_permission_ui(&value),
        _ => Ok(PermissionUi::Web),
    }
}

fn parse_permission_ui(raw: &str) -> Result<PermissionUi, String> {
    match raw.trim() {
        "web" => Ok(PermissionUi::Web),
        "mac" => Ok(PermissionUi::Mac),
        "print" => Ok(PermissionUi::Print),
        "none" => Ok(PermissionUi::None),
        other => Err(format!(
            "permission UI must be web, mac, print, or none, got {other:?}"
        )),
    }
}

fn print_permission_required(
    required: &terrane_host::permission::PermissionRequired,
    ui: PermissionUi,
) {
    eprintln!("permission required");
    eprintln!("  request id: {}", required.request_id);
    eprintln!("  app: {} ({})", required.app_name, required.app);
    eprintln!("  source: {}", required.source);
    eprintln!("  resources: {}", required.missing_resources.join(", "));
    match ui {
        PermissionUi::Web => eprintln!("  open: {}", required.admin_url),
        PermissionUi::Mac => eprintln!(
            "  mac permission UI is not available; open: {}",
            required.admin_url
        ),
        PermissionUi::Print => eprintln!("  admin url: {}", required.admin_url),
        PermissionUi::None => eprintln!("  permission UI disabled"),
    }
}

fn print_host_help() {
    println!(
        "terrane-host — the terrane CLI plus the app runtime entry point\n\n\
         \x20 terrane-host run [--permission-ui web|mac|print|none] [--permission-wait] [--permission-timeout seconds] <app> [input…]\n\
         \x20                                     run an app backend\n\n\
         All standard terrane commands also work:\n"
    );
}
