//! Post-processing of long-format `ls` output: thousands separators in the
//! size column, with re-alignment per block.

use std::io::{self, BufRead, Write};

/// Describes the columns of the long listing being processed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Whether output is in long format at all (otherwise passed through).
    pub long: bool,
    /// Number of fields between the mode token and the size column
    /// (nlink, owner, group, author, flags).
    pub fields_before_size: usize,
}

/// Insert "," thousands separators into a run of ASCII digits.
pub fn group_thousands(digits: &[u8]) -> Vec<u8> {
    let n = digits.len();
    let mut out = Vec::with_capacity(n + n / 3);
    for (i, &b) in digits.iter().enumerate() {
        if i > 0 && (n - i).is_multiple_of(3) {
            out.push(b',');
        }
        out.push(b);
    }
    out
}

fn grouped_len(n: usize) -> usize {
    if n == 0 {
        0
    } else {
        n + (n - 1) / 3
    }
}

fn all_digits(t: &[u8]) -> bool {
    !t.is_empty() && t.iter().all(u8::is_ascii_digit)
}

fn is_ws(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// Whitespace-separated tokens as (start, end) offsets, at most `max` of them.
fn tokenize(s: &[u8], max: usize) -> Vec<(usize, usize)> {
    let mut toks = Vec::with_capacity(max);
    let mut i = 0;
    while i < s.len() && toks.len() < max {
        while i < s.len() && is_ws(s[i]) {
            i += 1;
        }
        if i >= s.len() {
            break;
        }
        let start = i;
        while i < s.len() && !is_ws(s[i]) {
            i += 1;
        }
        toks.push((start, i));
    }
    toks
}

fn is_mode(t: &[u8]) -> bool {
    if t.len() != 10 && t.len() != 11 {
        return false;
    }
    if !b"-bcdlpsDwn?CM".contains(&t[0]) {
        return false;
    }
    if !t[1..10].iter().all(|c| b"rwxsStTlL-".contains(c)) {
        return false;
    }
    t.len() == 10 || b".+@".contains(&t[10])
}

/// Tokens allowed before the mode token: inode / block counts (`123`, `4.0K`).
fn is_skippable(t: &[u8]) -> bool {
    t.first().is_some_and(u8::is_ascii_digit)
        && t.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'.')
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    /// Start of the size token (after its padding).
    tok_start: usize,
    /// End of the size column (end of size token, or of the minor number).
    col_end: usize,
    /// Whether the size is a plain digit run that gets separators.
    sized: bool,
}

impl Entry {
    /// How many columns the size column grows by.
    fn delta(&self) -> usize {
        if self.sized {
            let l = self.col_end - self.tok_start;
            grouped_len(l) - l
        } else {
            0
        }
    }
}

/// Try to interpret `body` (a line without its newline) as a long entry.
fn parse_entry(body: &[u8], layout: &Layout) -> Option<Entry> {
    let toks = tokenize(body, 3 + layout.fields_before_size + 3);
    let tok = |i: usize| &body[toks[i].0..toks[i].1];
    let k = (0..toks.len().min(3)).find(|&k| is_mode(tok(k)))?;
    if !(0..k).all(|j| is_skippable(tok(j))) {
        return None;
    }
    let size_idx = k + 1 + layout.fields_before_size;
    if toks.len() <= size_idx || !all_digits(tok(k + 1)) {
        return None;
    }
    let (tok_start, size_end) = toks[size_idx];
    let size = tok(size_idx);
    if size.last() == Some(&b',') {
        // Device node: "major," then "minor".
        let minor = toks.get(size_idx + 1)?;
        return Some(Entry {
            col_end: minor.1,
            tok_start,
            sized: false,
        });
    }
    Some(Entry {
        col_end: size_end,
        tok_start,
        sized: all_digits(size),
    })
}

struct Item {
    line: Vec<u8>,
    entry: Option<Entry>,
}

fn flush_block<W: Write>(block: &mut Vec<Item>, w: &mut W) -> io::Result<()> {
    // ls right-aligns the size column, so every entry's size ends at the same
    // column. Growing the widest value by `shift` moves that end by `shift`;
    // every other entry gets enough extra padding to follow it.
    let shift = block
        .iter()
        .filter_map(|it| it.entry.as_ref())
        .map(Entry::delta)
        .max()
        .unwrap_or(0);
    for it in block.drain(..) {
        let Some(e) = it.entry else {
            w.write_all(&it.line)?;
            continue;
        };
        let line = &it.line;
        w.write_all(&line[..e.tok_start])?;
        write_spaces(w, shift - e.delta())?;
        if e.sized {
            w.write_all(&group_thousands(&line[e.tok_start..e.col_end]))?;
        } else {
            w.write_all(&line[e.tok_start..e.col_end])?;
        }
        w.write_all(&line[e.col_end..])?;
    }
    Ok(())
}

