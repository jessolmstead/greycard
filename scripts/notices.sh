#!/bin/sh
# Write THIRD-PARTY-NOTICES, the notices of everything greycard's
# packages carry that is not greycard's own.
#
#     scripts/notices.sh [output]      (target/THIRD-PARTY-NOTICES if none)
#
# scripts/package.sh runs this into every package, and CI runs it on
# every pull request, so the file is made from the tree it ships with
# and a dependency under a license about.toml does not accept fails
# the pull request rather than the release. It is one file for every
# platform: about.toml names each triple a release builds.
#
# The parts, in order:
#
#   packaging/notices/bundled.txt    hand-kept: the native libraries
#                                    package.sh puts beside the
#                                    binaries, and the artwork compiled
#                                    into them
#   crates/greycard-slint/ui/icons/LICENSE
#                                    the interface icons' license, read
#                                    where the icons are kept
#   packaging/notices/ported.txt     hand-kept: code ported into
#                                    greycard, and what is downloaded
#                                    rather than shipped
#   cargo-about's output             the Rust crates in greycard-ui's
#                                    tree (the command line's is a
#                                    subset of it), from about.toml and
#                                    packaging/notices/crates.hbs
#   packaging/notices/crate-notices  the files some crates keep their
#                                    other notices in, one line a crate
#                                    version, copied from where cargo
#                                    resolved each version (the list
#                                    packaging/notices/crates.tsv.hbs
#                                    has cargo-about write) and checked
#                                    by sum
#   packaging/notices/onnxruntime-ThirdPartyNotices.txt
#                                    Microsoft's, for ONNX Runtime
#                                    1.28.0, as its repository has it at
#                                    the v1.28.0 tag; replace it when
#                                    ort-dylib's runtime moves
#   packaging/notices/DirectXShaderCompiler.txt
#                                    LICENSE.TXT and ThirdPartyNotices.txt
#                                    of microsoft/DirectXShaderCompiler,
#                                    for the two DXC libraries on Windows
#
# cargo-about runs offline, after a cargo fetch for every platform, so
# the crates' own license files are what it reads and two runs on one
# commit write the same file. It is pinned: install it with
#
#     cargo install cargo-about --locked --version 0.9.3 --features cli

set -eu

cd "$(dirname "$0")/.."

about_version=0.9.3
out=${1:-target/THIRD-PARTY-NOTICES}

if ! have=$(cargo about --version 2>/dev/null); then
    echo "notices.sh: cargo-about is missing; install it with" >&2
    echo "  cargo install cargo-about --locked --version $about_version --features cli" >&2
    exit 1
fi
if [ "$have" != "cargo-about $about_version" ]; then
    echo "notices.sh: this wants cargo-about $about_version, and $have is installed" >&2
    exit 1
fi

# Every platform's crates, not only this host's: the Windows and macOS
# ones are in the file too.
cargo fetch --locked --quiet

mkdir -p "$(dirname "$out")"
crates="$out.crates"
tsv="$out.tsv"
list="$out.list"
log="$out.log"
extra="$out.extra"
rm -f "$out" "$crates" "$tsv" "$list" "$log" "$extra"
trap 'rm -f "$crates" "$tsv" "$tsv.raw" "$list" "$log" "$extra" "$out.part"' EXIT

fail() {
    for line in "$@"; do echo "$line" >&2; done
    exit 1
}

# cargo-about over the tree, into $2 by the template $1. --fail stops
# on a license it cannot resolve, but a clarification whose checksum
# no longer matches is only a warning, after which cargo-about falls
# back to scanning the crate's sources and emits them as license
# texts. So any warning fails here too. Its log is colored whatever
# --color says, so the escapes come off before it is shown.
esc=$(printf '\033')
about() {
    if ! cargo about --color never generate --locked --offline --fail \
        -m crates/greycard-ui/Cargo.toml -c about.toml \
        -o "$2" "$1" 2>"$log"; then
        sed "s/$esc\[[0-9;]*m//g" "$log" >&2
        fail "notices.sh: cargo-about failed; see above"
    fi
    if grep -q 'WARN' "$log"; then
        sed "s/$esc\[[0-9;]*m//g" "$log" >&2
        fail "notices.sh: cargo-about warned, so the notices would not be" \
            "  what the crates ship. A checksum mismatch means the crate's" \
            "  license files changed: read them, then put their new checksums" \
            "  (each file's sha256sum) into about.toml."
    fi
}
about packaging/notices/crates.hbs "$crates"
# Every crate in those notices, as name, version and the Cargo.toml
# cargo metadata resolved it to (registry, git or vendored).
# Carriage returns come off everything read here, so a checkout with
# CRLF line ends (.gitattributes asks for LF) reads the same.
about packaging/notices/crates.tsv.hbs "$tsv.raw"
tr -d '\r' <"$tsv.raw" >"$tsv"
rm -f "$tsv.raw"
tab=$(printf '\t')

