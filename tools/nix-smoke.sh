#!/usr/bin/env bash
# Exercises an installed midy package: tools/nix-smoke.sh <store path>.
set -euo pipefail

package=$(realpath -e "${1:?package output required}")
source_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$source_dir/Cargo.toml" | head -n 1)
scratch=$(mktemp -d "${TMPDIR:-/tmp}/midy-nix-smoke.XXXXXXXX")
trap 'rm -rf "$scratch"' EXIT
cd "$scratch"
midy="$package/bin/midy"

test "$("$midy" --version)" = "midy $version"
"$midy" --help > /dev/null
"$midy" schema > /dev/null

# One track holding middle C for a beat at 480 ticks per beat.
base64 -d <<< 'TVRoZAAAAAYAAQABAeBNVHJrAAAADQCQPECDYIA8QAD/LwA=' > song.mid

"$midy" read song.mid > song.txt
grep -q 'key=60' song.txt
"$midy" lint song.mid --exit-code > /dev/null

"$midy" apply song.mid song.txt -o same.mid > /dev/null
"$midy" diff song.mid same.mid --exit-code > /dev/null

"$midy" apply song.mid -e 'TRANSPOSE semitones=2' -o up.mid > /dev/null
"$midy" read up.mid > up.txt
grep -q 'key=62' up.txt

# Reaches the packaged FluidSynth from an otherwise empty PATH.
: > fake.sf2
render=$(env -i PATH=/nonexistent "$midy" render song.mid out.wav --soundfont fake.sf2 2>&1 || true)
case "$render" in
  *"could not run"*)
    printf '%s\n' "$render" >&2
    exit 1
    ;;
esac

printf 'Installed midy smoke checks passed\n'
