use std::env;
use std::ffi::{OsStr, OsString};
use std::io::{self, BufReader, BufWriter, IsTerminal, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use ll::{build_args, process, Flavor};

const HELP: &str = "\
ll - ls -l with thousands separators in the size column

Usage: ll [LS OPTIONS] [FILE]...

Runs `ls -l` (plus LL_OPTS, plus your arguments) and inserts ',' thousands
separators into the size column. All arguments are passed to ls.

Environment:
  LL_LS         ls program to run (default: ls; on macOS uu-ls, uls, gls, then ls)
  LL_OPTS       extra whitespace-separated options placed after -l
  LL_LS_FLAVOR  force ls flavor detection: gnu, uutils or bsd

  --ll-help     show this help
  --ll-version  show version
";

fn find_in_path(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = env::var_os("PATH")?;
    env::split_paths(&path).map(|d| d.join(name)).find(|p| {
        p.metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    })
}

fn choose_ls() -> OsString {
    if let Some(v) = env::var_os("LL_LS") {
        if !v.is_empty() {
            return v;
        }
    }
    if cfg!(target_os = "macos") {
        for name in ["uu-ls", "uls", "gls", "ls"] {
            if let Some(p) = find_in_path(name) {
                return p.into_os_string();
            }
        }
    }
    OsString::from("ls")
}

fn detect_flavor(ls: &OsStr) -> Flavor {
    if let Some(v) = env::var("LL_LS_FLAVOR")
        .ok()
        .and_then(|v| Flavor::from_name(&v))
    {
        return v;
    }
    let out = Command::new(ls)
        .arg("--version")
        .stdin(Stdio::null())
        .output();
    match out {
        Ok(o) => {
            let mut all = o.stdout;
            all.extend_from_slice(&o.stderr);
            Flavor::from_version_output(&all)
        }
        Err(_) => Flavor::Bsd,
    }
}

fn split_opts(s: &OsStr) -> Vec<Vec<u8>> {
    s.as_bytes()
        .split(|b| b.is_ascii_whitespace())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_vec())
        .collect()
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    match status.code() {
        Some(c) => c,
        None => 128 + status.signal().unwrap_or(0),
    }
}

fn main() {
    let user: Vec<Vec<u8>> = env::args_os()
        .skip(1)
        .map(|a| a.as_bytes().to_vec())
        .collect();
    match user.first().map(|a| a.as_slice()) {
        Some(b"--ll-version") => {
            println!("ll {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Some(b"--ll-help") => {
            print!("{HELP}");
            return;
        }
        _ => {}
    }

    let ls = choose_ls();
    let flavor = detect_flavor(&ls);
    let opts = env::var_os("LL_OPTS")
        .map(|v| split_opts(&v))
        .unwrap_or_default();
    let tty = io::stdout().is_terminal();
    let (child_args, analysis) = build_args(
        flavor,
        &opts,
        &user,
        tty,
        env::var_os("QUOTING_STYLE").is_some(),
    );

    let mut cmd = Command::new(&ls);
    cmd.args(child_args.into_iter().map(OsString::from_vec))
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit())
        .stdout(Stdio::piped());
    if tty && flavor.is_bsd() && env::var_os("CLICOLOR").is_some() {
        cmd.env("CLICOLOR_FORCE", "1");
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("ll: cannot run '{}': {}", ls.to_string_lossy(), e);
            std::process::exit(127);
        }
    };

    let stdout = child.stdout.take().expect("piped stdout");
    let reader = BufReader::new(stdout);
    let so = io::stdout();
    let mut writer = BufWriter::new(so.lock());
    let result = process(reader, &mut writer, &analysis.layout());
    let result = result.and_then(|_| writer.flush());

    if let Err(e) = result {
        let _ = child.kill();
        let _ = child.wait();
        if e.kind() == io::ErrorKind::BrokenPipe {
            std::process::exit(141);
        }
        eprintln!("ll: {e}");
        std::process::exit(1);
    }
    drop(writer);
    match child.wait() {
        Ok(st) => std::process::exit(exit_code(st)),
        Err(e) => {
            eprintln!("ll: wait failed: {e}");
            std::process::exit(1);
        }
    }
}
