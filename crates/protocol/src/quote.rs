//! Windows command-line quoting (portable string logic). Used by the core to build `launch`
//! parameters and by the settings app to show an argument list as one line.
//!
//! Follows the rules `CommandLineToArgvW` and the MSVC runtime use to split a command line, so
//! each list element reaches the launched program as exactly one argument.

/// Joins `args` into one parameter string, quoting each one as needed.
pub fn join_args(args: &[String]) -> String {
    let mut out = String::new();
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        push_quoted(&mut out, arg);
    }
    out
}

/// Appends `arg`, quoted if it is empty or contains whitespace or `"`.
fn push_quoted(out: &mut String, arg: &str) {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\x0B', '"']) {
        out.push_str(arg);
        return;
    }
    out.push('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                // Backslashes before a quote are doubled, then the quote is escaped.
                push_backslashes(out, backslashes * 2 + 1);
                out.push('"');
                backslashes = 0;
            }
            _ => {
                push_backslashes(out, backslashes);
                out.push(c);
                backslashes = 0;
            }
        }
    }
    // Backslashes before the closing quote are doubled.
    push_backslashes(out, backslashes * 2);
    out.push('"');
}

fn push_backslashes(out: &mut String, n: usize) {
    out.extend(std::iter::repeat_n('\\', n));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(arg: &str) -> String {
        join_args(&[arg.to_string()])
    }

    #[test]
    fn quoting_rules() {
        assert_eq!(q("plain"), "plain");
        assert_eq!(q(r"C:\dir\file.txt"), r"C:\dir\file.txt");
        assert_eq!(q(r"C:\dir\"), r"C:\dir\");
        assert_eq!(q("a b"), r#""a b""#);
        assert_eq!(q("tab\there"), "\"tab\there\"");
        assert_eq!(q(r#"x"y"#), r#""x\"y""#);
        assert_eq!(q(r"C:\my dir\"), r#""C:\my dir\\""#);
        assert_eq!(q(r#"a\"b"#), r#""a\\\"b""#);
        assert_eq!(q(r"a\\b c"), r#""a\\b c""#);
        assert_eq!(q(""), r#""""#);
    }

    #[test]
    fn joins_with_spaces() {
        let args = [
            "/c".to_string(),
            "a b".to_string(),
            String::new(),
            "z".to_string(),
        ];
        assert_eq!(join_args(&args), r#"/c "a b" "" z"#);
        assert_eq!(join_args(&[]), "");
    }
}
