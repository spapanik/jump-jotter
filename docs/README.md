# Jump Jotter

A directory changer with a directory history.

Install with `cargo install jump-jotter` (or `cargo install --path .` from a
checkout), then add the appropriate line to your
shell configuration (`~/.bashrc` or `~/.zshrc`):

```sh
eval "$(jj --init bash)"  # use zsh instead for zsh
```

Open a new shell or reload the configuration. `jj path` changes directory, and
`jj` goes home. Each change adds the directory you left to that shell's queue.

- `jj --queue` lists the queue, most recent first, with one-based numbers.
- `jj --pop` returns to the previous directory.
- `jj --pop 3` returns to the directory from three visits ago, removing those three
  entries from the history.
- `jj --keep Downloads` changes to `Downloads` and saves its resolved absolute
  path. `jj --keep .` saves the current directory without moving.
- `jj Downloads` opens a local `Downloads` directory if one exists; otherwise,
  it opens the most recently kept directory named `Downloads`.

Kept directories are shared between shells and persist across sessions in
`$XDG_DATA_HOME/jump-jotter/kept`, or `~/.local/share/jump-jotter/kept` if
`XDG_DATA_HOME` is unset or relative. Only explicitly kept directories are saved.
Names refer to the final component of the resolved path, including when a
directory is kept through a symlink. Paths containing separators are treated
as paths rather than kept-directory names.

Each initialized shell has its own queue, stored in a temporary file. A missing
or invalid destination leaves the queue unchanged. Shell integration is needed
because an executable cannot change its parent shell's directory.
