#!/usr/bin/env python3
"""
YouTube cookie rotator using Camoufox (hardened Firefox).

Bootstrap (first run — imports existing cookies into persistent browser profile):
  python rotate.py --bootstrap
  Reads YTDLP_COOKIES env var (base64 Netscape format), imports into the browser
  profile stored on Azure Files, visits YouTube to trigger session refresh, exports
  renewed cookies, and updates the lytt Container App secret.

Rotation (weekly scheduled job — no args):
  python rotate.py
  Loads the existing authenticated profile from Azure Files, visits YouTube,
  exports fresh cookies, updates the lytt Container App secret, and triggers
  a new lytt revision so the secret takes effect immediately.

Environment variables:
  AZURE_SUBSCRIPTION_ID   Azure subscription ID (required)
  YTDLP_COOKIES           base64-encoded Netscape cookies (required for --bootstrap)
"""

import argparse
import asyncio
import base64
import os
import sys
import time
from pathlib import Path

PROFILE_DIR = Path("/mnt/profile")
RESOURCE_GROUP = "dyngeseth-rg"
CONTAINER_APP_NAME = "lytt"
SECRET_NAME = "ytdlpcookiesv2"

# Domains whose cookies yt-dlp needs for YouTube authentication
YOUTUBE_DOMAINS = {
    ".youtube.com",
    "youtube.com",
    ".google.com",
    "google.com",
    ".ggpht.com",
    ".gstatic.com",
    ".ytimg.com",
}


def cookies_to_netscape(cookies: list[dict]) -> str:
    """Convert Playwright cookie dicts to Netscape cookie file format for yt-dlp."""
    lines = ["# Netscape HTTP Cookie File"]
    for c in cookies:
        domain = c["domain"]
        if not any(domain == d or domain.endswith(d) for d in YOUTUBE_DOMAINS):
            continue
        flag = "TRUE" if domain.startswith(".") else "FALSE"
        secure = "TRUE" if c.get("secure") else "FALSE"
        expires = int(c.get("expires", 0))
        if expires < 0:
            # Session cookies: set 1-year expiry so yt-dlp accepts them
            expires = int(time.time()) + 60 * 60 * 24 * 365
        lines.append(
            f"{domain}\t{flag}\t{c['path']}\t{secure}\t{expires}\t{c['name']}\t{c['value']}"
        )
    return "\n".join(lines) + "\n"


def netscape_to_playwright(text: str) -> list[dict]:
    """Parse Netscape cookie file into Playwright cookie dicts for seeding a context."""
    cookies = []
    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split("\t")
        if len(parts) < 7:
            continue
        domain, _flag, path, secure, expires, name, value = parts[:7]
        cookies.append({
            "name": name,
            "value": value,
            "domain": domain,
            "path": path,
            "expires": int(expires),
            "httpOnly": "#HttpOnly_" in line,
            "secure": secure == "TRUE",
            "sameSite": "None",
        })
    return cookies


async def run(bootstrap: bool) -> None:
    from camoufox.async_api import AsyncCamoufox

    PROFILE_DIR.mkdir(parents=True, exist_ok=True)

    seed_cookies: list[dict] | None = None
    if bootstrap:
        raw_b64 = os.environ.get("YTDLP_COOKIES", "").strip()
        if not raw_b64:
            print("[rotator] ERROR: YTDLP_COOKIES env var not set; required for --bootstrap", file=sys.stderr)
            sys.exit(1)
        netscape = base64.b64decode(raw_b64).decode("utf-8", errors="replace")
        seed_cookies = netscape_to_playwright(netscape)
        print(f"[rotator] Bootstrap: seeding {len(seed_cookies)} cookies into profile", file=sys.stderr)

    async with AsyncCamoufox(
        headless=True,
        persistent_context=True,
        user_data_dir=str(PROFILE_DIR),
        geoip=True,
    ) as context:
        if seed_cookies:
            await context.add_cookies(seed_cookies)

        page = await context.new_page()

        print("[rotator] Visiting youtube.com ...", file=sys.stderr)
        await page.goto("https://www.youtube.com", wait_until="domcontentloaded", timeout=60_000)
        # Wait for YouTube to settle and issue refreshed Set-Cookie headers
        await page.wait_for_timeout(5_000)

        # Sanity check: logged-in users see an avatar/account button
        avatar = await page.query_selector("button[aria-label*='Account']")
        if avatar:
            print("[rotator] Session looks active (account button found)", file=sys.stderr)
        else:
            print("[rotator] WARNING: account button not found — may not be logged in", file=sys.stderr)

        cookies = await context.cookies(["https://www.youtube.com", "https://google.com"])
        print(f"[rotator] Collected {len(cookies)} cookies", file=sys.stderr)

    netscape_text = cookies_to_netscape(cookies)
    youtube_count = netscape_text.count("\n") - 1  # exclude header line
    print(f"[rotator] Filtered to {youtube_count} YouTube/Google cookies", file=sys.stderr)

    cookies_b64 = base64.b64encode(netscape_text.encode()).decode()
    update_secret(cookies_b64)
    print("[rotator] Done.", file=sys.stderr)


def update_secret(cookies_b64: str) -> None:
    """Update YTDLP_COOKIES in the lytt Container App via Managed Identity."""
    from azure.identity import ManagedIdentityCredential
    from azure.mgmt.appcontainers import ContainerAppsAPIClient
    from azure.mgmt.appcontainers.models import Secret

    subscription_id = os.environ.get("AZURE_SUBSCRIPTION_ID")
    if not subscription_id:
        print("[rotator] ERROR: AZURE_SUBSCRIPTION_ID env var not set", file=sys.stderr)
        sys.exit(1)

    credential = ManagedIdentityCredential()
    client = ContainerAppsAPIClient(credential, subscription_id)

    print(f"[rotator] Fetching current definition of Container App '{CONTAINER_APP_NAME}' ...", file=sys.stderr)
    app = client.container_apps.get(RESOURCE_GROUP, CONTAINER_APP_NAME)

    # Replace the target secret, keep all others intact
    secrets = [s for s in (app.configuration.secrets or []) if s.name != SECRET_NAME]
    secrets.append(Secret(name=SECRET_NAME, value=cookies_b64))
    app.configuration.secrets = secrets

    # Force a new revision so the updated secret is picked up immediately
    suffix = str(int(time.time()))
    app.template.revision_suffix = suffix

    print(f"[rotator] Updating secret and creating revision (suffix={suffix}) ...", file=sys.stderr)
    client.container_apps.begin_create_or_update(RESOURCE_GROUP, CONTAINER_APP_NAME, app).result()
    print(f"[rotator] Secret '{SECRET_NAME}' updated and new revision deployed.", file=sys.stderr)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Refresh YouTube cookies for yt-dlp via Camoufox")
    parser.add_argument(
        "--bootstrap",
        action="store_true",
        help="First-run mode: seed YTDLP_COOKIES env var into the persistent browser profile",
    )
    args = parser.parse_args()
    # Auto-detect bootstrap: if no Firefox profile exists yet, seed from YTDLP_COOKIES env var
    is_new_profile = not (PROFILE_DIR / "prefs.js").exists()
    asyncio.run(run(bootstrap=args.bootstrap or is_new_profile))
