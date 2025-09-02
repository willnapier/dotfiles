# Helix Cursor Positioning Issues - Troubleshooting Guide

## Common Issue: Cursor Opens at Top of File Instead of Specified Line

### Symptom
When opening a file in Helix with line positioning (e.g., `hx file.md:3`), the cursor appears at the top of the file instead of the specified line.

## Root Causes & Solutions

### 1. Terminal Multiplexer Interference (Zellij/Tmux)

**Issue**: Line positioning syntax (`file:line:column`) may not work correctly inside terminal multiplexers like Zellij.

**Diagnostic Steps**:
1. Test if the command works outside the multiplexer
2. Test if it fails inside the multiplexer
3. Check environment variables: `echo $ZELLIJ` or `echo $TMUX`

**Solution**:
```bash
# Detect if inside multiplexer and adjust behavior
if [[ -n "$ZELLIJ" ]]; then
    # Zellij-specific workaround
    exec hx "$FILE:$LINE:1"  # Try column 1 instead of 0
else
    # Regular terminal
    exec hx "$FILE:$LINE:0"
fi
```

### 2. Layout Files Bypassing Script Logic

**Issue**: Zellij/Tmux layouts may call `hx` directly instead of using wrapper scripts that handle cursor positioning.

**Example Problem**:
```kdl
// BAD - Bypasses cursor positioning logic
args "-c" "hx (/path/to/daily-note --print-path)"

// GOOD - Uses the script with positioning logic
args "-c" "/path/to/daily-note"
```

**Diagnostic Steps**:
1. Check layout files in `~/.config/zellij/layouts/`
2. Search for direct `hx` calls: `grep -r "hx.*daily-note" ~/.config/zellij/layouts/`
3. Look for `--print-path` usage that bypasses the main script

**Solution**:
Replace direct `hx` calls with the wrapper script that includes positioning logic.

### 3. File Structure Issues

**Issue**: The target line may not exist if the file is truncated or malformed.

**Diagnostic Steps**:
```bash
# Check file line count
wc -l "$FILE"

# Check file structure
cat -n "$FILE"

# Verify file has expected content
hexdump -C "$FILE" | head -20
```

**Solution**:
Ensure the file has the expected number of lines. Recreate from template if necessary.

### 4. Template Processing Issues

**Issue**: When using templates with cursor markers, the marker might not be processed correctly.

**Example Template**:
```markdown
# {{date}} {{hdate}}

<cursor>

```

**Solution**:
```bash
# Track cursor position before removing marker
CURSOR_LINE=$(grep -n "<cursor>" "$TEMPLATE" | cut -d: -f1)

# Process template and remove marker
sed -e "s/<cursor>//g" "$TEMPLATE" > "$OUTPUT"

# Open at the line where cursor was
exec hx "$OUTPUT:$CURSOR_LINE"
```

## Debugging Checklist

### 1. Add Debug Output
```bash
echo "DEBUG: CURSOR_LINE = '$CURSOR_LINE'" >&2
echo "DEBUG: FILE = '$FILE'" >&2
echo "DEBUG: File exists: $(test -f "$FILE" && echo YES || echo NO)" >&2
echo "DEBUG: Line count: $(wc -l < "$FILE")" >&2
echo "DEBUG: Executing: hx \"$FILE:$CURSOR_LINE\"" >&2
```

### 2. Test Different Environments
- [ ] Direct terminal (WezTerm, Ghostty, etc.)
- [ ] Inside Zellij (`zj` command)
- [ ] Inside Tmux
- [ ] Through SSH session
- [ ] Via shell script
- [ ] Via layout/session manager

### 3. Test Different Syntaxes
```bash
# Different line positioning syntaxes
hx file.md:3          # Line 3, default column
hx file.md:3:0        # Line 3, column 0
hx file.md:3:1        # Line 3, column 1
hx "file.md:3"        # Quoted version
```

### 4. Check Configuration Files
- [ ] Zellij layouts: `~/.config/zellij/layouts/*.kdl`
- [ ] Tmux config: `~/.tmux.conf`
- [ ] Helix config: `~/.config/helix/config.toml`
- [ ] Shell aliases/functions that might wrap `hx`

## Common Patterns to Look For

### In Zellij Layouts
```kdl
// WRONG - Bypasses positioning
command "nu"
args "-c" "hx $(script --print-path)"

// CORRECT - Uses full script
command "nu"
args "-c" "/path/to/script"
```

### In Shell Scripts
```bash
# WRONG - Assumes line positioning always works
exec hx "$FILE:3"

# CORRECT - Handles different environments
if [[ -n "$ZELLIJ" ]]; then
    # Workaround for Zellij
    exec hx "$FILE:3:1"
else
    exec hx "$FILE:3"
fi
```

## Quick Fixes

### Force Cursor Position in Zellij
If line positioning isn't working in Zellij, try:
1. Using column 1 instead of 0: `file:3:1`
2. Opening file without position, then manually navigating
3. Using the script directly instead of layout commands

### Verify Script is Being Called
```bash
# Add to beginning of script
echo "Script is being executed!" >&2

# Check if seen when running from Zellij
```

## Prevention

1. **Always test cursor positioning in all environments** where the script will run
2. **Use wrapper scripts** instead of direct `hx` calls in layouts
3. **Add debug output** during development to understand execution flow
4. **Document expected behavior** in script comments
5. **Test with different file structures** (empty files, long files, etc.)

## Related Files
- `/Users/williamnapier/dotfiles/scripts/daily-note` - Main daily note script with cursor positioning
- `/Users/williamnapier/.config/zellij/layouts/laptop.kdl` - Laptop layout configuration
- `/Users/williamnapier/.config/zellij/layouts/desktop-quarters.kdl` - Desktop layout configuration
- `/Users/williamnapier/Obsidian.nosync/Forge/Areas/Obsidian/Templates/DayPage.md` - Template with cursor marker

## Key Learning
**Most cursor positioning issues come from indirection** - when layouts, aliases, or wrapper scripts call commands in ways that bypass the positioning logic. Always trace the full execution path from user action to final `hx` invocation.