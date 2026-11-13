# Jump Jotter

A directory changer with a directory history.

Install with `cargo install jump-jotter` (or `cargo install --path .` from a
checkout), then add the appropriate line to your
shell configuration (`~/.bashrc` or `~/.zshrc`):

```sh
eval "$(jj --init bash)"  # use zsh instead for zsh
```

Open a new shell or reload the configuration. `jj path` changes directory, and
`jj` goes home directly. Other `jj` changes add the directory you left to that
shell's queue.

- `jj --queue` lists the queue, most recent first, with one-based numbers.
- `jj --pop` returns to the previous directory.
- `jj --pop 3` returns to the directory from three visits ago, removing those three
  entries from the history.
- `jj --keep Downloads` changes to `Downloads` and saves its resolved absolute
  path. `jj --keep .` saves the current directory without moving.
- `jj Downloads` opens a local `Downloads` directory if one exists; otherwise,
  it opens the most recently kept directory named `Downloads`.

Kept directories are shared between shells and persist across sessions in a
SQLite database at `$XDG_STATE_HOME/jj/jj.sqlite3`, or
`~/.local/state/jj/jj.sqlite3` if `XDG_STATE_HOME` is unset or relative.
Existing text-based kept lists and active shell queues are imported on first use.
Only explicitly kept directories are saved.
Names refer to the final component of the resolved path, including when a
directory is kept through a symlink. Paths containing separators are treated
as paths rather than kept-directory names.

Each initialized shell gets a timestamp-and-PID session ID and its own queue in the
database. On later invocations, queues belonging to exited shells are removed
using the shell's PID and process start time. If the current shell cannot be
inspected, cleanup is skipped. A missing
or invalid destination leaves the queue unchanged. Shell integration is needed
because an executable cannot change its parent shell's directory.
