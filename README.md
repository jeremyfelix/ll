# ll

`ll` wraps the system `ls`, defaults to long (`-l`) output, and inserts `,`
thousands separators into the file-size column (`1234567` becomes `1,234,567`).

It replicates GNU coreutils `ls --block-size="'1"`, which the Rust uutils `ls`
(now the default on recent Ubuntu) and BSD/macOS `ls` do not support. The
separator is always `,`, regardless of locale. Columns are re-aligned, `total`
lines are grouped too, and everything else is passed through untouched.

## Install

```sh
cargo install --path .
```

No dependencies beyond the Rust standard library (Unix only).

## Usage

```sh
ll              # ls -l with separators
ll -a /var/log
ll -R src       # streams block by block
ll -1           # non-long formats pass through untouched
```

All arguments go to `ls` after `-l`, so `-1`, `-C`, `-x`, `-m` and
`--format=...` override the long format as usual. `ll --ll-help` and
`ll --ll-version` are handled by `ll` itself; `--help` / `--version` go to `ls`.

When stdout is a terminal, `ll` emulates what `ls` would do on a tty: GNU and
uutils get `--color=always` and `--quoting-style=shell-escape` (unless you pass
your own color or quoting options, or set `QUOTING_STYLE`); BSD gets `-q` and,
if `CLICOLOR` is set, `CLICOLOR_FORCE=1`.

## Environment

| Variable | Meaning |
| --- | --- |
| `LL_LS` | `ls` program to run. Default `ls`; on macOS the first of `uls`, `gls`, `ls` found in `PATH`. |
| `LL_OPTS` | Whitespace-separated options inserted after `-l` and before your arguments. |
| `LL_LS_FLAVOR` | Force flavor detection: `gnu`, `uutils` or `bsd`. Otherwise `ls --version` is inspected. |

Exit status is that of `ls` (128+signal if killed); if `ls` cannot be started,
`ll` prints `ll: cannot run '<ls>': <err>` and exits 127.

## Fish configuration

Replace an old wrapper function with `ll` plus an environment variable. GNU and
uutils `ls` understand `-v` and `--group-directories-first`; stock macOS BSD
`ls` does not, so set `LL_OPTS` conditionally:

```fish
# ~/.config/fish/config.fish
# ll uses uls/gls on macOS when present, otherwise ls; only GNU/uutils ls
# understand these options.
if command -q uls; or command -q gls; or ls --version >/dev/null 2>&1
    set -gx LL_OPTS "-v --group-directories-first"
end
```

Fish also ships a built-in `ll` function (`ls -lh`), and any `ll` function
shadows the binary. The built-in is loaded lazily the first time you type
`ll`, so `functions -e ll` in `config.fish` does not get rid of it. Instead,
define your own `ll` in `~/.config/fish/functions/ll.fish`. That directory is
searched before fish's built-in functions, and the function hands off to the
binary:

```fish
# ~/.config/fish/functions/ll.fish
function ll --wraps=ls --description "ls -l with thousands separators"
    command ll $argv
end
```

`command ll` skips functions and runs the `ll` binary on `PATH`; `--wraps=ls`
keeps `ls` tab completion. This file replaces any older `ll` fish function, so
delete that one. Check with `type ll`.

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

## License

MIT