fn write_spaces<W: Write>(w: &mut W, n: usize) -> io::Result<()> {
    const SPACES: [u8; 32] = [b' '; 32];
    let mut n = n;
    while n > 0 {
        let c = n.min(SPACES.len());
        w.write_all(&SPACES[..c])?;
        n -= c;
    }
    Ok(())
}

fn strip_newline(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\n").unwrap_or(line)
}

/// Add separators to a `total N` line (keeping any trailing newline).
fn fix_total(line: &[u8]) -> Option<Vec<u8>> {
    let body = strip_newline(line);
    let digits = body.strip_prefix(b"total ")?;
    if !all_digits(digits) {
        return None;
    }
    let mut out = b"total ".to_vec();
    out.extend(group_thousands(digits));
    out.extend_from_slice(&line[body.len()..]);
    Some(out)
}

/// Stream `r` to `w`, adding thousands separators to the size column of long
/// listings. Output is flushed after every block so recursive listings stream.
pub fn process<R: BufRead, W: Write>(mut r: R, w: &mut W, layout: &Layout) -> io::Result<()> {
    if !layout.long {
        loop {
            let buf = r.fill_buf()?;
            if buf.is_empty() {
                break;
            }
            w.write_all(buf)?;
            let n = buf.len();
            r.consume(n);
            w.flush()?;
        }
        return Ok(());
    }

    let mut block: Vec<Item> = Vec::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        if r.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        let body = strip_newline(&line);
        if let Some(entry) = parse_entry(body, layout) {
            block.push(Item {
                line: std::mem::take(&mut line),
                entry: Some(entry),
            });
        } else if !block.is_empty() && body.first().is_some_and(|&b| is_ws(b)) {
            // BSD ACL / xattr continuation line: stays inside the block.
            block.push(Item {
                line: std::mem::take(&mut line),
                entry: None,
            });
        } else {
            flush_block(&mut block, w)?;
            match fix_total(&line) {
                Some(fixed) => w.write_all(&fixed)?,
                None => w.write_all(&line)?,
            }
            w.flush()?;
        }
    }
    flush_block(&mut block, w)?;
    w.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    const L: Layout = Layout {
        long: true,
        fields_before_size: 3,
    };

    fn run(layout: Layout, input: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        process(input, &mut out, &layout).unwrap();
        out
    }

    fn s(layout: Layout, input: &str) -> String {
        String::from_utf8(run(layout, input.as_bytes())).unwrap()
    }

    #[test]
    fn grouping() {
        assert_eq!(group_thousands(b"0"), b"0");
        assert_eq!(group_thousands(b"999"), b"999");
        assert_eq!(group_thousands(b"1000"), b"1,000");
        assert_eq!(group_thousands(b"1234567"), b"1,234,567");
        assert_eq!(group_thousands(b"100000"), b"100,000");
        assert_eq!(
            group_thousands(b"12345678901234567890"),
            b"12,345,678,901,234,567,890"
        );
        assert_eq!(group_thousands(b""), b"");
    }

    #[test]
    fn gnu_basic() {
        let input = "total 1234
-rw-r--r-- 1 jf jf       0 Nov 22 07:03 a
-rw-r--r-- 1 jf jf    1000 Nov 22 07:03 b
-rw-r--r-- 1 jf jf 1234567 Nov 22 07:03 c d
";
        let want = "total 1,234
-rw-r--r-- 1 jf jf         0 Nov 22 07:03 a
-rw-r--r-- 1 jf jf     1,000 Nov 22 07:03 b
-rw-r--r-- 1 jf jf 1,234,567 Nov 22 07:03 c d
";
        assert_eq!(s(L, input), want);
    }

    #[test]
    fn small_sizes_unchanged() {
        let input = "total 8
-rw-r--r-- 1 jf jf  12 Nov 22 07:03 a
-rw-r--r-- 1 jf jf 999 Nov 22 07:03 b
";
        assert_eq!(s(L, input), input);
    }

    #[test]
    fn inode_and_blocks() {
        let input = "total 8
  131 -rw-r--r-- 1 jf jf 12345 Nov 22 07:03 a
99999 -rw-r--r-- 1 jf jf     7 Nov 22 07:03 b
";
        let want = "total 8
  131 -rw-r--r-- 1 jf jf 12,345 Nov 22 07:03 a
99999 -rw-r--r-- 1 jf jf      7 Nov 22 07:03 b
";
        assert_eq!(s(L, input), want);
        // -s -h style block counts, inode + blocks together
        let input = "total 12K
 12 4.0K -rw-r--r-- 1 jf jf 12345 Nov 22 07:03 a
  1  12 -rw-r--r-- 1 jf jf     7 Nov 22 07:03 b
";
        let want = "total 12K
 12 4.0K -rw-r--r-- 1 jf jf 12,345 Nov 22 07:03 a
  1  12 -rw-r--r-- 1 jf jf      7 Nov 22 07:03 b
";
        assert_eq!(s(L, input), want);
    }

    #[test]
    fn no_owner_no_group() {
        let g = Layout {
            long: true,
            fields_before_size: 2,
        };
        let input = "-rw-r--r-- 1 jf  5000 Nov 22 07:03 a
-rw-r--r-- 1 jf    50 Nov 22 07:03 b
";
        let want = "-rw-r--r-- 1 jf  5,000 Nov 22 07:03 a
-rw-r--r-- 1 jf     50 Nov 22 07:03 b
";
        assert_eq!(s(g, input), want);
        let g = Layout {
            long: true,
            fields_before_size: 1,
        };
        let input = "-rw-r--r-- 1 5000 Nov 22 07:03 a\n";
        assert_eq!(s(g, input), "-rw-r--r-- 1 5,000 Nov 22 07:03 a\n");
    }

    #[test]
    fn author_numeric() {
        let g = Layout {
            long: true,
            fields_before_size: 4,
        };
        let input = "-rw-r--r-- 1 jf jf jf 5000 Nov 22 07:03 a\n";
        assert_eq!(s(g, input), "-rw-r--r-- 1 jf jf jf 5,000 Nov 22 07:03 a\n");
        let input = "-rw-r--r-- 1 1000 1000 5000 Nov 22 07:03 a\n";
        assert_eq!(s(L, input), "-rw-r--r-- 1 1000 1000 5,000 Nov 22 07:03 a\n");
    }

    #[test]
    fn devices_mixed() {
        let input = "total 0
crw-rw-rw- 1 root root   1,   3 Nov 22 07:03 null
brw-rw---- 1 root disk   8,   0 Nov 22 07:03 sda
-rw-r--r-- 1 root root   123456 Nov 22 07:03 file
";
        let want = "total 0
crw-rw-rw- 1 root root    1,   3 Nov 22 07:03 null
brw-rw---- 1 root disk    8,   0 Nov 22 07:03 sda
-rw-r--r-- 1 root root   123,456 Nov 22 07:03 file
";
        assert_eq!(s(L, input), want);
    }

    #[test]
    fn devices_only_unchanged() {
        let input = "crw-rw-rw- 1 root root  1,   3 Nov 22 07:03 null
crw-rw-rw- 1 root root 10, 200 Nov 22 07:03 x
";
        assert_eq!(s(L, input), input);
    }

    #[test]
    fn human_unchanged() {
        let input = "total 4.0K
-rw-r--r-- 1 jf jf 1.2M Nov 22 07:03 a
-rw-r--r-- 1 jf jf  512 Nov 22 07:03 b
";
        assert_eq!(s(L, input), input);
    }

    #[test]
    fn recursive_blocks() {
        let input = ".:
total 2000
-rw-r--r-- 1 jf jf 1000000 Nov 22 07:03 a
drwxr-xr-x 2 jf jf    4096 Nov 22 07:03 sub

./sub:
total 4
-rw-r--r-- 1 jf jf 1234 Nov 22 07:03 x
";
        let want = ".:
total 2,000
-rw-r--r-- 1 jf jf 1,000,000 Nov 22 07:03 a
drwxr-xr-x 2 jf jf     4,096 Nov 22 07:03 sub

./sub:
total 4
-rw-r--r-- 1 jf jf 1,234 Nov 22 07:03 x
";
        assert_eq!(s(L, input), want);
    }

    #[test]
    fn ansi_and_spaces_in_names() {
        let input = "-rw-r--r-- 1 jf jf 12345 Nov 22 07:03 \x1b[0m\x1b[01;32ma  b 123456\x1b[0m
-rw-r--r-- 1 jf jf     1 Nov 22 07:03 'x y'
";
        let want = "-rw-r--r-- 1 jf jf 12,345 Nov 22 07:03 \x1b[0m\x1b[01;32ma  b 123456\x1b[0m
-rw-r--r-- 1 jf jf      1 Nov 22 07:03 'x y'
";
        assert_eq!(s(L, input), want);
    }

    #[test]
    fn non_utf8_name() {
        let mut input = b"-rw-r--r-- 1 jf jf 12345 Nov 22 07:03 a\xff\xfeb\n".to_vec();
        input.extend_from_slice(b"-rw-r--r-- 1 jf jf     1 Nov 22 07:03 \xc3\n");
        let mut want = b"-rw-r--r-- 1 jf jf 12,345 Nov 22 07:03 a\xff\xfeb\n".to_vec();
        want.extend_from_slice(b"-rw-r--r-- 1 jf jf      1 Nov 22 07:03 \xc3\n");
        assert_eq!(run(L, &input), want);
    }

    #[test]
    fn symlink() {
        let input = "lrwxrwxrwx 1 jf jf 11111 Nov 22 07:03 l -> /a/b
-rw-r--r-- 1 jf jf     1 Nov 22 07:03 b
";
        let want = "lrwxrwxrwx 1 jf jf 11,111 Nov 22 07:03 l -> /a/b
-rw-r--r-- 1 jf jf      1 Nov 22 07:03 b
";
        assert_eq!(s(L, input), want);
    }

    #[test]
    fn no_trailing_newline() {
        let input = "-rw-r--r-- 1 jf jf 12345 Nov 22 07:03 a
-rw-r--r-- 1 jf jf     1 Nov 22 07:03 b";
        let want = "-rw-r--r-- 1 jf jf 12,345 Nov 22 07:03 a
-rw-r--r-- 1 jf jf      1 Nov 22 07:03 b";
        assert_eq!(s(L, input), want);
        assert_eq!(s(L, "total 1234"), "total 1,234");
        assert_eq!(s(L, ""), "");
    }

    #[test]
    fn total_variants() {
        assert_eq!(s(L, "total 4.0K\n"), "total 4.0K\n");
        assert_eq!(s(L, "total 1000000\n"), "total 1,000,000\n");
        assert_eq!(s(L, "total x\n"), "total x\n");
    }

    #[test]
    fn non_long_passthrough() {
        let l = Layout {
            long: false,
            fields_before_size: 3,
        };
        let input = "-rw-r--r-- 1 jf jf 12345 Nov 22 07:03 a\ntotal 1234\n";
        assert_eq!(s(l, input), input);
    }

    #[test]
    fn help_output_untouched() {
        let input = "Usage: ls [OPTION]... [FILE]...
List information about the FILEs.

  -a, --all   do not ignore
";
        assert_eq!(s(L, input), input);
    }

    #[test]
    fn bsd_basic_and_flags() {
        let input = "total 16
-rw-r--r--  1 jf  staff  12345 Nov 22 07:03 a
-rw-r--r--  1 jf  staff      1 Nov 22 07:03 b
";
        let want = "total 16
-rw-r--r--  1 jf  staff  12,345 Nov 22 07:03 a
-rw-r--r--  1 jf  staff       1 Nov 22 07:03 b
";
        assert_eq!(s(L, input), want);

        let bf = Layout {
            long: true,
            fields_before_size: 4,
        };
        let input = "-rw-r--r--  1 jf  staff  uchg  12345 Nov 22 07:03 a
-rw-r--r--  1 jf  staff  -         1 Nov 22 07:03 b
";
        let want = "-rw-r--r--  1 jf  staff  uchg  12,345 Nov 22 07:03 a
-rw-r--r--  1 jf  staff  -          1 Nov 22 07:03 b
";
        assert_eq!(s(bf, input), want);
    }

    #[test]
    fn bsd_acl_and_xattr_continuations() {
        let input = "total 16
-rw-r--r--+ 1 jf  staff  12345 Nov 22 07:03 a
 0: user:jf allow read
-rw-r--r--@ 1 jf  staff      1 Nov 22 07:03 b
\tcom.apple.quarantine\t   22 
-rw-r--r--  1 jf  staff   2000 Nov 22 07:03 c
";
        let want = "total 16
-rw-r--r--+ 1 jf  staff  12,345 Nov 22 07:03 a
 0: user:jf allow read
-rw-r--r--@ 1 jf  staff       1 Nov 22 07:03 b
\tcom.apple.quarantine\t   22 
-rw-r--r--  1 jf  staff   2,000 Nov 22 07:03 c
";
        assert_eq!(s(L, input), want);
    }

    #[test]
    fn uutils_style() {
        let input = "total 12
-rw-r--r-- 1 jf jf 100000 Nov 22 07:03 a
drwxr-xr-x 2 jf jf   4096 Nov 22 07:03 sub
";
        let want = "total 12
-rw-r--r-- 1 jf jf 100,000 Nov 22 07:03 a
drwxr-xr-x 2 jf jf   4,096 Nov 22 07:03 sub
";
        assert_eq!(s(L, input), want);
    }

    #[test]
    fn padded_owner_group_columns() {
        let input = "-rw-r--r-- 1 jf   users 12345 Nov 22 07:03 a
-rw-r--r-- 1 root root      7 Nov 22 07:03 b
";
        let want = "-rw-r--r-- 1 jf   users 12,345 Nov 22 07:03 a
-rw-r--r-- 1 root root       7 Nov 22 07:03 b
";
        assert_eq!(s(L, input), want);
    }

    #[test]
    fn non_entry_line_with_digits() {
        let input = "123 foo bar baz qux\n";
        assert_eq!(s(L, input), input);
    }
}