# A clarification for a crate the tree does not have is ignored
# without a word, and the crate it was meant for loses its text. Every
# way TOML can spell the header: spaces around the parts, the name
# quoted, a comment after it.
q="[\"']?"
s="[[:space:]]*"
header="^$s\\[$s$q([A-Za-z0-9_-]+)$q$s\\.$s${q}clarify$q$s\\]$s(#.*)?\$"
for name in $(tr -d '\r' <about.toml | sed -n -E "s/$header/\\1/p"); do
    if ! grep -q "^$name$tab" "$tsv"; then
        fail "notices.sh: about.toml clarifies $name, which is not among the" \
            "  crates the notices list: rename the [$name.clarify] block to" \
            "  the crate it is for, or remove it"
    fi
done

# The notices packaging/notices/crate-notices lists, one line a crate
# version, each from that version's own source and checked against
# the sum kept there.
if command -v sha256sum >/dev/null 2>&1; then
    sha256() { sha256sum "$1" | cut -d ' ' -f 1; }
else
    sha256() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
fi
# The source directory of crate $1 at version $2, from cargo-about's
# list, as a path Git Bash's sh can open on Windows.
source_dir() {
    m=$(awk -F "$tab" -v n="$1" -v v="$2" \
        '$1 == n && $2 == v { print $3; exit }' "$tsv")
    m=${m%Cargo.toml}
    if [ -n "$m" ] && command -v cygpath >/dev/null 2>&1; then
        m=$(cygpath -u "$m")
    fi
    printf '%s' "$m"
}
tr -d '\r' <packaging/notices/crate-notices | grep -v '^#' | grep . >"$list" || :
for name in $(awk '{ print $1 }' "$list" | sort -u); do
    file=$(awk -v n="$name" '$1 == n { print $3; exit }' "$list")
    built=$(awk -F "$tab" -v n="$name" '$1 == n { print $2 }' "$tsv" | sort -u)
    if [ -z "$built" ]; then
        fail "notices.sh: $name is no longer in the tree; delete its lines" \
            "  from packaging/notices/crate-notices"
    fi
    for version in $built; do
        if ! awk -v n="$name" -v v="$version" \
            '$1 == n && $2 == v { f = 1 } END { exit !f }' "$list"; then
            dir=$(source_dir "$name" "$version")
            if [ -f "$dir$file" ]; then sum=$(sha256 "$dir$file"); else sum="<no $file>"; fi
            fail "notices.sh: $name $version is built, and" \
                "  packaging/notices/crate-notices does not name it. Its other" \
                "  versions' notice is $file; check that this version still" \
                "  keeps it there (sha256 below if it does), read it, then add" \
                "  $name $version $file $sum"
        fi
    done
done
while read -r name version file sum; do
    dir=$(source_dir "$name" "$version")
    if [ -z "$dir" ]; then
        fail "notices.sh: $name $version is no longer built; delete its" \
            "  line from packaging/notices/crate-notices"
    fi
    if [ ! -f "$dir$file" ]; then
        fail "notices.sh: $name $version has no $file; read what it ships" \
            "  now and update its line in packaging/notices/crate-notices"
    fi
    if [ "$(sha256 "$dir$file")" != "$sum" ]; then
        fail "notices.sh: $name $version's $file has changed: read it, then" \
            "  put its new sha256 in packaging/notices/crate-notices"
    fi
    printf '\n%s %s: %s\n\n' "$name" "$version" "$file"
    case " ${seen:-} " in
    *" $sum "*) printf 'The same text as an entry above.\n' ;;
    *) cat "$dir$file"; seen="${seen:-} $sum" ;;
    esac
done <"$list" >"$extra"

{
    cat packaging/notices/bundled.txt
    sed 's/^/    /; s/^ *$//' crates/greycard-slint/ui/icons/LICENSE
    cat packaging/notices/ported.txt
    cat "$crates"
    printf '\n================================================================================\n'
    printf 'NOTICES THE CRATES KEEP BESIDE THEIR LICENSES\n'
    printf '================================================================================\n'
    cat "$extra"
    printf '\n================================================================================\n'
    printf 'ONNX RUNTIME 1.28.0: THIRD-PARTY NOTICES, AS MICROSOFT PUBLISHES THEM\n'
    printf '================================================================================\n\n'
    cat packaging/notices/onnxruntime-ThirdPartyNotices.txt
    printf '\n================================================================================\n'
    printf 'DIRECTX SHADER COMPILER: LICENSE AND THIRD-PARTY NOTICES (WINDOWS x86_64)\n'
    printf '================================================================================\n\n'
    cat packaging/notices/DirectXShaderCompiler.txt
} >"$out.part"
rm -f "$crates" "$extra"
# The same fallback, caught by what it leaves behind: a source file
# read as a license. Nothing in a notice starts a line this way.
if grep -n -E '^#[[:space:]]*(include|ifndef)' "$out.part" >&2; then
    rm -f "$out.part"
    echo "notices.sh: the notices hold source code (lines above); a" >&2
    echo "  clarification in about.toml is out of date for that crate" >&2
    exit 1
fi
mv "$out.part" "$out"
echo "$out"
