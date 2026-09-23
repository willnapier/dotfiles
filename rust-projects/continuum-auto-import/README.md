# continuum-auto-import

Replaces the formerly host-local nimbini Bash script (no Dotter script mapping existed).
Runs the existing `continuum import --assistant codex|goose` commands, only for
present native stores. Existing log: `~/.local/share/continuum/auto-import.log`.
Records start and final counts; missing stores are explicit skips, failed imports
exit 1 and alert through `notify-user`. Exit 2 is invalid usage. Unit path unchanged.
First install must preserve the unmanaged old script in a private rollback location
before atomically replacing it. Synthetic command-output tests never read sessions.
