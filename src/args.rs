//! Analysis of the arguments handed to `ls`, and construction of the child argv.

/// Which family of `ls` we are wrapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    Gnu,
    Uutils,
    Bsd,
}

impl Flavor {
    /// Classify the output of `ls --version`.
    pub fn from_version_output(out: &[u8]) -> Flavor {
        if contains(out, b"uutils") {
            Flavor::Uutils
        } else if contains(out, b"GNU coreutils") {
            Flavor::Gnu
        } else {
            Flavor::Bsd
        }
    }

    /// Parse the `LL_LS_FLAVOR` override value.
    pub fn from_name(name: &str) -> Option<Flavor> {
        match name.to_ascii_lowercase().as_str() {
            "gnu" => Some(Flavor::Gnu),
            "uutils" => Some(Flavor::Uutils),
            "bsd" => Some(Flavor::Bsd),
            _ => None,
        }
    }

    pub fn is_bsd(self) -> bool {
        self == Flavor::Bsd
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// What the effective option set implies about the output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Analysis {
    /// Effective format is long.
    pub long: bool,
    /// Owner column present.
    pub owner: bool,
    /// Group column present.
    pub group: bool,
    /// Author column present (GNU `--author`).
    pub author: bool,
    /// Security-context column present (GNU `-Z`/`--context`).
    pub context: bool,
    /// BSD file-flags column present (`-o`).
    pub bsd_flags: bool,
    /// A `--color`/`--colour` option was given (GNU/uutils).
    pub has_color_opt: bool,
    /// A quoting-related option was given (GNU/uutils).
    pub has_quoting_opt: bool,
    /// One of `-B -b -w -q` was given (BSD).
    pub has_bsd_q_opt: bool,
}

impl Default for Analysis {
    fn default() -> Self {
        Analysis {
            long: true,
            owner: true,
            group: true,
            author: false,
            context: false,
            bsd_flags: false,
            has_color_opt: false,
            has_quoting_opt: false,
            has_bsd_q_opt: false,
        }
    }
}

impl Analysis {
    /// Layout used by the post-processor.
    pub fn layout(&self) -> crate::format::Layout {
        crate::format::Layout {
            long: self.long,
            fields_before_size: 1
                + self.owner as usize
                + self.group as usize
                + self.author as usize
                + self.context as usize
                + self.bsd_flags as usize,
        }
    }
}

/// GNU long options that take a separate value when written without `=`.
const LONG_WITH_VALUE: &[&[u8]] = &[
    b"block-size",
    b"format",
    b"hide",
    b"ignore",
    b"indicator-style",
    b"quoting-style",
    b"sort",
    b"time",
    b"time-style",
    b"tabsize",
    b"width",
];

/// Analyse `args` (LL_OPTS tokens followed by user args). The initial state is
/// long format, because `ll` always passes `-l` first.
pub fn analyze(flavor: Flavor, args: &[Vec<u8>]) -> Analysis {
    let gnu = !flavor.is_bsd();
    let mut a = Analysis::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if arg == b"--" {
            break;
        }
        if arg.len() < 2 || arg[0] != b'-' {
            continue;
        }
        if arg.starts_with(b"--") {
            if !gnu {
                continue;
            }
            let body = &arg[2..];
            let (name, mut value): (&[u8], Option<&[u8]>) =
                match body.iter().position(|&b| b == b'=') {
                    Some(p) => (&body[..p], Some(&body[p + 1..])),
                    None => (body, None),
                };
            if value.is_none() && LONG_WITH_VALUE.contains(&name) && i < args.len() {
                value = Some(&args[i]);
                i += 1;
            }
            match name {
                b"format" => {
                    a.long = matches!(value, Some(b"long") | Some(b"verbose"));
                }
                b"color" | b"colour" => a.has_color_opt = true,
                b"literal"
                | b"escape"
                | b"quote-name"
                | b"hide-control-chars"
                | b"show-control-chars" => a.has_quoting_opt = true,
                n if n.starts_with(b"quoting-style") => a.has_quoting_opt = true,
                b"no-group" => a.group = false,
                b"author" => a.author = true,
                b"context" => a.context = true,
                b"numeric-uid-gid" | b"full-time" => a.long = true,
                _ => {}
            }
            continue;
        }
        // Short option cluster.
        let cluster = &arg[1..];
        for (idx, &c) in cluster.iter().enumerate() {
            if gnu {
                match c {
                    b'l' | b'n' => a.long = true,
                    b'g' => {
                        a.long = true;
                        a.owner = false;
                    }
                    b'o' => {
                        a.long = true;
                        a.group = false;
                    }
                    b'1' | b'C' | b'x' | b'm' => a.long = false,
                    b'G' => a.group = false,
                    b'Z' => a.context = true,
                    b'N' | b'Q' | b'b' | b'q' => a.has_quoting_opt = true,
                    b'I' | b'T' | b'w' => {
                        if idx + 1 == cluster.len() {
                            i += 1; // value is the next argument
                        }
                        break;
                    }
                    _ => {}
                }
            } else {
                match c {
                    b'l' | b'n' => a.long = true,
                    b'g' => {
                        a.long = true;
                        a.owner = false;
                    }
                    b'o' => a.bsd_flags = true,
                    b'1' | b'C' | b'x' | b'm' => a.long = false,
                    b'B' | b'b' | b'w' | b'q' => a.has_bsd_q_opt = true,
                    _ => {}
                }
            }
        }
    }
    a
}

