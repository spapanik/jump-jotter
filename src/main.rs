use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;

const HELP: &str = "jj - change directories with a directory history\n\nUsage: jj [DIRECTORY]\n       jj --pop [NUMBER]\n       jj --queue\n       jj --init <bash|zsh>\n\nSet up your shell with: eval \"$(jj --init bash)\" (or zsh).\nThe queue is listed most recent first; --pop defaults to 1.\n";

fn main() {
    if let Err(error) = run() {
        eprintln!("jj: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.first().is_some_and(|a| a == "--init") {
        if args.len() != 2 || !(args[1] == "bash" || args[1] == "zsh") {
            return Err("usage: jj --init <bash|zsh>".into());
        }
        // The shell owns the queue identifier and performs the actual directory change.
        println!(
            r#"export JJ_QUEUE_FILE="${{TMPDIR:-/tmp}}/jj-queue-${{UID:-$(id -u)}}-$$"
jj() {{
    case "$1" in
        --queue|--help|-h|--init) command jj "$@" ;;
        *) local destination
           destination="$(command jj "$@")" || return
           builtin cd -- "$destination" ;;
    esac
}}"#
        );
        return Ok(());
    }
    if args.first().is_some_and(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return Ok(());
    }
    let queue_file = env::var_os("JJ_QUEUE_FILE")
        .ok_or("shell integration is not initialized; run eval \"$(jj --init bash)\" (or zsh)")?;
    let queue_path = Path::new(&queue_file);
    let mut queue = read_queue(queue_path)?;
    if args.first().is_some_and(|a| a == "--queue") {
        if args.len() != 1 {
            return Err("usage: jj --queue".into());
        }
        for (index, path) in queue.iter().rev().enumerate() {
            println!("{}  {}", index + 1, path.display());
        }
        return Ok(());
    }
    let current = env::current_dir()?;
    let destination = if args.first().is_some_and(|a| a == "--pop") {
        if args.len() > 2 {
            return Err("usage: jj --pop [NUMBER]".into());
        }
        let index: usize = match args.get(1) {
            Some(value) => value
                .to_str()
                .ok_or("invalid queue number")?
                .parse()
                .map_err(|_| "invalid queue number")?,
            None => 1,
        };
        if index == 0 || index > queue.len() {
            return Err("queue number is out of range".into());
        }
        let target = queue.len() - index;
        let path = queue[target].clone();
        validate_directory(&path)?;
        queue.truncate(target);
        path
    } else {
        if args.len() > 1 {
            return Err("usage: jj [DIRECTORY]".into());
        }
        let path = match args.first() {
            Some(arg) if arg.to_string_lossy().starts_with('-') => {
                return Err("unknown option".into());
            }
            Some(arg) => PathBuf::from(arg),
            None => PathBuf::from(env::var_os("HOME").ok_or("HOME is not set")?),
        };
        validate_directory(&path)?;
        fs::canonicalize(path)?
    };
    if args.first().is_none_or(|a| a != "--pop") && current != destination {
        queue.push(current);
    }
    write_queue(queue_path, &queue)?;
    println!("{}", destination.display());
    Ok(())
}

fn validate_directory(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if !path.is_dir() {
        return Err(format!("not a directory: {}", path.display()).into());
    }
    if path
        .to_str()
        .is_none_or(|s| s.contains('\n') || s.contains('\r'))
    {
        return Err("directory names must be valid UTF-8 and contain no newlines".into());
    }
    Ok(())
}

fn read_queue(path: &Path) -> io::Result<Vec<PathBuf>> {
    match fs::read_to_string(path) {
        Ok(data) => Ok(data.lines().map(PathBuf::from).collect()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn write_queue(path: &Path, queue: &[PathBuf]) -> io::Result<()> {
    let mut data = String::new();
    for entry in queue {
        let entry = entry
            .to_str()
            .filter(|s| !s.contains(['\n', '\r']))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "directory names must be valid UTF-8 and contain no newlines",
                )
            })?;
        data.push_str(entry);
        data.push('\n');
    }
    fs::write(path, data)
}
