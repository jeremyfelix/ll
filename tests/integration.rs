use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

// Serialise "write script + spawn" to avoid ETXTBSY races between threads.
static LOCK: Mutex<()> = Mutex::new(());
static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn tmpdir(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("{tag}-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// Fake ls: prints each argument as `ARG:<arg>`, then the contents of $FAKE_OUT
/// (if set), writes "fake-err" to stderr, and exits with $FAKE_EXIT.
fn fake_ls(dir: &Path) -> PathBuf {
    let p = dir.join("fake-ls");
    fs::write(
        &p,
        "#!/bin/sh\n\
         if [ \"$1\" = \"--version\" ]; then echo 'ls (GNU coreutils) 9.4'; exit 0; fi\n\
         for a in \"$@\"; do echo \"ARG:$a\"; done\n\
         [ -n \"$FAKE_OUT\" ] && cat \"$FAKE_OUT\"\n\
         echo fake-err >&2\n\
         exit ${FAKE_EXIT:-0}\n",
    )
    .unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn run_ll(envs: &[(&str, &str)], args: &[&str]) -> Output {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ll"));
    cmd.args(args)
        .env_remove("LL_OPTS")
        .env_remove("LL_LS_FLAVOR")
        .env_remove("QUOTING_STYLE")
        .stdin(Stdio::null());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.output().unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn arg_construction() {
    let d = tmpdir("args");
    let ls = fake_ls(&d);
    let o = run_ll(
        &[
            ("LL_LS", ls.to_str().unwrap()),
            ("LL_OPTS", "-v  --group-directories-first"),
            ("LL_LS_FLAVOR", "gnu"),
        ],
        &["-a", "b c"],
    );
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(
        stdout(&o),
        "ARG:-l\nARG:-v\nARG:--group-directories-first\nARG:-a\nARG:b c\n"
    );
}

#[test]
fn flavor_autodetect_and_no_color_when_piped() {
    let d = tmpdir("nocolor");
    let ls = fake_ls(&d);
    let o = run_ll(&[("LL_LS", ls.to_str().unwrap())], &["x"]);
    let out = stdout(&o);
    assert_eq!(out, "ARG:-l\nARG:x\n");
    assert!(!out.contains("--color"));
}

#[test]
fn exit_code_and_stderr_passthrough() {
    let d = tmpdir("exit");
    let ls = fake_ls(&d);
    let o = run_ll(
        &[
            ("LL_LS", ls.to_str().unwrap()),
            ("LL_LS_FLAVOR", "gnu"),
            ("FAKE_EXIT", "3"),
        ],
        &[],
    );
    assert_eq!(o.status.code(), Some(3));
    assert_eq!(String::from_utf8_lossy(&o.stderr), "fake-err\n");
}

#[test]
fn canned_output_is_processed() {
    let d = tmpdir("canned");
    let ls = fake_ls(&d);
    let out = d.join("out.txt");
    fs::write(
        &out,
        "total 1234\n\
-rw-r--r-- 1 jf jf 1234567 Nov 22 07:03 big\n\
-rw-r--r-- 1 jf jf       5 Nov 22 07:03 small",
    )
    .unwrap();
    let o = run_ll(
        &[
            ("LL_LS", ls.to_str().unwrap()),
            ("LL_LS_FLAVOR", "gnu"),
            ("FAKE_OUT", out.to_str().unwrap()),
        ],
        &[],
    );
    assert_eq!(
        stdout(&o),
        "ARG:-l\ntotal 1,234\n\
-rw-r--r-- 1 jf jf 1,234,567 Nov 22 07:03 big\n\
-rw-r--r-- 1 jf jf         5 Nov 22 07:03 small"
    );
}

#[test]
fn non_long_output_untouched() {
    let d = tmpdir("nonlong");
    let ls = fake_ls(&d);
    let out = d.join("out.txt");
    let canned = "-rw-r--r-- 1 jf jf 1234567 Nov 22 07:03 big\n";
    fs::write(&out, canned).unwrap();
    let o = run_ll(
        &[
            ("LL_LS", ls.to_str().unwrap()),
            ("LL_LS_FLAVOR", "gnu"),
            ("FAKE_OUT", out.to_str().unwrap()),
        ],
        &["-1"],
    );
    assert_eq!(stdout(&o), format!("ARG:-l\nARG:-1\n{canned}"));
}

#[test]
fn spawn_failure() {
    let o = run_ll(
        &[("LL_LS", "/nonexistent/ls-xyz"), ("LL_LS_FLAVOR", "gnu")],
        &[],
    );
    assert_eq!(o.status.code(), Some(127));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.starts_with("ll: cannot run '/nonexistent/ls-xyz': "),
        "{err}"
    );
}

#[test]
fn ll_version_and_help() {
    let o = run_ll(&[], &["--ll-version"]);
    assert_eq!(stdout(&o), format!("ll {}\n", env!("CARGO_PKG_VERSION")));
    let o = run_ll(&[], &["--ll-help"]);
    let s = stdout(&o);
    assert!(s.contains("LL_LS") && s.contains("LL_OPTS") && s.contains("LL_LS_FLAVOR"));
}

fn have_en_us() -> bool {
    Command::new("locale")
        .arg("-a")
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout).lines().any(|l| {
                l.eq_ignore_ascii_case("en_US.utf8") || l.eq_ignore_ascii_case("en_US.UTF-8")
            })
        })
        .unwrap_or(false)
}

#[test]
fn real_gnu_ls() {
    // Skip unless the system ls is GNU.
    let ver = Command::new("ls").arg("--version").output();
    let is_gnu = ver
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("GNU coreutils"))
        .unwrap_or(false);
    if !is_gnu {
        eprintln!("skipping: system ls is not GNU");
        return;
    }
    let d = tmpdir("real");
    let sizes: [(&str, u64); 4] = [
        ("f_0", 0),
        ("f_999", 999),
        ("f_1000", 1000),
        ("f_1234567", 1_234_567),
    ];
    for (n, sz) in sizes {
        fs::File::create(d.join(n)).unwrap().set_len(sz).unwrap();
    }
    let o = run_ll(&[("LL_LS", "ls"), ("TZ", "UTC")], &[d.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0));
    let out = stdout(&o);
    for want in [" 0 ", " 999 ", " 1,000 ", " 1,234,567 "] {
        assert!(out.contains(want), "missing {want:?} in:\n{out}");
    }
    assert!(!out.contains(" 1234567 "));

    // Columns align: every entry's name starts at the same offset.
    let offs: Vec<usize> = out
        .lines()
        .filter(|l| l.contains(" f_"))
        .map(|l| l.find(" f_").unwrap())
        .collect();
    assert_eq!(offs.len(), 4);
    assert!(offs.iter().all(|&o| o == offs[0]), "{out}");

    if have_en_us() {
        let r = Command::new("ls")
            .args(["-l", "--block-size='1", d.to_str().unwrap()])
            .env("LC_ALL", "en_US.UTF-8")
            .env("TZ", "UTC")
            .output()
            .unwrap();
        assert_eq!(out, String::from_utf8_lossy(&r.stdout));
    }
}
