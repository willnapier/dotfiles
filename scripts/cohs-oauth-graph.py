#!/usr/bin/env python3
"""
cohs-oauth-graph — Microsoft 365 OAuth2 token manager for Microsoft Graph API.

Sibling to `cohs-oauth` (which handles Outlook API scopes for IMAP/SMTP).
This script handles the Graph API scopes — specifically `Mail.Send` for
`/me/sendMail`, used by PracticeForge's GraphTransport to send as COHS
while the tenant blocks SMTP AUTH entirely.

Why a separate script with a different client ID:
- Microsoft v2 OAuth2 tokens are single-resource — one token can hold
  scopes for either outlook.office.com OR graph.microsoft.com, not both.
- Thunderbird's public Azure app (used by cohs-oauth) is registered for
  Outlook API only; it can't issue Graph tokens.
- Microsoft Graph CLI's public app (client ID below) IS registered for
  Graph resources and is tenant-agnostic — the canonical borrow-a-public-
  client pattern for Graph-speaking CLI tools.

Subcommands (same shape as cohs-oauth):
    init     — device code flow to acquire initial Graph tokens (interactive)
    refresh  — silently refresh access token (for timer use)
    show     — print current access token to stdout (refresh if expired)
    status   — print token state for debugging
"""

import json
import platform
import subprocess
import sys
from pathlib import Path

import msal

# Microsoft Graph CLI's public app — tenant-agnostic, pre-registered for
# Graph resources. Published in Microsoft's docs as a safe public-client
# default for CLI tools that speak Graph.
CLIENT_ID = "14d82eec-204b-4c2f-b7e8-296a70dab67e"  # Microsoft Graph CLI
AUTHORITY = "https://login.microsoftonline.com/common"
SCOPES = [
    "https://graph.microsoft.com/Mail.Send",
    # offline_access is added automatically by msal for refresh tokens.
]

# Distinct cache path from cohs-oauth — they have different accounts and
# different scope consents, so msal must not share state between them.
CACHE_PATH = Path.home() / ".config" / "cohs-auth-graph" / "cache.bin"
KEYCHAIN_SERVICE = "himalaya-cli"
KEYCHAIN_ACCESS = "cohs-m365-graph-access"
KEYCHAIN_REFRESH = "cohs-m365-graph-refresh"

# Platform dispatch: see cohs-oauth.py for rationale. macOS uses `security`,
# Linux uses `secret-tool` (libsecret) with matching attribute schema.
IS_MACOS = platform.system() == "Darwin"


def load_cache() -> msal.SerializableTokenCache:
    cache = msal.SerializableTokenCache()
    if CACHE_PATH.exists():
        cache.deserialize(CACHE_PATH.read_text())
    return cache


def save_cache(cache: msal.SerializableTokenCache) -> None:
    if cache.has_state_changed:
        CACHE_PATH.parent.mkdir(parents=True, exist_ok=True)
        CACHE_PATH.write_text(cache.serialize())
        CACHE_PATH.chmod(0o600)


def build_app(cache: msal.SerializableTokenCache) -> msal.PublicClientApplication:
    return msal.PublicClientApplication(CLIENT_ID, authority=AUTHORITY, token_cache=cache)


def keychain_set(account: str, value: str) -> None:
    if IS_MACOS:
        subprocess.run(
            ["security", "add-generic-password", "-U", "-s", KEYCHAIN_SERVICE, "-a", account, "-w", value],
            check=True, capture_output=True,
        )
    else:
        subprocess.run(
            ["secret-tool", "store", "--label", f"{KEYCHAIN_SERVICE}:{account}",
             "service", KEYCHAIN_SERVICE, "account", account],
            input=value, check=True, capture_output=True, text=True,
        )


def keychain_get(account: str) -> str | None:
    if IS_MACOS:
        r = subprocess.run(
            ["security", "find-generic-password", "-s", KEYCHAIN_SERVICE, "-a", account, "-w"],
            capture_output=True, text=True,
        )
    else:
        r = subprocess.run(
            ["secret-tool", "lookup", "service", KEYCHAIN_SERVICE, "account", account],
            capture_output=True, text=True,
        )
    return r.stdout.strip() if r.returncode == 0 else None


def store_tokens(result: dict) -> None:
    if "access_token" in result:
        keychain_set(KEYCHAIN_ACCESS, result["access_token"])
    if "refresh_token" in result:
        keychain_set(KEYCHAIN_REFRESH, result["refresh_token"])


def cmd_init() -> int:
    cache = load_cache()
    app = build_app(cache)
    flow = app.initiate_device_flow(scopes=SCOPES)
    if "user_code" not in flow:
        print(f"Failed to initiate device flow: {json.dumps(flow, indent=2)}", file=sys.stderr)
        return 1
    print(flow["message"], file=sys.stderr)
    result = app.acquire_token_by_device_flow(flow)
    save_cache(cache)
    if "access_token" in result:
        store_tokens(result)
        print(f"Success. Graph access token stored; expires in {result.get('expires_in', '?')} seconds.", file=sys.stderr)
        return 0
    print(f"Failed: {json.dumps(result, indent=2)}", file=sys.stderr)
    return 1


def cmd_refresh() -> int:
    cache = load_cache()
    app = build_app(cache)
    accounts = app.get_accounts()
    if not accounts:
        print("No cached account. Run 'cohs-oauth-graph init' first.", file=sys.stderr)
        return 1
    result = app.acquire_token_silent(SCOPES, account=accounts[0])
    save_cache(cache)
    if result and "access_token" in result:
        store_tokens(result)
        print("Refreshed.", file=sys.stderr)
        return 0
    print(f"Silent refresh failed. Run 'cohs-oauth-graph init' to re-auth. Detail: {result}", file=sys.stderr)
    return 1


def cmd_show() -> int:
    rc = cmd_refresh()
    if rc != 0:
        return rc
    tok = keychain_get(KEYCHAIN_ACCESS)
    if not tok:
        print("No Graph access token in keychain after refresh (unexpected).", file=sys.stderr)
        return 1
    print(tok)
    return 0


def cmd_status() -> int:
    cache = load_cache()
    app = build_app(cache)
    accounts = app.get_accounts()
    print(f"Cache path: {CACHE_PATH} (exists: {CACHE_PATH.exists()})")
    print(f"Accounts: {[a.get('username') for a in accounts]}")
    acc = keychain_get(KEYCHAIN_ACCESS)
    ref = keychain_get(KEYCHAIN_REFRESH)
    print(f"Keychain {KEYCHAIN_ACCESS}: {'present' if acc else 'MISSING'} ({len(acc) if acc else 0} chars)")
    print(f"Keychain {KEYCHAIN_REFRESH}: {'present' if ref else 'MISSING'} ({len(ref) if ref else 0} chars)")
    return 0


COMMANDS = {"init": cmd_init, "refresh": cmd_refresh, "show": cmd_show, "status": cmd_status}


def main() -> int:
    if len(sys.argv) != 2 or sys.argv[1] not in COMMANDS:
        print(f"Usage: {sys.argv[0]} {{{'|'.join(COMMANDS)}}}", file=sys.stderr)
        return 2
    return COMMANDS[sys.argv[1]]()


if __name__ == "__main__":
    sys.exit(main())
