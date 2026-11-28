#!/bin/sh
# The newest glibc a file asks for, held to a ceiling: the guide's
# system requirements name the glibc the editor runs on and the one
# the AI features need, and a toolchain or a runtime that starts
# asking for a newer one would make them wrong without a word.
#
#     scripts/glibc-floor.sh 2.35 bin/greycard bin/greycard-ui
#     scripts/glibc-floor.sh 2.38 bin/libonnxruntime.so
#
# Only symbols the loader must find count: a weak one (std's pidfd
# calls, from 2.39) is looked up and done without. Needs readelf.
set -eu
ceiling=$1; shift
status=0
for f in "$@"; do
    # Read first and checked after: a readelf that fails inside a
    # pipeline would pass as a file asking for nothing.
    syms=$(readelf --dyn-syms -W "$f")
    floor=$(printf '%s\n' "$syms" |
        awk '$5 != "WEAK" && $7 == "UND" && match($8, /@GLIBC_[0-9.]+/) {
                 print substr($8, RSTART + 7, RLENGTH - 7) }' |
        sort -uV | tail -n 1)
    if [ -z "$floor" ]; then
        echo "glibc-floor.sh: $f asks for no glibc symbol; not a glibc binary?" >&2
        status=1
        continue
    fi
    newest=$(printf '%s\n%s\n' "$floor" "$ceiling" | sort -V | tail -n 1)
    if [ "$newest" != "$ceiling" ]; then
        echo "glibc-floor.sh: $f needs glibc $floor, past the $ceiling the guide gives:" >&2
        printf '%s\n' "$syms" | awk -v v="@GLIBC_$floor" '$5 != "WEAK" && index($8, v) { print "  " $8 }' >&2
        status=1
    else
        echo "$f: glibc $floor (ceiling $ceiling)"
    fi
done
exit $status
