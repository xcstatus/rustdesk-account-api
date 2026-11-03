#!/usr/bin/env python3
"""Create private secret files once; never print their values."""
import argparse
import getpass
import os
from pathlib import Path
import secrets
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--backend', choices=['sqlite', 'postgres'], required=True)
    parser.add_argument('--directory', type=Path, default=Path(__file__).parent / '.secrets')
    parser.add_argument('--oidc', action='store_true', help='prompt for confidential client secret')
    args = parser.parse_args()
    os.umask(0o077)
    args.directory.mkdir(mode=0o700, parents=True, exist_ok=False)
    password = secrets.token_urlsafe(36)
    app_password = secrets.token_urlsafe(36)
    values = {
        'database_url': ('sqlite:///app/data/accounts.sqlite' if args.backend == 'sqlite'
                         else 'postgres://rustdesk:' + app_password + '@postgres:5432/rustdesk'),
        'oidc_client_secret': getpass.getpass('OIDC client secret (empty for public client): ') if args.oidc else '',
        'bootstrap_password': secrets.token_urlsafe(30),
    }
    if args.backend == 'postgres':
        values['postgres_password'] = password
        values['app_password'] = app_password
    for name, value in values.items():
        with (args.directory / name).open('x', encoding='utf-8') as stream:
            stream.write(value + '\n')
        (args.directory / name).chmod(0o600)
    print('Secret files created with mode 0600; set container ownership before starting.')


if __name__ == '__main__':
    try:
        main()
    except (OSError, EOFError, KeyboardInterrupt):
        print('Secret initialization failed; existing files were not overwritten.', file=sys.stderr)
        sys.exit(1)