/// Build the child argv: `-l`, tty-emulation options, LL_OPTS tokens, user args.
/// Returns the argv and the analysis of the effective options.
pub fn build_args(
    flavor: Flavor,
    opts: &[Vec<u8>],
    user: &[Vec<u8>],
    tty: bool,
    quoting_style_env: bool,
) -> (Vec<Vec<u8>>, Analysis) {
    let mut all: Vec<Vec<u8>> = Vec::with_capacity(opts.len() + user.len());
    all.extend(opts.iter().cloned());
    all.extend(user.iter().cloned());
    let analysis = analyze(flavor, &all);

    let mut out: Vec<Vec<u8>> = vec![b"-l".to_vec()];
    if tty {
        if flavor.is_bsd() {
            if !analysis.has_bsd_q_opt {
                out.push(b"-q".to_vec());
            }
        } else {
            if !analysis.has_color_opt {
                out.push(b"--color=always".to_vec());
            }
            if !analysis.has_quoting_opt && !quoting_style_env {
                out.push(b"--quoting-style=shell-escape".to_vec());
            }
        }
    }
    out.extend(all);
    (out, analysis)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(args: &[&str]) -> Vec<Vec<u8>> {
        args.iter().map(|s| s.as_bytes().to_vec()).collect()
    }
    fn gnu(args: &[&str]) -> Analysis {
        analyze(Flavor::Gnu, &v(args))
    }
    fn bsd(args: &[&str]) -> Analysis {
        analyze(Flavor::Bsd, &v(args))
    }

    #[test]
    fn default_is_long() {
        let a = gnu(&[]);
        assert!(a.long && a.owner && a.group && !a.author);
        assert_eq!(a.layout().fields_before_size, 3);
    }

    #[test]
    fn clusters() {
        let a = gnu(&["-lagh"]);
        assert!(a.long && !a.owner && a.group);
        let a = gnu(&["-lo"]);
        assert!(!a.group && a.owner);
        let a = gnu(&["-lG"]);
        assert!(!a.group);
        let a = gnu(&["--no-group"]);
        assert!(!a.group);
        let a = gnu(&["--author"]);
        assert!(a.author);
        assert_eq!(gnu(&["-lZ"]).layout().fields_before_size, 4);
        assert_eq!(gnu(&["--context"]).layout().fields_before_size, 4);
        assert_eq!(a.layout().fields_before_size, 4);
        let a = gnu(&["-go"]);
        assert_eq!(a.layout().fields_before_size, 1);
    }

    #[test]
    fn value_options() {
        // -w80 consumes the rest of cluster; "-1" afterwards still applies.
        assert!(gnu(&["-w80"]).long);
        // -w 80: the 80 is a value, not a filename; later -1 applies.
        assert!(!gnu(&["-w", "80", "-1"]).long);
        // value that looks like an option must be swallowed
        assert!(gnu(&["-I", "-1"]).long);
        assert!(gnu(&["-lI", "-1"]).long);
        assert!(gnu(&["--hide", "-1"]).long);
        assert!(gnu(&["--width=-1"]).long);
        // BSD -w takes no value
        let b = bsd(&["-w", "-1"]);
        assert!(!b.long);
        assert!(b.has_bsd_q_opt);
    }

    #[test]
    fn double_dash_and_dash() {
        assert!(gnu(&["--", "-1"]).long);
        assert!(gnu(&["-", "foo"]).long);
        assert!(!gnu(&["-1", "--", "-l"]).long);
    }

    #[test]
    fn format_option() {
        assert!(!gnu(&["--format=across"]).long);
        assert!(!gnu(&["--format", "commas"]).long);
        assert!(gnu(&["--format=across", "--format=long"]).long);
        assert!(gnu(&["-1", "--format", "verbose"]).long);
    }

    #[test]
    fn last_wins() {
        assert!(!gnu(&["-l", "-1"]).long);
        assert!(gnu(&["-1", "-l"]).long);
        assert!(!gnu(&["-C"]).long);
        assert!(!gnu(&["-x"]).long);
        assert!(!gnu(&["-m"]).long);
        assert!(gnu(&["-m", "-n"]).long);
        assert!(gnu(&["-1", "-g"]).long);
    }

    #[test]
    fn color_and_quoting() {
        assert!(gnu(&["--color=never"]).has_color_opt);
        assert!(gnu(&["--colour"]).has_color_opt);
        assert!(!gnu(&["-a"]).has_color_opt);
        for o in [
            "-N",
            "-Q",
            "-b",
            "-q",
            "--literal",
            "--escape",
            "--quote-name",
            "--quoting-style=c",
            "--hide-control-chars",
            "--show-control-chars",
        ] {
            assert!(gnu(&[o]).has_quoting_opt, "{o}");
        }
        assert!(gnu(&["-laQ"]).has_quoting_opt);
    }

    #[test]
    fn bsd_flags() {
        let b = bsd(&["-lo"]);
        assert!(b.bsd_flags && b.group && b.owner);
        assert_eq!(b.layout().fields_before_size, 4);
        let b = bsd(&["-lg"]);
        assert!(!b.owner);
        // long options are ignored for BSD
        assert!(bsd(&["--format=across"]).long);
    }

    #[test]
    fn build_non_tty() {
        let (a, _) = build_args(Flavor::Gnu, &v(&["-v"]), &v(&["x"]), false, false);
        assert_eq!(a, v(&["-l", "-v", "x"]));
    }

    #[test]
    fn build_tty_gnu() {
        let (a, _) = build_args(Flavor::Gnu, &[], &v(&["x"]), true, false);
        assert_eq!(
            a,
            v(&["-l", "--color=always", "--quoting-style=shell-escape", "x"])
        );
        let (a, _) = build_args(
            Flavor::Uutils,
            &[],
            &v(&["--color=never", "-Q"]),
            true,
            false,
        );
        assert_eq!(a, v(&["-l", "--color=never", "-Q"]));
        let (a, _) = build_args(Flavor::Gnu, &[], &[], true, true);
        assert_eq!(a, v(&["-l", "--color=always"]));
    }

    #[test]
    fn build_tty_bsd() {
        let (a, _) = build_args(Flavor::Bsd, &[], &v(&["-a"]), true, false);
        assert_eq!(a, v(&["-l", "-q", "-a"]));
        let (a, _) = build_args(Flavor::Bsd, &[], &v(&["-b"]), true, false);
        assert_eq!(a, v(&["-l", "-b"]));
    }

    #[test]
    fn flavor_detection() {
        assert_eq!(
            Flavor::from_version_output(b"ls (uutils coreutils) 0.1"),
            Flavor::Uutils
        );
        assert_eq!(
            Flavor::from_version_output(b"ls (GNU coreutils) 9.4"),
            Flavor::Gnu
        );
        assert_eq!(Flavor::from_version_output(b"usage: ls"), Flavor::Bsd);
        assert_eq!(Flavor::from_name("GNU"), Some(Flavor::Gnu));
        assert_eq!(Flavor::from_name("x"), None);
    }
}
