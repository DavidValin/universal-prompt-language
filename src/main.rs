use std::env;
use std::fs::File;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use crossterm::execute;
use crossterm::cursor::MoveTo;
use crossterm::style::{
    Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use inquire::ui::{Color as InquireColor, RenderConfig, StyleSheet};

use universal_prompt_language::upl::builder::PromptBuilder;
use universal_prompt_language::manager::ui_prompts_list;
use universal_prompt_language::upl::parser::PromptParser;
use universal_prompt_language::repository::protocol::Visibility;
use universal_prompt_language::repository::client;
use universal_prompt_language::repository::server;
use universal_prompt_language::editor::ui_prompt_editor;

fn read_file(path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut file = File::open(path)?;
    let mut content = String::new();
    file.read_to_string(&mut content)?;
    Ok(content)
}

fn usage(prog: &str) {
    let version = env!("CARGO_PKG_VERSION");
    eprintln!("upl {version}");
    eprintln!();
    eprintln!("Usage:");
    eprintln!("  {prog} [build|b] [<folder> | <prompt.upl|txt>]");
    eprintln!("  {prog} [build|b] --no-input <prompt.upl|txt>");
    eprintln!("  {prog} init");
    eprintln!("  {prog} login");
    eprintln!("  {prog} push <prompt.upl|txt> [--visibility public|private]");
    eprintln!("  {prog} pull <username>/<prompt_name>[/<version>]");
    eprintln!("  {prog} del <prompt_name>");
    eprintln!("  {prog} set-rep <host:port> [--tls] [gpg_key_file]");
    eprintln!("  {prog} get-rep");
    eprintln!("  {prog} start_repository <tls_cert> [bind_addr]");
    eprintln!("  {prog} register_user");
    eprintln!("  {prog} delete_user <username>");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  build (alias: b)  Browse a prompt library or build a single prompt file.");
    eprintln!("  init              Create a new UPL prompt from the skeleton in the editor.");
    eprintln!("  login             Log in to the configured repository (stores a session token).");
    eprintln!("  push             Push a local prompt to the repository (uses its `name` as name).");
    eprintln!("  pull             Pull a prompt from the repository into ~/.upl/prompts.");
    eprintln!("  del              Delete all versions of one of your prompts from the repository.");
    eprintln!("  set-rep          Configure the repository endpoint (and optional TLS / GPG key).");
    eprintln!("  get-rep          Show the configured repository endpoint.");
    eprintln!("  start_repository Start the repository TCP/TLS server.");
    eprintln!("  register_user    Create a repository user (local admin, run on the server host).");
    eprintln!("  delete_user      Remove a repository user (local admin, run on the server host).");
    eprintln!();
    eprintln!("Arguments:");
    eprintln!("  <folder>          Load the prompt library from <folder> and pick one via a TUI.");
    eprintln!("  <prompt file>     Build the given .upl/.txt prompt file directly.");
    eprintln!("  (none)            Load the prompt library from ~/.upl/prompts (default).");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --no-input        Skip the interactive prompts and render using the declared `def:` defaults.");
    eprintln!("  --user, -u        Provide a username non-interactively (login, register_user).");
    eprintln!("  --password, -p    Provide a password non-interactively (login, register_user).");
    eprintln!("  --visibility      Visibility for `push`: public|private (default: private).");
    eprintln!("  --tls             Use TLS when connecting to the repository (set-rep).");
    eprintln!("  --gpg-key, -g     Path to a GPG public key file or key id (register_user).");
    eprintln!("  --help, -h        Show this message.");
}

fn print_build_header(title: &str) {
    let mut err = std::io::stderr();
    let _ = execute!(err, EnterAlternateScreen, MoveTo(0, 0));
    let _ = execute!(
        err,
        SetBackgroundColor(Color::DarkGreen),
        SetForegroundColor(Color::White),
        SetAttribute(Attribute::Bold),
        Print(" Building prompt "),
        ResetColor,
        SetAttribute(Attribute::Reset),
        Print(" "),
        SetForegroundColor(Color::Yellow),
        Print(title),
        ResetColor,
        Print("\n\n"),
    );
    let _ = err.flush();
}

fn end_build_header() {
    let mut err = std::io::stderr();
    let _ = execute!(err, LeaveAlternateScreen);
    let _ = err.flush();
}

fn emit_prompt(rendered: &str) {
    let mut out = std::io::stdout();
    let _ = out.write_all(rendered.as_bytes());
    let _ = out.write_all(b"\n");
    let _ = out.flush();
}

/// Build a single prompt file (interactive by default, or with defaults when
/// `no_input` is set).
fn build_prompt_file(path: &str, no_input: bool) -> Result<(), Box<dyn std::error::Error>> {
    let p = Path::new(path);
    if !universal_prompt_language::upl::parser::has_valid_extension(p) {
        return Err(
            "prompt files must use the '.txt' or '.upl' extension".into(),
        );
    }
    let content = read_file(path)?;
    let prompt = PromptParser::parse(&content)?;
    universal_prompt_language::upl::parser::validate_prompt_file(&prompt, p)
        .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?;

    let title = prompt.title.clone().unwrap_or_else(|| path.to_string());
    print_build_header(&title);

    let rendered = (|| -> Result<String, Box<dyn std::error::Error>> {
        if no_input {
            Ok(PromptBuilder::new(prompt).render_with_defaults()?)
        } else {
            Ok(PromptBuilder::new(prompt).build_interactive()?)
        }
    })();
    end_build_header();
    let rendered = rendered?;
    emit_prompt(&rendered);
    Ok(())
}

/// Load a prompt library (TUI picker) and build the selected prompt.
fn build_library(folder: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    ui_prompts_list::run(folder)?;
    Ok(())
}

fn run_init() -> Result<(), Box<dyn std::error::Error>> {
    // Open the editor with the skeleton template. The editor handles saving
    // the new prompt to ~/.upl/prompts/<name>.txt on Ctrl+S.
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return Err("init must be run in a TTY".into());
    }
    ui_prompt_editor::run_editor_standalone(ui_prompt_editor::SKELETON)?;
    // After the editor closes (saved or quit), drop into the prompt browser
    // so the user lands on their prompt list rather than the shell.
    build_library(None)
}

