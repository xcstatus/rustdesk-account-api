#!/usr/bin/env python3
"""Read-only, redacted server evidence for the explicitly authorized GUI test."""
import argparse
import datetime
import json
from pathlib import Path
import sqlite3
from urllib.parse import quote


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--marker", default="M0-本机实测")
    args = parser.parse_args()
    manifest = json.loads((Path(__file__).parent / ".runtime/manual-client.json").read_text())
    database = Path(manifest["database_directory"]) / "accounts.sqlite"
    connection = sqlite3.connect("file:" + quote(str(database)) + "?mode=ro", uri=True)
    now = int(datetime.datetime.now(datetime.timezone.utc).timestamp())
    evidence = {"observed_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                "run_namespace": manifest["run_namespace"], "api_url": manifest["api_url"],
                "marker_alias": args.marker, "accounts": []}
    for user_id, source, role, disabled, admitted in connection.execute(
            "SELECT id,source,role,disabled,admitted FROM users ORDER BY source,id"):
        counts = connection.execute(
            "SELECT COUNT(*),COALESCE(SUM(CASE WHEN revoked=0 AND expires_at>? THEN 1 ELSE 0 END),0),"
            "COALESCE(SUM(revoked),0) FROM sessions WHERE user_id=?", (now, user_id)).fetchone()
        snapshot = json.loads(connection.execute("SELECT snapshot FROM books WHERE user_id=?", (user_id,)).fetchone()[0])
        peers = snapshot.get("peers", [])
        evidence["accounts"].append({"user_id": user_id, "source": source, "role": role,
            "disabled": bool(disabled), "admitted": bool(admitted),
            "sessions_total": counts[0], "sessions_active": counts[1], "sessions_revoked": counts[2],
            "peer_count": len(peers), "marker_present": any(p.get("alias") == args.marker for p in peers),
            "credential_fields_stored": any("hash" in p or "password" in p for p in peers)})
    print(json.dumps(evidence, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
