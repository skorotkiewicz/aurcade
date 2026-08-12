#!/usr/bin/env bash
source "$(dirname "$0")/lib.sh"
require AURCADE_SSH_KEY_A AURCADE_SSH_KEY_B
wait_for_stack aurcade

for key in "$AURCADE_SSH_KEY_A" "$AURCADE_SSH_KEY_B"; do
    [[ -r $key ]] || { echo "SSH key is not readable: $key" >&2; exit 2; }
done

work=$(mktemp -d)
token=$(date +%s)-$$
repo="$AURCADE_USER_A/test-$token"
ssh_base=(-p 2222 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o IdentitiesOnly=yes)
cleanup() {
    ssh "${ssh_base[@]}" -i "$AURCADE_SSH_KEY_A" git@"$AURCADE_HOST" \
        "delete $repo --confirm $repo" >/dev/null 2>&1 || true
    rm -rf "$work"
}
trap cleanup EXIT

ssh "${ssh_base[@]}" -i "$AURCADE_SSH_KEY_A" git@"$AURCADE_HOST" </dev/null \
    | grep -F 'AURCADE' >/dev/null

git -C "$work" init -q --initial-branch=main source
git -C "$work/source" config user.name 'AURcade tests'
git -C "$work/source" config user.email "tests@$AURCADE_DOMAIN"
printf 'git integration %s\n' "$token" > "$work/source/README.md"
git -C "$work/source" add README.md
git -C "$work/source" commit -qm 'integration test'
GIT_SSH_COMMAND="ssh ${ssh_base[*]} -i $AURCADE_SSH_KEY_A" \
    git -C "$work/source" push -q "ssh://git@$AURCADE_HOST:2222/$repo.git" main

scheme=http
curl_args=(-fsS)
if [[ $AURCADE_TLS == true ]]; then scheme=https; curl_args=(-kfsS); fi
git -c http.sslVerify=false clone -q "$scheme://$AURCADE_HOST:8080/$repo.git" "$work/clone"
grep -F "git integration $token" "$work/clone/README.md" >/dev/null
curl "${curl_args[@]}" "$scheme://$AURCADE_HOST:8080/$repo/" | grep -F "test-$token" >/dev/null

if GIT_SSH_COMMAND="ssh ${ssh_base[*]} -i $AURCADE_SSH_KEY_B" \
    git -C "$work/source" push -q "ssh://git@$AURCADE_HOST:2222/$repo.git" main 2>/dev/null; then
    echo 'cross-user Git push unexpectedly succeeded' >&2
    exit 1
fi

ssh "${ssh_base[@]}" -i "$AURCADE_SSH_KEY_A" git@"$AURCADE_HOST" \
    "delete $repo --confirm $repo" | grep -F 'RESTORE POINT SAVED' >/dev/null
trap - EXIT
rm -rf "$work"
pass 'SSH lobby, first push, HTTPS clone/cgit, access denial, and recoverable delete'
