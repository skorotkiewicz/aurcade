# https://github.com/casey/just

[private]
default:
    @just --list

build:
    cargo build --release

build-all:
    cargo build --release --all-features

run *args:
    cargo run --all-features -- {{ args }}

fmt:
    cargo fmt
    cargo clippy --all-targets --all-features -- -D warnings
    # cargo shear --fix # cargo install shear

check:
    cargo fmt --check
    cargo clippy --all-targets --all-features -- -D warnings

test: fmt
    cargo test

test-services:
    tests/all.sh

install-hook:
    @printf '#!/bin/sh\nset -e\njust check\n' > .git/hooks/pre-commit
    @chmod +x .git/hooks/pre-commit

remove-hook:
    @rm .git/hooks/pre-commit

add-tag ORIGIN="origin":
    #!/usr/bin/env bash
    set -euo pipefail
    VERSION=$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)
    git push "{{ ORIGIN }}" main
    git tag -a "v${VERSION}" -m "Release v${VERSION}"
    git push "{{ ORIGIN }}" "v${VERSION}"

# `just remove-tag github v0.0.0` or `just remove-tag github` (uses fzf)
remove-tag ORIGIN="origin" VERSION="":
    #!/usr/bin/env bash
    set -euo pipefail
    tag="{{ VERSION }}"
    [ -z "$tag" ] && tag=$(git tag | sort -V | fzf --prompt="Select tag to remove: ")
    [ -z "$tag" ] && echo "No tag selected" && exit 1
    git tag -d "$tag"
    git push --delete "{{ ORIGIN }}" "$tag"

# `just push-all main`
push-all BRANCH="main":
    #!/usr/bin/env bash
    set -euo pipefail
    mapfile -t remotes < <(git remote)
    ((${#remotes[@]})) || { echo "No remotes configured"; exit 1; }
    for remote in "${remotes[@]}"; do
        git push "$remote" "{{ BRANCH }}"
    done

# add-tag-all:
#     #!/usr/bin/env bash
#     set -euo pipefail
#     VERSION=$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)
#     just push-all main
#     git tag -a "v${VERSION}" -m "Release v${VERSION}"
#     mapfile -t remotes < <(git remote)
#     for remote in "${remotes[@]}"; do
#         git push "$remote" "v${VERSION}"
#     done

# # `just remove-tag-all v0.0.0` or `just remove-tag-all` (uses fzf)
# remove-tag-all VERSION="":
#     #!/usr/bin/env bash
#     set -euo pipefail
#     mapfile -t remotes < <(git remote)
#     ((${#remotes[@]})) || { echo "No remotes configured"; exit 1; }
#     tag="{{ VERSION }}"
#     [ -z "$tag" ] && tag=$(git tag | sort -V | fzf --prompt="Select tag to remove: ")
#     [ -z "$tag" ] && echo "No tag selected" && exit 1
#     git tag -d "$tag"
#     for remote in "${remotes[@]}"; do
#         git push --delete "$remote" "$tag"
#     done
