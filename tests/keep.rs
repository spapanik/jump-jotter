#[cfg(unix)]
#[test]
fn no_arguments_goes_home_with_nounset() {
    use std::{
        fs,
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    let root = std::env::temp_dir().join(format!(
        "jj-home-test-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    for shell in ["bash", "zsh"] {
        let output = match Command::new(shell)
            .arg("-uc")
            .arg(
                "eval \"$(\"$JJ_BIN\" --init \"$JJ_SHELL\")\"; cd /; jj; test \"$PWD\" = \"$HOME\"",
            )
            .env("JJ_BIN", env!("CARGO_BIN_EXE_jj"))
            .env("JJ_SHELL", shell)
            .env("HOME", &root)
            .env("XDG_STATE_HOME", root.join("state"))
            .output()
        {
            Ok(output) => output,
            Err(error) if shell == "zsh" && error.kind() == std::io::ErrorKind::NotFound => {
                continue;
            }
            Err(error) => panic!("failed to run {shell}: {error}"),
        };
        assert!(
            output.status.success(),
            "{shell}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(!root.join("state").exists());
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn kept_directories_work_across_shells() {
    use std::process::Command;

    let binary = env!("CARGO_BIN_EXE_jj");
    let output = Command::new("bash")
        .arg("-c")
        .arg(r#"
set -eu
root=$(mktemp -d)
root=$(cd "$root" && pwd -P)
trap 'rm -rf "$root"' EXIT
export HOME="$root/home" XDG_DATA_HOME="$root/data" XDG_STATE_HOME="$root/state"
export PATH="$(dirname "$JJ_BIN"):$PATH"
mkdir -p "$HOME" "$root/first/Downloads" "$root/second/Downloads" "$root/start"
ln -s "$root/first/Downloads" "$root/link"
eval "$(jj --init bash)"
first_session=$JJ_SESSION_ID
case "$first_session" in *-$$) ;; *) exit 1 ;; esac
test ! -e "$XDG_STATE_HOME/jj/jj.sqlite3"
eval "$(jj --init bash)"
second_session=$JJ_SESSION_ID
test "$first_session" != "$second_session"
cd "$root/start"
jj --keep "$root/link"
test "$PWD" = "$root/first/Downloads"
test -f "$XDG_STATE_HOME/jj/jj.sqlite3"
jj --keep .
jj --keep "$root/second/Downloads"
cd "$root/start"
jj Downloads
test "$PWD" = "$root/second/Downloads"
cd "$root/start"
mkdir Downloads
jj Downloads
test "$PWD" = "$root/start/Downloads"
cd "$root/start"
if jj ./missing/Downloads 2>/dev/null; then exit 1; fi
if jj --keep "$root/missing" 2>/dev/null; then exit 1; fi
# A fresh shell has its own queue but sees the same kept directories.
rm -r "$root/start/Downloads"
bash -c 'eval "$(jj --init bash)"; test -z "$(jj --queue)"; cd "$1"; jj Downloads; test "$PWD" = "$2"' _ "$root/start" "$root/second/Downloads"
"#)
        .env("JJ_BIN", binary)
        .output()
        .expect("run shell integration test");
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn imports_legacy_kept_and_queue_once() {
    use std::process::Command;

    let output = Command::new("bash")
        .arg("-c")
        .arg(
            r#"
set -eu
root=$(mktemp -d)
root=$(cd "$root" && pwd -P)
trap 'rm -rf "$root"' EXIT
export HOME="$root/home" XDG_DATA_HOME="$root/data" XDG_STATE_HOME="$root/state"
export JJ_QUEUE_FILE="$root/old-queue"
unset JJ_SESSION_ID
mkdir -p "$HOME" "$XDG_DATA_HOME/jump-jotter" "$root/saved/Downloads" "$root/start" "$root/previous"
printf '%s\n' "$root/saved/Downloads" > "$XDG_DATA_HOME/jump-jotter/kept"
printf '%s\n' "$root/previous" > "$JJ_QUEUE_FILE"
cd "$root/start"
test "$("$JJ_BIN" --queue)" = "1  $root/previous"
test "$("$JJ_BIN" Downloads)" = "$root/saved/Downloads"
# The old text files must not be imported a second time.
test "$("$JJ_BIN" --queue | wc -l | tr -d ' ')" = 2
"#,
        )
        .env("JJ_BIN", env!("CARGO_BIN_EXE_jj"))
        .output()
        .expect("run migration test");
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[test]
fn removes_queue_after_shell_exits() {
    use std::{
        fs,
        process::Command,
        time::{SystemTime, UNIX_EPOCH},
    };

    let root = std::env::temp_dir().join(format!(
        "jj-session-test-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(root.join("home")).unwrap();
    let run_shell = || {
        let output = Command::new("bash")
            .arg("-c")
            .arg("eval \"$(\"$JJ_BIN\" --init bash)\"; \"$JJ_BIN\" \"$HOME\" >/dev/null")
            .env("JJ_BIN", env!("CARGO_BIN_EXE_jj"))
            .env("HOME", root.join("home"))
            .env("XDG_STATE_HOME", root.join("state"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run_shell();
    let db = rusqlite::Connection::open(root.join("state/jj/jj.sqlite3")).unwrap();
    let first: String = db
        .query_row("SELECT id FROM sessions", [], |row| row.get(0))
        .unwrap();
    run_shell();
    let (count, second): (i64, String) = db
        .query_row("SELECT count(*), min(id) FROM sessions", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(count, 1);
    assert_ne!(first, second);
    let old_queue: i64 = db
        .query_row(
            "SELECT count(*) FROM queue WHERE shell = ?1",
            [&first],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(old_queue, 0);
    drop(db);
    fs::remove_dir_all(root).unwrap();
}
