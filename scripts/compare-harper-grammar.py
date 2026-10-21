#!/usr/bin/env python3
"""Prepare and report a local ML comparison over the production Harper export."""
import argparse
import json
from pathlib import Path
import sys

from grammar_compare.harper_comparison import make_request, prepare, finish

HERE = Path(__file__).resolve().parent


def samples(value):
    try:
        number = int(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError('Samples must be an integer between 1 and 20.') from error
    if not 1 <= number <= 20:
        raise argparse.ArgumentTypeError('Samples must be an integer between 1 and 20.')
    return number


def arguments(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    command = sub.add_parser('request', help='Generate the complete Rust probe request from source fixtures.')
    command.add_argument('--fixtures', type=Path, default=HERE / 'grammar_compare/harper-fixtures.json')
    command.add_argument('--output', type=Path, required=True)
    command.add_argument('--samples', type=samples, default=3)
    command = sub.add_parser('prepare')
    command.add_argument('--fixtures', type=Path, required=True)
    command.add_argument('--probe', type=Path, required=True)
    command.add_argument('--corpus', type=Path, required=True)
    command.add_argument('--sidecar', type=Path, required=True)
    command = sub.add_parser('finish')
    command.add_argument('--results', type=Path, required=True)
    command.add_argument('--sidecar', type=Path, required=True)
    command.add_argument('--output', type=Path, required=True)
    return parser.parse_args(argv)


def main(argv=None):
    args = arguments(argv)
    if args.command == 'request':
        result = make_request(args.fixtures, args.output, args.samples)
    elif args.command == 'prepare':
        result = prepare(args.fixtures, args.probe, args.corpus, args.sidecar)
    else:
        result = finish(args.results, args.sidecar, args.output)
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0


if __name__ == '__main__':
    try:
        raise SystemExit(main())
    except (ValueError, OSError) as error:
        print(f'Harper grammar comparison: {error}', file=sys.stderr)
        raise SystemExit(2)
