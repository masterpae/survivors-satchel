#!/bin/sh
# Builds in a throwaway container. Needs only Docker.
#   sh build/build.sh        cross-builds dist/SurvivorsSatchel.exe for Windows
#   sh build/build.sh test   runs the core tests (Linux, natively)
#   sh build/build.sh preview   renders the screens to dist/preview/*.ppm (Linux, natively)
#   sh build/build.sh catalog   refreshes catalog/catalog.json from catalog/sources.toml and GitHub;
#                               the addon files it reads are downloaded inside the container only
# Environment:
#   SATCHEL_BUILD_CACHE   host folder for crates, the Windows SDK and build output (default ./.build-cache)
#   SATCHEL_DOCKER_ARGS   extra `docker run` arguments, e.g. "--cpus 6 --memory 16g"
#   SATCHEL_GIT_COMMIT    the commit, shown in the app's About when set
#   SATCHEL_REPOSITORY    owner/name: a build that knows its repository looks for newer releases
#                         there (the release workflow sets it); without it the app never asks
#   SATCHEL_LOCKED=1      build exactly what Cargo.lock says (release builds)
#   GITHUB_TOKEN          `catalog` only: its GitHub API calls carry it when set (the catalog
#                         workflow); without it they're anonymous, 60 an hour
set -eu
cd "$(dirname "$0")/.."

image=survivors-satchel-build
cache=${SATCHEL_BUILD_CACHE:-$PWD/.build-cache}
target=x86_64-pc-windows-msvc

docker build -q -t "$image" build/ >/dev/null
mkdir -p "$cache" dist

locked=
[ "${SATCHEL_LOCKED:-0}" = 1 ] && locked=--locked
# Passed into the container only where it's needed.
extra=

run() {
    # shellcheck disable=SC2086  # SATCHEL_DOCKER_ARGS and $extra are split on purpose
    docker run --rm ${SATCHEL_DOCKER_ARGS:-} \
        --user "$(id -u):$(id -g)" \
        -e SATCHEL_GIT_COMMIT -e SATCHEL_REPOSITORY $extra \
        -v "$PWD:/src" -v "$cache:/cache" \
        "$image" "$@"
}

case "${1:-exe}" in
    test)
        run cargo test $locked -p satchel-core -p satchel-catalog
        ;;
    catalog)
        extra="-e GITHUB_TOKEN"
        run cargo run --release $locked -p satchel-catalog -- catalog/sources.toml catalog/catalog.json
        ;;
    preview)
        rm -rf dist/preview
        run cargo run --release $locked -p ui-preview -- dist/preview
        ;;
    exe)
        run cargo xwin build --release $locked --target "$target"
        cp "$cache/target/$target/release/SurvivorsSatchel.exe" dist/
        echo "dist/SurvivorsSatchel.exe"
        ;;
    *)
        echo "usage: build.sh [exe|test|preview|catalog]" >&2
        exit 2
        ;;
esac
