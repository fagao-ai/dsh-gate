#!/usr/bin/env bash
# Recreate the dsh-gw container with new login credentials.
# Usage: ./set-password.sh
set -euo pipefail

read -rp "New username (default: hezz): " user
user=${user:-hezz}
read -rsp "New password: " pass
echo
if [ -z "$pass" ]; then
  echo "password cannot be empty" >&2
  exit 1
fi

docker rm -f dsh-gw >/dev/null 2>&1 || true
docker run -d --name dsh-gw --restart unless-stopped -p 8080:8080 \
  -e "AUTH_USER=$user" \
  -e "AUTH_PASSWORD=$pass" \
  -e BACKEND=http://host.docker.internal:3080 \
  dsh-rs-gateway
echo "dsh-gw recreated — login with $user at https://dsh.hezz.eu.org"