fn run_build(args: &[String], prog: &str) -> Result<(), Box<dyn std::error::Error>> {
    if args.is_empty() {
        return build_library(None);
    }

    match args[0].as_str() {
        "--help" | "-h" => {
            usage(prog);
            Ok(())
        }
        "--no-input" => {
            if args.len() < 2 {
                usage(prog);
                return Err("missing prompt file after --no-input".into());
            }
            build_prompt_file(&args[1], true)
        }
        other => {
            let path = Path::new(other);
            if path.is_dir() {
                build_library(Some(other))
            } else if path.is_file() {
                build_prompt_file(other, false)
            } else {
                usage(prog);
                Err(format!("not a file or folder: {other}").into())
            }
        }
    }
}

fn run_login(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    // `upl login [--user <name>] [--password <pw>]`
    // Without flags, prompts interactively via inquire.
    let mut username: Option<String> = None;
    let mut password: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--user" | "-u" => {
                username = args.get(i + 1).map(|s| s.clone());
                if username.is_none() {
                    return Err("missing value after --user".into());
                }
                i += 2;
            }
            "--password" | "-p" => {
                password = args.get(i + 1).map(|s| s.clone());
                if password.is_none() {
                    return Err("missing value after --password".into());
                }
                i += 2;
            }
            other => return Err(format!("unexpected argument: {other}").into()),
        }
    }
    let username = match username {
        Some(u) => u,
        None => {
            if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                return Err("username required (pass --user, or run in a TTY)".into());
            }
            use inquire::Text;
            Text::new("username:").prompt()?
        }
    };
    let password = match password {
        Some(p) => p,
        None => {
            if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                return Err("password required (pass --password, or run in a TTY)".into());
            }
            use inquire::Password;
            Password::new("password:").without_confirmation().prompt()?
        }
    };
    client::login(&username, &password)
}

