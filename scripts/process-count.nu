#!/usr/bin/env nu
# Universal process counting utility using native Nushell

# Count processes by name pattern
def main [pattern: string] {
    ps | where name =~ $pattern | length
}

# Get detailed process info by pattern
def details [pattern: string] {
    ps | where name =~ $pattern | select name pid cpu mem
}

# Check if process is running
def running [pattern: string] {
    (ps | where name =~ $pattern | length) > 0
}