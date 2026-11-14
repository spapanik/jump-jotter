use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::{Path, PathBuf};
use std::process;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use sysinfo::{Pid, ProcessesToUpdate, System};

const HELP: &str = "jj - change directories with a directory history\n\nUsage: jj [DIRECTORY]\n       jj --keep [DIRECTORY]\n       jj --pop [NUMBER|--interactive|-i]\n       jj --queue\n       jj --init [--use-aliases] <bash|zsh>\n\nSet up your shell with: eval \"$(jj --init bash)\" (or zsh).\nOptions: -k, --keep; -p, --pop; -q, --queue; -V, --version; -h, --help.\nThe queue is listed most recent first; --pop defaults to 1.\nUse --pop --interactive to select with the arrow keys.\n--keep defaults to the current directory.\nKept directories can be revisited by their final path component.\n";

fn main() {
    if let Err(error) = run() {
        eprintln!("jj: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<_> = env::args_os().skip(1).collect();
    // Normalize the equivalent interactive pop spellings before dispatch.
    if args.first().is_some_and(|a| a == "-pi" || a == "-ip") {
        args.splice(0..1, ["--pop".into(), "--interactive".into()]);
    } else if args
        .first()
        .is_some_and(|a| a == "-i" || a == "--interactive")
        && args.get(1).is_some_and(|a| a == "-p" || a == "--pop")
    {
        args.swap(0, 1);
    }
    if args.first().is_some_and(|a| a == "--init") {
        let use_alias = args.get(1).is_some_and(|a| a == "--use-aliases");
        let shell = if use_alias { args.get(2) } else { args.get(1) };
        if args.len() != if use_alias { 3 } else { 2 }
            || !shell.is_some_and(|s| s == "bash" || s == "zsh")
        {
            return Err("usage: jj --init [--use-aliases] <bash|zsh>".into());
        }
        // The shell owns the queue identifier and performs the actual directory change.
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        println!(
            r#"export JJ_SESSION_ID="{timestamp}-$$"
export JJ_SHELL_PID="$$"
jj() {{
    if [ "$#" -eq 0 ]; then
        builtin cd -- "$HOME"
        return
    fi
    case "$1" in
        --queue|-q|--help|-h|--version|-V|--init) command jj "$@" ;;
        *) local destination
           destination="$(command jj "$@")" || return
           builtin cd -- "$destination" ;;
    esac
}}"#
        );
        if use_alias {
            println!(
                "alias jjq='jj --queue'\nalias jjk='jj --keep'\nalias jjp='jj --pop'\nalias jjpi='jj --pop --interactive'"
            );
        }
        return Ok(());
    }
    if args.first().is_some_and(|a| a == "--version" || a == "-V") {
        println!("jj {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.first().is_some_and(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return Ok(());
    }
    let legacy_queue = env::var_os("JJ_QUEUE_FILE");
    let session_id = env::var("JJ_SESSION_ID").ok().filter(|id| !id.is_empty());
    let queue_id = session_id
        .clone()
        .or_else(|| {
            legacy_queue
                .as_ref()
                .map(|id| id.to_string_lossy().into_owned())
        })
        .ok_or("shell integration is not initialized; run eval \"$(jj --init bash)\" (or zsh)")?;
    let mut db = open_database()?;
    let mut tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    migrate_legacy(
        &tx,
        if session_id.is_none() {
            legacy_queue.as_deref().map(Path::new)
        } else {
            None
        },
    )?;
    if session_id.is_some() {
        maintain_sessions(&tx, &queue_id)?;
    }
    let mut queue = {
        let mut statement = tx.prepare("SELECT path FROM queue WHERE shell = ?1 ORDER BY id")?;
        let entries = statement.query_map([queue_id.as_str()], |row| row.get::<_, String>(0))?;
        entries
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>()
    };
    if args.first().is_some_and(|a| a == "--queue" || a == "-q") {
        if args.len() != 1 {
            return Err("usage: jj --queue".into());
        }
        for (index, path) in queue.iter().rev().enumerate() {
            println!("{}  {}", index + 1, path.display());
        }
        tx.commit()?;
        return Ok(());
    }
    let current = env::current_dir()?;
    let keep = args.first().is_some_and(|a| a == "--keep" || a == "-k");
    let pop = args.first().is_some_and(|a| a == "--pop" || a == "-p");
    let destination = if pop {
        if args.len() > 2 {
            return Err("usage: jj --pop [NUMBER|--interactive|-i]".into());
        }
        let index: usize = match args.get(1) {
            Some(value) if value == "--interactive" || value == "-i" => {
                if queue.is_empty() {
                    return Err("queue is empty".into());
                }
                tx.commit()?;
                let selected = select_queue_entry(&queue)?;
                tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                let current_queue = {
                    let mut statement =
                        tx.prepare("SELECT path FROM queue WHERE shell = ?1 ORDER BY id")?;
                    let entries =
                        statement.query_map([queue_id.as_str()], |row| row.get::<_, String>(0))?;
                    entries
                        .collect::<Result<Vec<_>, _>>()?
                        .into_iter()
                        .map(PathBuf::from)
                        .collect::<Vec<_>>()
                };
                if current_queue != queue {
                    return Err("queue changed while selecting; please try again".into());
                }
                selected
            }
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
        if args.len() > 2 {
            return Err("usage: jj --keep [DIRECTORY]".into());
        }
        let path = canonical_directory(args.get(1).map_or(Path::new("."), Path::new))?;
        tx.execute("DELETE FROM kept WHERE path = ?1", [path.to_str().unwrap()])?;
        tx.execute(
            "INSERT INTO kept(path) VALUES (?1)",
            [path.to_str().unwrap()],
        )?;
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
            let kept = {
                let mut statement = tx.prepare("SELECT path FROM kept ORDER BY id")?;
                let entries = statement.query_map([], |row| row.get::<_, String>(0))?;
                entries
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .map(PathBuf::from)
                    .collect::<Vec<_>>()
            };
            let candidate = kept
                .iter()
                .rev()
                .find(|entry| entry.file_name() == path.file_name() && entry.is_dir());
            canonical_directory(candidate.unwrap_or(&path))?
        } else {
            canonical_directory(&path)?
        }
    };
    if !pop && current != destination {
        queue.push(current);
    }
    tx.execute("DELETE FROM queue WHERE shell = ?1", [queue_id.as_str()])?;
    for path in &queue {
        let value = path.to_str().ok_or("directory names must be valid UTF-8")?;
        tx.execute(
            "INSERT INTO queue(shell, path) VALUES (?1, ?2)",
            params![queue_id.as_str(), value],
        )?;
    }
    tx.commit()?;
    println!("{}", destination.display());
    Ok(())
}

fn state_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let base = match env::var_os("XDG_STATE_HOME").filter(|value| Path::new(value).is_absolute()) {
        Some(value) => PathBuf::from(value),
        None => PathBuf::from(env::var_os("HOME").ok_or("HOME is not set")?).join(".local/state"),
    };
    Ok(base.join("jj"))
}

