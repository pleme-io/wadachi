//! The zsh/bash hook for shells that are not frost (`wadachi init`).
//!
//! Defines `z` / `zi` (and, with `--cmd NAME`, `NAME` / `NAMEi`, e.g. a `cd`
//! with frecency fallback), plus the per-directory record hook. Nothing here
//! writes to stderr on its own: recording is backgrounded with output
//! discarded, and a failed jump is a silent `return 1`. The one exception is
//! `cd` itself, which keeps the builtin's normal error for a real miss.

/// Which shell the hook targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    Zsh,
    Bash,
}

const CORE: &str = r#"__wadachi_z() {
  if [ "$#" -eq 0 ]; then builtin cd; return; fi
  case "$1" in -?*) builtin cd "$@"; return ;; esac
  if builtin cd "$@" 2>/dev/null; then return 0; fi
  __wadachi_t=$(command wadachi resolve "$*" 2>/dev/null) || { unset __wadachi_t; return 1; }
  builtin cd -- "$__wadachi_t"; __wadachi_rc=$?; unset __wadachi_t; return $__wadachi_rc
}
__wadachi_cd() {
  __wadachi_z "$@" && return 0
  [ "$#" -eq 0 ] && return 1
  builtin cd "$@"
}
__wadachi_zi() {
  __wadachi_t=$(command wadachi query --paths --limit 10000 "$*" 2>/dev/null | command "${WADACHI_PICKER:-sk}" 2>/dev/null) || { unset __wadachi_t; return 1; }
  [ -n "$__wadachi_t" ] || { unset __wadachi_t; return 1; }
  builtin cd -- "$__wadachi_t"; __wadachi_rc=$?; unset __wadachi_t; return $__wadachi_rc
}
z() { __wadachi_z "$@"; }
zi() { __wadachi_zi "$@"; }
zoxide() { command wadachi zoxide "$@"; }
"#;

const ZSH: &str = r#"__wadachi_hook() { command wadachi add -- "$PWD" >/dev/null 2>&1 &! }
autoload -Uz add-zsh-hook
add-zsh-hook chpwd __wadachi_hook
"#;

const BASH: &str = r#"__wadachi_hook() {
  [ "$__wadachi_pwd" = "$PWD" ] && return
  __wadachi_pwd=$PWD
  (command wadachi add -- "$PWD" >/dev/null 2>&1 &)
}
case ";${PROMPT_COMMAND:-};" in *";__wadachi_hook;"*) ;; *) PROMPT_COMMAND="__wadachi_hook;${PROMPT_COMMAND:-}" ;; esac
"#;

/// A `--cmd` name: a shell identifier, so it can never inject syntax.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CmdName(String);

impl std::str::FromStr for CmdName {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let ok = s
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if ok {
            Ok(Self(s.to_owned()))
        } else {
            Err("must be a shell identifier".to_owned())
        }
    }
}

/// The full hook text.
#[must_use]
pub fn render(shell: Shell, cmd: Option<&CmdName>) -> String {
    let mut out = String::from(CORE);
    if let Some(CmdName(name)) = cmd {
        let target = if name == "cd" {
            "__wadachi_cd"
        } else {
            "__wadachi_z"
        };
        for (suffix, body) in [("", target), ("i", "__wadachi_zi")] {
            out.push_str(name);
            out.push_str(suffix);
            out.push_str("() { ");
            out.push_str(body);
            out.push_str(" \"$@\"; }\n");
        }
    }
    out.push_str(match shell {
        Shell::Zsh => ZSH,
        Shell::Bash => BASH,
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_name_rejects_syntax() {
        assert!("cd".parse::<CmdName>().is_ok());
        assert!("x;rm".parse::<CmdName>().is_err());
        assert!("".parse::<CmdName>().is_err());
        assert!("1a".parse::<CmdName>().is_err());
    }

    #[test]
    fn cd_cmd_uses_the_fallback_wrapper() {
        let h = render(Shell::Zsh, Some(&"cd".parse().unwrap()));
        assert!(h.contains("cd() { __wadachi_cd \"$@\"; }"));
        assert!(h.contains("cdi() { __wadachi_zi \"$@\"; }"));
        assert!(render(Shell::Bash, None).contains("PROMPT_COMMAND"));
    }
}