fn run_push(args: &[String], prog: &str) -> Result<(), Box<dyn std::error::Error>> {
    if args.is_empty() {
        usage(prog);
        return Err("usage: upl push <prompt.upl|txt> [--visibility public|private]".into());
    }
    let mut file: Option<&str> = None;
    let mut visibility = Visibility::Private;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--visibility" | "-v" => {
                if i + 1 >= args.len() {
                    return Err("missing value after --visibility".into());
                }
                visibility = Visibility::parse(&args[i + 1])?;
                i += 2;
            }
            "--public" => {
                visibility = Visibility::Public;
                i += 1;
            }
            "--private" => {
                visibility = Visibility::Private;
                i += 1;
            }
            other => {
                if file.is_none() {
                    file = Some(other);
                } else {
                    return Err(format!("unexpected argument: {other}").into());
                }
                i += 1;
            }
        }
    }
    let file = file.ok_or("missing prompt file")?;
    client::push(file, visibility)
}

fn run_pull(args: &[String], prog: &str) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() != 1 {
        usage(prog);
        return Err("usage: upl pull <username>/<prompt_name>[/<version>]".into());
    }
    client::pull(&args[0])
}

fn run_del(args: &[String], prog: &str) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() != 1 {
        usage(prog);
        return Err("usage: upl del <prompt_name>".into());
    }
    client::del(&args[0])
}

fn run_set_rep(args: &[String], prog: &str) -> Result<(), Box<dyn std::error::Error>> {
    if args.is_empty() {
        usage(prog);
        return Err("usage: upl set-rep <host:port> [--tls] [gpg_key_file]".into());
    }
    let mut host: Option<String> = None;
    let mut tls = false;
    let mut gpg_key_file: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--tls" => {
                tls = true;
                i += 1;
            }
            other => {
                if host.is_none() {
                    host = Some(other.to_string());
                } else if gpg_key_file.is_none() {
                    gpg_key_file = Some(other.to_string());
                } else {
                    return Err(format!("unexpected argument: {other}").into());
                }
                i += 1;
            }
        }
    }
    let host = host.ok_or("missing host:port")?;
    client::set_rep(&host, tls, gpg_key_file.as_deref())
}

fn run_get_rep() -> Result<(), Box<dyn std::error::Error>> {
    client::get_rep()
}

fn run_start_repository(args: &[String], prog: &str) -> Result<(), Box<dyn std::error::Error>> {
    // `upl start_repository [tls_cert] [bind_addr]`
    // No tls_cert -> plain TCP mode.
    let mut tls_cert: Option<&str> = None;
    let mut bind: Option<&str> = None;
    for a in args {
        // Skip flags; treat first non-flag as the cert, second as bind.
        if tls_cert.is_none() {
            tls_cert = Some(a);
        } else if bind.is_none() {
            bind = Some(a);
        } else {
            usage(prog);
            return Err(format!("unexpected argument: {a}").into());
        }
    }
    if tls_cert.is_none() && bind.is_some() {
        // `start_repository <bind>` with no cert: interpret the arg as bind
        // address for plain TCP mode.
        bind = tls_cert.take();
    }
    server::start_repository(tls_cert, bind)
}

