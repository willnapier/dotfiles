# CLAUDE.md — Dotfiles Repo Constraints

## Dotter Change Procedure

1. **Add/edit files** in `~/dotfiles/` — never edit targets in `~/.config/` or `~/.local/bin/`
2. **Register new files** in `.dotter/global.toml` under the appropriate section
3. **Deploy**: `cd ~/dotfiles; dotter deploy`
4. **Verify**: test the actual functionality, then `dotter-orphan-detector-v2`
5. **Commit**: `git add -A; git commit`

Never use directory-level symlinks (`type = "symbolic"` on directories). Always use individual file entries.

## Cross-Platform Rules

- **Shebangs**: Always `#!/usr/bin/env nu` — never platform-specific paths
- **LaunchAgent PATH**: macOS launchd doesn't inherit user PATH. Add `EnvironmentVariables` with explicit paths to `nu` in plists
- **Systemd ExecStart**: Use full paths or ensure `~/.local/bin` is in the service's `Environment=`
- **Dotter sections**: Shared scripts go in `[shared.files]`. Platform-specific configs go in `[macos.files]` or `[linux.files]`

## Pre-Commit Discipline

Before committing scripts, verify they parse:
```
nu -c 'nu-check <script>'
```

## Reference

- Reusable lessons: `~/Assistants/shared/LESSONS-LEARNED.md`
- Session history archive: `CLAUDE-ARCHIVE.md` (this repo)
- System priorities: `~/Assistants/shared/PRIORITIES-AND-INTENT.md`
