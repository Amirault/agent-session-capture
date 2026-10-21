use std::io::Write;

use agent_session_capture::js::string::to_output_text;

fn main() {
    let argv: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let outcome = agent_session_capture::cli::run(&argv);
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in &outcome.stdout {
        let _ = writeln!(out, "{}", to_output_text(line));
    }
    let _ = out.flush();
    let stderr = std::io::stderr();
    let mut err = stderr.lock();
    for line in &outcome.stderr {
        let _ = writeln!(err, "{}", to_output_text(line));
    }
    std::process::exit(outcome.exit_code);
}