fn run_register_user(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    // `upl register_user [--user <name>] [--password <pw>] [--gpg-key <file|id>]`
    // Without flags, prompts interactively via inquire.
    let mut username: Option<String> = None;
    let mut password: Option<String> = None;
    let mut gpg_key_file: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--user" | "-u" => {
                username = args.get(i + 1).map(|s| s.clone());
                if username.is_none() {
                    return Err("missing value after --user".into());
                }
                i += 2;
            }
            "--password" | "-p" => {
                password = args.get(i + 1).map(|s| s.clone());
                if password.is_none() {
                    return Err("missing value after --password".into());
                }
                i += 2;
            }
            "--gpg-key" | "-g" => {
                gpg_key_file = args.get(i + 1).map(|s| s.clone());
                if gpg_key_file.is_none() {
                    return Err("missing value after --gpg-key".into());
                }
                i += 2;
            }
            other => return Err(format!("unexpected argument: {other}").into()),
        }
    }
    let username = match username {
        Some(u) => u,
        None => {
            if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                return Err("username required (pass --user, or run in a TTY)".into());
            }
            use inquire::Text;
            Text::new("username:").prompt()?
        }
    };
    let password = match password {
        Some(p) => p,
        None => {
            if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                return Err("password required (pass --password, or run in a TTY)".into());
            }
            use inquire::Password;
            let pw = Password::new("password:")
                .with_help_message("stored as argon2id hash")
                .prompt()?;
            let pw2 = Password::new("confirm password:").prompt()?;
            if pw != pw2 {
                return Err("passwords do not match".into());
            }
            pw
        }
    };
    let gpg_key_file = match gpg_key_file {
        Some(g) => Some(g),
        None => {
            // Only prompt for a GPG key interactively when stdin is a TTY.
            // In non-interactive contexts, default to no GPG key.
            if std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                use inquire::{Confirm, Text};
                if Confirm::new("Register a GPG key for this user?")
                    .with_default(false)
                    .prompt()?
                {
                    Some(Text::new("path to GPG public key file (or Key id/fingerprint):").prompt()?)
                } else {
                    None
                }
            } else {
                None
            }
        }
    };
    server::register_user(&username, &password, gpg_key_file.as_deref())
}

fn run_delete_user(args: &[String], prog: &str) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() != 1 {
        usage(prog);
        return Err("usage: upl delete_user <username>".into());
    }
    server::delete_user(&args[0])
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let prog = args[0].as_str();

    // Style inquire prompts: field names in light green, descriptions in
    // white, default values in grey, user-entered text in yellow, final
    // answers in yellow too.
    let mut cfg = RenderConfig::default_colored();
    cfg.prompt = StyleSheet::new().with_fg(InquireColor::LightGreen);
    cfg.help_message = StyleSheet::new().with_fg(InquireColor::Grey);
    cfg.default_value = StyleSheet::new().with_fg(InquireColor::Grey);
    cfg.text_input = StyleSheet::new().with_fg(InquireColor::LightYellow);
    cfg.answer = StyleSheet::new().with_fg(InquireColor::LightYellow);
    inquire::set_global_render_config(cfg);

    // First-run setup: seed ~/.upl with the bundled sample prompts and
    // tags_db if it does not exist yet.
    universal_prompt_language::seed::ensure()?;

    // No command at all: behave like `build` with no arguments (load the
    // default prompt library from ~/.upl/prompts).
    if args.len() < 2 {
        return run_build(&[], prog);
    }

    match args[1].as_str() {
        "build" | "b" => run_build(&args[2..], prog),
        "init" => run_init(),
        "login" => run_login(&args[2..]),
        "push" => run_push(&args[2..], prog),
        "pull" => run_pull(&args[2..], prog),
        "del" => run_del(&args[2..], prog),
        "set-rep" => run_set_rep(&args[2..], prog),
        "get-rep" => run_get_rep(),
        "start_repository" => run_start_repository(&args[2..], prog),
        "register_user" => run_register_user(&args[2..]),
        "delete_user" => run_delete_user(&args[2..], prog),
        "--help" | "-h" => {
            usage(prog);
            Ok(())
        }
        other => {
            usage(prog);
            Err(format!("unknown command: {other}").into())
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::from(1)
        }
    }
}
