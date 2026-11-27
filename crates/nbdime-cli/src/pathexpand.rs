//! `os.path.expanduser`, `os.path.expandvars` and the passwd database they consult.

use crate::osstr;
use std::collections::HashMap;
use std::env;

/// Where environment variables come from; tests supply a map instead of the process environment.
pub trait Env {
    fn get(&self, name: &str) -> Option<String>;
}

pub struct ProcessEnv;

impl Env for ProcessEnv {
    fn get(&self, name: &str) -> Option<String> {
        env::var_os(osstr::to_os(name)).map(|value| osstr::from_os(&value))
    }
}

impl Env for HashMap<String, String> {
    fn get(&self, name: &str) -> Option<String> {
        HashMap::get(self, name).cloned()
    }
}

#[cfg(unix)]
mod passwd {
    use std::ffi::{CStr, CString};

    use crate::osstr;

    #[derive(Clone, Copy)]
    pub(super) enum Who<'a> {
        Uid(libc::uid_t),
        Name(&'a str),
    }

    /// The home directory of a user from the passwd database (`pwd.getpwuid(...).pw_dir`).
    #[allow(unsafe_code)]
    pub(super) fn passwd_home(who: Who) -> Option<String> {
        let name = match who {
            Who::Name(name) => Some(CString::new(osstr::to_bytes(name).into_owned()).ok()?),
            Who::Uid(_) => None,
        };
        let mut buffer = vec![0u8; 1024];
        loop {
            // SAFETY: `passwd` is plain data that `getpw*_r` fills in; the all-zero value is valid.
            let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
            let mut found: *mut libc::passwd = std::ptr::null_mut();
            // SAFETY: `entry`, `buffer` and `found` outlive the call and the buffer length is passed
            // along; `name`, when used, is a NUL-terminated string that outlives the call.
            let status = unsafe {
                match (&name, who) {
                    (Some(name), _) => libc::getpwnam_r(
                        name.as_ptr(),
                        &mut entry,
                        buffer.as_mut_ptr().cast(),
                        buffer.len(),
                        &mut found,
                    ),
                    (None, Who::Uid(uid)) => libc::getpwuid_r(
                        uid,
                        &mut entry,
                        buffer.as_mut_ptr().cast(),
                        buffer.len(),
                        &mut found,
                    ),
                    (None, Who::Name(_)) => unreachable!("a name was converted above"),
                }
            };
            if status == libc::ERANGE && buffer.len() < (1 << 20) {
                buffer.resize(buffer.len() * 4, 0);
                continue;
            }
            if status != 0 || found.is_null() || entry.pw_dir.is_null() {
                return None;
            }
            // SAFETY: `pw_dir` points into `buffer` and is NUL-terminated.
            let dir = unsafe { CStr::from_ptr(entry.pw_dir) };
            return Some(osstr::from_bytes(dir.to_bytes()));
        }
    }

    #[allow(unsafe_code)]
    pub(super) fn current_uid() -> libc::uid_t {
        // SAFETY: `getuid` has no preconditions and cannot fail.
        unsafe { libc::getuid() }
    }
}

/// `os.path.expanduser` (`posixpath`).
#[cfg(unix)]
pub fn expanduser(path: &str, env: &dyn Env) -> String {
    if !path.starts_with('~') {
        return path.to_owned();
    }
    let end = path.find('/').unwrap_or(path.len());
    let home = if end == 1 {
        match env.get("HOME") {
            Some(home) => home,
            None => match passwd::passwd_home(passwd::Who::Uid(passwd::current_uid())) {
                Some(home) => home,
                None => return path.to_owned(),
            },
        }
    } else {
        match passwd::passwd_home(passwd::Who::Name(&path[1..end])) {
            Some(home) => home,
            None => return path.to_owned(),
        }
    };
    let expanded = format!("{}{}", home.trim_end_matches('/'), &path[end..]);
    if expanded.is_empty() {
        "/".to_owned()
    } else {
        expanded
    }
}

