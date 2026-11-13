use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;

const HELP: &str = "jj - change directories with a directory history\n\nUsage: jj [DIRECTORY]\n       jj --keep <DIRECTORY>\n       jj --pop [NUMBER]\n       jj --queue\n       jj --init <bash|zsh>\n\nSet up your shell with: eval \"$(jj --init bash)\" (or zsh).\nThe queue is listed most recent first; --pop defaults to 1.\nKept directories can be revisited by their final path component.\n";

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
    let keep = args.first().is_some_and(|a| a == "--keep");
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
    } else if keep {
        if args.len() != 2 {
            return Err("usage: jj --keep <DIRECTORY>".into());
        }
        let path = canonical_directory(Path::new(&args[1]))?;
        let kept_path = kept_file()?;
        let mut kept = read_queue(&kept_path)?;
        kept.retain(|entry| entry != &path);
        kept.push(path.clone());
        if let Some(parent) = kept_path.parent() {
            fs::create_dir_all(parent)?;
        }
        write_queue(&kept_path, &kept)?;
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
        if path.is_dir() {
            canonical_directory(&path)?
        } else if args.first().is_some()
            && path.components().count() == 1
            && path.file_name() == Some(path.as_os_str())
        {
            let kept = read_queue(&kept_file()?)?;
            let candidate = kept
                .iter()
                .rev()
                .find(|entry| entry.file_name() == path.file_name() && entry.is_dir());
            canonical_directory(candidate.unwrap_or(&path))?
        } else {
            canonical_directory(&path)?
        }
    };
    if args.first().is_none_or(|a| a != "--pop") && current != destination {
        queue.push(current);
    }
    write_queue(queue_path, &queue)?;
    println!("{}", destination.display());
    Ok(())
}

fn kept_file() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let base = match env::var_os("XDG_DATA_HOME").filter(|value| Path::new(value).is_absolute()) {
        Some(value) => PathBuf::from(value),
        None => PathBuf::from(env::var_os("HOME").ok_or("HOME is not set")?).join(".local/share"),
    };
    Ok(base.join("jump-jotter/kept"))
}

fn canonical_directory(path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    validate_directory(path)?;
    let path = fs::canonicalize(path)?;
    validate_directory(&path)?;
    Ok(path)
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
