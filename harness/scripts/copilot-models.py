#!/usr/bin/env python3
"""List the GitHub Copilot models your account is actually entitled to.

Prefers the Copilot bearer token from ~/.pi/agent/auth.json, then retries an
unauthorized request with a fresh `omp token github-copilot` credential. The
token's `proxy-ep` claim selects the correct individual/enterprise API host.

Usage:
    just copilot-models            # list every entitled model id
    just copilot-models mai        # filter ids containing "mai" (case-insensitive)
    just copilot-models --json     # raw JSON passthrough
"""

import json
import os
import sys
import subprocess
import urllib.error
import urllib.request

AUTH_PATH = os.path.expanduser("~/.pi/agent/auth.json")
EDITOR_HEADERS = {
    "Copilot-Integration-Id": "vscode-chat",
    "Editor-Version": "vscode/1.107.0",
    "Editor-Plugin-Version": "copilot-chat/0.35.0",
    "User-Agent": "GitHubCopilotChat/0.35.0",
}


def load_pi_token() -> str | None:
    try:
        with open(AUTH_PATH) as auth_file:
            auth = json.load(auth_file)
    except FileNotFoundError:
        return None
    token = auth.get("github-copilot", {}).get("access")
    return token if isinstance(token, str) and token else None


def load_omp_token() -> str:
    try:
        result = subprocess.run(
            ["omp", "token", "github-copilot"],
            check=True,
            capture_output=True,
            text=True,
            timeout=30,
        )
    except (FileNotFoundError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as exc:
        sys.exit(f"❌ unable to refresh the GitHub Copilot token through OMP: {exc}")
    token = result.stdout.strip()
    if not token:
        sys.exit("❌ OMP returned an empty GitHub Copilot token")
    return token


def host_for(token: str) -> str:
    """Pick the API host from the token's proxy-ep claim.

    Enterprise tokens 421 (Misdirected) against api.individual.githubcopilot.com,
    and vice-versa, so derive the host instead of hardcoding it.
    """
    proxy = ""
    for seg in token.split(";"):
        if seg.startswith("proxy-ep="):
            proxy = seg.split("=", 1)[1]
            break
    if "enterprise" in proxy:
        return "api.enterprise.githubcopilot.com"
    if "individual" in proxy:
        return "api.individual.githubcopilot.com"
    # Generic host works for most tokens as a fallback.
    return "api.githubcopilot.com"


def fetch_models(token: str) -> dict:
    host = host_for(token)
    req = urllib.request.Request(
        f"https://{host}/models",
        headers={"Authorization": f"Bearer {token}", **EDITOR_HEADERS},
    )
    with urllib.request.urlopen(req, timeout=30) as resp:
        body = resp.read().decode()
    return {"host": host, "payload": json.loads(body)}


def main() -> None:
    args = sys.argv[1:]
    as_json = "--json" in args
    filters = [a.lower() for a in args if not a.startswith("--")]

    token = load_pi_token() or load_omp_token()
    try:
        result = fetch_models(token)
    except urllib.error.HTTPError as exc:
        if exc.code != 401:
            sys.exit(f"❌ HTTP {exc.code} from {host_for(token)}/models: {exc.read().decode()[:200]}")
        token = load_omp_token()
        try:
            result = fetch_models(token)
        except urllib.error.HTTPError as retry_exc:
            sys.exit(
                f"❌ HTTP {retry_exc.code} from {host_for(token)}/models after OMP token refresh: "
                f"{retry_exc.read().decode()[:200]}"
            )
    payload = result["payload"]

    if as_json:
        print(json.dumps(payload, indent=2))
        return

    ids = sorted({m.get("id", "?") for m in payload.get("data", [])})
    if filters:
        ids = [i for i in ids if any(f in i.lower() for f in filters)]

    print(f"host: {result['host']}")
    print(f"{len(ids)} model id(s):")
    for i in ids:
        print(f"  {i}")
    if not ids:
        print("  (no matches)")


if __name__ == "__main__":
    main()