/// `os.path.expanduser` (`ntpath`): `USERPROFILE`, else `HOMEDRIVE` and `HOMEPATH`; `~name` is
/// a sibling of the current user's home.
#[cfg(not(unix))]
pub fn expanduser(path: &str, env: &dyn Env) -> String {
    if !path.starts_with('~') {
        return path.to_owned();
    }
    let end = path[1..].find(['/', '\\']).map_or(path.len(), |i| i + 1);
    let home = match (env.get("USERPROFILE"), env.get("HOMEPATH")) {
        (Some(home), _) => home,
        (None, Some(homepath)) => env.get("HOMEDRIVE").unwrap_or_default() + &homepath,
        (None, None) => return path.to_owned(),
    };
    let home = if end == 1 {
        home
    } else {
        let user = &path[1..end];
        let (parent, current) = home.rsplit_once(['/', '\\']).unwrap_or(("", &home));
        if user == current {
            home.clone()
        } else {
            format!("{parent}\\{user}")
        }
    };
    format!("{home}{}", &path[end..])
}

/// `os.path.expandvars`: `$name` and `${name}` of variables that exist.
pub fn expandvars(path: &str, env: &dyn Env) -> String {
    let mut path = path.to_owned();
    let mut start = 0;
    while let Some(offset) = path[start..].find('$') {
        let dollar = start + offset;
        let rest = &path[dollar + 1..];
        let (name, end) = if let Some(braced) = rest.strip_prefix('{') {
            match braced.find('}') {
                Some(close) => (&braced[..close], dollar + 1 + 1 + close + 1),
                None => {
                    start = dollar + 1;
                    continue;
                }
            }
        } else {
            let len = rest
                .bytes()
                .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
                .count();
            if len == 0 {
                start = dollar + 1;
                continue;
            }
            (&rest[..len], dollar + 1 + len)
        };
        match env.get(name) {
            Some(value) => {
                path.replace_range(dollar..end, &value);
                start = dollar + value.len();
            }
            None => start = end,
        }
    }
    path
}

/// `traitlets.utils.expand_path`.
pub fn expand_path(path: &str, env: &dyn Env) -> String {
    expandvars(&expanduser(path, env), env)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn variables_are_expanded_where_they_exist() {
        let env = vars(&[("A", "x y")]);
        assert_eq!(
            expandvars("$A/${A}/$NOPE/${}/$/$A_/${A", &env),
            "x y/x y/$NOPE/${}/$/$A_/${A"
        );
    }

    #[cfg(unix)]
    #[test]
    fn tilde_follows_home_and_passwd() {
        use passwd::{current_uid, passwd_home, Who};
        let env = vars(&[("HOME", "/h/")]);
        assert_eq!(expanduser("plain", &env), "plain");
        assert_eq!(expanduser("~/x", &env), "/h/x");
        assert_eq!(expanduser("~", &vars(&[("HOME", "")])), "/");
        assert_eq!(
            expanduser("~no-such-user-nbdime/x", &env),
            "~no-such-user-nbdime/x"
        );
        let root = passwd_home(Who::Name("root")).expect("root has a passwd entry");
        assert_eq!(
            expanduser("~root/x", &env),
            format!("{}/x", root.trim_end_matches('/'))
        );
        // HOME unset: the passwd entry of the current user.
        let own = passwd_home(Who::Uid(current_uid())).expect("the current user has an entry");
        assert_eq!(
            expanduser("~/x", &vars(&[])),
            format!("{}/x", own.trim_end_matches('/'))
        );
    }

    #[cfg(not(unix))]
    #[test]
    fn tilde_follows_userprofile() {
        let env = vars(&[("USERPROFILE", r"C:\Users\me")]);
        assert_eq!(expanduser("plain", &env), "plain");
        assert_eq!(expanduser(r"~\x", &env), r"C:\Users\me\x");
        assert_eq!(expanduser("~me/x", &env), r"C:\Users\me/x");
        assert_eq!(expanduser("~you", &env), r"C:\Users\you");
        let drive = vars(&[("HOMEDRIVE", "D:"), ("HOMEPATH", r"\h")]);
        assert_eq!(expanduser("~", &drive), r"D:\h");
        assert_eq!(expanduser("~/x", &vars(&[])), "~/x");
    }
}
