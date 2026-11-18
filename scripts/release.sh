#!/bin/sh
# Construit le CLI pour toutes les plateformes et, avec --upload, le publie sur R2 (bucket public
# « pulse-send », servi par https://media.send.noburi.space/cli/). public/install.sh lit
# cli/latest.txt puis cli/<version>/pulsesend-<version>-<cible>.tar.gz (+ .sha256).
#
#   cli/scripts/release.sh            construit dans cli/dist/<version>/
#   cli/scripts/release.sh --upload   construit puis publie (clés R2_* du .env de l'app)
#
# Prérequis : rustup (cibles ajoutées ici), cargo-zigbuild et zig dans le PATH, zip.
set -eu

cd "$(dirname "$0")/.."
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
dist="dist/$version"
# Windows en gnu ici (zig ne sait pas faire msvc) ; la CI GitHub publie la version msvc.
targets="x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-apple-darwin aarch64-apple-darwin x86_64-pc-windows-gnu"

rustup target add $targets > /dev/null
rm -rf "$dist"
mkdir -p "$dist"

for t in $targets; do
    echo "== $t"
    cargo zigbuild --release --target "$t" --quiet
    stage="$(mktemp -d)"
    case "$t" in
        *windows*)
            cp "target/$t/release/pulsesend.exe" "$stage/"
            archive="pulsesend-$version-$t.zip"
            (cd "$stage" && zip -q "$OLDPWD/$dist/$archive" pulsesend.exe)
            ;;
        *)
            cp "target/$t/release/pulsesend" "$stage/"
            archive="pulsesend-$version-$t.tar.gz"
            tar -czf "$dist/$archive" -C "$stage" pulsesend
            ;;
    esac
    rm -rf "$stage"
    (cd "$dist" && sha256sum "$archive" > "$archive.sha256")
done

echo "$version" > dist/latest.txt
ls -l "$dist"

[ "${1:-}" = "--upload" ] || exit 0

# Clés R2 de l'app (jeton limité au bucket pulse-send), lues dans le .env, jamais affichées.
env_file=../.env
val() { sed -n "s/^$1=//p" "$env_file" | tr -d '"'; }
R2_ACCESS_KEY_ID="$(val R2_ACCESS_KEY_ID)"
R2_SECRET_ACCESS_KEY="$(val R2_SECRET_ACCESS_KEY)"
R2_ENDPOINT="$(val R2_ENDPOINT)"
R2_BUCKET="$(val R2_BUCKET)"
[ -n "$R2_ACCESS_KEY_ID" ] || { echo "error: R2_ACCESS_KEY_ID missing from $env_file" >&2; exit 1; }

put() { # $1 = fichier local, $2 = clé dans le bucket, $3 = Content-Type, $4 = Cache-Control
    code="$(curl -s -o /dev/null -w '%{http_code}' --aws-sigv4 'aws:amz:auto:s3' \
        --user "$R2_ACCESS_KEY_ID:$R2_SECRET_ACCESS_KEY" \
        -H "Content-Type: $3" -H "Cache-Control: $4" \
        -T "$1" "$R2_ENDPOINT/$R2_BUCKET/$2")"
    [ "$code" = 200 ] || { echo "error: upload of $2 failed (HTTP $code)" >&2; exit 1; }
    echo "uploaded $2"
}

for f in "$dist"/*; do
    name="$(basename "$f")"
    case "$name" in
        *.sha256) type=text/plain ;;
        *.zip) type=application/zip ;;
        *) type=application/gzip ;;
    esac
    # Une version publiée ne change plus : cache long.
    put "$f" "cli/$version/$name" "$type" "public, max-age=31536000, immutable"
done
# latest.txt en dernier, une fois toutes les archives en ligne ; cache court.
put dist/latest.txt cli/latest.txt text/plain "public, max-age=300"
