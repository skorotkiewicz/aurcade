#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
require AURCADE_SSH_KEY_A
wait_for_stack aurcade
[[ -r $AURCADE_SSH_KEY_A ]] || { echo "SSH key is not readable: $AURCADE_SSH_KEY_A" >&2; exit 2; }

work=$(mktemp -d)
token=$(date +%s)-$$
package="demo-test-$token"
repo="$AURCADE_USER_A/aur/$package"
ssh_base=(-T -p 2222 -o ForwardX11=no -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o IdentitiesOnly=yes)
cleanup() {
    ssh "${ssh_base[@]}" -i "$AURCADE_SSH_KEY_A" git@"$AURCADE_HOST" \
        "delete $repo --confirm $repo" >/dev/null 2>&1 || true
    rm -rf "$work"
}
trap cleanup EXIT

git -C "$work" init -q --initial-branch=main source
git -C "$work/source" config user.name 'AURcade tests'
git -C "$work/source" config user.email "tests@$AURCADE_DOMAIN"
cat > "$work/source/.SRCINFO" <<EOF
pkgbase = $package
pkgdesc = AURcade integration package
pkgver = 1.0
pkgrel = 1
arch = any
license = MIT
pkgname = $package
EOF
cat > "$work/source/PKGBUILD" <<EOF
pkgname=$package
pkgver=1.0
pkgrel=1
pkgdesc='AURcade integration package'
arch=('any')
license=('MIT')
package() {
    install -Dm644 /dev/null "\$pkgdir/usr/share/\$pkgname/test"
}
EOF
git -C "$work/source" add .SRCINFO PKGBUILD
git -C "$work/source" commit -qm 'integration test package'
GIT_SSH_COMMAND="ssh ${ssh_base[*]} -i $AURCADE_SSH_KEY_A" \
    git -C "$work/source" push -q "ssh://git@$AURCADE_HOST:2222/$repo.git" main

scheme=http
curl_args=(-fsS)
if [[ $AURCADE_TLS == true ]]; then scheme=https; curl_args=(-kfsS); fi
base="$scheme://$AURCADE_HOST:8080/aur"

curl "${curl_args[@]}" "$base/rpc?v=5&type=info&arg%5B%5D=$package" \
    | jq -e --arg package "$package" --arg maintainer "$AURCADE_USER_A" \
        '.resultcount == 1 and .results[0].Name == $package and
         .results[0].PackageBase == $package and .results[0].Version == "1.0-1" and
         .results[0].Maintainer == $maintainer' >/dev/null
curl "${curl_args[@]}" "$base/rpc?arg=demo-test&by=name-desc&type=search&v=5" \
    | jq -e --arg package "$package" \
        '.type == "search" and any(.results[]; .Name == $package)' >/dev/null
curl "${curl_args[@]}" "$base/packages.gz" | gzip -dc | grep -Fx "$package" >/dev/null
curl "${curl_args[@]}" "$base/cgit/aur.git/plain/PKGBUILD?h=$package" > "$work/PKGBUILD"
cmp -s "$work/source/PKGBUILD" "$work/PKGBUILD"
git -c http.sslVerify=false clone -q "$base/$package.git" "$work/clone"
cmp -s "$work/source/.SRCINFO" "$work/clone/.SRCINFO"
cmp -s "$work/source/PKGBUILD" "$work/clone/PKGBUILD"

ssh "${ssh_base[@]}" -i "$AURCADE_SSH_KEY_A" git@"$AURCADE_HOST" \
    "delete $repo --confirm $repo" | grep -F 'RESTORE POINT SAVED' >/dev/null
trap - EXIT
rm -rf "$work"
pass 'AUR RPC, package list, raw PKGBUILD, and smart HTTP clone'
