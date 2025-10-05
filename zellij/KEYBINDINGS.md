# Zellij Keybinding Translation Sheet
**Tutorial (Default) vs Your Custom Colemak-DH Config**

---

## 🔑 CRITICAL DIFFERENCE: Main Modifier Key

**Tutorial (Default):** Uses `Alt` as primary modifier for shortcuts
**Your Config:** Uses `Ctrl` for mode switching (Alt removed to avoid conflicts)

---

## Mode Switching Commands

| Action | Tutorial (Default) | Your Custom Config | Notes |
|--------|-------------------|-------------------|-------|
| **Pane mode** | `Ctrl p` | `Ctrl p` | ✅ Same |
| **Tab mode** | `Ctrl t` | `Ctrl t` | ✅ Same |
| **Resize mode** | `Ctrl n` | `Ctrl r` | ⚠️ **DIFFERENT** |
| **Scroll mode** | `Ctrl s` | `Ctrl s` | ✅ Same |
| **Session mode** | `Ctrl o` | `Ctrl o` | ✅ Same |
| **Move mode** | `Ctrl h` | `Ctrl h` | ✅ Same |
| **Locked mode** | `Ctrl g` | `Ctrl g` | ✅ Same |

---

## Quick Actions in Normal Mode

| Action | Tutorial (Default) | Your Custom Config | Notes |
|--------|-------------------|-------------------|-------|
| **New pane** | `Alt n` | `Ctrl n` | ⚠️ **Uses Ctrl instead of Alt** |
| **Navigate focus** | `Alt h/j/k/l` | `Shift + Arrow keys` | ⚠️ **VERY DIFFERENT** |
| **Switch tabs** | `Alt [/]` | `Alt n/o` | ⚠️ Different keys (Colemak-DH) |
| **Resize panes** | `Alt +/-` | `Ctrl +/-` | ⚠️ **Uses Ctrl, smaller increments** |
| **Toggle floating** | `Alt f` | ❌ Removed | Enter pane mode (`Ctrl p`) then `w` |
| **Detach session** | `Ctrl o` then `d` | `Ctrl Alt d` | ⚠️ **DIFFERENT** (moved to avoid Helix conflict) |
| **Quit** | - | `Ctrl q` | Added in your config |

---

## Pane Mode (`Ctrl p`)

| Action | Tutorial (Default) | Your Custom Config | Notes |
|--------|-------------------|-------------------|-------|
| **Move focus** | `h/j/k/l` (vim) | `n/e/i/o` (Colemak) | ⚠️ **Colemak navigation** |
| | | `Arrow keys` | ✅ Arrow fallback available |
| **New pane left** | `h` or `p` | `N` (capital) | ⚠️ Different |
| **New pane down** | `j` or `n` | `E` (capital) | ⚠️ Different |
| **New pane up** | `k` | `I` (capital) | ⚠️ Different |
| **New pane right** | `l` | `O` (capital) | ⚠️ Different |
| **Close pane** | `x` | `x` | ✅ Same |
| **Fullscreen toggle** | `f` | `f` | ✅ Same |
| **Rename pane** | `c` | `r` | ⚠️ **DIFFERENT** |
| **Break to new tab** | `b` | `b` | ✅ Same |
| **Floating toggle** | `w` | `w` | ✅ Same |
| **Exit pane mode** | `Esc` or `Ctrl c` | `Esc` or `Ctrl c` | ✅ Same |

---

## Tab Mode (`Ctrl t`)

| Action | Tutorial (Default) | Your Custom Config | Notes |
|--------|-------------------|-------------------|-------|
| **Previous tab** | `h` or `Left` | `n` or `Left` | ⚠️ Colemak `n` instead of `h` |
| **Next tab** | `l` or `Right` | `o` or `Right` | ⚠️ Colemak `o` instead of `l` |
| **First tab** | - | `e` or `Down` | Extra feature in your config |
| **Last tab** | - | `i` or `Up` | Extra feature in your config |
| **New tab** | `n` | `t` | ⚠️ **DIFFERENT** |
| **Close tab** | `x` | `x` | ✅ Same |
| **Rename tab** | `r` | `r` | ✅ Same |
| **Go to tab 1-9** | `1-9` | `1-9` | ✅ Same |
| **Toggle last tab** | `Tab` | `Tab` | ✅ Same |
| **Sync tab** | `s` | `s` | ✅ Same |
| **Exit tab mode** | `Esc` or `Ctrl c` | `Esc` or `Ctrl c` | ✅ Same |

---

## Resize Mode

