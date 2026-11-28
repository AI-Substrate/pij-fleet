# Files

`pij send-file` hands one file, up to 50 MiB, to another seat: on this machine, or on a machine paired with this one. The main case is across machines, where nothing else (SSH, a shared folder) connects them; the file travels over the pairing's own connection, daemon to daemon. `pij send-file --help` and `pij file --help` print the grammar.

## Send one

    pij send-file <seat|role:<id>[/<instance>]|seat@machine> <path> [--note <text>] [--force --reason <text>]

A relative `<path>` is read from your folder. pij refuses a file over 50 MiB with its size, anything that is not a regular file, and a store that would leave free disk under `PIJ_DISK_FLOOR_GB` (default 25 GiB) on the recipient's machine. A `role:` address goes to the role's one live holder on this machine; with no live holder or several it refuses, and a file is never queued for a role. `--force --reason` wakes a cold recipient, as on `pij send`.

The recipient's daemon keeps the file in `~/.pij-rs/transfers/<code>` (file 0600, folder 0700) and sends the recipient an ordinary pij message: who sent it, its name, size and sha256, your note, its code, and the two commands to save or decline it. The code is 128 random bits; it is how the file is named, never its own name.

To `seat@machine` your daemon first takes a snapshot of the file (so the sha256 sent ahead matches the bytes that follow), streams it to the paired daemon (`POST /v1/files/inbound`, under the pairing's key, never through a proxy), and deletes the snapshot once that daemon answers. The paired daemon checks the recipient, a cold wake and its disk floor before it reads a byte, refuses more bytes than were described, and keeps nothing whose sha256 does not match. If the upload fails, nothing is kept on either machine: send it again.

## Receive one

    pij file list
    pij file accept <code> --to <path> [--force]
    pij file decline <code>

`list` shows the files waiting for you: code, name, size, sender and age.

`accept` writes the file to `<path>` (relative to your folder; the folder must exist). A path that already exists is refused unless you add `--force`, and a symlink there is never followed: without `--force` it is refused like any existing path, with it the link itself is replaced by the file, and what it pointed to is untouched. pij writes to a hidden temporary file beside `<path>`, checks the bytes against the sha256 recorded when the file was sent, and only then puts the file in place (0600). Then the stored copy is deleted at once.

`decline` deletes the file unsaved.

Only the seat a file was sent to may accept or decline it. Any other seat is told there is no such file, in the same words as for a code that does not exist, so a code is never confirmed to a seat it was not sent to.

A file is data. pij never opens, runs or inspects it beyond its size and hash.

## How it ends

The sender is told how it ended, by FYI: accepted, declined, or expired. A file nobody accepts within 24 hours is deleted, and on expiry the recipient is told too. Expiry runs when the daemon starts and whenever a file is sent to it; there is no timer. Across machines the recipient's daemon tells the sender's, as an FYI from the recipient seat.

## Cost

No loop, poll, timer or process launch. Each verb is one read of the transfers folder at most. Disk is bounded by 50 MiB per pending file and the 24-hour expiry, and no file is stored that would take free disk under the floor. A cross-machine send is streamed: neither daemon holds the whole file in memory.
