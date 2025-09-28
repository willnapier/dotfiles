# Zotero Automation Files

This directory contains the source files for Zotero PDF automation on macOS.

## Files

### Move PDFs to ZoteroImport Direct.applescript
**Purpose**: AppleScript for Folder Actions that automatically moves PDFs to ~/Documents/ZoteroImport/
**Status**: Compiled version installed at `~/Library/Scripts/Folder Action Scripts/Move PDFs to ZoteroImport Direct.scpt`
**Usage**: Attach as Folder Action to Downloads folder to auto-move research PDFs

### Zotero Import Service.workflow
**Purpose**: macOS Service workflow for Zotero import automation
**Status**: Source file - may need to be installed in ~/Library/Services/ if using

## Related Scripts
Additional Zotero automation scripts are in `~/dotfiles/scripts/`:
- `zotero-pdf-watcher-renu` - Monitors for new PDFs
- `zotero-bridge-renu` - Bridges between file system and Zotero

## Installation Notes
The AppleScript is already compiled and installed in the system Folder Actions location.
These source files are kept here for:
1. Version control
2. Documentation
3. Reinstallation if needed

## Active Folder Actions
Check Folder Actions setup:
```bash
ls -la ~/Library/Scripts/"Folder Action Scripts"
```

The compiled `.scpt` files in that directory are the active versions used by macOS.