#!/usr/bin/env python3
"""Create a new private credential key ring; never overwrite or print key material."""
import argparse
import base64
import json
import os
from pathlib import Path
import secrets
import sys


def create(path, owner_uid=None, owner_gid=None):
    # The existing secret directory must already exist. O_EXCL also rejects a
    # dangling symlink; do not follow or replace any existing destination.
    value = {'active_key': 'v1', 'keys': {'v1': base64.b64encode(secrets.token_bytes(32)).decode('ascii')}}
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, 'w', encoding='utf-8') as stream:
        os.fchmod(stream.fileno(), 0o600)
        if owner_uid is not None or owner_gid is not None:
            os.fchown(stream.fileno(), owner_uid if owner_uid is not None else -1,
                      owner_gid if owner_gid is not None else -1)
        json.dump(value, stream)
        stream.write('\n')
        stream.flush()
        os.fsync(stream.fileno())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--file', type=Path, default=Path(__file__).parent / '.secrets/book_keys.json')
    parser.add_argument('--owner-uid', type=int, help='Linux API UID is 10001; requires chown permission')
    parser.add_argument('--owner-gid', type=int, help='Linux API GID is 10001; requires chown permission')
    args = parser.parse_args()
    if any(value is not None and value < 0 for value in (args.owner_uid, args.owner_gid)):
        parser.error('owner IDs must be non-negative')
    os.umask(0o077)
    create(args.file, args.owner_uid, args.owner_gid)
    print('Credential key file created with mode 0600. Verify UID 10001 can read it; back it up separately.')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError):
        print('Credential key initialization failed; no existing file was overwritten. Keep any new file private.',
              file=sys.stderr)
        sys.exit(1)
