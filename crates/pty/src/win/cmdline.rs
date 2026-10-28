//! Pure helpers for building a `CreateProcessW` call: command-line quoting,
//! UTF-16 environment blocks and program lookup. No Windows APIs are used so
//! these are unit-tested on every platform.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

/// Build a command line following the `CommandLineToArgvW` / MSVCRT rules.
/// The program is always quoted (so paths with spaces work) and must not
/// contain `"`. Fails on interior NULs.
pub(crate) fn command_line(program: &str, args: &[String]) -> io::Result<String> {
    if program.contains('"') || program.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "program name contains a quote or NUL",
        ));
    }
    let mut out = String::with_capacity(program.len() + 3);
    out.push('"');
    out.push_str(program);
    out.push('"');
    for a in args {
        if a.contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "argument contains a NUL",
            ));
        }
        out.push(' ');
        push_quoted_arg(&mut out, a);
    }
    Ok(out)
}

fn push_quoted_arg(out: &mut String, arg: &str) {
    let needs_quotes = arg.is_empty()
        || arg
            .chars()
            .any(|c| matches!(c, ' ' | '\t' | '\n' | '\x0b' | '"'));
    if !needs_quotes {
        out.push_str(arg);
        return;
    }
    out.push('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                // Double the run of backslashes, then escape the quote.
                out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.extend(std::iter::repeat_n('\\', backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    // Backslashes before the closing quote must be doubled.
    out.extend(std::iter::repeat_n('\\', backslashes * 2));
    out.push('"');
}

/// A Windows environment: names are case-insensitive, the last `set` wins and
/// keeps the casing it was given.
#[derive(Default)]
pub(crate) struct EnvBlock {
    // Upper-cased name -> (name as given, value).
    map: BTreeMap<String, (Vec<u16>, Vec<u16>)>,
}

fn env_key(name: &[u16]) -> String {
    String::from_utf16_lossy(name).to_uppercase()
}

impl EnvBlock {
    pub(crate) fn set(&mut self, name: Vec<u16>, value: Vec<u16>) {
        self.map.insert(env_key(&name), (name, value));
    }

    pub(crate) fn get(&self, name: &str) -> Option<&[u16]> {
        let key = name.to_uppercase();
        self.map.get(&key).map(|(_, v)| v.as_slice())
    }

    /// The `lpEnvironment` block for `CREATE_UNICODE_ENVIRONMENT`: sorted
    /// `NAME=VALUE\0` entries followed by an extra NUL.
    pub(crate) fn to_block(&self) -> Vec<u16> {
        let mut out = Vec::new();
        for (name, value) in self.map.values() {
            out.extend_from_slice(name);
            out.push(u16::from(b'='));
            out.extend_from_slice(value);
            out.push(0);
        }
        if self.map.is_empty() {
            // An empty block still needs two terminating NULs.
            out.push(0);
        }
        out.push(0);
        out
    }
}

/// Candidate file names for `name`: as given if it has an extension, then
/// with `.exe` appended.
fn candidates(name: &OsStr) -> Vec<OsString> {
    let mut v = Vec::new();
    let p = Path::new(name);
    let is_exe = p
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
    if p.extension().is_some() {
        v.push(name.to_os_string());
    }
    if !is_exe {
        let mut with = name.to_os_string();
        with.push(".exe");
        v.push(with);
    }
    v
}

/// Resolve `cmd` to an existing executable file. Names containing a path
/// separator or drive colon are taken as paths (relative ones against `cwd`);
/// bare names are searched in `path_var`. `None` if nothing exists, in which
/// case the caller lets `CreateProcessW` try its own search.
pub(crate) fn resolve_program(
    cmd: &Path,
    cwd: Option<&Path>,
    path_var: Option<&OsStr>,
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let s = cmd.to_string_lossy();
    if s.contains(['/', '\\', ':']) {
        let full = if cmd.is_absolute() {
            cmd.to_path_buf()
        } else {
            match cwd {
                Some(c) => c.join(cmd),
                None => cmd.to_path_buf(),
            }
        };
        let name = full.file_name()?;
        let dir = full.parent().unwrap_or_else(|| Path::new(""));
        return candidates(name)
            .into_iter()
            .map(|c| dir.join(c))
            .find(|p| exists(p));
    }
    let path_var = path_var?;
    for dir in std::env::split_paths(path_var) {
        for c in candidates(cmd.as_os_str()) {
            let p = dir.join(c);
            if exists(&p) {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn cl(prog: &str, args: &[&str]) -> String {
        let a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        command_line(prog, &a).unwrap()
    }

    #[test]
    fn quotes_program_and_plain_args() {
        assert_eq!(cl(r"C:\Program Files\x.exe", &[]), r#""C:\Program Files\x.exe""#);
        assert_eq!(cl("cmd.exe", &["/c", "dir"]), r#""cmd.exe" /c dir"#);
    }

    #[test]
    fn quotes_special_args() {
        assert_eq!(cl("p", &[""]), r#""p" """#);
        assert_eq!(cl("p", &["a b"]), r#""p" "a b""#);
        assert_eq!(cl("p", &[r#"say "hi""#]), r#""p" "say \"hi\"""#);
        // Backslashes only matter before a quote or the closing quote.
        assert_eq!(cl("p", &[r"C:\a\b"]), r#""p" C:\a\b"#);
        assert_eq!(cl("p", &[r"C:\a b\"]), r#""p" "C:\a b\\""#);
        assert_eq!(cl("p", &[r#"a\"b c"#]), r#""p" "a\\\"b c""#);
    }

    #[test]
    fn rejects_nul_and_quote_in_program() {
        assert!(command_line("a\"b", &[]).is_err());
        assert!(command_line("a\0b", &[]).is_err());
        assert!(command_line("a", &["x\0y".to_string()]).is_err());
    }

    #[test]
    fn env_block_sorted_case_insensitive_and_overridden() {
        let mut e = EnvBlock::default();
        e.set(w("Path"), w("C:\\a"));
        e.set(w("zeta"), w("1"));
        e.set(w("Alpha"), w("2"));
        e.set(w("PATH"), w("C:\\b")); // replaces "Path", keeps new casing
        let block = e.to_block();
        let text = String::from_utf16(&block).unwrap();
        assert_eq!(text, "Alpha=2\0PATH=C:\\b\0zeta=1\0\0");
        assert_eq!(e.get("path").unwrap(), w("C:\\b").as_slice());
        assert!(e.get("nope").is_none());
    }

    #[test]
    fn empty_env_block_has_two_nuls() {
        assert_eq!(EnvBlock::default().to_block(), vec![0, 0]);
    }

    fn exists_in<'a>(set: &'a HashSet<PathBuf>) -> impl Fn(&Path) -> bool + 'a {
        move |p| set.contains(p)
    }

    #[test]
    fn resolves_bare_names_via_path() {
        let path = std::env::join_paths(["/a", "/b"]).unwrap();
        let present: HashSet<PathBuf> = [PathBuf::from("/b/pwsh.exe")].into();
        let got = resolve_program(Path::new("pwsh"), None, Some(&path), exists_in(&present));
        assert_eq!(got, Some(PathBuf::from("/b/pwsh.exe")));
        let got = resolve_program(Path::new("pwsh.exe"), None, Some(&path), exists_in(&present));
        assert_eq!(got, Some(PathBuf::from("/b/pwsh.exe")));
        let got = resolve_program(Path::new("nope"), None, Some(&path), exists_in(&present));
        assert_eq!(got, None);
        assert_eq!(resolve_program(Path::new("pwsh"), None, None, exists_in(&present)), None);
    }

    #[test]
    fn dotted_names_try_as_is_then_exe() {
        let path = std::env::join_paths(["/a"]).unwrap();
        let present: HashSet<PathBuf> = [PathBuf::from("/a/python3.11.exe")].into();
        let got = resolve_program(Path::new("python3.11"), None, Some(&path), exists_in(&present));
        assert_eq!(got, Some(PathBuf::from("/a/python3.11.exe")));
    }

    #[test]
    fn explicit_paths_are_not_searched() {
        let present: HashSet<PathBuf> = [PathBuf::from("/x/tools/sh.exe")].into();
        let got = resolve_program(Path::new("/x/tools/sh"), None, None, exists_in(&present));
        assert_eq!(got, Some(PathBuf::from("/x/tools/sh.exe")));
        // Relative paths with a separator resolve against cwd.
        let got = resolve_program(
            Path::new("tools/sh.exe"),
            Some(Path::new("/x")),
            None,
            exists_in(&present),
        );
        assert_eq!(got, Some(PathBuf::from("/x/tools/sh.exe")));
    }
}
