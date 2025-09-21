#!/usr/bin/env nu

# Yazi Project Content Search - Native Nushell Re-nu Version
# Wrapper for content search mode of yazi-search-renu.nu
# Re-nu Phase 2: Productivity Script Excellence
# Created: 2025-09-10

def main [search_term: string] {
    # Use the unified yazi-search-renu.nu with content mode
    nu /dotfiles/scripts/yazi-search-renu.nu content $search_term
}