fn open_database() -> Result<Connection, Box<dyn std::error::Error>> {
    let directory = state_dir()?;
    fs::create_dir_all(&directory)?;
    let db = Connection::open(directory.join("jj.sqlite3"))?;
    db.busy_timeout(Duration::from_secs(5))?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS kept (id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE);
        CREATE TABLE IF NOT EXISTS queue (id INTEGER PRIMARY KEY, shell TEXT NOT NULL, path TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS queue_shell ON queue(shell, id);
        CREATE TABLE IF NOT EXISTS sessions (id TEXT PRIMARY KEY, pid INTEGER NOT NULL, started INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS migrations (name TEXT PRIMARY KEY);")?;
    Ok(db)
}

fn maintain_sessions(db: &Connection, session_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let Some(pid) = env::var("JJ_SHELL_PID")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
    else {
        return Ok(());
    };
    let Ok(mut system) = System::new() else {
        return Ok(());
    };
    system.refresh_processes(ProcessesToUpdate::All, true);
    // If process inspection is restricted, preserve all queues rather than guessing.
    let Some(current) = system.process(Pid::from_u32(pid)) else {
        return Ok(());
    };
    let started = current.start_time();
    if started == 0 {
        return Ok(());
    }
    let stale = {
        let mut stmt = db.prepare("SELECT id, pid, started FROM sessions WHERE id != ?1")?;
        let rows = stmt.query_map([session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let mut stale = Vec::new();
        for row in rows {
            let (id, pid, started) = row?;
            match system.process(Pid::from_u32(pid)) {
                None => stale.push(id),
                Some(process)
                    if process.start_time() != 0
                        && i64::try_from(process.start_time()).ok() != Some(started) =>
                {
                    stale.push(id)
                }
                _ => {}
            }
        }
        stale
    };
    for id in stale {
        db.execute("DELETE FROM queue WHERE shell = ?1", [&id])?;
        db.execute("DELETE FROM sessions WHERE id = ?1", [&id])?;
    }
    db.execute(
        "INSERT OR IGNORE INTO sessions(id, pid, started) VALUES (?1, ?2, ?3)",
        params![session_id, pid, i64::try_from(started)?],
    )?;
    Ok(())
}

fn migrate_legacy(
    db: &Connection,
    queue_path: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let migrated: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM migrations WHERE name = 'kept')",
        [],
        |row| row.get(0),
    )?;
    if !migrated {
        let state_file = state_dir()?.join("kept");
        let base =
            match env::var_os("XDG_DATA_HOME").filter(|value| Path::new(value).is_absolute()) {
                Some(value) => PathBuf::from(value),
                None => PathBuf::from(env::var_os("HOME").ok_or("HOME is not set")?)
                    .join(".local/share"),
            };
        let legacy = if state_file.exists() {
            state_file
        } else {
            base.join("jump-jotter/kept")
        };
        for path in read_queue(&legacy)? {
            let value = path.to_str().ok_or("invalid legacy kept path")?;
            db.execute("DELETE FROM kept WHERE path = ?1", [value])?;
            db.execute("INSERT INTO kept(path) VALUES (?1)", [value])?;
        }
        db.execute("INSERT INTO migrations(name) VALUES ('kept')", [])?;
    }
    let Some(queue_path) = queue_path else {
        return Ok(());
    };
    let id = queue_path.to_string_lossy();
    let marker = format!("queue:{id}");
    let migrated = db
        .query_row(
            "SELECT 1 FROM migrations WHERE name = ?1",
            [&marker],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !migrated {
        for path in read_queue(queue_path)? {
            let value = path.to_str().ok_or("invalid legacy queue path")?;
            db.execute(
                "INSERT INTO queue(shell, path) VALUES (?1, ?2)",
                params![id.as_ref(), value],
            )?;
        }
        db.execute("INSERT INTO migrations(name) VALUES (?1)", [&marker])?;
    }
    Ok(())
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

// The shell captures stdout, so the picker talks directly to the terminal.
fn select_queue_entry(queue: &[PathBuf]) -> Result<usize, Box<dyn std::error::Error>> {
    let mut tty = if unsafe { libc::isatty(libc::STDERR_FILENO) } == 1 {
        let fd = unsafe { libc::dup(libc::STDERR_FILENO) };
        if fd < 0 {
            return Err(io::Error::last_os_error().into());
        }
        unsafe { fs::File::from_raw_fd(fd) }
    } else {
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .map_err(|_| "interactive pop requires a terminal")?
    };
    let fd = tty.as_raw_fd();
    let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
    if unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    let original = unsafe { original.assume_init() };
    let mut raw = original;
    unsafe { libc::cfmakeraw(&mut raw) };
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    struct Restore(libc::c_int, libc::termios);
    impl Drop for Restore {
        fn drop(&mut self) {
            unsafe { libc::tcsetattr(self.0, libc::TCSANOW, &self.1) };
        }
    }
    let _restore = Restore(fd, original);
    let mut selected = 0usize;
    let mut first = 0usize;
    let mut previous_lines = 0usize;
    loop {
        let mut size = std::mem::MaybeUninit::<libc::winsize>::zeroed();
        let (rows, columns) =
            if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, size.as_mut_ptr()) } == 0 {
                let size = unsafe { size.assume_init() };
                (
                    if size.ws_row == 0 {
                        24
                    } else {
                        usize::from(size.ws_row).max(3)
                    },
                    if size.ws_col == 0 {
                        80
                    } else {
                        usize::from(size.ws_col).max(20)
                    },
                )
            } else {
                (24, 80)
            };
        let visible = rows.saturating_sub(3).max(1).min(queue.len());
        if selected < first {
            first = selected;
        }
        if selected >= first + visible {
            first = selected + 1 - visible;
        }
        if previous_lines > 0 {
            write!(tty, "\x1b[{}A", previous_lines)?;
        }
        write!(tty, "\r\x1b[2KSelect: arrows, Enter; Esc cancels\r\n")?;
        for index in first..first + visible {
            let path: String = queue[queue.len() - 1 - index]
                .to_string_lossy()
                .chars()
                .flat_map(|c| {
                    if c.is_control() {
                        c.escape_default().collect::<Vec<_>>()
                    } else {
                        vec![c]
                    }
                })
                .collect();
            let line = format!(
                "{} {}  {}",
                if index == selected { ">" } else { " " },
                index + 1,
                path
            );
            // Limit each row so redrawing stays within the visible region.
            let line: String = line.chars().take(columns.saturating_sub(1)).collect();
            write!(tty, "\r\x1b[2K{line}\r\n")?;
        }
        previous_lines = visible + 1;
        tty.flush()?;
        let mut byte = [0u8; 1];
        tty.read_exact(&mut byte)?;
        match byte[0] {
            b'\r' | b'\n' => return Ok(selected + 1),
            3 | b'q' => return Err("selection cancelled".into()),
            27 => {
                let mut pollfd = libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                if unsafe { libc::poll(&mut pollfd, 1, 50) } <= 0 {
                    return Err("selection cancelled".into());
                }
                tty.read_exact(&mut byte)?;
                if byte[0] == b'[' || byte[0] == b'O' {
                    if unsafe { libc::poll(&mut pollfd, 1, 50) } <= 0 {
                        continue;
                    }
                    tty.read_exact(&mut byte)?;
                    match byte[0] {
                        b'A' => selected = selected.saturating_sub(1),
                        b'B' => selected = (selected + 1).min(queue.len() - 1),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}
