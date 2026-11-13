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
export HOME="$root/home" XDG_DATA_HOME="$root/data"
export PATH="$(dirname "$JJ_BIN"):$PATH"
mkdir -p "$HOME" "$root/first/Downloads" "$root/second/Downloads" "$root/start"
ln -s "$root/first/Downloads" "$root/link"
eval "$(jj --init bash)"
cd "$root/start"
jj --keep "$root/link"
test "$PWD" = "$root/first/Downloads"
test "$(cat "$XDG_DATA_HOME/jump-jotter/kept")" = "$root/first/Downloads"
jj --keep .
test "$(wc -l < "$XDG_DATA_HOME/jump-jotter/kept" | tr -d ' ')" = 1
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
test "$(wc -l < "$XDG_DATA_HOME/jump-jotter/kept" | tr -d ' ')" = 2
# A fresh shell has its own queue but sees the same kept directories.
rm -r "$root/start/Downloads"
bash -c 'eval "$(jj --init bash)"; cd "$1"; jj Downloads; test "$PWD" = "$2"' _ "$root/start" "$root/second/Downloads"
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