| Action | Tutorial (Default) | Your Custom Config | Notes |
|--------|-------------------|-------------------|-------|
| **Enter resize** | `Ctrl n` | `Ctrl r` | ⚠️ **DIFFERENT KEY** |
| **Resize** | `h/j/k/l` (vim) | `n/e/i/o` (Colemak) | ⚠️ **Colemak navigation** |
| | | `Arrow keys` | ✅ Arrow fallback available |
| **Bigger steps** | - | `N/E/I/O` (capitals) | Extra: 3x bigger resize |
| **Increase** | `+` or `=` | `+` or `=` | ✅ Same |
| **Decrease** | `-` | `-` | ✅ Same |
| **Exit resize** | `Esc` or `Ctrl c` | `Esc` or `Ctrl c` | ✅ Same |

---

## Scroll Mode (`Ctrl s`)

| Action | Tutorial (Default) | Your Custom Config | Notes |
|--------|-------------------|-------------------|-------|
| **Scroll down** | `j` or `Down` | `e/j` or `Down` | Colemak `e` added |
| **Scroll up** | `k` or `Up` | `i/k` or `Up` | Colemak `i` added |
| **Page down** | `Ctrl f` / `PageDown` | `o` / `PageDown` | Colemak `o` added |
| **Page up** | `Ctrl b` / `PageUp` | `n` / `PageUp` | Colemak `n` added |
| **Half page down** | `d` or `Ctrl d` | `d` or `Ctrl d` | ✅ Same |
| **Half page up** | `u` or `Ctrl u` | `u` or `Ctrl u` | ✅ Same |
| **Top** | `g` | `g` | ✅ Same |
| **Bottom** | `G` | `G` | ✅ Same |
| **Search** | `s` or `/` or `Ctrl f` | `s` or `Ctrl f` | ✅ Same |
| **Exit scroll** | `Esc` / `Ctrl c` / `Enter` / `Space` | `Esc` / `Ctrl c` / `Enter` / `Space` | ✅ Same |

---

## Session Mode (`Ctrl o`)

| Action | Tutorial (Default) | Your Custom Config | Notes |
|--------|-------------------|-------------------|-------|
| **Detach** | `d` | `d` | ✅ Same |
| **Session manager** | `w` | `w` | ✅ Same |
| **Config/Create** | `c` | `c` | ✅ Same |
| **Exit session** | `Esc` or `Ctrl c` | `Esc` or `Ctrl c` | ✅ Same |

---

## Move Mode (`Ctrl h`)

| Action | Tutorial (Default) | Your Custom Config | Notes |
|--------|-------------------|-------------------|-------|
| **Move pane** | `n/p` | `n/p` | ✅ Same |
| **Move direction** | `h/j/k/l` | `h/j/k/l` | ✅ Same (vim-style kept) |
| **Break to new tab** | - | `t` | ✅ Extra feature |
| **Exit move** | `Esc` or `Ctrl c` | `Esc` or `Ctrl c` | ✅ Same |

---

## 🎯 Quick Reference Guide

### When Tutorial Says → You Do

**"Press `Alt n` for new pane"** → `Ctrl n`

**"Press `Alt h/j/k/l` to navigate"** → `Shift + Arrow keys`

**"Press `Ctrl n` for resize mode"** → `Ctrl r`

**"Use `h/j/k/l` in a mode"** → Use `n/e/i/o` (Colemak) or arrow keys

---

## 💡 Key Patterns to Remember

### Colemak-DH Navigation (NEIO replaces HJKL)
- `n` = left (like vim `h`)
- `e` = down (like vim `j`)
- `i` = up (like vim `k`)
- `o` = right (like vim `l`)

### Your Config Philosophy
- **Ctrl** for mode switching (not Alt)
- **Shift + Arrows** for focus navigation in normal mode
- **Arrow keys always work** as fallback in all modes
- **Alt shortcuts removed** - replaced by universal CLI tools (`fsh`, `fwl`, `fcit`, `fsem`, `fsearch`)

### Resize Behavior Notes
- **`Ctrl +/-` in normal mode**: Non-directional, resizes both dimensions, smaller increments (`stacked_resize false`)
- **`Ctrl r` then directional keys**: Directional control, but may require switching panes to grow certain edges
- **Capitals (`N/E/I/O`) in resize mode**: 3x bigger steps

### Status Bar Limitation
- ⚠️ The status bar shows mode-switching keys but **NOT** the actions within each mode
- You'll need this cheat sheet to remember what keys do once you're in a mode
- Example: Status bar shows `<p> PANE` but doesn't show that `n/e/i/o` moves focus within pane mode

---

## 🖥️ WezTerm Font Size Controls

**macOS:**
- `Cmd -` → Decrease font
- `Cmd +` → Increase font
- `Cmd 0` → Reset font

**Linux:**
- `Alt -` → Decrease font
- `Alt +` → Increase font
- `Alt 0` → Reset font

---

**Last Updated:** 2025-10-04
